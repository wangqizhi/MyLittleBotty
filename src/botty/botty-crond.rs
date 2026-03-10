use serde_json::{self, Value};
use std::collections::HashSet;
use std::env;
use std::ffi::CString;
use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::io::BufRead;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::Duration;

const TELEGRAM_API_BASE: &str = "https://api.telegram.org";
const FEISHU_API_BASE: &str = "https://open.feishu.cn/open-apis";
const CHAT_META_PREFIX: &str = "__botty_meta__";

pub fn run() {
    let _pid_guard = match acquire_crond_pid_guard() {
        Ok(Some(guard)) => guard,
        Ok(None) => {
            eprintln!("Botty-crond is already running, exiting duplicate instance");
            return;
        }
        Err(err) => {
            eprintln!("Botty-crond failed to acquire pid file: {err}");
            return;
        }
    };

    set_process_name(crond_process_name());
    loop {
        if let Err(err) = tick() {
            eprintln!("Botty-crond tick failed: {err}");
        }
        thread::sleep(Duration::from_millis(500));
    }
}

fn tick() -> io::Result<()> {
    let now = local_time_string()?;
    let now_ts = parse_local_datetime(&now)?.to_timestamp()?;
    let mut reminders = load_reminders()?;
    let mut dirty = false;

    for reminder in &mut reminders {
        if !reminder.enabled {
            continue;
        }
        let due_at = match reminder.next_due_at(now_ts)? {
            Some(due_at) => due_at,
            None => {
                if reminder.should_mark_done(now_ts)? && reminder.status != "done" {
                    reminder.status = "done".to_string();
                    reminder.updated_at = now.clone();
                    dirty = true;
                }
                continue;
            }
        };

        let (status, output) = match execute_reminder(reminder, &due_at, &now) {
            Ok(output) => ("ok".to_string(), output),
            Err(err) => ("error".to_string(), err.to_string()),
        };

        append_result_line(&now, &status, &format!("{} {}", reminder.id, output))?;
        let _ = push_result_notifications(reminder, &status, &output, &now);
        reminder.last_run_at = due_at;
        reminder.status = if reminder.repeat == "once" {
            "done".to_string()
        } else {
            "pending".to_string()
        };
        reminder.updated_at = now.clone();
        dirty = true;
    }

    if dirty {
        save_reminders(&reminders)?;
    }

    Ok(())
}

fn execute_reminder(reminder: &ReminderRecord, due_at: &str, now: &str) -> io::Result<String> {
    match reminder.task_type.as_str() {
        "ask_guy" => {
            let payload = serde_json::json!({
                "original_request": reminder.task_text,
                "schedule_anchor": reminder.schedule_at,
                "scheduled_at": due_at,
                "current_time": now,
                "repeat": reminder.repeat,
            })
            .to_string();
            let reply = ask_leader_guy("crond", "scheduler", &format!("/reminder-now {payload}"))?;
            Ok(format!("scheduled {due_at}, executed at {now}: {reply}"))
        }
        "run_script" => Ok(format!(
            "scheduled {due_at}, executed at {now}: run_script is reserved and not implemented yet for {}",
            reminder.script_path
        )),
        other => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsupported reminder task_type: {other}"),
        )),
    }
}

fn push_result_notifications(
    _reminder: &ReminderRecord,
    status: &str,
    output: &str,
    executed_at: &str,
) -> io::Result<()> {
    let config = load_chatbot_config()?;
    let text = if status == "ok" {
        output
            .strip_prefix(&format!("executed at {executed_at}: "))
            .unwrap_or(output)
            .to_string()
    } else {
        format!("提醒执行失败：{}", output)
    };

    if config.telegram_enabled {
        for chat_id in &config.telegram_targets {
            let _ = send_telegram_message(
                &config.telegram_api_base,
                &config.telegram_apikey,
                *chat_id,
                &text,
            );
        }
    }

    if config.feishu_enabled && !config.feishu_chat_id.is_empty() {
        let _ = send_feishu_message(
            &config.feishu_api_base,
            &config.feishu_apikey,
            &config.feishu_chat_id,
            &text,
        );
    }

    Ok(())
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
}

struct CrondPidGuard {
    path: PathBuf,
}

