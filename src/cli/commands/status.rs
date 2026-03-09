use crate::botty_boss;
use clap::Args;
use std::io;

#[derive(Args, Debug, Default)]
pub struct StatusCommand;

impl StatusCommand {
    pub fn run(self) -> io::Result<()> {
        botty_boss::print_status().map_err(|err| {
            io::Error::new(err.kind(), format!("failed to query Botty status: {err}"))
        })
    }
}
