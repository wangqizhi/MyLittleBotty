#[path = "botty/botty-boss.rs"]
mod botty_boss;
#[path = "botty/botty-guy.rs"]
mod botty_guy;
mod io;

use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.get(1).map(|s| s.as_str()) == Some("version") {
        println!("mylittlebotty {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    if args.get(1).map(|s| s.as_str()) == Some("stop") {
        if let Err(err) = botty_boss::stop_all() {
            eprintln!("failed to stop Botty processes: {err}");
            std::process::exit(1);
        }
        return;
    }

    if args.get(1).map(|s| s.as_str()) == Some("restart") {
        if let Err(err) = botty_boss::restart_all() {
            eprintln!("failed to restart Botty processes: {err}");
            std::process::exit(1);
        }
        return;
    }

    if args.get(1).map(|s| s.as_str()) == Some("status") {
        if let Err(err) = botty_boss::print_status() {
            eprintln!("failed to query Botty status: {err}");
            std::process::exit(1);
        }
        return;
    }

    if args.get(1).map(|s| s.as_str()) == Some("update") {
        if let Err(err) = botty_boss::update_self() {
            eprintln!("failed to update mylittlebotty: {err}");
            std::process::exit(1);
        }
        return;
    }

    if args.get(1).map(|s| s.as_str()) == Some("tui") {
        if let Err(err) = io::run("tui") {
            eprintln!("failed to run tui: {err}");
            std::process::exit(1);
        }
        return;
    }

    if args.iter().any(|a| a == "--guy") {
        botty_guy::run();
        return;
    }

    if args.iter().any(|a| a == "--input-telegram") {
        botty_guy::run_telegram_input();
        return;
    }

    if args.iter().any(|a| a == "--input-feishu") {
        botty_guy::run_feishu_input();
        return;
    }

    if args.iter().any(|a| a == "--boss-daemon") {
        botty_boss::run_supervisor();
        return;
    }

    if let Ok(true) = botty_boss::is_boss_running() {
        println!("Botty-Boss is already running, skip duplicate start");
        return;
    }

    if let Err(err) = botty_boss::start_daemon() {
        if err.kind() == std::io::ErrorKind::AlreadyExists {
            println!("Botty-Boss is already running, skip duplicate start");
            return;
        }
        eprintln!("failed to start Botty-Boss daemon: {err}");
        std::process::exit(1);
    }

    println!("Botty-Boss started as daemon");
}
