use crate::botty_brain::BottyBrain;
use crate::llm_provider::{
    ProviderMessage, ProviderResponse, ProviderToolDefinition, ProviderToolUse,
};
use crate::skill::buildin_crond::BuildinCrondSkill;
use crate::skill::buildin_watch::BuildinWatchSkill;
use crate::skill::BottySkill;
use serde_json::Value;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

const TOOL_SYSTEM_PROMPT: &str = "You are Botty. You can use local tools. Use watch when the user asks to inspect, open, read, or show a file. Use crond when the user asks to query, create, or edit reminders or scheduled tasks. For crond create/edit, always provide schedule_at in exact local format YYYY-MM-DD HH:MM:SS and choose task_type precisely. Only use crond for scheduling-related requests. If you use a tool, rely on the tool result to answer the user. Do not describe a tool call to the user.";
const DEEP_MEMORY_CONTEXT_ROUNDS: usize = 10;
const REMEMBER_MAX_LINES: usize = 100;
const REMEMBER_SYSTEM_PROMPT: &str = "You maintain long-term memory for Botty. Output Markdown only. Keep the final remember.md within 100 lines. Focus only on key events, the user's recent important requests, and what has been solved, is pending, or changed. Omit trivial chat. Compress aggressively and prefer recent, actionable context. When needed, forget older low-value details.";
const REMINDER_TRIGGER_COMMAND: &str = "/reminder-now";
const REMINDER_SYSTEM_PROMPT: &str = "You are Botty. A scheduled reminder is due right now. Do not use conversation history or long-term memory. Only use the reminder payload provided in this request. Reply with the reminder message that should be sent to the user now.";

pub struct AssistantReply {
    pub text: String,
    pub thinking: Option<String>,
}

pub struct BottyBody {
    brain: BottyBrain,
    skills: Vec<Box<dyn BottySkill>>,
}

impl BottyBody {
    pub fn from_setup() -> io::Result<Self> {
        Ok(Self {
            brain: BottyBrain::from_setup()?,
            skills: vec![
                Box::new(BuildinWatchSkill::new()),
                Box::new(BuildinCrondSkill::new()),
            ],
        })
    }

    pub fn think(&self, input: &str) -> io::Result<AssistantReply> {
        if let Some((name, argument)) = parse_debug_tool_call(input) {
            return Ok(AssistantReply {
                text: self.execute_tool(name, &argument)?,
                thinking: None,
            });
        }
        if let Some(payload) = parse_special_command_argument(input, REMINDER_TRIGGER_COMMAND) {
            return Ok(AssistantReply {
                text: self.think_due_reminder(payload)?,
                thinking: None,
            });
        }
        if matches_special_command(input, "/remember") {
            return Ok(AssistantReply {
                text: self.remember_deep_memory()?,
                thinking: None,
            });
        }

        let tools = self.tool_definitions();
        let system_prompt = build_system_prompt_with_deep_memory(DEEP_MEMORY_CONTEXT_ROUNDS)?;
        let conversation = [ProviderMessage::UserText(input.to_string())];
        let first_response = self.brain.think(&system_prompt, &conversation, &tools)?;

        match first_response {
            ProviderResponse::Text(reply) => Ok(AssistantReply {
                text: reply.text,
                thinking: reply.thinking,
            }),
            ProviderResponse::ToolUse(tool_use) => self.complete_tool_call(input, &tools, tool_use),
        }
    }

    fn complete_tool_call(
        &self,
        input: &str,
        tools: &[ProviderToolDefinition],
        tool_use: ProviderToolUse,
    ) -> io::Result<AssistantReply> {
        let tool_result =
            self.execute_tool(tool_use.name.as_str(), tool_use.input_json.as_str())?;
        let system_prompt = build_system_prompt_with_deep_memory(DEEP_MEMORY_CONTEXT_ROUNDS)?;
        let conversation = [
            ProviderMessage::UserText(input.to_string()),
            ProviderMessage::AssistantToolUse {
                assistant_content_json: tool_use.assistant_content_json,
            },
            ProviderMessage::UserToolResult {
                tool_use_id: tool_use.id,
                content: tool_result,
            },
        ];
        let final_response = self.brain.think(&system_prompt, &conversation, tools)?;

        match final_response {
            ProviderResponse::Text(reply) => Ok(AssistantReply {
                text: reply.text,
                thinking: reply.thinking,
            }),
            ProviderResponse::ToolUse(_) => {
                Err(io::Error::other("llm returned unexpected nested tool call"))
            }
        }
    }

