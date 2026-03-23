use base64::Engine;
use openssl::symm::{decrypt, Cipher};
use serde::Deserialize;
use serde_json::json;
use std::fs::File;
use std::io::{self, Read};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_API_BASE: &str = "https://ilinkai.weixin.qq.com";
pub const DEFAULT_CDN_BASE: &str = "https://novac2c.cdn.weixin.qq.com/c2c";
pub const DEFAULT_BOT_TYPE: &str = "3";
pub const SESSION_EXPIRED_ERRCODE: i32 = -14;

#[derive(Clone, Debug)]
pub struct WeixinInboundMessage {
    pub message_id: String,
    pub user_id: String,
    pub text: String,
    pub context_token: String,
    pub images: Vec<WeixinInboundImage>,
}

#[derive(Clone, Debug)]
pub struct WeixinInboundImage {
    pub local_path: String,
    pub mime_type: Option<String>,
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
    cdn_base: String,
    bot_token: String,
}

impl WeixinClient {
    pub fn new(api_base: String, cdn_base: String, bot_token: String) -> Self {
        Self {
            api_base,
            cdn_base,
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
                let user_id = response
                    .ilink_user_id
                    .unwrap_or_default()
                    .trim()
                    .to_string();
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
        let url = format!(
            "{}/ilink/bot/getupdates",
            trim_trailing_slash(&self.api_base)
        );
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
        let response: WeixinGetUpdatesResponse = parse_json_response(&body, "weixin getupdates")?;
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
            let images = match extract_message_images(
                &message.item_list,
                &self.bot_token,
                &self.cdn_base,
                &user_id,
            ) {
                Ok(images) => images,
                Err(err) => {
                    eprintln!("weixin extract image failed for user_id={user_id}: {err}");
                    Vec::new()
                }
            };
            if user_id.is_empty()
                || context_token.is_empty()
                || (text.trim().is_empty() && images.is_empty())
            {
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
                images,
            });
        }
        Ok(WeixinUpdatesResult {
            messages,
            get_updates_buf: response.get_updates_buf.unwrap_or_default(),
            longpolling_timeout_ms: response.longpolling_timeout_ms,
        })
    }

    pub fn send_message(
        &self,
        to_user_id: &str,
        text: &str,
        context_token: &str,
    ) -> io::Result<()> {
        let url = format!(
            "{}/ilink/bot/sendmessage",
            trim_trailing_slash(&self.api_base)
        );
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
    image_item: Option<WeixinImageItem>,
    pic_item: Option<WeixinImageItem>,
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
struct WeixinImageItem {
    media: Option<WeixinCdnMedia>,
    thumb_media: Option<WeixinCdnMedia>,
    aeskey: Option<String>,
    url: Option<String>,
    cdn_url: Option<String>,
    download_url: Option<String>,
    mime_type: Option<String>,
    file_ext: Option<String>,
    base64: Option<String>,
}

#[derive(Deserialize)]
struct WeixinCdnMedia {
    encrypt_query_param: Option<String>,
    aes_key: Option<String>,
    encrypt_type: Option<i32>,
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

fn extract_message_images(
    item_list: &Option<Vec<WeixinMessageItem>>,
    token: &str,
    api_base: &str,
    user_id: &str,
) -> io::Result<Vec<WeixinInboundImage>> {
    let Some(items) = item_list.as_ref() else {
        return Ok(Vec::new());
    };
    let mut images = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let image = item
            .image_item
            .as_ref()
            .or(item.pic_item.as_ref())
            .filter(|_| item.item_type == Some(2) || item.item_type == Some(4));
        let Some(image) = image else {
            continue;
        };
        if let Some(encrypt_query_param) = image
            .media
            .as_ref()
            .and_then(|media| media.encrypt_query_param.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let ext = image_file_extension(image);
            let path = temp_weixin_image_path(user_id, index, &ext);
            download_and_decrypt_weixin_image(
                encrypt_query_param,
                image_aes_key_base64(image),
                api_base,
                &path,
            )?;
            images.push(WeixinInboundImage {
                local_path: path.to_string_lossy().to_string(),
                mime_type: image
                    .mime_type
                    .clone()
                    .or_else(|| infer_mime_from_ext(&ext)),
            });
            continue;
        }
        if let Some(base64_data) = image.base64.as_deref() {
            let ext = image_file_extension(image);
            let path = temp_weixin_image_path(user_id, index, &ext);
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(base64_data.trim())
                .map_err(|err| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("decode weixin image base64 failed: {err}"),
                    )
                })?;
            std::fs::write(&path, bytes)?;
            images.push(WeixinInboundImage {
                local_path: path.to_string_lossy().to_string(),
                mime_type: image
                    .mime_type
                    .clone()
                    .or_else(|| infer_mime_from_ext(&ext)),
            });
            continue;
        }

        let Some(url) = image
            .download_url
            .as_deref()
            .or(image.cdn_url.as_deref())
            .or(image.url.as_deref())
        else {
            continue;
        };
        if !looks_like_downloadable_url(url) {
            eprintln!(
                "weixin image payload ignored because url is not downloadable: user_id={user_id} item_index={index} raw_url={} cdn_url={} download_url={} encrypt_query_param={} encrypt_type={} mime_type={} file_ext={} has_base64={}",
                truncate_for_log(url),
                truncate_for_log(image.cdn_url.as_deref().unwrap_or("")),
                truncate_for_log(image.download_url.as_deref().unwrap_or("")),
                truncate_for_log(
                    image
                        .media
                        .as_ref()
                        .and_then(|media| media.encrypt_query_param.as_deref())
                        .unwrap_or(""),
                ),
                image
                    .media
                    .as_ref()
                    .and_then(|media| media.encrypt_type)
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
                image.mime_type.as_deref().unwrap_or(""),
                image.file_ext.as_deref().unwrap_or(""),
                image
                    .base64
                    .as_deref()
                    .map(|value| !value.trim().is_empty())
                    .unwrap_or(false)
            );
            continue;
        }
        let ext = image_file_extension(image);
        let path = temp_weixin_image_path(user_id, index, &ext);
        download_weixin_image(url, token, api_base, &path)?;
        images.push(WeixinInboundImage {
            local_path: path.to_string_lossy().to_string(),
            mime_type: image
                .mime_type
                .clone()
                .or_else(|| infer_mime_from_ext(&ext)),
        });
    }
    Ok(images)
}

