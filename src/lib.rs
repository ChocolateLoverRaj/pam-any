use std::collections::HashMap;
use std::ffi::CStr;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::thread;

use crate::mode::Mode;
use pam_bindings::constants::PamResultCode::{PAM_AUTH_ERR, PAM_SUCCESS};
use pam_bindings::constants::{PamFlag, PamResultCode};
use pam_bindings::conv::Conv;
use pam_bindings::module::{PamHandle, PamHooks};
use pam_bindings::pam_try;
use pam_client2::{Context, Flag};
use serde::{Deserialize, Serialize};

use crate::pam_any_conversation::PamAnyConversation;
use crate::un_hide_input::un_hide_input;
use crate::unsafe_send::UnsafeSend;

mod mode;
mod pam_any_conversation;
mod un_hide_input;
mod unsafe_send;

struct PamAny;
pam_bindings::pam_hooks!(PamAny);

#[derive(Serialize, Deserialize, Debug)]
struct Input {
    mode: Mode,
    modules: HashMap<String, String>,
}

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

        let conv = match pamh.get_item::<Conv>() {
            Ok(Some(conv)) => conv,
            Ok(None) => todo!(),
            Err(err) => {
                println!("Couldn't get pam_conv");
                return err;
            }
        };
        let conv = Arc::new(Mutex::new(UnsafeSend { conv }));
        let user = pam_try!(pamh.get_user(None));

        let (tx, rx) = channel::<pam_client2::Result<()>>();
        let _handles = input
            .modules
            .iter()
            .map(|(service, service_display_name)| {
                let service = service.to_owned();
                let tx = tx.clone();
                let conv = conv.clone();
                let user = user.clone();
                let service_display_name = service_display_name.to_owned();
                thread::spawn(move || {
                    let mut context = Context::new(
                        &service,
                        Some(&user.clone()),
                        PamAnyConversation {
                            service_display_name,
                            user,
                            conv,
                        },
                    )
                    .unwrap();
                    let result = context.authenticate(Flag::empty());
                    let _ = tx.send(result);
                })
            })
            .collect::<Vec<_>>();
        match input.mode {
            Mode::One => {
                let mut failed_modules = 0;
                for result in rx {
                    if result.is_ok() {
                        un_hide_input().unwrap();
                        return PAM_SUCCESS;
                    } else {
                        failed_modules += 1;
                        if failed_modules == input.modules.len() {
                            un_hide_input().unwrap();
                            return PAM_AUTH_ERR;
                        }
                    }
                }
                PAM_AUTH_ERR
            }
            Mode::All => {
                let mut successful_modules = 0;
                for result in rx {
                    if result.is_ok() {
                        successful_modules += 1;
                        if successful_modules == input.modules.len() {
                            un_hide_input().unwrap();
                            return PAM_SUCCESS;
                        }
                    } else {
                        un_hide_input().unwrap();
                        return PAM_AUTH_ERR;
                    }
                }
                PAM_AUTH_ERR
            }
        }
    }
}
