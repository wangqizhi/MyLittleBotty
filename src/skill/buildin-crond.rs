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
      "description": "Anchor local time in YYYY-MM-DD HH:MM:SS format"
    },
    "repeat": {
      "type": "string",
      "enum": ["once", "every_minute", "every_hour", "every_day", "every_week", "every_month"],
      "description": "Run once or repeat by minute/hour/day/week/month"
    },
    "window_start": {
      "type": "string",
      "description": "Optional activation start in YYYY-MM-DD HH:MM:SS format"
    },
    "window_end": {
      "type": "string",
      "description": "Optional activation end in YYYY-MM-DD HH:MM:SS format"
    },
    "task_type": {
      "type": "string",
      "enum": ["ask_guy", "assign_tasks", "run_script"],
      "description": "ask_guy generates a reminder reply, assign_tasks executes a real scheduled task through leader routing and returns the result, run_script schedules a script placeholder"
    },
    "task_text": {
      "type": "string",
      "description": "Content for ask_guy or assign_tasks task"
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
  "required": ["action"],
  "allOf": [
    {
      "if": {
        "properties": {
          "action": { "const": "create" }
        }
      },
      "then": {
        "required": ["schedule_at"],
        "anyOf": [
          { "required": ["task_type"] },
          { "required": ["task_text"] },
          { "required": ["script_path"] }
        ]
      }
    },
    {
      "if": {
        "properties": {
          "action": { "const": "edit" }
        }
      },
      "then": {
        "required": ["id"]
      }
    }
  ]
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
        "Query, create, or edit local reminders with one-time or recurring schedules stored in ~/.mylittlebotty/reminder.rec"
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
            "- id={} schedule={} enabled={} status={} last_run={} task={}",
            reminder.id,
            reminder.schedule_summary(),
            reminder.enabled,
            reminder.status,
            display_optional(&reminder.last_run_at),
            reminder.task_summary()
        ));
    }
    Ok(lines.join("\n"))
}

fn create_reminder(input: &Value) -> io::Result<String> {
    let mut reminders = load_reminders()?;
    let next_id = next_reminder_id(&reminders);
    let schedule_at = required_string(input, "schedule_at")?.to_string();
    let task_type = resolve_task_type(input)?;
    let route = current_job_route();
    let reminder = ReminderRecord {
        id: format!("r{next_id:04}"),
        schedule_at: schedule_at.clone(),
        repeat: optional_string(input, "repeat")
            .unwrap_or("once")
            .to_string(),
        window_start: optional_string(input, "window_start")
            .unwrap_or(schedule_at.as_str())
            .to_string(),
        window_end: optional_string(input, "window_end")
            .unwrap_or_default()
            .to_string(),
        task_type,
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
        route_source: route
            .as_ref()
            .map(|route| route.source.clone())
            .unwrap_or_default(),
        route_user_id: route
            .as_ref()
            .map(|route| route.user_id.clone())
            .unwrap_or_default(),
        route_target: route.map(|route| route.target).unwrap_or_default(),
    };
    validate_reminder(&reminder)?;
    reminders.push(reminder.clone());
    save_reminders(&reminders)?;

    Ok(format!(
        "Created reminder {} with {} for {}.",
        reminder.id,
        reminder.schedule_summary(),
        reminder.task_summary()
    ))
}

