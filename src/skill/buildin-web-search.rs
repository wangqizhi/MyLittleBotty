use crate::acp;
use crate::skill::BottySkill;
use std::io;

pub struct BuildinWebSearchSkill;

impl BuildinWebSearchSkill {
    pub fn new() -> Self {
        Self
    }
}

impl BottySkill for BuildinWebSearchSkill {
    fn name(&self) -> &'static str {
        "web-search"
    }

    fn description(&self) -> &'static str {
        "Run deterministic web search in Chrome using direct Google, Bing, and Baidu results URLs, with default fallback order Google -> Bing -> Baidu. Only use the built-in X Grok flow when the user explicitly asks to search on X, Twitter, x.com, or Grok."
    }

    fn input_schema_json(&self) -> &'static str {
        r#"{"type":"object","properties":{"query":{"type":"string","description":"Search keywords or the user's natural search request. If it explicitly mentions X, Twitter, x.com, Grok, or 推特, the skill will route to the X Grok search flow when engine is omitted."},"engine":{"type":"string","description":"Optional override: google, bing, baidu, x, grok, all, or a comma-separated subset"},"max_results":{"type":"integer","description":"Optional max results per engine, 1-10"},"timeout_seconds":{"type":"integer","description":"Optional timeout waiting for results"},"headless":{"type":"boolean","description":"Optional override for whether Chrome should launch headless when creating a session"}},"required":["query"]}"#
    }

    fn execute(&self, input_json: &str) -> io::Result<String> {
        acp::handle_web_search_skill_request(input_json)
    }
}
