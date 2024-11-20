use std::ffi::{CStr, CString};

use crossbeam_channel::{bounded, Receiver, Sender, unbounded};
use pam::Conversation;

use crate::message::{Message, MessageType};
use crate::prompt::{PromptInput, PromptOutput, PromptType};

pub struct PamAnyConversation {
    service_display_name: String,
    user: String,
    info_tx: Sender<Message>,
    prompt_input: Sender<PromptInput>,
    prompt_output: Receiver<PromptOutput>,
}

impl PamAnyConversation {
    pub fn new(
        service_display_name: String,
        user: String,
    ) -> (
        Receiver<Message>,
        Sender<PromptOutput>,
        Receiver<PromptInput>,
        Self,
    ) {
        let (info_tx, info_rx) = unbounded();
        let (prompt_input_tx, prompt_input_rx) = bounded(1);
        let (prompt_output_tx, prompt_output_rx) = bounded(1);
        (
            info_rx,
            prompt_output_tx,
            prompt_input_rx,
            Self {
                service_display_name,
                user,
                info_tx,
                prompt_input: prompt_input_tx,
                prompt_output: prompt_output_rx,
            },
        )
    }

    fn message(&mut self, msg: &CStr, message_type: MessageType) {
        let msg = msg.to_str().map_err(|_e| ()).unwrap();
        let msg = format!("[{}] {}", self.service_display_name, msg);
        self.info_tx
            .send(Message {
                message_type,
                message: msg,
            })
            .unwrap();
    }
}

impl Conversation for PamAnyConversation {
    fn prompt_echo(&mut self, _msg: &CStr) -> Result<CString, ()> {
        CString::new(&*self.user).map_err(|_e| ())
    }

    fn prompt_blind(&mut self, msg: &CStr) -> Result<CString, ()> {
        let msg = msg.to_str().map_err(|_e| ())?;
        let msg = format!("[{}] {}", self.service_display_name, msg);
        self.prompt_input
            .send(PromptInput {
                prompt_type: PromptType::Blind,
                message: msg,
            })
            .map_err(|_e| ())?;
        let response = self
            .prompt_output
            .recv()
            .map_err(|_e| ())?
            .map_err(|_e| ())?;
        Ok(response)
    }

    fn info(&mut self, msg: &CStr) {
        self.message(msg, MessageType::Info)
    }

    fn error(&mut self, msg: &CStr) {
        self.message(msg, MessageType::Error)
    }
}
