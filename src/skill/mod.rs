#[path = "buildin-crond.rs"]
pub mod buildin_crond;
#[path = "buildin-leader.rs"]
pub mod buildin_leader;
#[path = "buildin-list.rs"]
pub mod buildin_list;
#[path = "buildin-remember.rs"]
pub mod buildin_remember;
#[path = "buildin-watch.rs"]
pub mod buildin_watch;
#[path = "buildin-write.rs"]
pub mod buildin_write;

use std::io;

use crate::skill::buildin_crond::BuildinCrondSkill;
use crate::skill::buildin_leader::BuildinLeaderSkill;
use crate::skill::buildin_list::BuildinListSkill;
use crate::skill::buildin_remember::BuildinRememberSkill;
use crate::skill::buildin_watch::BuildinWatchSkill;
use crate::skill::buildin_write::BuildinWriteSkill;

pub trait BottySkill {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn input_schema_json(&self) -> &'static str;
    fn execute(&self, input_json: &str) -> io::Result<String>;
}

pub fn build_skill(name: &str) -> Option<Box<dyn BottySkill>> {
    match name {
        "list" => Some(Box::new(BuildinListSkill::new())),
        "watch" => Some(Box::new(BuildinWatchSkill::new())),
        "write" => Some(Box::new(BuildinWriteSkill::new())),
        "remember" => Some(Box::new(BuildinRememberSkill::new())),
        "crond" => Some(Box::new(BuildinCrondSkill::new())),
        "leader" => Some(Box::new(BuildinLeaderSkill::new())),
        _ => None,
    }
}
