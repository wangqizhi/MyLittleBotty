use crate::skill::BottySkill;
use serde_json::Value;
use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;

const CROND_TOOL_SCHEMA_JSON: &str = r#"{
  "type": "object",
  "properties": {
    "action": {
      "type": "string",
      "enum": ["query", "create", "edit"],
      "description": "query lists reminders, create adds a reminder, edit updates an existing reminder"
    },
    "id": {
      "type": "string",
      "description": "Reminder id for edit"
    },
    "schedule_at": {
      "type": "string",
      "description": "Local time in YYYY-MM-DD HH:MM:SS format"
    },
    "task_type": {
      "type": "string",
      "enum": ["ask_guy", "run_script"],
      "description": "ask_guy sends a task to leader Botty-Guy, run_script schedules a script placeholder"
    },
    "task_text": {
      "type": "string",
      "description": "Content for ask_guy task"
    },
    "script_path": {
      "type": "string",
      "description": "Script path for run_script reminders"
    },
    "script_args": {
      "type": "array",
      "items": { "type": "string" },
      "description": "Optional script arguments for run_script reminders"
    },
    "enabled": {
      "type": "boolean",
      "description": "Whether the reminder stays enabled after edit"
    }
  },
  "required": ["action"]
}"#;

pub struct BuildinCrondSkill;

impl BuildinCrondSkill {
    pub fn new() -> Self {
        Self
    }
}

impl BottySkill for BuildinCrondSkill {
    fn name(&self) -> &'static str {
        "crond"
    }

    fn description(&self) -> &'static str {
        "Query, create, or edit local second-level reminders stored in ~/.mylittlebotty/reminder.rec"
    }

    fn input_schema_json(&self) -> &'static str {
        CROND_TOOL_SCHEMA_JSON
    }

    fn execute(&self, input_json: &str) -> io::Result<String> {
        let input: Value = serde_json::from_str(input_json).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("parse crond tool input json failed: {err}"),
            )
        })?;

        match required_string(&input, "action")? {
            "query" => query_reminders(),
            "create" => create_reminder(&input),
            "edit" => edit_reminder(&input),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unsupported crond action: {other}"),
            )),
        }
    }
}

fn query_reminders() -> io::Result<String> {
    let reminders = load_reminders()?;
    if reminders.is_empty() {
        return Ok("No reminders scheduled.".to_string());
    }

    let mut lines = Vec::new();
    for reminder in reminders {
        lines.push(format!(
            "- id={} time={} type={} enabled={} status={} task={}",
            reminder.id,
            reminder.schedule_at,
            reminder.task_type,
            reminder.enabled,
            reminder.status,
            reminder.task_summary()
        ));
    }
    Ok(lines.join("\n"))
}

fn create_reminder(input: &Value) -> io::Result<String> {
    let mut reminders = load_reminders()?;
    let next_id = next_reminder_id(&reminders);
    let reminder = ReminderRecord {
        id: format!("r{next_id:04}"),
        schedule_at: required_string(input, "schedule_at")?.to_string(),
        task_type: required_string(input, "task_type")?.to_string(),
        task_text: optional_string(input, "task_text")
            .unwrap_or_default()
            .to_string(),
        script_path: optional_string(input, "script_path")
            .unwrap_or_default()
            .to_string(),
        script_args: optional_string_array(input, "script_args"),
        enabled: input
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        status: "pending".to_string(),
        created_at: local_time_string()?,
        updated_at: local_time_string()?,
        last_run_at: String::new(),
    };
    validate_reminder(&reminder)?;
    reminders.push(reminder.clone());
    save_reminders(&reminders)?;

    Ok(format!(
        "Created reminder {} at {} for {}.",
        reminder.id,
        reminder.schedule_at,
        reminder.task_summary()
    ))
}

fn edit_reminder(input: &Value) -> io::Result<String> {
    let id = required_string(input, "id")?;
    let mut reminders = load_reminders()?;
    let reminder = reminders
        .iter_mut()
        .find(|item| item.id == id)
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, format!("reminder not found: {id}"))
        })?;

    if let Some(schedule_at) = optional_string(input, "schedule_at") {
        reminder.schedule_at = schedule_at.to_string();
    }
    if let Some(task_type) = optional_string(input, "task_type") {
        reminder.task_type = task_type.to_string();
    }
    if let Some(task_text) = optional_string(input, "task_text") {
        reminder.task_text = task_text.to_string();
    }
    if let Some(script_path) = optional_string(input, "script_path") {
        reminder.script_path = script_path.to_string();
    }
    if input.get("script_args").is_some() {
        reminder.script_args = optional_string_array(input, "script_args");
    }
    if let Some(enabled) = input.get("enabled").and_then(Value::as_bool) {
        reminder.enabled = enabled;
    }
    if reminder.status == "done" {
        reminder.status = "pending".to_string();
        reminder.last_run_at.clear();
    }
    reminder.updated_at = local_time_string()?;
    validate_reminder(reminder)?;
    let summary = format!(
        "Updated reminder {} to time={} type={} enabled={} task={}.",
        reminder.id,
        reminder.schedule_at,
        reminder.task_type,
        reminder.enabled,
        reminder.task_summary()
    );
    save_reminders(&reminders)?;

    Ok(summary)
}

