use clap::Args;
use serde_json::Value;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

const DAYS_TO_SHOW: &str = "10";
const TEXT_PREVIEW_LIMIT: usize = 32;
const CONTENT_PREVIEW_LIMIT: usize = 48;
const MAX_UNTIMESTAMPED_HISTORY_LINES: usize = 200;
const FOLLOW_POLL_INTERVAL: Duration = Duration::from_millis(700);

#[derive(Args, Debug, Default)]
pub struct LogCommand {
    #[arg(short = 'f', long = "follow")]
    follow: bool,
}

impl LogCommand {
    pub fn run(self) -> io::Result<()> {
        let threshold = threshold_timestamp(DAYS_TO_SHOW)?;
        let debug_files = collect_log_files("brain-debug")?;
        let boss_files = collect_log_files("boss")?;

        print_history(&debug_files, &threshold, true)?;
        print_history(&boss_files, &threshold, false)?;

        if self.follow {
            follow_logs(debug_files, boss_files)?;
        }

        Ok(())
    }
}

fn print_history(files: &[PathBuf], threshold: &str, is_debug: bool) -> io::Result<()> {
    for path in files {
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        };
        let lines: Vec<&str> = content.lines().collect();
        let has_line_timestamps = lines.iter().any(|line| split_timestamped_line(line).is_some());

        let header = if is_debug { "debug" } else { "log" };
        println!("== {header}: {} ==", path.display());

        let start_index = if has_line_timestamps {
            0
        } else {
            lines.len().saturating_sub(MAX_UNTIMESTAMPED_HISTORY_LINES)
        };

        if !has_line_timestamps && lines.len() > MAX_UNTIMESTAMPED_HISTORY_LINES {
            println!(
                "{}",
                render_line(
                    path,
                    &format!(
                        "history trimmed to last {MAX_UNTIMESTAMPED_HISTORY_LINES} lines because this file has no per-line timestamp"
                    ),
                    false
                )
            );
        }

        for line in &lines[start_index..] {
            if should_show_line(line, threshold, path, has_line_timestamps)? {
                println!("{}", render_line(path, line, is_debug));
            }
        }
    }

    Ok(())
}

fn follow_logs(debug_files: Vec<PathBuf>, boss_files: Vec<PathBuf>) -> io::Result<()> {
    let mut states = Vec::new();

    for path in debug_files {
        states.push(FollowState::new(path, true)?);
    }
    for path in boss_files {
        states.push(FollowState::new(path, false)?);
    }

    loop {
        for state in &mut states {
            state.poll()?;
        }
        thread::sleep(FOLLOW_POLL_INTERVAL);
    }
}

struct FollowState {
    path: PathBuf,
    is_debug: bool,
    offset: u64,
}

impl FollowState {
    fn new(path: PathBuf, is_debug: bool) -> io::Result<Self> {
        let offset = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        Ok(Self {
            path,
            is_debug,
            offset,
        })
    }

    fn poll(&mut self) -> io::Result<()> {
        let mut file = match File::open(&self.path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(err),
        };

        let len = file.metadata()?.len();
        if len < self.offset {
            self.offset = 0;
        }
        if len == self.offset {
            return Ok(());
        }

        file.seek(SeekFrom::Start(self.offset))?;
        let mut buf = String::new();
        file.read_to_string(&mut buf)?;
        self.offset = len;

        for line in buf.lines() {
            println!("{}", render_line(&self.path, line, self.is_debug));
        }

        Ok(())
    }
}

fn render_line(path: &Path, line: &str, is_debug: bool) -> String {
    if !is_debug {
        return format!("[log:{}] {}", path_label(path), format_log_line(line));
    }

    format!("[debug:{}] {}", path_label(path), format_debug_line(line))
}

fn format_log_line(line: &str) -> String {
    let Some((timestamp, payload)) = split_timestamped_line(line) else {
        return line.to_string();
    };

    format!("[{timestamp}] {}", payload.trim())
}

fn format_debug_line(line: &str) -> String {
    let Some((timestamp, payload)) = split_timestamped_line(line) else {
        return line.to_string();
    };

    if let Some(url) = payload.strip_prefix("request-url: ") {
        return format!("[{timestamp}] request-url {}", url.trim());
    }

    if let Some(json) = payload.strip_prefix("request: ") {
        return format!("[{timestamp}] {}", summarize_debug_request(json));
    }

    if let Some(json) = payload.strip_prefix("response: ") {
        return format!("[{timestamp}] {}", summarize_debug_response(json));
    }

    if let Some(stderr) = payload.strip_prefix("response-stderr: ") {
        return format!(
            "[{timestamp}] response-stderr {}",
            truncate_text(stderr.trim(), CONTENT_PREVIEW_LIMIT)
        );
    }

    line.to_string()
}

fn summarize_debug_request(json: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(json) else {
        return format!("request {}", truncate_text(json, CONTENT_PREVIEW_LIMIT));
    };

    let user_text = value
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|messages| messages.last())
        .and_then(extract_message_preview)
        .unwrap_or_else(|| "-".to_string());

    format!("request text={} ", user_text)
        .trim_end()
        .to_string()
}

