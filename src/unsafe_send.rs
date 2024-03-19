use std::ops::Deref;
use pam_bindings::conv::Conv;

pub struct UnsafeSend {
    pub conv: Conv<'static>,
}

impl Deref for UnsafeSend{
    type Target = Conv<'static>;

    fn deref(&self) -> &Self::Target {
        &self.conv
    }
}

unsafe impl Send for UnsafeSend{}
