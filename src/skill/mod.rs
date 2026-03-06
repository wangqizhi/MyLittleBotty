#[path = "buildin-watch.rs"]
pub mod buildin_watch;
#[path = "buildin-crond.rs"]
pub mod buildin_crond;

use std::io;

pub trait BottySkill {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn input_schema_json(&self) -> &'static str;
    fn execute(&self, input_json: &str) -> io::Result<String>;
}
