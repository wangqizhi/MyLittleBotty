mod commands;

use clap::{Args, Parser, Subcommand};
use std::ffi::OsString;
use std::io;

#[derive(Parser, Debug)]
#[command(
    name = "mylittlebotty",
    version,
    about = "Botty,Botty! Do my order!",
    long_about = None,
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[command(flatten)]
    internal: InternalFlags,
}

#[derive(Subcommand, Debug)]
enum Command {
    Start(commands::start::StartCommand),
    Version(commands::version::VersionCommand),
    WeixinLogin(commands::weixin_login::WeixinLoginCommand),
    Status(commands::status::StatusCommand),
    Crond(commands::crond::CrondCommand),
    Stop(commands::stop::StopCommand),
    Restart(commands::restart::RestartCommand),
    Update(commands::update::UpdateCommand),
    Log(commands::log::LogCommand),
    Watchjobs(commands::watchjobs::WatchJobsCommand),
    Watchapp(commands::watchapp::WatchAppCommand),
    Tui(commands::tui::TuiCommand),
    Webui(commands::webui::WebuiCommand),
    App(commands::app::AppCommand),
}

#[derive(Args, Debug, Default)]
struct InternalFlags {
    #[arg(long, hide = true)]
    boss_daemon: bool,

    #[arg(long, hide = true)]
    guy: bool,

    #[arg(long = "input-telegram", hide = true)]
    input_telegram: bool,

    #[arg(long = "input-feishu", hide = true)]
    input_feishu: bool,

    #[arg(long = "input-weixin", hide = true)]
    input_weixin: bool,

    #[arg(long, hide = true)]
    crond: bool,
}

impl InternalFlags {
    fn dispatch(self) -> bool {
        if self.guy {
            commands::internal::run_guy();
            return true;
        }

        if self.input_telegram {
            commands::internal::run_telegram_input();
            return true;
        }

        if self.input_feishu {
            commands::internal::run_feishu_input();
            return true;
        }

        if self.input_weixin {
            commands::internal::run_weixin_input();
            return true;
        }

        if self.crond {
            commands::internal::run_crond();
            return true;
        }

        if self.boss_daemon {
            commands::internal::run_boss_daemon();
            return true;
        }

        false
    }
}

pub fn run() -> io::Result<()> {
    let cli = Cli::parse_from(normalize_args(std::env::args_os()));

    if cli.internal.dispatch() {
        return Ok(());
    }

    match cli.command.unwrap_or_default() {
        Command::Start(command) => command.run(),
        Command::Version(command) => command.run(),
        Command::WeixinLogin(command) => command.run(),
        Command::Status(command) => command.run(),
        Command::Crond(command) => command.run(),
        Command::Stop(command) => command.run(),
        Command::Restart(command) => command.run(),
        Command::Update(command) => command.run(),
        Command::Log(command) => command.run(),
        Command::Watchjobs(command) => command.run(),
        Command::Watchapp(command) => command.run(),
        Command::Tui(command) => command.run(),
        Command::Webui(command) => command.run(),
        Command::App(command) => command.run(),
    }
}

impl Default for Command {
    fn default() -> Self {
        Self::Start(commands::start::StartCommand::default())
    }
}

fn normalize_args<I>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = OsString>,
{
    let mut normalized = Vec::new();
    let mut previous_was_crond = false;

    for arg in args {
        if previous_was_crond && arg == OsString::from("-list") {
            normalized.push(OsString::from("--list"));
        } else {
            normalized.push(arg.clone());
        }
        previous_was_crond = arg == OsString::from("crond");
    }

    normalized
}