    fn execute_tool(&self, name: &str, argument: &str) -> io::Result<String> {
        for skill in &self.skills {
            if skill.name() == name {
                return skill.execute(argument);
            }
        }

        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unknown tool: {name}"),
        ))
    }

    fn tool_definitions(&self) -> Vec<ProviderToolDefinition> {
        self.skills
            .iter()
            .map(|skill| ProviderToolDefinition {
                name: skill.name(),
                description: skill.description(),
                input_schema_json: skill.input_schema_json(),
            })
            .collect()
    }

    fn remember_deep_memory(&self) -> io::Result<String> {
        let summary_dir = botty_root_dir().join("memory").join("summary");
        fs::create_dir_all(&summary_dir)?;

        let remember_path = summary_dir.join("remember.md");
        let rec_time_path = summary_dir.join("rec.time");
        let existing_summary = read_trimmed_file(&remember_path)?;
        let rec_time = read_trimmed_file(&rec_time_path)?;

        let entries = load_deep_memory_entries()?;
        let entries = filter_entries_after_rec_time(entries, rec_time.as_deref());
        let entries = entries
            .into_iter()
            .filter(|entry| !is_control_memory_entry(entry))
            .collect::<Vec<_>>();

        if entries.is_empty() {
            return Ok("No new deep memory to remember.".to_string());
        }

        let latest_timestamp = entries
            .last()
            .map(|entry| entry.timestamp.clone())
            .ok_or_else(|| io::Error::other("remember input is unexpectedly empty"))?;
        let transcript = format_memory_transcript(&entries);
        let mut summary = extract_remember_text(
            &self.generate_remember_summary(existing_summary.as_deref(), &transcript)?,
        );
        if line_count(&summary) > REMEMBER_MAX_LINES {
            summary = extract_remember_text(&self.compress_remember_summary(&summary)?);
        }
        summary = trim_to_max_lines(&summary, REMEMBER_MAX_LINES);

        fs::write(&remember_path, ensure_trailing_newline(&summary))?;
        fs::write(&rec_time_path, format!("{latest_timestamp}\n"))?;

        Ok("I'have remembered".to_string())
    }

    fn generate_remember_summary(
        &self,
        existing_summary: Option<&str>,
        transcript: &str,
    ) -> io::Result<String> {
        let user_prompt = match existing_summary.filter(|text| !text.trim().is_empty()) {
            Some(summary) => format!(
                "Current remember.md:\n```md\n{summary}\n```\n\nNew deep memory transcript:\n```text\n{transcript}\n```\n\nUpdate remember.md. Keep it under 100 lines. Keep only key events and the user's recent important requests plus solution status."
            ),
            None => format!(
                "Deep memory transcript to summarize:\n```text\n{transcript}\n```\n\nWrite remember.md in Markdown. Keep it under 100 lines. Keep only key events and the user's recent important requests plus solution status."
            ),
        };
        self.run_summary_prompt(&user_prompt)
    }

    fn compress_remember_summary(&self, summary: &str) -> io::Result<String> {
        let user_prompt = format!(
            "This remember.md is too long. Rewrite it to at most 100 lines. You may forget long-term low-value details. Keep only key events, recent important requests, and current resolution status.\n\n```md\n{summary}\n```"
        );
        self.run_summary_prompt(&user_prompt)
    }

    fn run_summary_prompt(&self, user_prompt: &str) -> io::Result<String> {
        let response = self.brain.think(
            REMEMBER_SYSTEM_PROMPT,
            &[ProviderMessage::UserText(user_prompt.to_string())],
            &[],
        )?;
        match response {
            ProviderResponse::Text(reply) => Ok(reply.text.trim().to_string()),
            ProviderResponse::ToolUse(_) => Err(io::Error::other(
                "remember summary unexpectedly returned a tool call",
            )),
        }
    }

    fn think_due_reminder(&self, payload: &str) -> io::Result<String> {
        let payload: Value = serde_json::from_str(payload).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("parse reminder trigger payload failed: {err}"),
            )
        })?;
        let original_request = payload
            .get("original_request")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let scheduled_at = payload
            .get("scheduled_at")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let current_time = payload
            .get("current_time")
            .and_then(Value::as_str)
            .unwrap_or_default();

        let user_prompt = format!(
            "Original user reminder request:\n{original_request}\n\nScheduled time:\n{scheduled_at}\n\nCurrent local time:\n{current_time}\n\nThe reminder time has arrived. Reply with the message that should now be sent to the user."
        );
        match self.brain.think(
            REMINDER_SYSTEM_PROMPT,
            &[ProviderMessage::UserText(user_prompt)],
            &[],
        )? {
            ProviderResponse::Text(reply) => Ok(reply.text),
            ProviderResponse::ToolUse(_) => Err(io::Error::other(
                "reminder trigger unexpectedly returned a tool call",
            )),
        }
    }
}

