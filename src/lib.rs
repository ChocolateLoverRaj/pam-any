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

struct PamAny;
pam_bindings::pam_hooks!(PamAny);
impl PamHooks for PamAny {
    fn sm_authenticate(pamh: &mut PamHandle, args: Vec<&CStr>, _flags: PamFlag) -> PamResultCode {
        let arg_string = args
            .iter()
            .map(|s| s.to_str().unwrap())
            .collect::<Vec<_>>()
            .join(" ");
        // println!("Input: {}", arg_string);
        let input = pam_try!(serde_json::from_str::<Input>(&arg_string).map_err(|_e| PAM_AUTH_ERR));
        // println!("Input: {:#?}", input);

        let conv = pam_try!(pam_try!(pamh.get_item::<Conv>()).ok_or(PAM_AUTH_ERR));
        let user = Arc::new(pam_try!(pamh.get_user(None)));

        let (ref msg_tx, msg_rx) = std::sync::mpsc::channel();
        let channels = input
            .modules
            .into_iter()
            .map(|(service, service_display_name)| {
                let user = user.clone();
                let main_thread = thread::current();
                let msg_tx = msg_tx.clone();

                let (req_tx, req_rx) = std::sync::mpsc::sync_channel(1);
                let (res_tx, res_rx) = std::sync::mpsc::sync_channel(1);
                let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
                thread::spawn(move || {
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
                            main_thread.unpark();
                            return;
                        }
                    };
                    let result = context.authenticate(Flag::empty());
                    let _ = result_tx.send(result);
                    main_thread.unpark();
                });
                (req_rx, res_tx, result_rx)
            })
            .collect::<Box<[_]>>();
        // If mode is One we count fails to know if all failed
        // If mode is All we count successes to know if all succeeded
        let mut count = 0;
        loop {
            let mut nothing_to_process = true;

            // Print any messages first
            while let Ok(msg) = msg_rx.try_recv() {
                nothing_to_process = false;
                pam_try!(conv.send(msg._type.style(), &msg.msg));
            }

            // Process results
            for result in channels
                .iter()
                .filter_map(|(_, _, result_rx)| result_rx.try_recv().ok())
            {
                nothing_to_process = false;
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

            // Handle prompts, prioritizing the prompts in order of services as they're listed in the input
            if let Some((req, res_tx)) = channels.iter().find_map(|(req_rx, res_tx, _)| {
                if let Ok(req) = req_rx.try_recv() {
                    nothing_to_process = false;
                    Some((req, res_tx))
                } else {
                    None
                }
            }) {
                let response = pam_try!(conv.send(req._type.style(), &req.str));
                let _ = res_tx.send(response);
            }

            if nothing_to_process {
                // Wait until there is something to process (this (main) thread will be unparked by another thread)
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
                str: self.format_msg(prompt),
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
                str: self.format_msg(prompt),
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
            let _ = self.msg_tx.send(Msg {
                _type: MsgType::Info,
                msg: self.format_msg(msg),
            });
            self.main_thread.unpark();
        }
    }

    fn error_msg(&mut self, msg: &CStr) {
        if let Ok(msg) = msg.to_str() {
            let _ = self.msg_tx.send(Msg {
                _type: MsgType::Error,
                msg: self.format_msg(msg),
            });
            self.main_thread.unpark();
        }
    }
}
