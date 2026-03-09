use crate::frontend;
use clap::Args;
use std::io;

#[derive(Args, Debug, Default)]
#[command(about = "Reserved app entry (not implemented)")]
pub struct AppCommand;

impl AppCommand {
    pub fn run(self) -> io::Result<()> {
        frontend::run("app")
            .map_err(|err| io::Error::new(err.kind(), format!("failed to run app frontend: {err}")))
    }
}