fn parse_debug_tool_call(input: &str) -> Option<(&str, String)> {
    let trimmed = input.trim();
    let rest = trimmed.strip_prefix("/test ")?;
    let (name, argument) = rest.split_once(' ')?;
    let argument = argument.trim();
    if argument.is_empty() {
        return None;
    }
    Some((name.trim(), argument.to_string()))
}

fn matches_special_command(input: &str, command: &str) -> bool {
    extract_special_command(input) == Some(command)
}

fn parse_special_command_argument<'a>(input: &'a str, command: &str) -> Option<&'a str> {
    let trimmed = input.trim();
    if let Some(rest) = trimmed.strip_prefix(command) {
        return Some(rest.trim());
    }

    let (_, rest) = trimmed.split_once(": ")?;
    let rest = rest.trim();
    let rest = rest.strip_prefix(command)?;
    Some(rest.trim())
}

fn extract_special_command(input: &str) -> Option<&str> {
    let trimmed = input.trim();
    if trimmed.starts_with('/') {
        return Some(trimmed);
    }

    let (_, rest) = trimmed.split_once(": ")?;
    let rest = rest.trim();
    if rest.starts_with('/') {
        Some(rest)
    } else {
        None
    }
}

#[derive(Clone)]
struct DeepMemoryEntry {
    timestamp: String,
    role: DeepMemoryRole,
    message: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeepMemoryRole {
    User,
    Assistant,
}

fn build_system_prompt_with_deep_memory(rounds: usize) -> io::Result<String> {
    let remember = load_remember_summary()?;
    let memory = load_recent_deep_memory_transcript(rounds)?;
    let current_local_time = local_time_string()?;
    let mut prompt = TOOL_SYSTEM_PROMPT.to_string();
    if remember.is_empty() && memory.is_empty() {
        prompt.push_str(&format!("\n\nCurrent local time: {current_local_time}"));
        return Ok(prompt);
    }

    if !remember.is_empty() {
        prompt.push_str("\n\nLong-term memory summary from memory/summary/remember.md:\n");
        prompt.push_str(&remember);
    }
    if !memory.is_empty() {
        prompt.push_str(&format!(
            "\n\nRecent conversation history from memory/deep (latest {rounds} rounds):\n{memory}"
        ));
    }
    prompt.push_str(&format!("\n\nCurrent local time: {current_local_time}"));
    Ok(prompt)
}

fn load_recent_deep_memory_transcript(rounds: usize) -> io::Result<String> {
    if rounds == 0 {
        return Ok(String::new());
    }

    let marker = read_trimmed_file(&new_session_marker_path())?;
    let mut entries = load_deep_memory_entries()?;
    if let Some(marker) = marker.as_deref() {
        entries.retain(|entry| entry.timestamp.as_str() > marker);
    }
    entries.retain(|entry| !is_control_memory_entry(entry));
    if entries.is_empty() {
        return Ok(String::new());
    }

    entries.reverse();
    let mut collected = Vec::new();
    let mut user_count = 0usize;

    for entry in entries {
        if entry.role == DeepMemoryRole::User {
            user_count += 1;
        }
        collected.push(entry);
        if user_count >= rounds {
            break;
        }
    }

    collected.reverse();

    Ok(collected
        .into_iter()
        .map(|entry| format_deep_memory_message(&entry))
        .collect::<Vec<_>>()
        .join("\n"))
}

fn load_deep_memory_entries() -> io::Result<Vec<DeepMemoryEntry>> {
    let root = botty_root_dir().join("memory").join("deep");
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
    collect_deep_memory_files(&root, &mut files)?;
    files.sort_by_key(|path| deep_memory_file_sort_key(path));

    let mut entries = Vec::new();
    for file in files {
        let content = fs::read_to_string(file)?;
        for line in content.lines() {
            if let Some(entry) = parse_deep_memory_entry(line) {
                entries.push(entry);
            }
        }
    }
    Ok(entries)
}

fn collect_deep_memory_files(dir: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_deep_memory_files(&path, files)?;
        } else if file_type.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn deep_memory_file_sort_key(path: &Path) -> (u32, u32, u32, String) {
    let year = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);

