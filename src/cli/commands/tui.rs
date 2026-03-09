use crate::frontend;
use clap::Args;
use std::io;

#[derive(Args, Debug, Default)]
pub struct TuiCommand;

impl TuiCommand {
    pub fn run(self) -> io::Result<()> {
        frontend::run("tui")
            .map_err(|err| io::Error::new(err.kind(), format!("failed to run tui: {err}")))
    }
}
