use crate::botty_brain::BottyBrain;
use crate::llm_provider::{ProviderMessage, ProviderResponse};
use crate::skill::BottySkill;
use serde_json::Value;
use std::env;
use std::io;
use std::path::PathBuf;
use std::process::Command;

const REMEMBER_TOOL_SCHEMA_JSON: &str = r#"{
  "type": "object",
  "properties": {
    "query": {
      "type": "string",
      "description": "The current user topic or question to search in ~/.mylittlebotty/memory/deep"
    }
  },
  "required": ["query"]
}"#;
const KEYWORD_SYSTEM_PROMPT: &str = "Extract 1 to 5 high-signal search keywords or short phrases for retrieving relevant past chat logs. Prefer concrete nouns, project names, file names, commands, and distinctive phrases from the user query. Output plain text only, one keyword or phrase per line, no numbering, no bullets, no explanation.";
const MAX_KEYWORDS: usize = 5;
const GREP_CONTEXT_LINES: usize = 2;
const MAX_SEARCH_OUTPUT_BYTES: usize = 16 * 1024;

pub struct BuildinRememberSkill;

impl BuildinRememberSkill {
    pub fn new() -> Self {
        Self
    }
}

impl BottySkill for BuildinRememberSkill {
    fn name(&self) -> &'static str {
        "remember"
    }

    fn description(&self) -> &'static str {
        "Extract search keywords from the current user topic and search older conversation memory in ~/.mylittlebotty/memory/deep"
    }

    fn input_schema_json(&self) -> &'static str {
        REMEMBER_TOOL_SCHEMA_JSON
    }

    fn execute(&self, input_json: &str) -> io::Result<String> {
        let query = parse_query_argument(input_json)?;
        if query.trim().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "remember tool requires a non-empty query",
            ));
        }

        let keywords = extract_search_keywords(&query)?;
        if keywords.is_empty() {
            return Ok(format!(
                "No search keywords extracted for query: {}",
                query.trim()
            ));
        }

        let search_root = botty_root_dir().join("memory").join("deep");
        if !search_root.exists() {
            return Ok(format!(
                "No related deep memory found for query: {}\nSearched keywords: {}\nMemory directory does not exist: {}",
                query.trim(),
                keywords.join(", "),
                search_root.display()
            ));
        }

        let mut sections = Vec::new();
        for keyword in &keywords {
            let output = search_with_context(&search_root, keyword)?;
            if output.trim().is_empty() {
                continue;
            }
            sections.push(format!("## keyword: {keyword}\n{output}"));
        }

        if sections.is_empty() {
            return Ok(format!(
                "No related deep memory found for query: {}\nSearched keywords: {}",
                query.trim(),
                keywords.join(", ")
            ));
        }

        let body = truncate_utf8(&sections.join("\n\n"), MAX_SEARCH_OUTPUT_BYTES);
        Ok(format!(
            "Related deep memory found for query: {}\nSearched keywords: {}\nContext lines: +/-{}\n\n{}",
            query.trim(),
            keywords.join(", "),
            GREP_CONTEXT_LINES,
            body
        ))
    }
}

fn parse_query_argument(input: &str) -> io::Result<String> {
    let value: Value = serde_json::from_str(input).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("parse remember tool input json failed: {err}"),
        )
    })?;
    let query = value.get("query").and_then(Value::as_str).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "remember tool input requires string field `query`",
        )
    })?;
    Ok(query.to_string())
}

fn extract_search_keywords(query: &str) -> io::Result<Vec<String>> {
    let brain = BottyBrain::from_setup()?;
    let response = brain.think(
        KEYWORD_SYSTEM_PROMPT,
        &[ProviderMessage::UserText(query.to_string())],
        &[],
    )?;
    let text = match response {
        ProviderResponse::Text(reply) => reply.text,
        ProviderResponse::ToolUse(_) => {
            return Err(io::Error::other(
                "keyword extraction unexpectedly returned a tool call",
            ));
        }
    };

    let mut keywords = Vec::new();
    for line in text.lines() {
        let cleaned = clean_keyword_line(line);
        if cleaned.is_empty() {
            continue;
        }
        if keywords.iter().any(|item| item == &cleaned) {
            continue;
        }
        keywords.push(cleaned);
        if keywords.len() >= MAX_KEYWORDS {
            break;
        }
    }
    Ok(keywords)
}

fn clean_keyword_line(line: &str) -> String {
    let trimmed = line.trim();
    let trimmed = trimmed
        .trim_start_matches(|ch: char| ch.is_ascii_digit() || matches!(ch, '.' | '-' | '*' | ')'))
        .trim();
    trimmed
        .trim_matches(|ch: char| matches!(ch, '"' | '\'' | '`'))
        .trim()
        .to_string()
}

fn search_with_context(root: &PathBuf, keyword: &str) -> io::Result<String> {
    match run_rg_search(root, keyword) {
        Ok(output) => Ok(output),
        Err(err) if err.kind() == io::ErrorKind::NotFound => run_grep_search(root, keyword),
        Err(err) => Err(err),
    }
}

fn run_rg_search(root: &PathBuf, keyword: &str) -> io::Result<String> {
    let output = Command::new("rg")
        .arg("-n")
        .arg("-i")
        .arg("-F")
        .arg("-C")
        .arg(GREP_CONTEXT_LINES.to_string())
        .arg("--no-heading")
        .arg("--color")
        .arg("never")
        .arg(keyword)
        .arg(root)
        .output()?;
    parse_search_command_output(output, keyword)
}

fn run_grep_search(root: &PathBuf, keyword: &str) -> io::Result<String> {
    let output = Command::new("grep")
        .arg("-R")
        .arg("-n")
        .arg("-i")
        .arg("-F")
        .arg("-C")
        .arg(GREP_CONTEXT_LINES.to_string())
        .arg(keyword)
        .arg(root)
        .output()?;
    parse_search_command_output(output, keyword)
}

fn parse_search_command_output(output: std::process::Output, keyword: &str) -> io::Result<String> {
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }

    if output.status.code() == Some(1) {
        return Ok(String::new());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(io::Error::other(format!(
        "search deep memory failed for keyword `{keyword}`: {stderr}"
    )))
}

fn truncate_utf8(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }

    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...[truncated]", &text[..end])
}

fn botty_root_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".mylittlebotty")
}
