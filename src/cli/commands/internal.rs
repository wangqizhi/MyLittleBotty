use crate::{botty_boss, botty_crond, botty_guy};

pub fn run_boss_daemon() {
    botty_boss::run_supervisor();
}

pub fn run_guy() {
    botty_guy::run();
}

pub fn run_telegram_input() {
    botty_guy::run_telegram_input();
}

pub fn run_feishu_input() {
    botty_guy::run_feishu_input();
}

pub fn run_crond() {
    botty_crond::run();
}
