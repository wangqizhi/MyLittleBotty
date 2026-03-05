use std::io;

pub trait TransportPlugin {
    fn request(&mut self, message: &str) -> io::Result<String>;
}
