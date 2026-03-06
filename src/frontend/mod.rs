pub mod app;
pub mod frontend_app;
pub mod frontend_service;
pub mod tui;
pub mod webui;

use std::io;

pub fn run(channel: &str) -> io::Result<()> {
    match channel {
        "tui" => tui::run(),
        "webui" => webui::run(),
        "app" => app::run(),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsupported frontend channel: {channel}"),
        )),
    }
}
