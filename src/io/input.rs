use std::io;

#[allow(dead_code)]
pub trait InputPlugin {
    fn read_message(&mut self) -> io::Result<Option<String>>;
}
