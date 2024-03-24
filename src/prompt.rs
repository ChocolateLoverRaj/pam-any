use std::ffi::CString;
use pam_bindings::constants::{PAM_PROMPT_ECHO_OFF, PAM_PROMPT_ECHO_ON};
use pam_bindings::conv::Conv;
use pam_bindings::module::PamResult;

pub enum PromptType {
    Echo,
    Blind,
}

pub struct PromptInput {
    pub prompt_type: PromptType,
    pub message: String,
}

pub type PromptOutput = PamResult<CString>;

pub trait ConvPrompt {
    fn prompt(&self, prompt_input: &PromptInput) -> PromptOutput;
}

impl<'a> ConvPrompt for Conv<'a> {
    fn prompt(&self, prompt_input: &PromptInput) -> PromptOutput {
        self.send(match prompt_input.prompt_type {
            PromptType::Echo => PAM_PROMPT_ECHO_ON,
            PromptType::Blind => PAM_PROMPT_ECHO_OFF
        }, &prompt_input.message).map(|response| response.unwrap().to_owned())
    }
}