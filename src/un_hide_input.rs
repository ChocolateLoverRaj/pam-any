use std::io;
use termios::{tcsetattr, Termios, ECHO, ICANON, TCSANOW};

pub fn un_hide_input() -> io::Result<()> {
    let mut termios = Termios::from_fd(libc::STDIN_FILENO)?;
    termios.c_lflag |= ECHO | ICANON;
    tcsetattr(libc::STDIN_FILENO, TCSANOW, &termios)?;
    Ok(())
}
