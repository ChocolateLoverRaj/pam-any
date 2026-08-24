#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, SyncSender};
use std::thread::{self, Thread};

use pam_bindings::constants::PamResultCode::{PAM_AUTH_ERR, PAM_SUCCESS};
use pam_bindings::constants::{
    PAM_ERROR_MSG, PAM_PROMPT_ECHO_OFF, PAM_RADIO_TYPE, PAM_TEXT_INFO, PamFlag, PamMessageStyle,
    PamResultCode,
};
use pam_bindings::conv::Conv;
use pam_bindings::module::{PamHandle, PamHooks};
use pam_bindings::pam_try;
use pam_client2::{Context, ConversationHandler, ErrorCode, Flag};
use rustix::event::{PollFd, PollFlags, poll};
use rustix::net::{SendFlags, send};
use rustix::termios::{LocalModes, OptionalActions, SpecialCodeIndex, tcgetattr, tcsetattr};
use serde::{Deserialize, Serialize};

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
pub enum Mode {
    One,
    All,
}

#[derive(Serialize, Deserialize, Debug)]
struct Input {
    mode: Mode,
    modules: HashMap<String, String>,
    #[serde(default)]
    silence_messages: bool,
}

enum MsgType {
    Info,
    Error,
}
impl MsgType {
    fn style(&self) -> PamMessageStyle {
        match self {
            Self::Info => PAM_TEXT_INFO,
            Self::Error => PAM_ERROR_MSG,
        }
    }
}
struct Msg {
    _type: MsgType,
    msg: String,
}
enum PromptType {
    EchoOff,
    YesNo,
}
impl PromptType {
    fn style(&self) -> PamMessageStyle {
        match self {
            Self::EchoOff => PAM_PROMPT_ECHO_OFF,
            Self::YesNo => PAM_RADIO_TYPE,
        }
    }
}
struct Req {
    _type: PromptType,
    str: String,
}

fn read_password_interruptible(
    tty: &mut File,
    notify: &mut UnixStream,
    prompt: &str,
) -> Option<CString> {
    let _ = tty.write_all(prompt.as_bytes());

    let orig = tcgetattr(&*tty).ok()?;

    // ECHO/ICANON off for raw single-byte reads; ISIG off so Ctrl-C is byte 0x03, not a real SIGINT.
    let mut raw = orig.clone();
    raw.local_modes -= LocalModes::ECHO | LocalModes::ICANON | LocalModes::ISIG;
    raw.special_codes[SpecialCodeIndex::VMIN] = 1;
    raw.special_codes[SpecialCodeIndex::VTIME] = 0;
    if tcsetattr(&*tty, OptionalActions::Now, &raw).is_err() {
        // Couldn't disable echo, so bail out rather than risk echoing the password
        return None;
    }

    let mut input: Vec<u8> = Vec::new();
    let result = 'read: loop {
        let mut fds = [
            PollFd::new(&*tty, PollFlags::IN),
            PollFd::new(&*notify, PollFlags::IN),
        ];
        if poll(&mut fds, None).is_err() {
            break None;
        }
        let (tty_ready, notify_ready) =
            (!fds[0].revents().is_empty(), !fds[1].revents().is_empty());

        if notify_ready {
            // Another auth method succeeded, so erase the prompt line
            let _ = tty.write_all(b"\r\x1b[2K");
            // Drain the socket so it doesn't fire again
            let mut drain = [0u8; 64];
            let _ = notify.read(&mut drain);
            break None;
        }

        if tty_ready {
            let mut b = [0u8; 1];
            if tty.read(&mut b).unwrap_or(0) == 0 {
                break None;
            }
            match b[0] {
                b'\n' | b'\r' => {
                    let _ = tty.write_all(b"\n");
                    break 'read CString::new(input).ok();
                }
                0x7f | 0x08 => {
                    input.pop(); // backspace / DEL
                }
                0x03 | 0x04 => break None, // Ctrl-C / Ctrl-D
                c => input.push(c),
            }
        }
    };

    let _ = tcsetattr(&*tty, OptionalActions::Now, &orig);
    result
}