fn edit_reminder(input: &Value) -> io::Result<String> {
    let id = required_string(input, "id")?;
    let mut reminders = load_reminders()?;
    let route = current_job_route();
    let reminder = reminders
        .iter_mut()
        .find(|item| item.id == id)
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, format!("reminder not found: {id}"))
        })?;

    if let Some(schedule_at) = optional_string(input, "schedule_at") {
        reminder.schedule_at = schedule_at.to_string();
        if reminder.window_start.is_empty() {
            reminder.window_start = schedule_at.to_string();
        }
    }
    if let Some(repeat) = optional_string(input, "repeat") {
        reminder.repeat = repeat.to_string();
    }
    if input.get("window_start").is_some() {
        reminder.window_start = optional_string(input, "window_start")
            .unwrap_or_default()
            .to_string();
    }
    if input.get("window_end").is_some() {
        reminder.window_end = optional_string(input, "window_end")
            .unwrap_or_default()
            .to_string();
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
    if let Some(route) = route {
        reminder.route_source = route.source;
        reminder.route_user_id = route.user_id;
        reminder.route_target = route.target;
    }
    if reminder.status == "done" {
        reminder.status = "pending".to_string();
    }
    reminder.updated_at = local_time_string()?;
    validate_reminder(reminder)?;
    let summary = format!(
        "Updated reminder {} to {} enabled={} task={}.",
        reminder.id,
        reminder.schedule_summary(),
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
    repeat: String,
    window_start: String,
    window_end: String,
    task_type: String,
    task_text: String,
    script_path: String,
    script_args: Vec<String>,
    enabled: bool,
    status: String,
    created_at: String,
    updated_at: String,
    last_run_at: String,
    route_source: String,
    route_user_id: String,
    route_target: String,
}

#[derive(Clone)]
struct ReminderRoute {
    source: String,
    user_id: String,
    target: String,
}

impl ReminderRecord {
    fn task_summary(&self) -> String {
        match self.task_type.as_str() {
            "ask_guy" | "assign_tasks" => self.task_text.clone(),
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

    fn schedule_summary(&self) -> String {
        let mut summary = format!("repeat={} anchor={}", self.repeat, self.schedule_at);
        if !self.window_start.is_empty() {
            summary.push_str(&format!(" window_start={}", self.window_start));
        }
        if !self.window_end.is_empty() {
            summary.push_str(&format!(" window_end={}", self.window_end));
        }
        summary
    }

    fn to_json_line(&self) -> String {
        serde_json::json!({
            "id": self.id,
            "schedule_at": self.schedule_at,
            "repeat": self.repeat,
            "window_start": self.window_start,
            "window_end": self.window_end,
            "task_type": self.task_type,
            "task_text": self.task_text,
            "script_path": self.script_path,
            "script_args": self.script_args,
            "enabled": self.enabled,
            "status": self.status,
            "created_at": self.created_at,
            "updated_at": self.updated_at,
            "last_run_at": self.last_run_at,
            "route_source": self.route_source,
            "route_user_id": self.route_user_id,
            "route_target": self.route_target,
        })
        .to_string()
    }

    fn from_value(value: Value) -> Option<Self> {
        let schedule_at = value.get("schedule_at")?.as_str()?.to_string();
        Some(Self {
            id: value.get("id")?.as_str()?.to_string(),
            schedule_at: schedule_at.clone(),
            repeat: value
                .get("repeat")
                .and_then(Value::as_str)
                .unwrap_or("once")
                .to_string(),
            window_start: value
                .get("window_start")
                .and_then(Value::as_str)
                .unwrap_or(schedule_at.as_str())
                .to_string(),
            window_end: value
                .get("window_end")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
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
            route_source: value
                .get("route_source")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            route_user_id: value
                .get("route_user_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            route_target: value
                .get("route_target")
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
    validate_repeat(&reminder.repeat)?;
    validate_schedule_at(&reminder.schedule_at)?;
    validate_schedule_at(&reminder.window_start)?;
    if !reminder.window_end.is_empty() {
        validate_schedule_at(&reminder.window_end)?;
        if reminder.window_end < reminder.window_start {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "window_end must be >= window_start",
            ));
        }
    }
    if reminder.schedule_at < reminder.window_start {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "schedule_at must be >= window_start",
        ));
    }
    match reminder.task_type.as_str() {
        "ask_guy" | "assign_tasks" => {
            if reminder.task_text.trim().is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{} reminder requires task_text", reminder.task_type),
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

fn current_job_route() -> Option<ReminderRoute> {
    let source = env::var("BOTTY_CURRENT_JOB_SOURCE").ok()?;
    let target = env::var("BOTTY_CURRENT_JOB_TARGET").ok()?;
    let source = source.trim();
    let target = target.trim();
    if source.is_empty() || target.is_empty() {
        return None;
    }
    Some(ReminderRoute {
        source: source.to_string(),
        user_id: env::var("BOTTY_CURRENT_JOB_USER_ID").unwrap_or_default(),
        target: target.to_string(),
    })
}

fn resolve_task_type(input: &Value) -> io::Result<String> {
    if let Some(task_type) = optional_string(input, "task_type") {
        return Ok(task_type.to_string());
    }
    if optional_string(input, "task_text").is_some() {
        return Ok("ask_guy".to_string());
    }
    if optional_string(input, "script_path").is_some() {
        return Ok("run_script".to_string());
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "missing task_type; provide task_type, task_text, or script_path",
    ))
}

fn validate_repeat(repeat: &str) -> io::Result<()> {
    match repeat {
        "once" | "every_minute" | "every_hour" | "every_day" | "every_week" | "every_month" => {
            Ok(())
        }
        other => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsupported repeat: {other}"),
        )),
    }
}

fn validate_schedule_at(schedule_at: &str) -> io::Result<()> {
    let parts = parse_local_datetime(schedule_at)?;
    let ts = parts.to_timestamp()?;
    let roundtrip = DateTimeParts::from_timestamp(ts)?.to_string();
    if roundtrip != schedule_at {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "schedule fields must be valid local time",
        ));
    }
    Ok(())
}

fn parse_local_datetime(value: &str) -> io::Result<DateTimeParts> {
    if value.len() != 19 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "schedule fields must use YYYY-MM-DD HH:MM:SS",
        ));
    }
    let bytes = value.as_bytes();
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
                "schedule fields must use YYYY-MM-DD HH:MM:SS",
            ));
        }
    }

    let parse_u32 = |start: usize, end: usize| -> io::Result<u32> {
        value[start..end].parse::<u32>().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "schedule fields must use YYYY-MM-DD HH:MM:SS",
            )
        })
    };

    Ok(DateTimeParts {
        year: value[0..4].parse::<i32>().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "schedule fields must use YYYY-MM-DD HH:MM:SS",
            )
        })?,
        month: parse_u32(5, 7)?,
        day: parse_u32(8, 10)?,
        hour: parse_u32(11, 13)?,
        minute: parse_u32(14, 16)?,
        second: parse_u32(17, 19)?,
    })
}

