use std::io;

pub trait OutputPlugin {
    fn show_user_message(&mut self, message: &str) -> io::Result<()>;
    fn show_bot_message(&mut self, message: &str) -> io::Result<()>;
    fn show_system_message(&mut self, message: &str) -> io::Result<()>;
}