struct PamAny;
pam_bindings::pam_hooks!(PamAny);
impl PamHooks for PamAny {
    fn sm_authenticate(pamh: &mut PamHandle, args: Vec<&CStr>, _flags: PamFlag) -> PamResultCode {
        let arg_string = args
            .iter()
            .map(|s| s.to_str().unwrap())
            .collect::<Vec<_>>()
            .join(" ");
        let input = pam_try!(serde_json::from_str::<Input>(&arg_string).map_err(|_e| PAM_AUTH_ERR));

        let conv = pam_try!(pam_try!(pamh.get_item::<Conv>()).ok_or(PAM_AUTH_ERR));
        let user = Arc::new(pam_try!(pamh.get_user(None)));

        let prompt = format!(
            "{}: ",
            input
                .modules
                .values()
                .cloned()
                .collect::<Vec<_>>()
                .join(" or ")
        );

        // Falls back to conv.send if /dev/tty is unavailable (e.g. non-interactive sessions).
        let mut tty_file: Option<File> = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .ok();

        // Notification pipe: each thread gets a cloned write end. Writing to it wakes
        // the main thread out of poll() when a result arrives.
        let (mut notify_r, notify_w): (Option<UnixStream>, Option<UnixStream>) =
            if tty_file.is_some() {
                match UnixStream::pair() {
                    Ok((r, w)) => (Some(r), Some(w)),
                    Err(_) => (None, None),
                }
            } else {
                (None, None)
            };

        let mode = input.mode;
        let (ref msg_tx, msg_rx) = std::sync::mpsc::channel();
        let channels = input
            .modules
            .into_iter()
            .map(|(service, service_display_name)| {
                let user = user.clone();
                let main_thread = thread::current();
                let msg_tx = msg_tx.clone();

                // Each thread owns a cloned write end so it can signal independently
                let thread_notify_w: Option<UnixStream> =
                    notify_w.as_ref().and_then(|w| w.try_clone().ok());

                let (req_tx, req_rx) = std::sync::mpsc::sync_channel(1);
                let (res_tx, res_rx) = std::sync::mpsc::sync_channel(1);
                let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
                thread::spawn(move || {
                    // Only signals when this result decides the overall outcome, so a fast-failing service can't cancel a still-running one's prompt.
                    let pipe_signal = |ok: bool, mut stream: Option<UnixStream>| {
                        let should_write = match mode {
                            Mode::One => ok,
                            Mode::All => !ok,
                        };
                        if should_write && let Some(stream) = stream.as_mut() {
                            // `NOSIGNAL`: if the main thread already returned and dropped the
                            // read end, this must fail with EPIPE rather than SIGPIPE the host.
                            let _ = send(&*stream, b"x", SendFlags::NOSIGNAL);
                        }
                        // `stream` closes automatically here when it drops.
                        main_thread.unpark();
                    };

                    let mut context = match Context::new(
                        &service,
                        Some(&user),
                        PamAnyConversationHandler {
                            service_display_name: &service_display_name,
                            main_thread: &main_thread,
                            msg_tx: msg_tx.clone(),
                            req_tx: req_tx.clone(),
                            res_rx,
                            username: &user,
                        },
                    ) {
                        Ok(context) => context,
                        Err(e) => {
                            let _ = result_tx.send(Err(e));
                            pipe_signal(false, thread_notify_w);
                            return;
                        }
                    };
                    let result = context.authenticate(Flag::empty());
                    let ok = result.is_ok();
                    let _ = result_tx.send(result);
                    pipe_signal(ok, thread_notify_w);
                });
                (req_rx, res_tx, result_rx)
            })
            .collect::<Box<[_]>>();

        // Drop the master write end since threads own their cloned copies.
        drop(notify_w);

        // If mode is One we count fails to know if all failed
        // If mode is All we count successes to know if all succeeded
        let mut count = 0;
        let mut prompt_responded = false;
        loop {
            let mut nothing_to_process = true;

            // Check results first, since a success here must not be delayed by pending messages
            for result in channels
                .iter()
                .filter_map(|(_, _, result_rx)| result_rx.try_recv().ok())
            {
                nothing_to_process = false;
                prompt_responded = false;
                match input.mode {
                    Mode::One => {
                        if result.is_ok() {
                            return PAM_SUCCESS;
                        } else {
                            count += 1;
                            if count == channels.len() {
                                return PAM_AUTH_ERR;
                            }
                        }
                    }
                    Mode::All => {
                        if result.is_ok() {
                            count += 1;
                            if count == channels.len() {
                                return PAM_SUCCESS;
                            }
                        } else {
                            return PAM_AUTH_ERR;
                        }
                    }
                }
            }

            // Sub-service messages are shown by default (matching the upstream rewrite);
            // "silence_messages": true in the config drops them instead.
            while let Ok(msg) = msg_rx.try_recv() {
                nothing_to_process = false;
                if !input.silence_messages {
                    pam_try!(conv.send(msg._type.style(), &msg.msg));
                }
            }

            if !prompt_responded
                && let Some((req, res_tx)) = channels.iter().find_map(|(req_rx, res_tx, _)| {
                    if let Ok(req) = req_rx.try_recv() {
                        nothing_to_process = false;
                        Some((req, res_tx))
                    } else {
                        None
                    }
                })
            {
                let response = match (&req._type, tty_file.as_mut(), notify_r.as_mut()) {
                    // Echo-off: bypass conv.send, read from tty with poll() so another
                    // method's success can interrupt the read without waiting for Enter
                    (PromptType::EchoOff, Some(tty), Some(nfd)) => {
                        read_password_interruptible(tty, nfd, &prompt)
                    }
                    // Echo-off without a usable tty: same combined prompt, blocking
                    (PromptType::EchoOff, ..) => {
                        pam_try!(conv.send(req._type.style(), &prompt))
                    }
                    // A yes/no question keeps the sub-module's own wording. The
                    // combined prompt only reads correctly as "type your password
                    // or use one of the other methods".
                    (PromptType::YesNo, ..) => {
                        pam_try!(conv.send(req._type.style(), &req.str))
                    }
                };
                let _ = res_tx.send(response);
                prompt_responded = true;
            }

            if nothing_to_process {
                // Wait until a thread unparks us (new result, message, or prompt ready)
                thread::park();
            }
        }
    }
}

