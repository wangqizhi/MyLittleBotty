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
    let mut reminders = load_reminders()?;
    let mut dirty = false;

    for reminder in &mut reminders {
        if !reminder.enabled || reminder.status == "done" {
            continue;
        }
        if reminder.schedule_at.as_str() > now.as_str() {
            continue;
        }

        let (status, output) = match execute_reminder(reminder, &now) {
            Ok(output) => ("ok".to_string(), output),
            Err(err) => ("error".to_string(), err.to_string()),
        };

        append_result_line(&now, &status, &format!("{} {}", reminder.id, output))?;
        let _ = push_result_notifications(reminder, &status, &output, &now);
        reminder.status = "done".to_string();
        reminder.last_run_at = now.clone();
        reminder.updated_at = now.clone();
        dirty = true;
    }

    if dirty {
        save_reminders(&reminders)?;
    }

    Ok(())
}

fn execute_reminder(reminder: &ReminderRecord, now: &str) -> io::Result<String> {
    match reminder.task_type.as_str() {
        "ask_guy" => {
            let payload = serde_json::json!({
                "original_request": reminder.task_text,
                "scheduled_at": reminder.schedule_at,
                "current_time": now,
            })
            .to_string();
            let reply = ask_leader_guy(
                "crond",
                "scheduler",
                &format!("/reminder-now {payload}"),
            )?;
            Ok(format!("executed at {now}: {reply}"))
        }
        "run_script" => Ok(format!(
            "executed at {now}: run_script is reserved and not implemented yet for {}",
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
            enabled: value.get("enabled").and_then(Value::as_bool).unwrap_or(true),
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