fn image_file_extension(image: &WeixinImageItem) -> String {
    image
        .file_ext
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("jpg")
        .to_string()
}

fn image_aes_key_base64(image: &WeixinImageItem) -> Option<String> {
    if let Some(aeskey) = image
        .aeskey
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let bytes = decode_hex_bytes(aeskey).ok()?;
        return Some(base64::engine::general_purpose::STANDARD.encode(bytes));
    }
    image
        .media
        .as_ref()
        .and_then(|media| media.aes_key.as_ref())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn download_weixin_image(
    url: &str,
    token: &str,
    api_base: &str,
    path: &std::path::Path,
) -> io::Result<()> {
    let mut command = Command::new("curl");
    command.arg("-fsS").arg("-L");
    if !token.trim().is_empty() {
        command
            .arg("-H")
            .arg(format!("Authorization: Bearer {}", token.trim()));
    }
    command
        .arg("-H")
        .arg("AuthorizationType: ilink_bot_token")
        .arg("-o")
        .arg(path)
        .arg(normalize_weixin_media_url(url, api_base));
    let output = command.output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "curl weixin download image failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

fn download_and_decrypt_weixin_image(
    encrypt_query_param: &str,
    aes_key_base64: Option<String>,
    api_base: &str,
    path: &std::path::Path,
) -> io::Result<()> {
    let url = build_weixin_cdn_download_url(encrypt_query_param, api_base);
    let encrypted = download_weixin_image_bytes(&url)?;
    let plaintext = if let Some(aes_key_base64) = aes_key_base64 {
        decrypt_weixin_media_bytes(&encrypted, &aes_key_base64)?
    } else {
        encrypted
    };
    std::fs::write(path, plaintext)
}

fn download_weixin_image_bytes(url: &str) -> io::Result<Vec<u8>> {
    let output = Command::new("curl")
        .arg("-fsS")
        .arg("-L")
        .arg(url)
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "curl weixin download image failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output.stdout)
}

fn decrypt_weixin_media_bytes(ciphertext: &[u8], aes_key_base64: &str) -> io::Result<Vec<u8>> {
    let key = parse_weixin_aes_key(aes_key_base64)?;
    decrypt(Cipher::aes_128_ecb(), &key, None, ciphertext).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("decrypt weixin media failed: {err}"),
        )
    })
}

