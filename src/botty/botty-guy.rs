use crate::botty_body::{AssistantReply, BottyBody};
use serde_json::{self, json};
use std::collections::HashSet;
use std::collections::VecDeque;
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
const TELEGRAM_POLL_INTERVAL_SECONDS_DEFAULT: u64 = 1;
const FEISHU_API_BASE: &str = "https://open.feishu.cn/open-apis";
const FEISHU_POLL_INTERVAL_SECONDS_DEFAULT: u64 = 1;
const FEISHU_SEEN_CACHE_LIMIT: usize = 200;
const CHAT_MEMORY_MAX_BYTES: u64 = 200 * 1024;
const CHAT_META_PREFIX: &str = "__botty_meta__";

pub fn run() {
    set_process_name(guy_process_name());
    let body = match BottyBody::from_setup() {
        Ok(body) => body,
        Err(err) => {
            eprintln!("Botty-Guy failed to load body config: {err}");
            return;
        }
    };
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut lines = stdin.lock().lines();

    while let Some(line_result) = lines.next() {
        let line = match line_result {
            Ok(line) => line,
            Err(err) => {
                eprintln!("Botty-Guy failed to read input: {err}");
                break;
            }
        };
        let message = match decode_ipc_line(line.trim_end()) {
            Ok(message) => message,
            Err(err) => {
                eprintln!("Botty-Guy failed to decode input: {err}");
                continue;
            }
        };
        let message = message.trim();
        if message.is_empty() {
            continue;
        }

        let reply = match body.think(message) {
            Ok(reply) => reply,
            Err(err) => {
                eprintln!("Botty-Guy failed to process input: {err}");
                AssistantReply {
                    text: err.to_string(),
                    thinking: None,
                }
            }
        };
        let encoded_reply = match encode_ipc_line(&encode_assistant_reply(&reply)) {
            Ok(reply) => reply,
            Err(err) => {
                eprintln!("Botty-Guy failed to encode output: {err}");
                break;
            }
        };
        if let Err(err) = writeln!(stdout, "{encoded_reply}") {
            eprintln!("Botty-Guy failed to write output: {err}");
            break;
        }
        if let Err(err) = stdout.flush() {
            eprintln!("Botty-Guy failed to flush output: {err}");
            break;
        }
    }
}

pub fn run_telegram_input() {
    set_process_name(telegram_input_process_name());

    let config = match load_chatbot_config() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("Botty-input-telegram failed to load config: {err}");
            return;
        }
    };

    if !config.enabled {
        return;
    }
    if config.apikey.is_empty() {
        eprintln!("Botty-input-telegram skipped: chatbot.telegram.apikey is empty");
        return;
    }

    let interval = config.poll_interval();
    let mut plugin = TelegramProviderPlugin::new(
        config.apikey,
        config.telegram_api_base,
        interval,
        config.telegram_whitelist_user_ids.clone(),
    );
    run_input_provider_loop(&mut plugin);
}

pub fn run_feishu_input() {
    set_process_name(feishu_input_process_name());

    let config = match load_chatbot_config() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("Botty-input-feishu failed to load config: {err}");
            return;
        }
    };

    if !config.feishu_enabled {
        return;
    }
    if config.feishu_apikey.is_empty() {
        eprintln!("Botty-input-feishu skipped: chatbot.feishu.apikey is empty");
        return;
    }
    if config.feishu_chat_id.is_empty() {
        eprintln!("Botty-input-feishu skipped: chatbot.feishu.chat_id is empty");
        return;
    }

    let interval = config.feishu_poll_interval();
    let mut plugin = FeishuProviderPlugin::new(
        config.feishu_apikey,
        config.feishu_api_base,
        config.feishu_chat_id,
        interval,
    );
    run_input_provider_loop(&mut plugin);
}

struct InboundMessage {
    message_id: String,
    target: String,
    user_id: String,
    text: String,
}

trait ChatbotProviderPlugin {
    fn provider_name(&self) -> &'static str;
    fn poll_interval(&self) -> Duration;
    fn fetch_messages(&mut self) -> io::Result<Vec<InboundMessage>>;
    fn user_id<'a>(&self, message: &'a InboundMessage) -> &'a str;
    fn is_user_allowed(&self, _user_id: &str) -> bool {
        true
    }
    fn send_reply(&mut self, target: &str, text: &str) -> io::Result<Option<String>>;
}

