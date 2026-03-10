use crate::acp;
use crate::skill::BottySkill;
use std::io;

pub struct BuildinTerminalSkill;

impl BuildinTerminalSkill {
    pub fn new() -> Self {
        Self
    }
}

impl BottySkill for BuildinTerminalSkill {
    fn name(&self) -> &'static str {
        "terminal"
    }

    fn description(&self) -> &'static str {
        "Interact with the terminal app that runs a coding agent in a PTY. Use it to execute a coding task end to end, inspect transcript output, interrupt, terminate, or continue a terminal session."
    }

    fn input_schema_json(&self) -> &'static str {
        r#"{"type":"object","properties":{"action":{"type":"string","description":"One of: execute_task, continue_session, status, transcript, interrupt, terminate, restart, list"},"session_id":{"type":"string","description":"Existing terminal session id for follow-up actions"},"provider":{"type":"string","description":"Terminal agent provider to start or restart. Currently codex is supported and claude is reserved."},"prompt":{"type":"string","description":"Task or follow-up input to send to the terminal agent"},"wait_seconds":{"type":"integer","description":"Optional wait before returning status or transcript. Polling-based execute_task ignores this and uses the built-in 10 second loop."}},"required":["action"]}"#
    }

    fn execute(&self, input_json: &str) -> io::Result<String> {
        acp::handle_skill_request(input_json)
    }
}