fn parse_weixin_aes_key(aes_key_base64: &str) -> io::Result<Vec<u8>> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(aes_key_base64.trim())
        .map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("decode weixin aes key failed: {err}"),
            )
        })?;
    if decoded.len() == 16 {
        return Ok(decoded);
    }
    if decoded.len() == 32 && decoded.iter().all(|byte| byte.is_ascii_hexdigit()) {
        return decode_hex_bytes(std::str::from_utf8(&decoded).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("weixin aes hex key is not valid utf-8: {err}"),
            )
        })?);
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "weixin aes key must decode to 16 bytes or 32-char hex, got {} bytes",
            decoded.len()
        ),
    ))
}

fn decode_hex_bytes(value: &str) -> io::Result<Vec<u8>> {
    let trimmed = value.trim();
    if trimmed.len() % 2 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "hex string length must be even",
        ));
    }
    let mut bytes = Vec::with_capacity(trimmed.len() / 2);
    let raw = trimmed.as_bytes();
    for index in (0..raw.len()).step_by(2) {
        let high = decode_hex_nibble(raw[index])?;
        let low = decode_hex_nibble(raw[index + 1])?;
        bytes.push((high << 4) | low);
    }
    Ok(bytes)
}

fn decode_hex_nibble(byte: u8) -> io::Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid hex character: {}", byte as char),
        )),
    }
}

fn build_weixin_cdn_download_url(encrypt_query_param: &str, api_base: &str) -> String {
    format!(
        "{}/download?encrypted_query_param={}",
        trim_trailing_slash(api_base),
        encode_query_component(encrypt_query_param)
    )
}

fn normalize_weixin_media_url(url: &str, api_base: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else if url.starts_with('/') {
        format!("{}{}", trim_trailing_slash(api_base), url)
    } else {
        url.to_string()
    }
}

fn looks_like_downloadable_url(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return true;
    }
    if trimmed.starts_with('/') {
        return true;
    }
    false
}

fn truncate_for_log(value: &str) -> String {
    const LIMIT: usize = 160;
    let trimmed = value.trim();
    if trimmed.len() <= LIMIT {
        trimmed.to_string()
    } else {
        format!("{}...", &trimmed[..LIMIT])
    }
}

fn temp_weixin_image_path(user_id: &str, index: usize, ext: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("mylittlebotty-chatbot-images");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(format!(
        "weixin-{}-{index}.{ext}",
        user_id.replace('/', "-").replace(':', "-")
    ))
}

fn infer_mime_from_ext(ext: &str) -> Option<String> {
    let lower = ext.trim().to_ascii_lowercase();
    let mime = match lower.as_str() {
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "jpeg" | "jpg" => "image/jpeg",
        _ => return None,
    };
    Some(mime.to_string())
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
    command
        .arg("-fsS")
        .arg("--max-time")
        .arg(max_time_secs.to_string());
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
            image_item: self.image_item.as_ref().map(|item| WeixinImageItem {
                media: item.media.as_ref().map(|media| WeixinCdnMedia {
                    encrypt_query_param: media.encrypt_query_param.clone(),
                    aes_key: media.aes_key.clone(),
                    encrypt_type: media.encrypt_type,
                }),
                thumb_media: item.thumb_media.as_ref().map(|media| WeixinCdnMedia {
                    encrypt_query_param: media.encrypt_query_param.clone(),
                    aes_key: media.aes_key.clone(),
                    encrypt_type: media.encrypt_type,
                }),
                aeskey: item.aeskey.clone(),
                url: item.url.clone(),
                cdn_url: item.cdn_url.clone(),
                download_url: item.download_url.clone(),
                mime_type: item.mime_type.clone(),
                file_ext: item.file_ext.clone(),
                base64: item.base64.clone(),
            }),
            pic_item: self.pic_item.as_ref().map(|item| WeixinImageItem {
                media: item.media.as_ref().map(|media| WeixinCdnMedia {
                    encrypt_query_param: media.encrypt_query_param.clone(),
                    aes_key: media.aes_key.clone(),
                    encrypt_type: media.encrypt_type,
                }),
                thumb_media: item.thumb_media.as_ref().map(|media| WeixinCdnMedia {
                    encrypt_query_param: media.encrypt_query_param.clone(),
                    aes_key: media.aes_key.clone(),
                    encrypt_type: media.encrypt_type,
                }),
                aeskey: item.aeskey.clone(),
                url: item.url.clone(),
                cdn_url: item.cdn_url.clone(),
                download_url: item.download_url.clone(),
                mime_type: item.mime_type.clone(),
                file_ext: item.file_ext.clone(),
                base64: item.base64.clone(),
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
