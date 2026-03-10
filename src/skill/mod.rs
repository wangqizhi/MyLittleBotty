#[path = "buildin-browser.rs"]
pub mod buildin_browser;
#[path = "buildin-crond.rs"]
pub mod buildin_crond;
#[path = "buildin-leader.rs"]
pub mod buildin_leader;
#[path = "buildin-list.rs"]
pub mod buildin_list;
#[path = "buildin-remember.rs"]
pub mod buildin_remember;
#[path = "buildin-terminal.rs"]
pub mod buildin_terminal;
#[path = "buildin-watch.rs"]
pub mod buildin_watch;
#[path = "buildin-write.rs"]
pub mod buildin_write;
#[path = "custom-skill.rs"]
pub mod custom_skill;

use std::io;

use crate::skill::buildin_browser::BuildinBrowserSkill;
use crate::skill::buildin_crond::BuildinCrondSkill;
use crate::skill::buildin_leader::BuildinLeaderSkill;
use crate::skill::buildin_list::BuildinListSkill;
use crate::skill::buildin_remember::BuildinRememberSkill;
use crate::skill::buildin_terminal::BuildinTerminalSkill;
use crate::skill::buildin_watch::BuildinWatchSkill;
use crate::skill::buildin_write::BuildinWriteSkill;
use crate::skill::custom_skill::load_all_custom_skills;

pub trait BottySkill {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn input_schema_json(&self) -> &'static str;
    fn execute(&self, input_json: &str) -> io::Result<String>;
}

pub const BUILDIN_SKILL_NAMES: &[&str] = &[
    "list", "watch", "write", "remember", "crond", "leader", "terminal", "browser",
];

pub fn build_skill(name: &str) -> Option<Box<dyn BottySkill>> {
    match name {
        "terminal" => Some(Box::new(BuildinTerminalSkill::new())),
        "browser" => Some(Box::new(BuildinBrowserSkill::new())),
        "list" => Some(Box::new(BuildinListSkill::new())),
        "watch" => Some(Box::new(BuildinWatchSkill::new())),
        "write" => Some(Box::new(BuildinWriteSkill::new())),
        "remember" => Some(Box::new(BuildinRememberSkill::new())),
        "crond" => Some(Box::new(BuildinCrondSkill::new())),
        "leader" => Some(Box::new(BuildinLeaderSkill::new())),
        _ => build_custom_skill(name),
    }
}

fn build_custom_skill(name: &str) -> Option<Box<dyn BottySkill>> {
    let customs = load_all_custom_skills();
    for skill in customs {
        if skill.skill_name == name {
            return Some(Box::new(skill));
        }
    }
    None
}

pub fn all_available_skill_names() -> Vec<String> {
    let mut names: Vec<String> = BUILDIN_SKILL_NAMES.iter().map(|s| s.to_string()).collect();
    for skill in load_all_custom_skills() {
        if !names.contains(&skill.skill_name) {
            names.push(skill.skill_name);
        }
    }
    names
}
