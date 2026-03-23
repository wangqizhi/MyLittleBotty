use base64::Engine;
use serde::Deserialize;
use serde_json::json;
use std::fs::File;
use std::io::{self, Read};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_API_BASE: &str = "https://ilinkai.weixin.qq.com";
pub const DEFAULT_BOT_TYPE: &str = "3";
pub const SESSION_EXPIRED_ERRCODE: i32 = -14;

#[derive(Clone, Debug)]
pub struct WeixinInboundMessage {
    pub message_id: String,
    pub user_id: String,
    pub text: String,
    pub context_token: String,
}

#[derive(Clone, Debug)]
pub struct WeixinLoginStart {
    pub qrcode_url: String,
    pub qrcode: String,
}

#[derive(Clone, Debug)]
pub struct WeixinLoginResult {
    pub bot_token: String,
    pub account_id: String,
    pub user_id: String,
    pub base_url: String,
}

pub struct WeixinUpdatesResult {
    pub messages: Vec<WeixinInboundMessage>,
    pub get_updates_buf: String,
    pub longpolling_timeout_ms: Option<u64>,
}

pub struct WeixinClient {
    api_base: String,
    bot_token: String,
}

impl WeixinClient {
    pub fn new(api_base: String, bot_token: String) -> Self {
        Self {
            api_base,
            bot_token,
        }
    }

    pub fn start_login(&self) -> io::Result<WeixinLoginStart> {
        let url = format!(
            "{}/ilink/bot/get_bot_qrcode?bot_type={}",
            trim_trailing_slash(&self.api_base),
            DEFAULT_BOT_TYPE
        );
        let body = run_curl_get(&url, &[], 15, "weixin get_bot_qrcode")?;
        let response: WeixinQrCodeResponse = parse_json_response(&body, "weixin get_bot_qrcode")?;
        let qrcode_url = response.qrcode_img_content.trim().to_string();
        let qrcode = response.qrcode.trim().to_string();
        if qrcode_url.is_empty() || qrcode.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "weixin get_bot_qrcode returned empty qrcode data",
            ));
        }
        Ok(WeixinLoginStart { qrcode_url, qrcode })
    }

    pub fn poll_login_status(&self, qrcode: &str) -> io::Result<Option<WeixinLoginResult>> {
        let url = format!(
            "{}/ilink/bot/get_qrcode_status?qrcode={}",
            trim_trailing_slash(&self.api_base),
            encode_query_component(qrcode)
        );
        let headers = [String::from("iLink-App-ClientVersion: 1")];
        let body = run_curl_get(&url, &headers, 40, "weixin get_qrcode_status")?;
        let response: WeixinQrStatusResponse =
            parse_json_response(&body, "weixin get_qrcode_status")?;
        match response.status.trim() {
            "wait" | "scaned" => Ok(None),
            "confirmed" => {
                let bot_token = response.bot_token.unwrap_or_default().trim().to_string();
                let account_id = response.ilink_bot_id.unwrap_or_default().trim().to_string();
                let user_id = response.ilink_user_id.unwrap_or_default().trim().to_string();
                let base_url = response
                    .baseurl
                    .unwrap_or_else(|| self.api_base.clone())
                    .trim()
                    .to_string();
                if bot_token.is_empty() || account_id.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "weixin login confirmed but bot token/account id is missing",
                    ));
                }
                Ok(Some(WeixinLoginResult {
                    bot_token,
                    account_id,
                    user_id,
                    base_url,
                }))
            }
            "expired" => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "weixin login QR code expired",
            )),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown weixin login status: {other}"),
            )),
        }
    }

    pub fn fetch_updates(
        &self,
        get_updates_buf: &str,
        timeout_ms: u64,
    ) -> io::Result<WeixinUpdatesResult> {
        let url = format!("{}/ilink/bot/getupdates", trim_trailing_slash(&self.api_base));
        let payload = json!({
            "get_updates_buf": get_updates_buf,
            "base_info": {
                "channel_version": env!("CARGO_PKG_VERSION"),
            },
        })
        .to_string();
        let headers = build_weixin_headers(&self.bot_token, &payload);
        let max_time_secs = ((timeout_ms.saturating_add(5_000)) / 1_000).max(10);
        let body = run_curl_post(&url, &headers, &payload, max_time_secs, "weixin getupdates")?;
        let response: WeixinGetUpdatesResponse =
            parse_json_response(&body, "weixin getupdates")?;
        let ret = response.ret.unwrap_or(0);
        let errcode = response.errcode.unwrap_or(0);
        if ret != 0 || errcode != 0 {
            let detail = response.errmsg.unwrap_or_default();
            return Err(io::Error::other(format!(
                "weixin getupdates failed: ret={ret} errcode={errcode} errmsg={detail}"
            )));
        }
        let mut messages = Vec::new();
        for message in response.msgs.unwrap_or_default() {
            if message.message_type.unwrap_or(0) != 1 {
                continue;
            }
            let user_id = message.from_user_id.unwrap_or_default().trim().to_string();
            let context_token = message.context_token.unwrap_or_default().trim().to_string();
            let text = extract_message_text(&message.item_list);
            if user_id.is_empty() || context_token.is_empty() || text.trim().is_empty() {
                continue;
            }
            let message_id = message
                .message_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| fallback_message_id(&user_id, message.create_time_ms));
            messages.push(WeixinInboundMessage {
                message_id,
                user_id,
                text,
                context_token,
            });
        }
        Ok(WeixinUpdatesResult {
            messages,
            get_updates_buf: response.get_updates_buf.unwrap_or_default(),
            longpolling_timeout_ms: response.longpolling_timeout_ms,
        })
    }

    pub fn send_message(&self, to_user_id: &str, text: &str, context_token: &str) -> io::Result<()> {
        let url = format!("{}/ilink/bot/sendmessage", trim_trailing_slash(&self.api_base));
        let payload = json!({
            "msg": {
                "from_user_id": "",
                "to_user_id": to_user_id,
                "client_id": fallback_message_id(to_user_id, None),
                "message_type": 2,
                "message_state": 2,
                "item_list": [
                    {
                        "type": 1,
                        "text_item": {
                            "text": text,
                        }
                    }
                ],
                "context_token": context_token,
            },
            "base_info": {
                "channel_version": env!("CARGO_PKG_VERSION"),
            },
        })
        .to_string();
        let headers = build_weixin_headers(&self.bot_token, &payload);
        let _ = run_curl_post(&url, &headers, &payload, 20, "weixin sendmessage")?;
        Ok(())
    }
}