struct PamAnyConversationHandler<'a> {
    service_display_name: &'a str,
    username: &'a str,
    main_thread: &'a Thread,
    msg_tx: Sender<Msg>,
    req_tx: SyncSender<Req>,
    res_rx: Receiver<Option<CString>>,
}

impl PamAnyConversationHandler<'_> {
    fn format_msg(&self, msg: &str) -> String {
        format!("[{}] {}", self.service_display_name, msg)
    }

    fn send_msg(&self, _type: MsgType, msg: String) {
        let _ = self.msg_tx.send(Msg { _type, msg });
        self.main_thread.unpark();
    }
}

impl<'a> ConversationHandler for PamAnyConversationHandler<'a> {
    fn prompt_echo_on(
        &mut self,
        _prompt: &CStr,
    ) -> Result<std::ffi::CString, pam_client2::ErrorCode> {
        CString::new(self.username).map_err(|_e| ErrorCode::CONV_ERR)
    }

    fn prompt_echo_off(
        &mut self,
        prompt: &CStr,
    ) -> Result<std::ffi::CString, pam_client2::ErrorCode> {
        let prompt = prompt.to_str().map_err(|_| ErrorCode::CONV_ERR)?;
        self.req_tx
            .send(Req {
                _type: PromptType::EchoOff,
                str: prompt.to_string(),
            })
            .map_err(|_| ErrorCode::CONV_ERR)?;
        self.main_thread.unpark();
        let res = self
            .res_rx
            .recv()
            .map_err(|_| ErrorCode::CONV_ERR)?
            .ok_or(ErrorCode::CONV_ERR)?;
        Ok(res)
    }

    fn radio_prompt(&mut self, prompt: &CStr) -> Result<bool, ErrorCode> {
        let prompt = prompt.to_str().map_err(|_| ErrorCode::CONV_ERR)?;
        self.req_tx
            .send(Req {
                _type: PromptType::YesNo,
                str: prompt.to_string(),
            })
            .map_err(|_| ErrorCode::CONV_ERR)?;
        self.main_thread.unpark();
        let res = self
            .res_rx
            .recv()
            .map_err(|_| ErrorCode::CONV_ERR)?
            .ok_or(ErrorCode::CONV_ERR)?;
        // This string to bool logic is copied from the default impl
        let res = matches!(res.as_bytes_with_nul()[0], b'Y' | b'y' | b'j' | b'J');
        Ok(res)
    }

    fn text_info(&mut self, msg: &CStr) {
        if let Ok(msg) = msg.to_str() {
            self.send_msg(MsgType::Info, self.format_msg(msg));
        }
    }

    fn error_msg(&mut self, msg: &CStr) {
        if let Ok(msg) = msg.to_str() {
            self.send_msg(MsgType::Error, self.format_msg(msg));
        }
    }
}