fn run_input_provider_loop(plugin: &mut impl ChatbotProviderPlugin) {
    let mut seen = HashSet::new();
    let mut seen_order = VecDeque::new();
    let mut initialized = false;

    loop {
        let messages = match plugin.fetch_messages() {
            Ok(messages) => messages,
            Err(err) => {
                eprintln!("{} fetch messages failed: {err}", plugin.provider_name());
                thread::sleep(plugin.poll_interval());
                continue;
            }
        };

        if !initialized {
            for message in messages {
                let _ = remember_message_id(&mut seen, &mut seen_order, &message.message_id);
            }
            initialized = true;
            thread::sleep(plugin.poll_interval());
            continue;
        }

        for message in messages {
            if !remember_message_id(&mut seen, &mut seen_order, &message.message_id) {
                continue;
            }

            let user_id = plugin.user_id(&message);
            if !plugin.is_user_allowed(user_id) {
                let _ =
                    persist_chat_message("user", plugin.provider_name(), user_id, &message.text);
                let _ = persist_chat_message(
                    "assistant",
                    plugin.provider_name(),
                    user_id,
                    "Sorry, I'm just a little Botty.",
                );
                match plugin.send_reply(&message.target, "Sorry, I'm just a little Botty.") {
                    Ok(Some(sent_id)) => {
                        let _ = remember_message_id(&mut seen, &mut seen_order, &sent_id);
                    }
                    Ok(None) => {}
                    Err(err) => eprintln!("{} send message failed: {err}", plugin.provider_name()),
                }
                continue;
            }

            let normalized = normalize_line_message(&message.text);
            if normalized.is_empty() {
                continue;
            }
            let prefixed = format!("{}: {normalized}", plugin.provider_name());

            let reply = match ask_leader_guy(plugin.provider_name(), user_id, &prefixed) {
                Ok(reply) => reply,
                Err(err) => {
                    eprintln!("{} ask leader failed: {err}", plugin.provider_name());
                    err.to_string()
                }
            };
            let outbound_reply = if plugin.provider_name() == "telegram" {
                "Get ur order!".to_string()
            } else {
                reply.clone()
            };

            match plugin.send_reply(&message.target, &outbound_reply) {
                Ok(Some(sent_id)) => {
                    let _ = remember_message_id(&mut seen, &mut seen_order, &sent_id);
                }
                Ok(None) => {}
                Err(err) => eprintln!("{} send message failed: {err}", plugin.provider_name()),
            }
        }

        thread::sleep(plugin.poll_interval());
    }
}

struct TelegramProviderPlugin {
    apikey: String,
    api_base: String,
    poll_interval: Duration,
    offset: i64,
    whitelist_user_ids: HashSet<String>,
}

impl TelegramProviderPlugin {
    fn new(
        apikey: String,
        api_base: String,
        poll_interval: Duration,
        whitelist_user_ids: HashSet<String>,
    ) -> Self {
        Self {
            apikey,
            api_base,
            poll_interval,
            offset: 0,
            whitelist_user_ids,
        }
    }
}

impl ChatbotProviderPlugin for TelegramProviderPlugin {
    fn provider_name(&self) -> &'static str {
        "telegram"
    }

    fn poll_interval(&self) -> Duration {
        self.poll_interval
    }

    fn fetch_messages(&mut self) -> io::Result<Vec<InboundMessage>> {
        let updates = fetch_telegram_updates(&self.api_base, &self.apikey, self.offset)?;
        let mut messages = Vec::new();
        for update in updates {
            if update.update_id >= self.offset {
                self.offset = update.update_id + 1;
            }
            messages.push(InboundMessage {
                message_id: update.update_id.to_string(),
                target: update.chat_id.to_string(),
                user_id: update.user_id.to_string(),
                text: update.text,
            });
        }
        Ok(messages)
    }

    fn user_id<'a>(&self, message: &'a InboundMessage) -> &'a str {
        message.user_id.as_str()
    }

    fn is_user_allowed(&self, user_id: &str) -> bool {
        self.whitelist_user_ids.contains(user_id)
    }

    fn send_reply(&mut self, target: &str, text: &str) -> io::Result<Option<String>> {
        let chat_id = target
            .parse::<i64>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid telegram chat_id"))?;
        send_telegram_message(&self.api_base, &self.apikey, chat_id, text)?;
        Ok(None)
    }
}

