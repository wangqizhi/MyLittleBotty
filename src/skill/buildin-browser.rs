use crate::acp;
use crate::skill::BottySkill;
use std::io;

pub struct BuildinBrowserSkill;

impl BuildinBrowserSkill {
    pub fn new() -> Self {
        Self
    }
}

impl BottySkill for BuildinBrowserSkill {
    fn name(&self) -> &'static str {
        "browser"
    }

    fn description(&self) -> &'static str {
        "Control a Chrome browser through the DevTools remote debugging protocol. Use it to open pages, inspect page content, click with real mouse events, focus inputs, type text via keyboard insertion, press keys, run page JavaScript, wait for UI changes, and capture screenshots."
    }

    fn input_schema_json(&self) -> &'static str {
        r#"{"type":"object","properties":{"action":{"type":"string","description":"One of: start, list, status, transcript, navigate, snapshot, click, focus, fill, press, eval, wait_for, screenshot, close"},"session_id":{"type":"string","description":"Existing browser session id for follow-up actions"},"url":{"type":"string","description":"URL to open for navigate"},"locator":{"type":"string","description":"Locator for click/focus/fill/wait_for. Supports CSS selectors, @ref from snapshot output, and prefixes like css=..., text=..., placeholder=..., aria=..., label=..., role=.... For wait_for this is optional: if omitted, the browser just waits for timeout_seconds and then returns a fresh snapshot."},"text":{"type":"string","description":"Text to type for fill after focusing the target element"},"key":{"type":"string","description":"Key to press for press, e.g. Enter, Tab, Escape, Backspace, ArrowDown, ArrowUp"},"script":{"type":"string","description":"JavaScript expression/body for eval"},"output_path":{"type":"string","description":"Optional screenshot file path"},"timeout_seconds":{"type":"integer","description":"Optional timeout in seconds for wait_for"},"headless":{"type":"boolean","description":"Optional override for whether Chrome should launch headless when creating a session"}},"required":["action"]}"#
    }

    fn execute(&self, input_json: &str) -> io::Result<String> {
        acp::handle_browser_skill_request(input_json)
    }
}