#[derive(Clone)]
struct ReminderRecord {
    id: String,
    schedule_at: String,
    task_type: String,
    task_text: String,
    script_path: String,
    script_args: Vec<String>,
    enabled: bool,
    status: String,
    created_at: String,
    updated_at: String,
    last_run_at: String,
}

impl ReminderRecord {
    fn task_summary(&self) -> String {
        match self.task_type.as_str() {
            "ask_guy" => self.task_text.clone(),
            "run_script" => {
                if self.script_args.is_empty() {
                    self.script_path.clone()
                } else {
                    format!("{} {}", self.script_path, self.script_args.join(" "))
                }
            }
            _ => self.task_text.clone(),
        }
    }

    fn to_json_line(&self) -> String {
        serde_json::json!({
            "id": self.id,
            "schedule_at": self.schedule_at,
            "task_type": self.task_type,
            "task_text": self.task_text,
            "script_path": self.script_path,
            "script_args": self.script_args,
            "enabled": self.enabled,
            "status": self.status,
            "created_at": self.created_at,
            "updated_at": self.updated_at,
            "last_run_at": self.last_run_at,
        })
        .to_string()
    }

    fn from_value(value: Value) -> Option<Self> {
        Some(Self {
            id: value.get("id")?.as_str()?.to_string(),
            schedule_at: value.get("schedule_at")?.as_str()?.to_string(),
            task_type: value.get("task_type")?.as_str()?.to_string(),
            task_text: value
                .get("task_text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            script_path: value
                .get("script_path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            script_args: value
                .get("script_args")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(|item| item.to_string())
                        .collect()
                })
                .unwrap_or_default(),
            enabled: value
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            status: value
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("pending")
                .to_string(),
            created_at: value
                .get("created_at")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            updated_at: value
                .get("updated_at")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            last_run_at: value
                .get("last_run_at")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        })
    }
}

fn load_reminders() -> io::Result<Vec<ReminderRecord>> {
    let path = reminder_rec_path();
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };

    let mut reminders = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            if let Some(reminder) = ReminderRecord::from_value(value) {
                reminders.push(reminder);
            }
        }
    }
    reminders.sort_by(|a, b| a.schedule_at.cmp(&b.schedule_at).then(a.id.cmp(&b.id)));
    Ok(reminders)
}

fn save_reminders(reminders: &[ReminderRecord]) -> io::Result<()> {
    let path = reminder_rec_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = path.with_extension("rec.tmp");
    let mut content = String::new();
    for reminder in reminders {
        content.push_str(&reminder.to_json_line());
        content.push('\n');
    }
    fs::write(&tmp_path, content)?;
    fs::rename(tmp_path, path)?;
    Ok(())
}

fn validate_reminder(reminder: &ReminderRecord) -> io::Result<()> {
    validate_schedule_at(&reminder.schedule_at)?;
    match reminder.task_type.as_str() {
        "ask_guy" => {
            if reminder.task_text.trim().is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "ask_guy reminder requires task_text",
                ));
            }
        }
        "run_script" => {
            if reminder.script_path.trim().is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "run_script reminder requires script_path",
                ));
            }
        }
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unsupported reminder task_type: {other}"),
            ));
        }
    }
    Ok(())
}

fn validate_schedule_at(schedule_at: &str) -> io::Result<()> {
    if schedule_at.len() != 19 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "schedule_at must use YYYY-MM-DD HH:MM:SS",
        ));
    }
    let bytes = schedule_at.as_bytes();
    let expected = [
        (4usize, b'-'),
        (7usize, b'-'),
        (10usize, b' '),
        (13usize, b':'),
        (16usize, b':'),
    ];
    for (index, marker) in expected {
        if bytes.get(index).copied() != Some(marker) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "schedule_at must use YYYY-MM-DD HH:MM:SS",
            ));
        }
    }
    Ok(())
}

fn next_reminder_id(reminders: &[ReminderRecord]) -> u32 {
    reminders
        .iter()
        .filter_map(|item| item.id.strip_prefix('r'))
        .filter_map(|item| item.parse::<u32>().ok())
        .max()
        .unwrap_or(0)
        + 1
}

fn required_string<'a>(value: &'a Value, key: &str) -> io::Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("missing {key}")))
}

fn optional_string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|item| !item.is_empty())
}

fn optional_string_array(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|item| item.trim())
                .filter(|item| !item.is_empty())
                .map(|item| item.to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn local_time_string() -> io::Result<String> {
    let output = std::process::Command::new("date")
        .arg("+%Y-%m-%d %H:%M:%S")
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("failed to get local time"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn reminder_rec_path() -> PathBuf {
    botty_root_dir().join(format!("reminder{}.rec", runtime_suffix()))
}

fn botty_root_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".mylittlebotty")
}

fn runtime_suffix() -> &'static str {
    if cfg!(debug_assertions) {
        "-dev"
    } else {
        ""
    }
}