struct FeishuProviderPlugin {
    apikey: String,
    api_base: String,
    chat_id: String,
    poll_interval: Duration,
}

impl FeishuProviderPlugin {
    fn new(apikey: String, api_base: String, chat_id: String, poll_interval: Duration) -> Self {
        Self {
            apikey,
            api_base,
            chat_id,
            poll_interval,
        }
    }
}

impl ChatbotProviderPlugin for FeishuProviderPlugin {
    fn provider_name(&self) -> &'static str {
        "feishu"
    }

    fn poll_interval(&self) -> Duration {
        self.poll_interval
    }

    fn fetch_messages(&mut self) -> io::Result<Vec<InboundMessage>> {
        let raw = fetch_feishu_messages(&self.api_base, &self.apikey, &self.chat_id)?;
        Ok(raw
            .into_iter()
            .map(|message| InboundMessage {
                message_id: message.message_id,
                target: self.chat_id.clone(),
                user_id: message.user_id,
                text: message.text,
            })
            .collect())
    }

    fn user_id<'a>(&self, message: &'a InboundMessage) -> &'a str {
        message.user_id.as_str()
    }

    fn send_reply(&mut self, target: &str, text: &str) -> io::Result<Option<String>> {
        send_feishu_message(&self.api_base, &self.apikey, target, text)
    }
}

fn guy_process_name() -> &'static str {
    if cfg!(debug_assertions) {
        "Botty-Guy-dev"
    } else {
        "Botty-Guy"
    }
}

fn telegram_input_process_name() -> &'static str {
    if cfg!(debug_assertions) {
        "Botty-input-telegram-dev"
    } else {
        "Botty-input-telegram"
    }
}

fn feishu_input_process_name() -> &'static str {
    if cfg!(debug_assertions) {
        "Botty-input-feishu-dev"
    } else {
        "Botty-input-feishu"
    }
}

struct ChatbotConfig {
    enabled: bool,
    apikey: String,
    telegram_api_base: String,
    poll_interval_seconds: u64,
    feishu_enabled: bool,
    feishu_apikey: String,
    feishu_api_base: String,
    feishu_chat_id: String,
    feishu_poll_interval_seconds: u64,
    telegram_whitelist_user_ids: HashSet<String>,
}

impl Default for ChatbotConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            apikey: String::new(),
            telegram_api_base: TELEGRAM_API_BASE.to_string(),
            poll_interval_seconds: TELEGRAM_POLL_INTERVAL_SECONDS_DEFAULT,
            feishu_enabled: false,
            feishu_apikey: String::new(),
            feishu_api_base: FEISHU_API_BASE.to_string(),
            feishu_chat_id: String::new(),
            feishu_poll_interval_seconds: FEISHU_POLL_INTERVAL_SECONDS_DEFAULT,
            telegram_whitelist_user_ids: HashSet::new(),
        }
    }
}

impl ChatbotConfig {
    fn poll_interval(&self) -> Duration {
        Duration::from_secs(self.poll_interval_seconds.max(1))
    }

    fn feishu_poll_interval(&self) -> Duration {
        Duration::from_secs(self.feishu_poll_interval_seconds.max(1))
    }
}

