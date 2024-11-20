use crate::mode::Mode;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Debug)]
pub struct Input {
    pub mode: Mode,
    pub modules: HashMap<String, String>,
    pub gross_hack: bool
}
