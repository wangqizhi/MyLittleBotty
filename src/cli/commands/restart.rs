use crate::botty_boss;
use clap::Args;
use std::io;

#[derive(Args, Debug, Default)]
pub struct RestartCommand;

impl RestartCommand {
    pub fn run(self) -> io::Result<()> {
        botty_boss::restart_all().map_err(|err| {
            io::Error::new(
                err.kind(),
                format!("failed to restart Botty processes: {err}"),
            )
        })
    }
}