#[derive(Deserialize)]
struct WeixinQrCodeResponse {
    qrcode: String,
    qrcode_img_content: String,
}

#[derive(Deserialize)]
struct WeixinQrStatusResponse {
    status: String,
    bot_token: Option<String>,
    ilink_bot_id: Option<String>,
    baseurl: Option<String>,
    ilink_user_id: Option<String>,
}

#[derive(Deserialize)]
struct WeixinGetUpdatesResponse {
    ret: Option<i32>,
    errcode: Option<i32>,
    errmsg: Option<String>,
    msgs: Option<Vec<WeixinRawMessage>>,
    get_updates_buf: Option<String>,
    longpolling_timeout_ms: Option<u64>,
}

#[derive(Deserialize)]
struct WeixinRawMessage {
    message_id: Option<i64>,
    from_user_id: Option<String>,
    create_time_ms: Option<u64>,
    message_type: Option<i32>,
    item_list: Option<Vec<WeixinMessageItem>>,
    context_token: Option<String>,
}

#[derive(Deserialize)]
struct WeixinMessageItem {
    #[serde(rename = "type")]
    item_type: Option<i32>,
    text_item: Option<WeixinTextItem>,
    voice_item: Option<WeixinVoiceItem>,
    ref_msg: Option<WeixinRefMessage>,
}

#[derive(Deserialize)]
struct WeixinTextItem {
    text: Option<String>,
}

#[derive(Deserialize)]
struct WeixinVoiceItem {
    text: Option<String>,
}

#[derive(Deserialize)]
struct WeixinRefMessage {
    title: Option<String>,
    message_item: Option<Box<WeixinMessageItem>>,
}

