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
mod frontend;
mod io;
mod llm_provider;
mod skill;

use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();
    let first_arg = args.get(1).map(|s| s.as_str());

    if matches!(first_arg, Some("help" | "-h" | "--help")) {
        print_help();
        return;
    }

    if first_arg == Some("version") {
        println!("mylittlebotty {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    if first_arg == Some("stop") {
        if let Err(err) = botty_boss::stop_all() {
            eprintln!("failed to stop Botty processes: {err}");
            std::process::exit(1);
        }
        return;
    }

    if first_arg == Some("restart") {
        if let Err(err) = botty_boss::restart_all() {
            eprintln!("failed to restart Botty processes: {err}");
            std::process::exit(1);
        }
        return;
    }

    if first_arg == Some("status") {
        if let Err(err) = botty_boss::print_status() {
            eprintln!("failed to query Botty status: {err}");
            std::process::exit(1);
        }
        return;
    }

    if first_arg == Some("update") {
        if let Err(err) = botty_boss::update_self() {
            eprintln!("failed to update mylittlebotty: {err}");
            std::process::exit(1);
        }
        return;
    }

    if first_arg == Some("tui") {
        if let Err(err) = frontend::run("tui") {
            eprintln!("failed to run tui: {err}");
            std::process::exit(1);
        }
        return;
    }

    if first_arg == Some("webui") {
        if let Err(err) = frontend::run("webui") {
            eprintln!("failed to run webui: {err}");
            std::process::exit(1);
        }
        return;
    }

    if first_arg == Some("app") {
        if let Err(err) = frontend::run("app") {
            eprintln!("failed to run app frontend: {err}");
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

    if args.iter().any(|a| a == "--crond") {
        botty_crond::run();
        return;
    }

    if args.iter().any(|a| a == "--boss-daemon") {
        botty_boss::run_supervisor();
        return;
    }

    if let Some(arg) = first_arg {
        if arg.starts_with('-') || arg != "mylittlebotty" {
            eprintln!("unknown command or flag: {arg}\n");
            print_help();
            std::process::exit(1);
        }
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

fn print_help() {
    println!(
        "\
MyLittleBotty {version}

Usage:
  mylittlebotty              Start the Botty-Boss daemon
  mylittlebotty help         Show this help message
  mylittlebotty version      Show the current version
  mylittlebotty status       Show Botty process status
  mylittlebotty stop         Stop Botty processes
  mylittlebotty restart      Restart Botty processes
  mylittlebotty update       Check for updates and self-update
  mylittlebotty tui          Start the TUI frontend
  mylittlebotty webui        Reserved WebUI entry (not implemented)
  mylittlebotty app          Reserved app entry (not implemented)

Options:
  -h, --help                 Show this help message

Internal flags:
  --boss-daemon              Run Botty-Boss supervisor
  --guy                      Run Botty-Guy worker
  --crond                    Run Botty-crond scheduler
  --input-telegram           Run Telegram input worker
  --input-feishu             Run Feishu input worker",
        version = env!("CARGO_PKG_VERSION")
    );
}
