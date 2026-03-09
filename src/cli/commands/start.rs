use crate::botty_boss;
use clap::Args;
use std::io;

#[derive(Args, Debug, Default)]
pub struct StartCommand;

impl StartCommand {
    pub fn run(self) -> io::Result<()> {
        if let Ok(true) = botty_boss::is_boss_running() {
            println!("Botty-Boss is already running, skip duplicate start");
            return Ok(());
        }

        if let Err(err) = botty_boss::start_daemon() {
            if err.kind() == io::ErrorKind::AlreadyExists {
                println!("Botty-Boss is already running, skip duplicate start");
                return Ok(());
            }
            return Err(io::Error::new(
                err.kind(),
                format!("failed to start Botty-Boss daemon: {err}"),
            ));
        }

        println!("Botty-Boss started as daemon");
        Ok(())
    }
}
