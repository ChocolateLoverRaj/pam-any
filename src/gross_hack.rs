use std::sync::{Arc, Mutex};
use std::sync::mpsc::channel;
use std::thread;

use crossbeam_channel::select;
use pam::{Client, PamResult};
use pam_bindings::constants::PamResultCode::{PAM_AUTH_ERR, PAM_SUCCESS};
use pam_bindings::constants::PamResultCode;
use pam_bindings::conv::Conv;

use crate::input::Input;
use crate::message::ConvSendMessage;
use crate::mode::Mode;
use crate::pam_any_conversation::{PamAnyConversation};
use crate::prompt::ConvPrompt;
use crate::un_hide_input::un_hide_input;
use crate::unsafe_send::UnsafeSend;

pub fn gross_hack(input: Input, conv: Conv<'static>, user: String) -> PamResultCode {
    let conv = Arc::new(Mutex::new(UnsafeSend { conv }));

    let (tx, rx) = channel::<PamResult<()>>();
    let _handles = input
        .modules
        .iter()
        .map(|(service, service_display_name)| {
            let service = service.to_owned();
            let tx = tx.clone();
            let conv = conv.clone();
            let user = user.clone();
            let service_display_name = service_display_name.to_owned();
            let (info_rx, prompt_tx, prompt_rx, conversation) =
                PamAnyConversation::new(service_display_name, user);
            thread::spawn(move || {
                thread::spawn(move || loop {
                    let conv = conv.clone();
                    select! {
                    recv(info_rx) -> info => {
                        if let Ok(info) = info {
                            thread::spawn(move || {
                                conv.lock().unwrap().send_message(&info).unwrap();
                            });
                        }
                    },
                    recv(prompt_rx) -> prompt => {
                        if let Ok(prompt) = prompt {
                            let response = conv.lock().unwrap().prompt(&prompt);
                            prompt_tx.send(response).unwrap();
                        }
                    }
                }
                });
                let mut client = Client::with_conversation(&service, conversation).unwrap();
                let result = client.authenticate();
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
                    return PAM_AUTH_ERR;
                }
            }
            PAM_AUTH_ERR
        }
    }
}
