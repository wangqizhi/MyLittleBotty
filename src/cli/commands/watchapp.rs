use crate::botty_boss;
use clap::Args;
use std::io;
use std::io::Write;
use std::thread;
use std::time::Duration;

#[derive(Args, Debug, Default)]
pub struct WatchAppCommand {
    #[arg(short = 'n', long = "name")]
    name: String,
}

impl WatchAppCommand {
    pub fn run(self) -> io::Result<()> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "app name is required, use `watchapp -n terminal`",
            ));
        }

        let mut stdout = io::stdout();
        loop {
            let rendered = botty_boss::render_watchapp(name).map_err(|err| {
                io::Error::new(err.kind(), format!("failed to inspect app output: {err}"))
            })?;
            write!(stdout, "\x1b[2J\x1b[H{rendered}")?;
            stdout.flush()?;
            thread::sleep(Duration::from_secs(1));
        }
    }
}
