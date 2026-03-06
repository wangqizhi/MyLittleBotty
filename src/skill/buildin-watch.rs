use crate::skill::BottySkill;
use serde_json::Value;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const MAX_OUTPUT_BYTES: usize = 16 * 1024;
const WATCH_TOOL_SCHEMA_JSON: &str = "{\"type\":\"object\",\"properties\":{\"path\":{\"type\":\"string\",\"description\":\"Path of the file to read\"}},\"required\":[\"path\"]}";

pub struct BuildinWatchSkill;

impl BuildinWatchSkill {
    pub fn new() -> Self {
        Self
    }
}

impl BottySkill for BuildinWatchSkill {
    fn name(&self) -> &'static str {
        "watch"
    }

    fn description(&self) -> &'static str {
        "Read the content of a text file from the local workspace"
    }

    fn input_schema_json(&self) -> &'static str {
        WATCH_TOOL_SCHEMA_JSON
    }

    fn execute(&self, input_json: &str) -> io::Result<String> {
        let path = parse_path_argument(input_json)?;
        let resolved = resolve_path(&path)?;
        let metadata = fs::metadata(&resolved)?;
        if metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "watch skill only supports files, not directories",
            ));
        }

        let content = fs::read_to_string(&resolved)?;
        let truncated = truncate_utf8(content.as_str(), MAX_OUTPUT_BYTES);
        let mut reply = format!("FILE {}\n{}", resolved.display(), truncated);
        if truncated.len() < content.len() {
            reply.push_str("\n...[truncated]");
        }
        Ok(reply)
    }
}

fn parse_path_argument(input: &str) -> io::Result<String> {
    if let Ok(value) = serde_json::from_str::<Value>(input) {
        if let Some(path) = value.get("path").and_then(Value::as_str) {
            return Ok(path.to_string());
        }
    }
    Ok(input.trim().to_string())
}

fn resolve_path(path: &str) -> io::Result<PathBuf> {
    let candidate = Path::new(path);
    let absolute = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        env::current_dir()?.join(candidate)
    };
    absolute.canonicalize()
}

fn truncate_utf8(content: &str, max_bytes: usize) -> &str {
    if content.len() <= max_bytes {
        return content;
    }

    let mut end = max_bytes;
    while !content.is_char_boundary(end) {
        end -= 1;
    }
    &content[..end]
}
