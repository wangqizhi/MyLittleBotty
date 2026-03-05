pub mod input;
pub mod output;
pub mod transport;
pub mod tui;

use std::io;

pub fn run(channel: &str) -> io::Result<()> {
    match channel {
        "tui" => tui::run(),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsupported io channel: {channel}"),
        )),
    }
}
