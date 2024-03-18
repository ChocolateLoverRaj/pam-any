#[derive(Debug, Copy, Clone)]
pub enum Mode {
    One,
    All
}

impl Mode {
    pub fn from_str(str: &str) -> Result<Mode, ()> {
        match str {
            "one" => Ok(Mode::One),
            "all"=> Ok(Mode::All),
            _ => Err(())
        }
    }
}