#[derive(Clone, Copy)]
struct DateTimeParts {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
}

impl DateTimeParts {
    fn to_timestamp(self) -> io::Result<i64> {
        if self.month == 0
            || self.month > 12
            || self.day == 0
            || self.day > 31
            || self.hour > 23
            || self.minute > 59
            || self.second > 59
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "schedule fields must be valid local time",
            ));
        }
        let mut tm = libc::tm {
            tm_sec: self.second as i32,
            tm_min: self.minute as i32,
            tm_hour: self.hour as i32,
            tm_mday: self.day as i32,
            tm_mon: self.month as i32 - 1,
            tm_year: self.year - 1900,
            tm_wday: 0,
            tm_yday: 0,
            tm_isdst: -1,
            #[cfg(any(
                target_os = "macos",
                target_os = "ios",
                target_os = "freebsd",
                target_os = "dragonfly",
                target_os = "openbsd",
                target_os = "netbsd"
            ))]
            tm_gmtoff: 0,
            #[cfg(any(
                target_os = "macos",
                target_os = "ios",
                target_os = "freebsd",
                target_os = "dragonfly",
                target_os = "openbsd",
                target_os = "netbsd"
            ))]
            tm_zone: std::ptr::null_mut(),
        };
        let timestamp = unsafe { libc::mktime(&mut tm) };
        if timestamp < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "schedule fields must be valid local time",
            ));
        }
        Ok(timestamp as i64)
    }

    fn from_timestamp(timestamp: i64) -> io::Result<Self> {
        let mut secs = timestamp as libc::time_t;
        let mut tm = libc::tm {
            tm_sec: 0,
            tm_min: 0,
            tm_hour: 0,
            tm_mday: 0,
            tm_mon: 0,
            tm_year: 0,
            tm_wday: 0,
            tm_yday: 0,
            tm_isdst: 0,
            #[cfg(any(
                target_os = "macos",
                target_os = "ios",
                target_os = "freebsd",
                target_os = "dragonfly",
                target_os = "openbsd",
                target_os = "netbsd"
            ))]
            tm_gmtoff: 0,
            #[cfg(any(
                target_os = "macos",
                target_os = "ios",
                target_os = "freebsd",
                target_os = "dragonfly",
                target_os = "openbsd",
                target_os = "netbsd"
            ))]
            tm_zone: std::ptr::null_mut(),
        };
        let rc = unsafe { libc::localtime_r(&mut secs, &mut tm) };
        if rc.is_null() {
            return Err(io::Error::other("failed to convert local time"));
        }
        Ok(Self {
            year: tm.tm_year + 1900,
            month: (tm.tm_mon + 1) as u32,
            day: tm.tm_mday as u32,
            hour: tm.tm_hour as u32,
            minute: tm.tm_min as u32,
            second: tm.tm_sec as u32,
        })
    }
}

impl std::fmt::Display for DateTimeParts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
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

fn display_optional(value: &str) -> &str {
    if value.is_empty() {
        "-"
    } else {
        value
    }
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
