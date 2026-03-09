use crate::botty_boss;
use clap::Args;
use std::io;

#[derive(Args, Debug, Default)]
pub struct UpdateCommand;

impl UpdateCommand {
    pub fn run(self) -> io::Result<()> {
        botty_boss::update_self().map_err(|err| {
            io::Error::new(err.kind(), format!("failed to update mylittlebotty: {err}"))
        })
    }
}
