use clap::Args;
use std::io;

#[derive(Args, Debug, Default)]
pub struct VersionCommand;

impl VersionCommand {
    pub fn run(self) -> io::Result<()> {
        println!("mylittlebotty {}", env!("CARGO_PKG_VERSION"));
        Ok(())
    }
}
