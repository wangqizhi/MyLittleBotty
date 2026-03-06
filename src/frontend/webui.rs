use std::io;

pub fn run() -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "webui frontend is not implemented yet",
    ))
}
