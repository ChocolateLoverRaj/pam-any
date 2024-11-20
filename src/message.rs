use pam::ffi::PAM_TEXT_INFO;
use pam_bindings::constants::PAM_ERROR_MSG;
use pam_bindings::conv::Conv;
use pam_bindings::module::PamResult;

pub enum MessageType {
    Info,
    Error,
}

pub struct Message {
    pub message_type: MessageType,
    pub message: String,
}

pub trait ConvSendMessage {
    fn send_message(&self, message: &Message) -> PamResult<()>;
}

impl<'a> ConvSendMessage for Conv<'a> {
    fn send_message(&self, message: &Message) -> PamResult<()> {
        self.send(match message.message_type {
            MessageType::Info => PAM_TEXT_INFO,
            MessageType::Error => PAM_ERROR_MSG
        }, &message.message)?;
        Ok(())
    }
}