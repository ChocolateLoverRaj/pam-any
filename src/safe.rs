use std::thread;

use crossbeam_channel::{Select, unbounded};
use pam::{Client, PamResult};
use pam_bindings::constants::PamResultCode;
use pam_bindings::constants::PamResultCode::{PAM_AUTH_ERR, PAM_SUCCESS};
use pam_bindings::conv::Conv;

use crate::input::Input;
use crate::message::ConvSendMessage;
use crate::mode::Mode;
use crate::pam_any_conversation::PamAnyConversation;
use crate::prompt::ConvPrompt;

pub fn safe(input: Input, conv: Conv<'static>, user: String) -> PamResultCode {
    let (result_tx, result_rx) = unbounded::<PamResult<()>>();
    let sub_modules = input.modules.into_iter().map(|(service, service_display_name)| {
        let result_tx = result_tx.clone();
        let (specific_info_rx, prompt_tx, prompt_rx, conversation) =
            PamAnyConversation::new(service_display_name.to_owned(), user.clone());
        (specific_info_rx, prompt_tx, prompt_rx, thread::spawn(move || {
            let mut client = Client::with_conversation(&service, conversation).unwrap();
            let result = client.authenticate();
            let _ = result_tx.send(result);
        }))
    }).collect::<Vec<_>>();
    let mut results = 0;
    loop {
        let mut select = Select::new();
        let operations = sub_modules.iter().map(|(info_rx, prompt_tx, prompt_rx, _handle)| {
            (select.recv(&info_rx), info_rx.to_owned(), select.recv(&prompt_rx), prompt_rx.to_owned(), prompt_tx.to_owned())
        }).collect::<Vec<_>>();
        let result_operation = select.recv(&result_rx);
        let operation = select.select();
        if operation.index() == result_operation {
            let result = operation.recv(&result_rx).unwrap();
            match input.mode {
                Mode::All => {
                    if result.is_ok() {
                        results += 1;
                        if results == sub_modules.len() {
                            return PAM_SUCCESS
                        }
                    } else {
                        return PAM_AUTH_ERR
                    }
                },
                Mode::One => {
                    if result.is_ok() {
                        return PAM_SUCCESS
                    } else {
                        results += 1;
                        if results == sub_modules.len() {
                            return PAM_AUTH_ERR
                        }
                    }
                }
            }
        } else {
            for (info_index, info_rx, prompt_index, prompt_rx, prompt_tx) in operations {
                match operation.index() {
                    i if i == info_index => {
                        let message = operation.recv(&info_rx).unwrap();
                        conv.send_message(&message).unwrap();
                        break;
                    },
                    i if i == prompt_index => {
                        println!("Yeet {} {}", i, prompt_index);
                        let prompt = operation.recv(&prompt_rx).unwrap();
                        let response = conv.prompt(&prompt);
                        prompt_tx.send(response).unwrap();
                        break;
                    },
                    _ => {}
                }
            }
            println!("no match!");
        }
    }
}
