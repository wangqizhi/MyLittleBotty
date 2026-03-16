use crate::botty_crond;
use clap::Args;
use std::io;

#[derive(Args, Debug, Default)]
pub struct CrondCommand {
    #[arg(short = 'l', long = "list", help = "List current scheduled reminders")]
    list: bool,

    #[arg(
        short = 'a',
        long = "all",
        help = "Include non-pending reminders when used with -list"
    )]
    all: bool,
}

impl CrondCommand {
    pub fn run(self) -> io::Result<()> {
        if self.all && !self.list {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "`-a` / `--all` must be used together with `-list`",
            ));
        }
        println!("{}", botty_crond::list_reminders(self.all)?);
        Ok(())
    }
}
