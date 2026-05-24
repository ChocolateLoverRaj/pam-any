use crate::unsafe_send::UnsafeSend;
use pam::{Conversation, ffi::PAM_ERROR_MSG};
use pam_bindings::constants::{PAM_PROMPT_ECHO_OFF, PAM_TEXT_INFO};
use std::{
    ffi::{CStr, CString},
    sync::{Arc, Mutex},
    thread,
};

pub struct PamAnyConversation {
    pub service_display_name: String,
    pub user: String,
    pub conv: Arc<Mutex<UnsafeSend>>,
}

impl Conversation for PamAnyConversation {
    fn prompt_echo(&mut self, _msg: &CStr) -> Result<CString, ()> {
        CString::new(&*self.user).map_err(|_e| ())
    }

    fn prompt_blind(&mut self, msg: &CStr) -> Result<CString, ()> {
        let msg = msg.to_str().map_err(|_e| ())?;
        let conv = self.conv.lock().map_err(|_e| ())?;
        let response = conv
            .send(
                PAM_PROMPT_ECHO_OFF,
                &format!("[{}] {}", self.service_display_name, msg),
            )
            .map_err(|_e| ())?;
        response.ok_or(())
    }

    fn info(&mut self, msg: &CStr) {
        let msg = msg.to_str().map_err(|_e| ()).unwrap();
        let msg = format!("[{}] {}", self.service_display_name, msg);
        let conv = self.conv.clone();
        thread::spawn(move || {
            conv.lock().unwrap().send(PAM_TEXT_INFO, &msg).unwrap();
        });
    }

    fn error(&mut self, msg: &CStr) {
        let msg = msg.to_str().map_err(|_e| ()).unwrap();
        let msg = format!("[{}] {}", self.service_display_name, msg);
        let conv = self.conv.clone();
        thread::spawn(move || {
            conv.lock().unwrap().send(PAM_ERROR_MSG, &msg).unwrap();
        });
    }
}