impl Drop for CrondPidGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl ReminderRecord {
    fn next_due_at(&self, now_ts: i64) -> io::Result<Option<String>> {
        if !self.enabled || self.status == "done" {
            return Ok(None);
        }
        let upper_bound = self
            .window_end_timestamp()?
            .map_or(now_ts, |end| end.min(now_ts));
        let window_start = self.window_start_timestamp()?;
        if upper_bound < window_start {
            return Ok(None);
        }
        let reference = if self.last_run_at.is_empty() {
            window_start.saturating_sub(1)
        } else {
            parse_local_datetime(&self.last_run_at)?.to_timestamp()?
        };
        let due = self.next_occurrence_after(reference)?;
        if let Some(due_ts) = due {
            if due_ts >= window_start && due_ts <= upper_bound {
                return Ok(Some(DateTimeParts::from_timestamp(due_ts)?.to_string()));
            }
        }
        Ok(None)
    }

    fn should_mark_done(&self, now_ts: i64) -> io::Result<bool> {
        if self.status == "done" {
            return Ok(false);
        }
        if self.repeat == "once" {
            let anchor_ts = parse_local_datetime(&self.schedule_at)?.to_timestamp()?;
            return Ok(!self.last_run_at.is_empty() || now_ts >= anchor_ts);
        }
        if let Some(window_end) = self.window_end_timestamp()? {
            if now_ts < window_end {
                return Ok(false);
            }
            return Ok(self.next_due_at(now_ts)?.is_none());
        }
        Ok(false)
    }

    fn next_occurrence_after(&self, reference_ts: i64) -> io::Result<Option<i64>> {
        let anchor = parse_local_datetime(&self.schedule_at)?;
        let anchor_ts = anchor.to_timestamp()?;
        if reference_ts < anchor_ts.saturating_sub(1) {
            return Ok(Some(anchor_ts));
        }

        let candidate = match self.repeat.as_str() {
            "once" => {
                if anchor_ts > reference_ts {
                    Some(anchor_ts)
                } else {
                    None
                }
            }
            "every_minute" => next_minutely_occurrence(anchor, reference_ts)?,
            "every_hour" => next_hourly_occurrence(anchor, reference_ts)?,
            "every_day" => next_daily_occurrence(anchor, reference_ts)?,
            "every_week" => next_weekly_occurrence(anchor, reference_ts)?,
            "every_month" => next_monthly_occurrence(anchor, reference_ts)?,
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unsupported repeat: {other}"),
                ));
            }
        };
        Ok(candidate)
    }

    fn window_start_timestamp(&self) -> io::Result<i64> {
        parse_local_datetime(&self.window_start_or_anchor())?.to_timestamp()
    }

    fn window_end_timestamp(&self) -> io::Result<Option<i64>> {
        if self.window_end.is_empty() {
            Ok(None)
        } else {
            Ok(Some(
                parse_local_datetime(&self.window_end)?.to_timestamp()?,
            ))
        }
    }

    fn window_start_or_anchor(&self) -> String {
        if self.window_start.is_empty() {
            self.schedule_at.clone()
        } else {
            self.window_start.clone()
        }
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
        })
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
        })
        .to_string()
    }
}

