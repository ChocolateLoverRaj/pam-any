use std::ffi::CStr;

use pam_bindings::constants::PamResultCode::PAM_AUTH_ERR;
use pam_bindings::constants::{PamFlag, PamResultCode};
use pam_bindings::conv::Conv;
use pam_bindings::module::{PamHandle, PamHooks};
use pam_bindings::pam_try;

use crate::gross_hack::gross_hack;
use crate::input::Input;
use crate::safe::safe;

mod gross_hack;
mod input;
mod mode;
mod pam_any_conversation;
mod un_hide_input;
mod unsafe_send;
mod safe;
mod message;
mod prompt;

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

        let conv = match pamh.get_item::<Conv>() {
            Ok(Some(conv)) => conv,
            Ok(None) => todo!(),
            Err(err) => {
                println!("Couldn't get pam_conv");
                return err;
            }
        };
        let user = pam_try!(pamh.get_user(None));

        match input.gross_hack {
            true => gross_hack(input, conv, user),
            false => safe(input, conv, user)
        }
    }
}