struct TelegramUpdate {
    update_id: i64,
    chat_id: i64,
    user_id: i64,
    text: String,
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
                config.enabled = value
                    .split(',')
                    .map(|s| s.trim())
                    .any(|provider| provider == "telegram");
                config.feishu_enabled = value
                    .split(',')
                    .map(|s| s.trim())
                    .any(|provider| provider == "feishu");
            }
            "chatbot.telegram.enabled" => config.enabled = parse_bool(value),
            "chatbot.telegram.apikey" => config.apikey = value.to_string(),
            "chatbot.telegram.api_base" => config.telegram_api_base = value.to_string(),
            "chatbot.telegram.whitelist_user_ids" => {
                config.telegram_whitelist_user_ids = parse_user_id_whitelist(value);
            }
            "chatbot.telegram.whitelise_user_ids" => {
                config.telegram_whitelist_user_ids = parse_user_id_whitelist(value);
            }
            "chatbot.feishu.enabled" => config.feishu_enabled = parse_bool(value),
            "chatbot.feishu.apikey" => config.feishu_apikey = value.to_string(),
            "chatbot.feishu.api_base" => config.feishu_api_base = value.to_string(),
            "chatbot.feishu.chat_id" => config.feishu_chat_id = value.to_string(),
            "chatbot.apikey" => {
                if config.apikey.is_empty() {
                    config.apikey = value.to_string();
                }
                if config.feishu_apikey.is_empty() {
                    config.feishu_apikey = value.to_string();
                }
            }
            "chatbot.telegram.poll_interval_seconds" => {
                if let Ok(seconds) = value.parse::<u64>() {
                    config.poll_interval_seconds = seconds.max(1);
                }
            }
            "chatbot.feishu.poll_interval_seconds" => {
                if let Ok(seconds) = value.parse::<u64>() {
                    config.feishu_poll_interval_seconds = seconds.max(1);
                }
            }
            _ => {}
        }
    }

    Ok(config)
}

fn parse_user_id_whitelist(value: &str) -> HashSet<String> {
    value
        .split(',')
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .map(|item| item.to_string())
        .collect()
}

fn persist_chat_message(role: &str, source: &str, user_id: &str, message: &str) -> io::Result<()> {
    let year = local_time_format("%Y")?;
    let month_day = local_time_format("%m%d")?;
    let timestamp = local_time_format("%Y-%m-%d %H:%M:%S")?;
    let sanitized = message.replace('\n', "\\n").replace('\r', "\\r");
    let line = format!("[{timestamp}] source={source} user_id={user_id} {role}: {sanitized}\n");

    let dir = botty_root_dir().join("memory").join("deep").join(year);
    fs::create_dir_all(&dir)?;

    let target = select_chat_memory_file(&dir, &month_day, line.len() as u64)?;
    let mut file = OpenOptions::new().create(true).append(true).open(target)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

fn select_chat_memory_file(
    dir: &PathBuf,
    month_day: &str,
    incoming_bytes: u64,
) -> io::Result<PathBuf> {
    for index in 1..=9_999u32 {
        let candidate = dir.join(format!("{month_day}-{index}.log"));
        let size = match fs::metadata(&candidate) {
            Ok(meta) => meta.len(),
            Err(err) if err.kind() == io::ErrorKind::NotFound => 0,
            Err(err) => return Err(err),
        };

        if size.saturating_add(incoming_bytes) <= CHAT_MEMORY_MAX_BYTES {
            return Ok(candidate);
        }
    }

    Err(io::Error::other(
        "too many chat memory files for current day",
    ))
}

fn local_time_format(format: &str) -> io::Result<String> {
    let output = Command::new("date").arg(format!("+{format}")).output()?;
    if !output.status.success() {
        return Err(io::Error::other("failed to get local time by date command"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn fetch_telegram_updates(
    api_base: &str,
    apikey: &str,
    offset: i64,
) -> io::Result<Vec<TelegramUpdate>> {
    let url = format!(
        "{api_base}/bot{apikey}/getUpdates?timeout=0&offset={offset}&allowed_updates=%5B%22message%22%5D"
    );
    let output = Command::new("curl").arg("-fsS").arg(url).output()?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!(
            "curl getUpdates failed: {}",
            detail.trim()
        )));
    }
    let body = String::from_utf8_lossy(&output.stdout);
    Ok(parse_telegram_updates(&body))
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
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!(
            "curl sendMessage failed: {}",
            detail.trim()
        )));
    }
    Ok(())
}

struct FeishuMessage {
    message_id: String,
    user_id: String,
    text: String,
}

