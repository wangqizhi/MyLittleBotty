use crate::frontend;
use clap::Args;
use std::io;

#[derive(Args, Debug, Default)]
#[command(about = "Reserved WebUI entry (not implemented)")]
pub struct WebuiCommand;

impl WebuiCommand {
    pub fn run(self) -> io::Result<()> {
        frontend::run("webui")
            .map_err(|err| io::Error::new(err.kind(), format!("failed to run webui: {err}")))
    }
}
