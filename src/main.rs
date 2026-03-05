#[path = "botty/botty-boss.rs"]
mod botty_boss;
#[path = "botty/botty-guy.rs"]
mod botty_guy;

use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.iter().any(|a| a == "--guy") {
        botty_guy::run();
        return;
    }

    if args.iter().any(|a| a == "--boss-daemon") {
        botty_boss::run_supervisor();
        return;
    }

    if let Err(err) = botty_boss::start_daemon() {
        eprintln!("failed to start Botty-Boss daemon: {err}");
        std::process::exit(1);
    }

    println!("Botty-Boss started as daemon");
}