fn fetch_feishu_messages(
    api_base: &str,
    apikey: &str,
    chat_id: &str,
) -> io::Result<Vec<FeishuMessage>> {
    let url = format!(
        "{api_base}/im/v1/messages?container_id_type=chat&container_id={chat_id}&sort_type=ByCreateTimeAsc&page_size=20"
    );
    let output = Command::new("curl")
        .arg("-fsS")
        .arg(url)
        .arg("-H")
        .arg(format!("Authorization: Bearer {apikey}"))
        .output()?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!(
            "curl feishu list messages failed: {}",
            detail.trim()
        )));
    }
    let body = String::from_utf8_lossy(&output.stdout);
    Ok(parse_feishu_messages(&body))
}

fn send_feishu_message(
    api_base: &str,
    apikey: &str,
    chat_id: &str,
    text: &str,
) -> io::Result<Option<String>> {
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
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!(
            "curl feishu send message failed: {}",
            detail.trim()
        )));
    }
    let body = String::from_utf8_lossy(&output.stdout);
    Ok(parse_string_field(&body, "\"message_id\""))
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
    let decoded = decode_ipc_line(reply.trim_end())?;
    Ok(decoded)
}

fn encode_assistant_reply(reply: &AssistantReply) -> String {
    json!({
        "text": reply.text,
        "thinking": reply.thinking,
    })
    .to_string()
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

fn parse_telegram_updates(body: &str) -> Vec<TelegramUpdate> {
    let mut updates = Vec::new();
    let mut start = 0usize;

    while let Some(rel) = body[start..].find("\"update_id\"") {
        let abs = start + rel;
        let end = match body[abs + 1..].find("\"update_id\"") {
            Some(next_rel) => abs + 1 + next_rel,
            None => body.len(),
        };
        let chunk = &body[abs..end];

        if let (Some(update_id), Some(chat_id), Some(user_id), Some(text)) = (
            parse_number_field(chunk, "\"update_id\""),
            parse_number_field(chunk, "\"chat\""),
            parse_number_field(chunk, "\"from\""),
            parse_string_field(chunk, "\"text\""),
        ) {
            updates.push(TelegramUpdate {
                update_id,
                chat_id,
                user_id,
                text,
            });
        }

        start = end;
    }

    updates
}

fn parse_feishu_messages(body: &str) -> Vec<FeishuMessage> {
    let mut messages = Vec::new();
    let mut start = 0usize;

    while let Some(rel) = body[start..].find("\"message_id\"") {
        let abs = start + rel;
        let end = match body[abs + 1..].find("\"message_id\"") {
            Some(next_rel) => abs + 1 + next_rel,
            None => body.len(),
        };
        let chunk = &body[abs..end];

        let Some(message_id) = parse_string_field(chunk, "\"message_id\"") else {
            start = end;
            continue;
        };

        let text = if let Some(raw_content) = parse_string_field(chunk, "\"content\"") {
            extract_text_from_content(&raw_content).unwrap_or_default()
        } else {
            String::new()
        };

        let user_id = parse_string_field(chunk, "\"sender_id\"")
            .or_else(|| parse_string_field(chunk, "\"open_id\""))
            .or_else(|| parse_string_field(chunk, "\"user_id\""))
            .unwrap_or_else(|| "unknown".to_string());

        messages.push(FeishuMessage {
            message_id,
            user_id,
            text,
        });
        start = end;
    }

    messages
}

fn extract_text_from_content(content: &str) -> Option<String> {
    parse_string_field(content, "\"text\"")
}

fn parse_number_field(chunk: &str, field_name: &str) -> Option<i64> {
    if field_name == "\"chat\"" || field_name == "\"from\"" {
        let object_idx = chunk.find(field_name)?;
        let object_part = &chunk[object_idx..];
        let id_idx = object_part.find("\"id\"")?;
        parse_number_after_colon(&object_part[id_idx + 4..])
    } else {
        let idx = chunk.find(field_name)?;
        parse_number_after_colon(&chunk[idx + field_name.len()..])
    }
}

fn parse_number_after_colon(s: &str) -> Option<i64> {
    let colon = s.find(':')?;
    let rest = s[colon + 1..].trim_start();
    let mut end = 0usize;
    for (i, ch) in rest.char_indices() {
        if ch.is_ascii_digit() || (i == 0 && ch == '-') {
            end = i + ch.len_utf8();
            continue;
        }
        break;
    }
    if end == 0 {
        return None;
    }
    rest[..end].parse::<i64>().ok()
}

fn parse_string_field(chunk: &str, field_name: &str) -> Option<String> {
    let idx = chunk.find(field_name)?;
    let after = &chunk[idx + field_name.len()..];
    let colon = after.find(':')?;
    let value = after[colon + 1..].trim_start();
    if !value.starts_with('"') {
        return None;
    }
    let mut out = String::new();
    let mut chars = value[1..].chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '"' {
            return Some(out);
        }

        if ch != '\\' {
            out.push(ch);
            continue;
        }

        let escaped = chars.next()?;
        match escaped {
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            '"' => out.push('"'),
            '\\' => out.push('\\'),
            '/' => out.push('/'),
            'b' => out.push('\u{0008}'),
            'f' => out.push('\u{000C}'),
            'u' => {
                let cp1 = parse_u16_hex_from_chars(&mut chars)?;
                if (0xD800..=0xDBFF).contains(&cp1) {
                    let backslash = chars.next()?;
                    let u = chars.next()?;
                    if backslash != '\\' || u != 'u' {
                        return None;
                    }
                    let cp2 = parse_u16_hex_from_chars(&mut chars)?;
                    if !(0xDC00..=0xDFFF).contains(&cp2) {
                        return None;
                    }
                    let high = (cp1 as u32) - 0xD800;
                    let low = (cp2 as u32) - 0xDC00;
                    let code = 0x10000 + ((high << 10) | low);
                    out.push(char::from_u32(code)?);
                } else if (0xDC00..=0xDFFF).contains(&cp1) {
                    return None;
                } else {
                    out.push(char::from_u32(cp1 as u32)?);
                }
            }
            _ => return None,
        }
    }
    None
}