fn summarize_debug_response(json: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(json) else {
        return format!("response {}", truncate_text(json, CONTENT_PREVIEW_LIMIT));
    };

    let stop_reason = value
        .get("stop_reason")
        .and_then(Value::as_str)
        .unwrap_or("-");
    let skills = collect_tool_uses(&value);
    let text = extract_assistant_text(&value).unwrap_or_else(|| "-".to_string());

    if skills.is_empty() {
        format!("response stop_reason={stop_reason} text={text}")
    } else {
        format!(
            "response stop_reason={stop_reason} skills=[{}] text={text}",
            skills.join(", ")
        )
    }
}

fn collect_tool_uses(value: &Value) -> Vec<String> {
    let mut skills = Vec::new();
    collect_tool_uses_inner(value, &mut skills);
    skills
}

fn collect_tool_uses_inner(value: &Value, skills: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if map.get("type").and_then(Value::as_str) == Some("tool_use") {
                let name = map.get("name").and_then(Value::as_str).unwrap_or("unknown");
                let input = map.get("input").unwrap_or(&Value::Null);
                skills.push(format!("{name}({})", summarize_json_fields(input)));
            }

            for child in map.values() {
                collect_tool_uses_inner(child, skills);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_tool_uses_inner(item, skills);
            }
        }
        _ => {}
    }
}

fn summarize_json_fields(value: &Value) -> String {
    let Some(map) = value.as_object() else {
        return truncate_text(&value.to_string(), CONTENT_PREVIEW_LIMIT);
    };

    let mut parts = Vec::new();
    for (key, value) in map {
        let rendered = match value {
            Value::String(text) => truncate_text(text, CONTENT_PREVIEW_LIMIT),
            _ => truncate_text(&value.to_string(), CONTENT_PREVIEW_LIMIT),
        };
        parts.push(format!("{key}={rendered}"));
    }
    parts.join(", ")
}

fn extract_assistant_text(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            if map.get("type").and_then(Value::as_str) == Some("text") {
                if let Some(text) = map.get("text").and_then(Value::as_str) {
                    return Some(truncate_text(text, TEXT_PREVIEW_LIMIT));
                }
            }
            for child in map.values() {
                if let Some(text) = extract_assistant_text(child) {
                    return Some(text);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(extract_assistant_text),
        _ => None,
    }
}

fn extract_message_preview(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            if let Some(text) = map.get("text").and_then(Value::as_str) {
                return Some(truncate_text(text, TEXT_PREVIEW_LIMIT));
            }
            if let Some(content) = map.get("content") {
                return extract_message_preview(content);
            }
            None
        }
        Value::Array(items) => items.iter().find_map(extract_message_preview),
        _ => None,
    }
}

fn should_show_line(
    line: &str,
    threshold: &str,
    path: &Path,
    has_line_timestamps: bool,
) -> io::Result<bool> {
    if let Some((timestamp, _)) = split_timestamped_line(line) {
        return Ok(timestamp >= threshold);
    }

    if has_line_timestamps {
        let modified = fs::metadata(path)?.modified()?;
        return Ok(modified >= ten_days_ago_system_time()?);
    }

    let modified = fs::metadata(path)?.modified()?;
    Ok(modified >= ten_days_ago_system_time()?)
}

fn split_timestamped_line(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix('[')?;
    let end = rest.find(']')?;
    let timestamp = &rest[..end];
    let payload = rest[end + 1..].trim_start();
    Some((timestamp, payload))
}

fn collect_log_files(prefix: &str) -> io::Result<Vec<PathBuf>> {
    let dir = botty_log_dir();
    let mut files = Vec::new();

    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(files),
        Err(err) => return Err(err),
    };

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let Some(name) = path.file_name().and_then(OsStr::to_str) else {
            continue;
        };

        if name.starts_with(prefix) && name.ends_with(".log") {
            files.push(path);
        }
    }

    files.sort();
    Ok(files)
}

fn threshold_timestamp(days: &str) -> io::Result<String> {
    let mac_output = std::process::Command::new("date")
        .args(["-v", &format!("-{days}d"), "+%Y-%m-%d %H:%M:%S"])
        .output();

    if let Ok(output) = mac_output {
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
        }
    }

    let gnu_output = std::process::Command::new("date")
        .args(["-d", &format!("{days} days ago"), "+%Y-%m-%d %H:%M:%S"])
        .output()?;

    if !gnu_output.status.success() {
        return Err(io::Error::other("failed to compute log threshold time"));
    }

    Ok(String::from_utf8_lossy(&gnu_output.stdout)
        .trim()
        .to_string())
}

fn ten_days_ago_system_time() -> io::Result<std::time::SystemTime> {
    let now = std::time::SystemTime::now();
    now.checked_sub(Duration::from_secs(10 * 24 * 60 * 60))
        .ok_or_else(|| io::Error::other("failed to compute ten days ago"))
}

fn truncate_text(text: &str, limit: usize) -> String {
    let normalized = text.replace("\\n", " ").replace('\n', " ");
    let mut chars = normalized.chars();
    let preview: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        format!("{preview}...")
    } else {
        preview
    }
}

fn path_label(path: &Path) -> String {
    path.file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("unknown")
        .to_string()
}

fn botty_log_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".mylittlebotty")
        .join("log")
}