fn load_reminders() -> io::Result<Vec<ReminderRecord>> {
    let content = match fs::read_to_string(reminder_rec_path()) {
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

fn acquire_crond_pid_guard() -> io::Result<Option<CrondPidGuard>> {
    let pid_path = crond_pid_file();
    if let Some(parent) = pid_path.parent() {
        fs::create_dir_all(parent)?;
    }

    if let Some(pid) = read_pid_file(&pid_path)? {
        if is_process_alive(pid) {
            return Ok(None);
        }
        let _ = fs::remove_file(&pid_path);
    }

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&pid_path)?;
    writeln!(file, "{}", std::process::id())?;

    Ok(Some(CrondPidGuard { path: pid_path }))
}

fn append_result_line(executed_at: &str, status: &str, output: &str) -> io::Result<()> {
    let path = reminder_result_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let sanitized = output.replace('\n', "\\n").replace('\r', "\\r");
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{executed_at}\t{status}\t{sanitized}")?;
    Ok(())
}

struct ChatbotConfig {
    telegram_enabled: bool,
    telegram_apikey: String,
    telegram_api_base: String,
    telegram_targets: Vec<i64>,
    feishu_enabled: bool,
    feishu_apikey: String,
    feishu_api_base: String,
    feishu_chat_id: String,
}

impl Default for ChatbotConfig {
    fn default() -> Self {
        Self {
            telegram_enabled: true,
            telegram_apikey: String::new(),
            telegram_api_base: TELEGRAM_API_BASE.to_string(),
            telegram_targets: Vec::new(),
            feishu_enabled: false,
            feishu_apikey: String::new(),
            feishu_api_base: FEISHU_API_BASE.to_string(),
            feishu_chat_id: String::new(),
        }
    }
}

fn load_chatbot_config() -> io::Result<ChatbotConfig> {
    let path = setup_config_file();
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(ChatbotConfig::default()),
        Err(err) => return Err(err),
    };

    let mut config = ChatbotConfig::default();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "chatbot.provider" => {
                config.telegram_enabled = value
                    .split(',')
                    .map(|item| item.trim())
                    .any(|provider| provider == "telegram");
                config.feishu_enabled = value
                    .split(',')
                    .map(|item| item.trim())
                    .any(|provider| provider == "feishu");
            }
            "chatbot.telegram.enabled" => config.telegram_enabled = parse_bool(value),
            "chatbot.telegram.apikey" => config.telegram_apikey = value.to_string(),
            "chatbot.telegram.api_base" => config.telegram_api_base = value.to_string(),
            "chatbot.telegram.whitelist_user_ids" => {
                config.telegram_targets = parse_telegram_targets(value);
            }
            "chatbot.telegram.whitelise_user_ids" => {
                config.telegram_targets = parse_telegram_targets(value);
            }
            "chatbot.feishu.enabled" => config.feishu_enabled = parse_bool(value),
            "chatbot.feishu.apikey" => config.feishu_apikey = value.to_string(),
            "chatbot.feishu.api_base" => config.feishu_api_base = value.to_string(),
            "chatbot.feishu.chat_id" => config.feishu_chat_id = value.to_string(),
            "chatbot.apikey" => {
                if config.telegram_apikey.is_empty() {
                    config.telegram_apikey = value.to_string();
                }
                if config.feishu_apikey.is_empty() {
                    config.feishu_apikey = value.to_string();
                }
            }
            _ => {}
        }
    }

    if config.telegram_apikey.is_empty() || config.telegram_targets.is_empty() {
        config.telegram_enabled = false;
    }
    if config.feishu_apikey.is_empty() || config.feishu_chat_id.is_empty() {
        config.feishu_enabled = false;
    }

    Ok(config)
}

fn parse_telegram_targets(value: &str) -> Vec<i64> {
    let mut seen = HashSet::new();
    let mut targets = Vec::new();
    for item in value.split(',').map(|part| part.trim()) {
        if item.is_empty() {
            continue;
        }
        if let Ok(chat_id) = item.parse::<i64>() {
            if seen.insert(chat_id) {
                targets.push(chat_id);
            }
        }
    }
    targets
}

fn send_telegram_message(api_base: &str, apikey: &str, chat_id: i64, text: &str) -> io::Result<()> {
    let url = format!("{api_base}/bot{apikey}/sendMessage");
    let output = Command::new("curl")
        .arg("-fsS")
        .arg("-X")
        .arg("POST")
        .arg(url)
        .arg("--data-urlencode")
        .arg(format!("chat_id={chat_id}"))
        .arg("--data-urlencode")
        .arg(format!("text={text}"))
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("curl sendMessage failed"));
    }
    Ok(())
}

fn send_feishu_message(api_base: &str, apikey: &str, chat_id: &str, text: &str) -> io::Result<()> {
    let url = format!("{api_base}/im/v1/messages?receive_id_type=chat_id");
    let escaped = escape_json_string(text);
    let payload = format!(
        "{{\"receive_id\":\"{chat_id}\",\"msg_type\":\"text\",\"content\":\"{{\\\"text\\\":\\\"{escaped}\\\"}}\"}}"
    );

    let output = Command::new("curl")
        .arg("-fsS")
        .arg("-X")
        .arg("POST")
        .arg(url)
        .arg("-H")
        .arg(format!("Authorization: Bearer {apikey}"))
        .arg("-H")
        .arg("Content-Type: application/json; charset=utf-8")
        .arg("-d")
        .arg(payload)
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("curl feishu send message failed"));
    }
    Ok(())
}