fn parse_u16_hex_from_chars(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<u16> {
    let mut hex = String::with_capacity(4);
    for _ in 0..4 {
        let ch = chars.next()?;
        if !ch.is_ascii_hexdigit() {
            return None;
        }
        hex.push(ch);
    }
    u16::from_str_radix(&hex, 16).ok()
}

fn normalize_line_message(message: &str) -> String {
    message
        .replace('\n', " ")
        .replace('\r', " ")
        .trim()
        .to_string()
}

fn remember_message_id(
    seen: &mut HashSet<String>,
    seen_order: &mut VecDeque<String>,
    message_id: &str,
) -> bool {
    if seen.contains(message_id) {
        return false;
    }
    let owned = message_id.to_string();
    seen.insert(owned.clone());
    seen_order.push_back(owned);

    while seen_order.len() > FEISHU_SEEN_CACHE_LIMIT {
        if let Some(oldest) = seen_order.pop_front() {
            seen.remove(&oldest);
        }
    }
    true
}

fn escape_json_string(text: &str) -> String {
    text.chars()
        .flat_map(|ch| match ch {
            '\\' => "\\\\".chars().collect::<Vec<_>>(),
            '"' => "\\\"".chars().collect::<Vec<_>>(),
            '\n' => "\\n".chars().collect::<Vec<_>>(),
            '\r' => "\\r".chars().collect::<Vec<_>>(),
            '\t' => "\\t".chars().collect::<Vec<_>>(),
            _ => vec![ch],
        })
        .collect()
}

fn parse_bool(value: &str) -> bool {
    matches!(value.trim(), "1" | "true" | "yes" | "on")
}

fn setup_config_file() -> PathBuf {
    botty_root_dir()
        .join("config")
        .join(format!("setup{}.conf", runtime_suffix()))
}

fn botty_root_dir() -> PathBuf {
    std::env::var_os("HOME")
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

fn set_process_name(name: &str) {
    #[cfg(target_os = "linux")]
    {
        if let Ok(c_name) = CString::new(name) {
            unsafe {
                libc::prctl(libc::PR_SET_NAME, c_name.as_ptr() as usize, 0, 0, 0);
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(c_name) = CString::new(name) {
            unsafe {
                libc::pthread_setname_np(c_name.as_ptr());
            }
        }
    }
}
