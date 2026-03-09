use crate::botty_boss;
use clap::Args;
use std::io;

#[derive(Args, Debug, Default)]
pub struct StopCommand;

impl StopCommand {
    pub fn run(self) -> io::Result<()> {
        botty_boss::stop_all().map_err(|err| {
            io::Error::new(err.kind(), format!("failed to stop Botty processes: {err}"))
        })
    }
}
