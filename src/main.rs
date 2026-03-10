mod acp;
#[path = "botty/botty-body.rs"]
mod botty_body;
#[path = "botty/botty-boss.rs"]
mod botty_boss;
#[path = "botty/botty-brain.rs"]
mod botty_brain;
#[path = "botty/botty-crond.rs"]
mod botty_crond;
#[path = "botty/botty-guy.rs"]
mod botty_guy;
#[path = "botty/botty-jobs.rs"]
mod botty_jobs;
mod cli;
mod frontend;
mod infra;
mod io;
mod llm_provider;
mod prompt;
mod skill;

fn main() {
    if let Err(err) = cli::run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