    let (month_day, index) = path
        .file_stem()
        .and_then(|value| value.to_str())
        .and_then(parse_deep_memory_file_stem)
        .unwrap_or((0, 0));

    (year, month_day, index, path.to_string_lossy().into_owned())
}

fn parse_deep_memory_file_stem(stem: &str) -> Option<(u32, u32)> {
    let (month_day, index) = stem.split_once('-')?;
    Some((month_day.parse().ok()?, index.parse().ok()?))
}

fn parse_deep_memory_entry(line: &str) -> Option<DeepMemoryEntry> {
    let (timestamp, rest) = parse_deep_memory_timestamp(line)?;
    if let Some((_, message)) = rest.split_once(" assistant: ") {
        return Some(DeepMemoryEntry {
            timestamp,
            role: DeepMemoryRole::Assistant,
            message: restore_deep_memory_message(message),
        });
    }
    if let Some((_, message)) = rest.split_once(" user: ") {
        return Some(DeepMemoryEntry {
            timestamp,
            role: DeepMemoryRole::User,
            message: restore_deep_memory_message(message),
        });
    }
    None
}

fn restore_deep_memory_message(message: &str) -> String {
    message.replace("\\n", "\n").replace("\\r", "\r")
}

fn format_deep_memory_message(entry: &DeepMemoryEntry) -> String {
    match entry.role {
        DeepMemoryRole::User => format!("user: {}", entry.message),
        DeepMemoryRole::Assistant => format!("assistant: {}", entry.message),
    }
}

fn parse_deep_memory_timestamp(line: &str) -> Option<(String, &str)> {
    let rest = line.strip_prefix('[')?;
    let (timestamp, rest) = rest.split_once("] ")?;
    Some((timestamp.to_string(), rest))
}

fn new_session_marker_path() -> PathBuf {
    botty_root_dir()
        .join("memory")
        .join("summary")
        .join("new.time")
}

fn remember_summary_path() -> PathBuf {
    botty_root_dir()
        .join("memory")
        .join("summary")
        .join("remember.md")
}

fn load_remember_summary() -> io::Result<String> {
    Ok(read_trimmed_file(&remember_summary_path())?.unwrap_or_default())
}

fn read_trimmed_file(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(content) => {
            let trimmed = content.trim().to_string();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed))
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

fn filter_entries_after_rec_time(
    entries: Vec<DeepMemoryEntry>,
    rec_time: Option<&str>,
) -> Vec<DeepMemoryEntry> {
    match rec_time {
        Some(rec_time) => entries
            .into_iter()
            .filter(|entry| entry.timestamp.as_str() > rec_time)
            .collect(),
        None => entries,
    }
}

fn is_control_memory_entry(entry: &DeepMemoryEntry) -> bool {
    matches!(entry.role, DeepMemoryRole::User)
        && matches!(entry.message.trim(), "/new" | "/remember")
}

fn format_memory_transcript(entries: &[DeepMemoryEntry]) -> String {
    entries
        .iter()
        .map(|entry| {
            format!(
                "[{}] {}",
                entry.timestamp,
                format_deep_memory_message(entry)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn line_count(text: &str) -> usize {
    text.lines().count()
}

fn ensure_trailing_newline(text: &str) -> String {
    if text.ends_with('\n') {
        text.to_string()
    } else {
        format!("{text}\n")
    }
}

fn trim_to_max_lines(text: &str, max_lines: usize) -> String {
    text.lines().take(max_lines).collect::<Vec<_>>().join("\n")
}

fn extract_remember_text(text: &str) -> String {
    let trimmed = text.trim();
    if let Some(body) = trimmed.strip_prefix("```md\n") {
        return body
            .strip_suffix("\n```")
            .unwrap_or(body)
            .trim()
            .to_string();
    }
    if let Some(body) = trimmed.strip_prefix("```\n") {
        return body
            .strip_suffix("\n```")
            .unwrap_or(body)
            .trim()
            .to_string();
    }
    trimmed.to_string()
}

fn botty_root_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".mylittlebotty")
}

fn local_time_string() -> io::Result<String> {
    let output = Command::new("date").arg("+%Y-%m-%d %H:%M:%S").output()?;
    if !output.status.success() {
        return Err(io::Error::other("failed to get local time by date command"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