fn extract_message_text(item_list: &Option<Vec<WeixinMessageItem>>) -> String {
    let Some(items) = item_list.as_ref() else {
        return String::new();
    };
    for item in items {
        if item.item_type == Some(1) {
            let text = item
                .text_item
                .as_ref()
                .and_then(|entry| entry.text.as_deref())
                .unwrap_or("")
                .trim();
            if text.is_empty() {
                continue;
            }
            if let Some(reference) = &item.ref_msg {
                let mut parts = Vec::new();
                if let Some(title) = reference.title.as_deref() {
                    let title = title.trim();
                    if !title.is_empty() {
                        parts.push(title.to_string());
                    }
                }
                if let Some(message_item) = &reference.message_item {
                    let nested = extract_message_text(&Some(vec![(**message_item).to_owned()]));
                    if !nested.trim().is_empty() {
                        parts.push(nested.trim().to_string());
                    }
                }
                if !parts.is_empty() {
                    return format!("[引用: {}]\n{text}", parts.join(" | "));
                }
            }
            return text.to_string();
        }
        if item.item_type == Some(3) {
            let text = item
                .voice_item
                .as_ref()
                .and_then(|entry| entry.text.as_deref())
                .unwrap_or("")
                .trim();
            if !text.is_empty() {
                return text.to_string();
            }
        }
    }
    String::new()
}

fn fallback_message_id(user_id: &str, timestamp_ms: Option<u64>) -> String {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    let ts = timestamp_ms.unwrap_or(now_ms);
    format!("wx-{user_id}-{ts}")
}

fn build_weixin_headers(token: &str, body: &str) -> Vec<String> {
    let mut headers = vec![
        String::from("Content-Type: application/json"),
        String::from("AuthorizationType: ilink_bot_token"),
        format!("Content-Length: {}", body.len()),
        format!("X-WECHAT-UIN: {}", random_wechat_uin()),
    ];
    if !token.trim().is_empty() {
        headers.push(format!("Authorization: Bearer {}", token.trim()));
    }
    headers
}

fn random_wechat_uin() -> String {
    let mut bytes = [0u8; 4];
    if File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .is_err()
    {
        let fallback = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.subsec_nanos())
            .unwrap_or(0);
        bytes = fallback.to_be_bytes();
    }
    let value = u32::from_be_bytes(bytes).to_string();
    base64::engine::general_purpose::STANDARD.encode(value)
}

fn trim_trailing_slash(value: &str) -> &str {
    value.trim_end_matches('/')
}

fn encode_query_component(value: &str) -> String {
    value
        .bytes()
        .flat_map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![b as char]
            }
            _ => format!("%{b:02X}").chars().collect(),
        })
        .collect()
}

fn run_curl_get(
    url: &str,
    headers: &[String],
    max_time_secs: u64,
    action: &str,
) -> io::Result<String> {
    let mut command = Command::new("curl");
    command.arg("-fsS").arg("--max-time").arg(max_time_secs.to_string());
    for header in headers {
        command.arg("-H").arg(header);
    }
    command.arg(url);
    run_curl_command(command, action)
}

fn run_curl_post(
    url: &str,
    headers: &[String],
    body: &str,
    max_time_secs: u64,
    action: &str,
) -> io::Result<String> {
    let mut command = Command::new("curl");
    command
        .arg("-fsS")
        .arg("--max-time")
        .arg(max_time_secs.to_string())
        .arg("-X")
        .arg("POST");
    for header in headers {
        command.arg("-H").arg(header);
    }
    command.arg("--data-binary").arg(body).arg(url);
    run_curl_command(command, action)
}

fn run_curl_command(mut command: Command, action: &str) -> io::Result<String> {
    let output = command.output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let code = output
            .status
            .code()
            .map(|value| value.to_string())
            .unwrap_or_else(|| "signal".to_string());
        return Err(io::Error::other(format!(
            "curl {action} failed: exit_code={code}, stderr={}",
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn parse_json_response<T>(body: &str, action: &str) -> io::Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_str(body).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{action} returned invalid json: {err}"),
        )
    })
}

impl Clone for WeixinMessageItem {
    fn clone(&self) -> Self {
        Self {
            item_type: self.item_type,
            text_item: self.text_item.as_ref().map(|item| WeixinTextItem {
                text: item.text.clone(),
            }),
            voice_item: self.voice_item.as_ref().map(|item| WeixinVoiceItem {
                text: item.text.clone(),
            }),
            ref_msg: self.ref_msg.as_ref().map(|reference| WeixinRefMessage {
                title: reference.title.clone(),
                message_item: reference
                    .message_item
                    .as_ref()
                    .map(|item| Box::new((**item).clone())),
            }),
        }
    }
}