fn ask_leader_guy(source: &str, user_id: &str, message: &str) -> io::Result<String> {
    let stream = UnixStream::connect(crate::botty_boss::chat_socket_path())?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = BufWriter::new(stream);
    writeln!(
        writer,
        "{}",
        encode_ipc_line(&encode_meta_message(source, user_id, message))?
    )?;
    writer.flush()?;

    let mut reply = String::new();
    let bytes = reader.read_line(&mut reply)?;
    if bytes == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "Botty-Boss closed socket",
        ));
    }

    decode_ipc_line(reply.trim_end())
}

fn encode_meta_message(source: &str, user_id: &str, message: &str) -> String {
    format!("{CHAT_META_PREFIX}|source={source}|user_id={user_id}|{message}")
}

fn encode_ipc_line(value: &str) -> io::Result<String> {
    serde_json::to_string(value)
        .map_err(|err| io::Error::other(format!("encode ipc line failed: {err}")))
}

fn decode_ipc_line(value: &str) -> io::Result<String> {
    serde_json::from_str(value).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("decode ipc line failed: {err}"),
        )
    })
}

fn reminder_rec_path() -> PathBuf {
    botty_root_dir().join(format!("reminder{}.rec", runtime_suffix()))
}

fn reminder_result_path() -> PathBuf {
    botty_root_dir().join(format!("reminder{}.result", runtime_suffix()))
}

fn crond_pid_file() -> PathBuf {
    botty_root_dir()
        .join("run")
        .join(format!("crond{}.pid", runtime_suffix()))
}

fn setup_config_file() -> PathBuf {
    botty_root_dir()
        .join("config")
        .join(format!("setup{}.conf", runtime_suffix()))
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

fn crond_process_name() -> &'static str {
    if cfg!(debug_assertions) {
        "Botty-crond-dev"
    } else {
        "Botty-crond"
    }
}

fn set_process_name(name: &str) {
    let safe_name = if name.as_bytes().contains(&0) {
        "botty-crond"
    } else {
        name
    };
    if let Ok(c_name) = CString::new(safe_name) {
        unsafe {
            libc::pthread_setname_np(c_name.as_ptr());
        }
    }
}

fn read_pid_file(path: &PathBuf) -> io::Result<Option<i32>> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    Ok(content.trim().parse::<i32>().ok())
}

fn is_process_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    let rc = unsafe { libc::kill(pid, 0) };
    if rc == 0 {
        return true;
    }
    matches!(io::Error::last_os_error().raw_os_error(), Some(libc::EPERM))
}

