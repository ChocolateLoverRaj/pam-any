use std::collections::HashMap;
use std::ffi::{CStr, CString};
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

fn dup_fd(fd: libc::c_int) -> Option<libc::c_int> {
    let fd = unsafe { libc::dup(fd) };
    if fd >= 0 { Some(fd) } else { None }
}

fn read_password_interruptible(tty_fd: libc::c_int, notify_fd: libc::c_int, prompt: &str) -> Option<CString> {
    unsafe {
        libc::write(
            tty_fd,
            prompt.as_ptr() as *const libc::c_void,
            prompt.len(),
        );

        let mut orig: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(tty_fd, &mut orig) != 0 {
            return None;
        }

        // ECHO/ICANON off for raw single-byte reads; ISIG off so Ctrl-C is byte 0x03, not a real SIGINT.
        let mut raw = orig;
        raw.c_lflag &= !(libc::ECHO | libc::ICANON | libc::ISIG);
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        if libc::tcsetattr(tty_fd, libc::TCSANOW, &raw) != 0 {
            // Couldn't disable echo — bail out rather than risk echoing the password
            return None;
        }

        let nfds = tty_fd.max(notify_fd) + 1;
        let mut input: Vec<u8> = Vec::new();
        let result;

        'read: loop {
            let mut rfds: libc::fd_set = std::mem::zeroed();
            libc::FD_ZERO(&mut rfds);
            libc::FD_SET(tty_fd, &mut rfds);
            libc::FD_SET(notify_fd, &mut rfds);

            if libc::select(
                nfds,
                &mut rfds,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ) <= 0
            {
                result = None;
                break;
            }

            if libc::FD_ISSET(notify_fd, &rfds) {
                // Another auth method succeeded — erase the prompt line
                let clear = b"\r\x1b[2K";
                libc::write(tty_fd, clear.as_ptr() as *const libc::c_void, clear.len());
                // Drain the pipe so it doesn't fire again
                let mut drain = [0u8; 64];
                let _ = libc::read(notify_fd, drain.as_mut_ptr() as *mut libc::c_void, drain.len());
                result = None;
                break;
            }

            if libc::FD_ISSET(tty_fd, &rfds) {
                let mut b = 0u8;
                if libc::read(tty_fd, &mut b as *mut u8 as *mut libc::c_void, 1) <= 0 {
                    result = None;
                    break;
                }
                match b {
                    b'\n' | b'\r' => {
                        libc::write(tty_fd, b"\n".as_ptr() as *const libc::c_void, 1);
                        result = CString::new(input).ok();
                        break 'read;
                    }
                    0x7f | 0x08 => {
                        input.pop(); // backspace / DEL
                    }
                    0x03 | 0x04 => {
                        result = None; // Ctrl-C / Ctrl-D
                        break;
                    }
                    c => input.push(c),
                }
            }
        }

        libc::tcsetattr(tty_fd, libc::TCSANOW, &orig);
        result
    }
}

// Closes on every return path, including early pam_try! exits.
struct FdGuard([Option<libc::c_int>; 2]);
impl Drop for FdGuard {
    fn drop(&mut self) {
        for fd in self.0.into_iter().flatten() {
            unsafe {
                libc::close(fd);
            }
        }
    }
}

struct PamAny;
pam_bindings::pam_hooks!(PamAny);
impl PamHooks for PamAny {
    fn sm_authenticate(pamh: &mut PamHandle, args: Vec<&CStr>, _flags: PamFlag) -> PamResultCode {
        // A pipe write after its reader already closed must not kill the host process via the default SIGPIPE disposition.
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        }

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
            input.modules.values().cloned().collect::<Vec<_>>().join(" or ")
        );

        // Falls back to conv.send if /dev/tty is unavailable (e.g. non-interactive sessions).
        let tty_fd: Option<libc::c_int> = unsafe {
            let fd = libc::open(b"/dev/tty\0".as_ptr() as *const libc::c_char, libc::O_RDWR | libc::O_CLOEXEC);
            if fd >= 0 { Some(fd) } else { None }
        };

        // Notification pipe: each thread gets a dup'd write end. Writing to it wakes
        // the main thread out of select() when a result arrives.
        let (notify_r, notify_w): (Option<libc::c_int>, Option<libc::c_int>) =
            if tty_fd.is_some() {
                let mut fds = [-1i32; 2];
                if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } == 0 {
                    (Some(fds[0]), Some(fds[1]))
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            };
        let _fd_guard = FdGuard([tty_fd, notify_r]);

        let mode = input.mode;
        let (ref msg_tx, msg_rx) = std::sync::mpsc::channel();
        let channels = input
            .modules
            .into_iter()
            .map(|(service, service_display_name)| {
                let user = user.clone();
                let main_thread = thread::current();
                let msg_tx = msg_tx.clone();

                // Each thread owns a dup'd copy of the write end so it can signal independently
                let thread_notify_w: Option<libc::c_int> = notify_w.and_then(dup_fd);

                let (req_tx, req_rx) = std::sync::mpsc::sync_channel(1);
                let (res_tx, res_rx) = std::sync::mpsc::sync_channel(1);
                let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
                thread::spawn(move || {
                    // Only signals when this result decides the overall outcome, so a fast-failing service can't cancel a still-running one's prompt.
                    let pipe_signal = |ok: bool, fd: Option<libc::c_int>| {
                        let should_write = match mode {
                            Mode::One => ok,
                            Mode::All => !ok,
                        };
                        if let Some(fd) = fd {
                            unsafe {
                                if should_write {
                                    libc::write(fd, b"x".as_ptr() as *const libc::c_void, 1);
                                }
                                libc::close(fd);
                            }
                        }
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

        // Close the master write end — threads own their dup'd copies.
        // Once all threads finish and close their copies, notify_r becomes readable with EOF.
        if let Some(w) = notify_w {
            unsafe { libc::close(w); }
        }

        // If mode is One we count fails to know if all failed
        // If mode is All we count successes to know if all succeeded
        let mut count = 0;
        let mut prompt_responded = false;
        loop {
            let mut nothing_to_process = true;

            // Check results first — a success here must not be delayed by pending messages
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

            // Drain all sub-service messages silently — text_info is redundant since the
            // combined prompt already describes every available method, and error_msg is
            // handled by the outer PAM stack (pam_faillock, sudo, etc.)
            while msg_rx.try_recv().is_ok() {
                nothing_to_process = false;
            }

            if !prompt_responded {
                if let Some((req, res_tx)) = channels.iter().find_map(|(req_rx, res_tx, _)| {
                    if let Ok(req) = req_rx.try_recv() {
                        nothing_to_process = false;
                        Some((req, res_tx))
                    } else {
                        None
                    }
                }) {
                    let response = match (&req._type, tty_fd, notify_r) {
                        // Echo-off: bypass conv.send, read from tty with select() so another
                        // method's success can interrupt the read without waiting for Enter
                        (PromptType::EchoOff, Some(tty), Some(nfd)) => {
                            read_password_interruptible(tty, nfd, &prompt)
                        }
                        // YesNo or no tty: fall back to PAM conversation (blocking)
                        _ => pam_try!(conv.send(req._type.style(), &prompt)),
                    };
                    let _ = res_tx.send(response);
                    prompt_responded = true;
                }
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
