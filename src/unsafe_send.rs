use std::ops::Deref;
use pam_bindings::conv::Conv;

pub struct UnsafeSend<'a> {
    pub conv: Conv<'a>,
}

impl<'a> Deref for UnsafeSend<'a> {
    type Target = Conv<'a>;

    fn deref(&self) -> &Self::Target {
        &self.conv
    }
}

unsafe impl<'a> Send for UnsafeSend<'a> {}