fn parse_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn local_time_string() -> io::Result<String> {
    let output = Command::new("date").arg("+%Y-%m-%d %H:%M:%S").output()?;
    if !output.status.success() {
        return Err(io::Error::other("failed to get local time"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn escape_json_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn next_minutely_occurrence(anchor: DateTimeParts, reference_ts: i64) -> io::Result<Option<i64>> {
    let reference = DateTimeParts::from_timestamp(reference_ts + 1)?;
    let candidate = normalize_local_parts(DateTimeParts {
        year: reference.year,
        month: reference.month,
        day: reference.day,
        hour: reference.hour,
        minute: reference.minute,
        second: anchor.second,
    })?;
    let candidate_ts = candidate.to_timestamp()?;
    if candidate_ts > reference_ts {
        Ok(Some(candidate_ts))
    } else {
        Ok(Some(add_seconds(candidate_ts, 60)?))
    }
}

fn next_hourly_occurrence(anchor: DateTimeParts, reference_ts: i64) -> io::Result<Option<i64>> {
    let reference = DateTimeParts::from_timestamp(reference_ts + 1)?;
    let candidate = normalize_local_parts(DateTimeParts {
        year: reference.year,
        month: reference.month,
        day: reference.day,
        hour: reference.hour,
        minute: anchor.minute,
        second: anchor.second,
    })?;
    let candidate_ts = candidate.to_timestamp()?;
    if candidate_ts > reference_ts {
        Ok(Some(candidate_ts))
    } else {
        Ok(Some(add_seconds(candidate_ts, 3600)?))
    }
}

fn next_daily_occurrence(anchor: DateTimeParts, reference_ts: i64) -> io::Result<Option<i64>> {
    let reference = DateTimeParts::from_timestamp(reference_ts + 1)?;
    let candidate = normalize_local_parts(DateTimeParts {
        year: reference.year,
        month: reference.month,
        day: reference.day,
        hour: anchor.hour,
        minute: anchor.minute,
        second: anchor.second,
    })?;
    let candidate_ts = candidate.to_timestamp()?;
    if candidate_ts > reference_ts {
        Ok(Some(candidate_ts))
    } else {
        Ok(Some(add_seconds(candidate_ts, 86_400)?))
    }
}

fn next_weekly_occurrence(anchor: DateTimeParts, reference_ts: i64) -> io::Result<Option<i64>> {
    let reference = DateTimeParts::from_timestamp(reference_ts + 1)?;
    let target_weekday = anchor.weekday_number()?;
    let current_weekday = reference.weekday_number()?;
    let mut days_ahead = (target_weekday + 7 - current_weekday) % 7;
    let candidate = normalize_local_parts(DateTimeParts {
        year: reference.year,
        month: reference.month,
        day: reference.day + days_ahead,
        hour: anchor.hour,
        minute: anchor.minute,
        second: anchor.second,
    })?;
    let candidate_ts = candidate.to_timestamp()?;
    if candidate_ts <= reference_ts {
        days_ahead += 7;
        let next_candidate = normalize_local_parts(DateTimeParts {
            year: reference.year,
            month: reference.month,
            day: reference.day + days_ahead,
            hour: anchor.hour,
            minute: anchor.minute,
            second: anchor.second,
        })?;
        Ok(Some(next_candidate.to_timestamp()?))
    } else {
        Ok(Some(candidate_ts))
    }
}

fn next_monthly_occurrence(anchor: DateTimeParts, reference_ts: i64) -> io::Result<Option<i64>> {
    let mut cursor = DateTimeParts::from_timestamp(reference_ts + 1)?;
    cursor.day = 1;
    cursor.hour = anchor.hour;
    cursor.minute = anchor.minute;
    cursor.second = anchor.second;
    let mut month_offset = 0i32;
    loop {
        let month_start = add_months(cursor, month_offset)?;
        if let Some(candidate) = with_day_if_valid(month_start, anchor.day)? {
            let candidate_ts = candidate.to_timestamp()?;
            if candidate_ts > reference_ts {
                return Ok(Some(candidate_ts));
            }
        }
        month_offset += 1;
    }
}

fn with_day_if_valid(base: DateTimeParts, day: u32) -> io::Result<Option<DateTimeParts>> {
    if day == 0 || day > days_in_month(base.year, base.month) {
        return Ok(None);
    }
    Ok(Some(DateTimeParts {
        year: base.year,
        month: base.month,
        day,
        hour: base.hour,
        minute: base.minute,
        second: base.second,
    }))
}

fn add_seconds(timestamp: i64, seconds: i64) -> io::Result<i64> {
    let target = timestamp.saturating_add(seconds);
    let _ = DateTimeParts::from_timestamp(target)?;
    Ok(target)
}

fn add_months(parts: DateTimeParts, months: i32) -> io::Result<DateTimeParts> {
    let total_months = parts.year * 12 + parts.month as i32 - 1 + months;
    let year = total_months.div_euclid(12);
    let month = total_months.rem_euclid(12) + 1;
    normalize_local_parts(DateTimeParts {
        year,
        month: month as u32,
        day: 1,
        hour: parts.hour,
        minute: parts.minute,
        second: parts.second,
    })
}

fn normalize_local_parts(parts: DateTimeParts) -> io::Result<DateTimeParts> {
    DateTimeParts::from_timestamp(parts.to_timestamp()?)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
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

    fn weekday_number(self) -> io::Result<u32> {
        let mut secs = self.to_timestamp()? as libc::time_t;
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
        Ok(match tm.tm_wday {
            0 => 7,
            value => value as u32,
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
