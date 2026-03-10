use crate::botty_boss;
use clap::Args;
use std::io;
use std::io::Write;
use std::thread;
use std::time::Duration;

#[derive(Args, Debug, Default)]
pub struct WatchJobsCommand {
    #[arg(short = 'f', long = "follow")]
    follow: bool,
}

impl WatchJobsCommand {
    pub fn run(self) -> io::Result<()> {
        if !self.follow {
            return botty_boss::print_watchjobs().map_err(|err| {
                io::Error::new(
                    err.kind(),
                    format!("failed to inspect Botty job queues: {err}"),
                )
            });
        }

        let mut stdout = io::stdout();
        loop {
            let rendered = botty_boss::render_watchjobs().map_err(|err| {
                io::Error::new(
                    err.kind(),
                    format!("failed to inspect Botty job queues: {err}"),
                )
            })?;
            write!(stdout, "\x1b[2J\x1b[H{rendered}")?;
            stdout.flush()?;
            thread::sleep(Duration::from_secs(1));
        }
    }
}
