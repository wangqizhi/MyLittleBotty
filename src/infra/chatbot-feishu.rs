use native_tls::TlsStream;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::io;
use std::net::TcpStream;
use std::process::Command;
use std::time::SystemTime;
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, Message, WebSocket};

pub const DEFAULT_API_BASE: &str = "https://open.feishu.cn/open-apis";

const FEISHU_WS_CONFIG_PATH: &str = "/callback/ws/endpoint";
const FEISHU_WS_EVENT_TYPE: &str = "im.message.receive_v1";
const FEISHU_WS_MESSAGE_TYPE_EVENT: &str = "event";
const FEISHU_WS_MESSAGE_TYPE_PING: &str = "ping";
const FEISHU_WS_MESSAGE_TYPE_PONG: &str = "pong";
const FEISHU_WS_FRAME_CONTROL: i32 = 0;
const FEISHU_WS_FRAME_DATA: i32 = 1;
const FEISHU_WS_CACHE_TTL: Duration = Duration::from_secs(10);
const FEISHU_WS_READ_TIMEOUT: Duration = Duration::from_secs(1);
const FEISHU_WS_WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const CURL_HTTP_CODE_MARKER: &str = "__botty_http_code__:";

#[derive(Clone, Debug)]
pub struct FeishuInboundMessage {
    pub message_id: String,
    pub chat_id: String,
    pub user_id: String,
    pub text: String,
    pub image_keys: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct FeishuInboundImage {
    pub local_path: String,
    pub mime_type: Option<String>,
}

pub struct FeishuClient {
    api_base: String,
    app_id: String,
    app_secret: String,
    access_token: String,
    token_expire_at: Option<Instant>,
}

impl FeishuClient {
    pub fn new(api_base: String, app_id: String, app_secret: String, access_token: String) -> Self {
        Self {
            api_base,
            app_id,
            app_secret,
            access_token,
            token_expire_at: None,
        }
    }

    pub fn send_message(&mut self, chat_id: &str, text: &str) -> io::Result<Option<String>> {
        let token = self.bearer_token()?.to_string();
        let url = format!("{}/im/v1/messages?receive_id_type=chat_id", self.api_base);
        let payload = build_send_message_payload(chat_id, text)?;
        let response = run_curl_json_post(
            &url,
            &[
                format!("Authorization: Bearer {token}"),
                "Content-Type: application/json; charset=utf-8".to_string(),
            ],
            &payload,
            "feishu send message",
        )?;
        if response.http_code >= 400 {
            return Err(io::Error::other(format!(
                "curl feishu send message failed: http={} {}",
                response.http_code,
                describe_feishu_response(&response.body)
            )));
        }
        ensure_feishu_success(&response.body, "feishu send message")?;
        Ok(parse_string_field(&response.body, "\"message_id\""))
    }

    pub fn download_message_image(
        &mut self,
        message_id: &str,
        image_key: &str,
    ) -> io::Result<FeishuInboundImage> {
        let token = self.bearer_token()?.to_string();
        let url = format!(
            "{}/im/v1/messages/{}/resources/{}?type=image",
            self.api_base, message_id, image_key
        );
        let path = temp_image_path("feishu", message_id, "jpg");
        let output = Command::new("curl")
            .arg("-fsS")
            .arg("-L")
            .arg("-H")
            .arg(format!("Authorization: Bearer {token}"))
            .arg("-o")
            .arg(&path)
            .arg(url)
            .output()?;
        if !output.status.success() {
            return Err(io::Error::other(format!(
                "curl feishu download image failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(FeishuInboundImage {
            local_path: path.to_string_lossy().to_string(),
            mime_type: Some("image/jpeg".to_string()),
        })
    }

    fn bearer_token(&mut self) -> io::Result<&str> {
        if !self.access_token.is_empty()
            && self
                .token_expire_at
                .map(|deadline| Instant::now() < deadline)
                .unwrap_or(true)
        {
            return Ok(self.access_token.as_str());
        }

        let (token, expires_in) =
            fetch_tenant_access_token(&self.api_base, &self.app_id, &self.app_secret)?;
        self.access_token = token;
        self.token_expire_at =
            Some(Instant::now() + Duration::from_secs(expires_in.saturating_sub(60).max(1)));
        Ok(self.access_token.as_str())
    }
}

pub struct FeishuLongConnClient {
    api_domain: String,
    app_id: String,
    app_secret: String,
    socket: Option<WebSocket<MaybeTlsStream<TcpStream>>>,
    ping_interval: Duration,
    reconnect_interval: Duration,
    service_id: i32,
    next_ping_at: Instant,
    fragments: HashMap<String, PartialEventPayload>,
}

impl FeishuLongConnClient {
    pub fn new(api_base: String, app_id: String, app_secret: String) -> Self {
        Self {
            api_domain: api_domain_from_api_base(&api_base),
            app_id,
            app_secret,
            socket: None,
            ping_interval: Duration::from_secs(90),
            reconnect_interval: Duration::from_secs(90),
            service_id: 0,
            next_ping_at: Instant::now() + Duration::from_secs(90),
            fragments: HashMap::new(),
        }
    }

    pub fn poll_message(&mut self) -> io::Result<Option<FeishuInboundMessage>> {
        self.ensure_connected()?;
        self.maybe_send_ping()?;

        let frame = match self.read_frame()? {
            Some(frame) => frame,
            None => return Ok(None),
        };

        self.handle_frame(frame)
    }

    fn ensure_connected(&mut self) -> io::Result<()> {
        if self.socket.is_some() {
            return Ok(());
        }

        let config = fetch_ws_connect_config(&self.api_domain, &self.app_id, &self.app_secret)?;
        self.ping_interval = Duration::from_secs(config.client_config.ping_interval.max(1));
        self.reconnect_interval =
            Duration::from_secs(config.client_config.reconnect_interval.max(1));
        self.service_id = parse_query_param(&config.url, "service_id")
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(0);

        let (mut socket, _) = connect(config.url.as_str())
            .map_err(|err| io::Error::other(format!("feishu ws connect failed: {err}")))?;
        set_socket_timeouts(&mut socket)?;

        self.next_ping_at = Instant::now() + self.ping_interval;
        self.socket = Some(socket);
        Ok(())
    }

    fn close_connection(&mut self) {
        if let Some(mut socket) = self.socket.take() {
            let _ = socket.close(None);
        }
    }

    fn maybe_send_ping(&mut self) -> io::Result<()> {
        if Instant::now() < self.next_ping_at {
            return Ok(());
        }

        let frame = PbFrame {
            seq_id: 0,
            log_id: 0,
            service: self.service_id,
            method: FEISHU_WS_FRAME_CONTROL,
            headers: vec![PbHeader::new("type", FEISHU_WS_MESSAGE_TYPE_PING)],
            payload_encoding: None,
            payload_type: None,
            payload: Vec::new(),
            log_id_new: None,
        };
        self.write_frame(&frame)?;
        self.next_ping_at = Instant::now() + self.ping_interval;
        Ok(())
    }

    fn read_frame(&mut self) -> io::Result<Option<PbFrame>> {
        let Some(socket) = self.socket.as_mut() else {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "feishu ws is not connected",
            ));
        };

        match socket.read() {
            Ok(Message::Binary(data)) => decode_frame(&data).map(Some),
            Ok(Message::Text(_)) => Ok(None),
            Ok(Message::Ping(payload)) => {
                socket
                    .send(Message::Pong(payload))
                    .map_err(|err| io::Error::other(format!("feishu ws pong failed: {err}")))?;
                Ok(None)
            }
            Ok(Message::Pong(_)) => Ok(None),
            Ok(Message::Close(_)) => {
                self.close_connection();
                std::thread::sleep(self.reconnect_interval);
                Ok(None)
            }
            Ok(Message::Frame(_)) => Ok(None),
            Err(tungstenite::Error::Io(err))
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                Ok(None)
            }
            Err(err) => {
                self.close_connection();
                Err(io::Error::other(format!("feishu ws read failed: {err}")))
            }
        }
    }

    fn handle_frame(&mut self, frame: PbFrame) -> io::Result<Option<FeishuInboundMessage>> {
        match frame.method {
            FEISHU_WS_FRAME_CONTROL => {
                self.handle_control_frame(&frame)?;
                Ok(None)
            }
            FEISHU_WS_FRAME_DATA => self.handle_event_frame(frame),
            _ => Ok(None),
        }
    }

    fn handle_control_frame(&mut self, frame: &PbFrame) -> io::Result<()> {
        let frame_type = header_value(&frame.headers, "type").unwrap_or_default();
        if frame_type == FEISHU_WS_MESSAGE_TYPE_PONG && !frame.payload.is_empty() {
            let payload: WsPongPayload = serde_json::from_slice(&frame.payload).map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid feishu ws pong payload: {err}"),
                )
            })?;

            self.ping_interval = Duration::from_secs(payload.ping_interval.max(1));
            self.reconnect_interval = Duration::from_secs(payload.reconnect_interval.max(1));
            self.next_ping_at = Instant::now() + self.ping_interval;
        }
        Ok(())
    }

    fn handle_event_frame(&mut self, frame: PbFrame) -> io::Result<Option<FeishuInboundMessage>> {
        let frame_type = header_value(&frame.headers, "type").unwrap_or_default();
        if frame_type != FEISHU_WS_MESSAGE_TYPE_EVENT {
            self.send_event_ack(&frame, 200)?;
            return Ok(None);
        }

        let merged_payload = merge_event_payload(&mut self.fragments, &frame)?;
        let Some(payload) = merged_payload else {
            return Ok(None);
        };

        let parsed = parse_long_conn_message(&payload);
        match parsed {
            Ok(message) => {
                self.send_event_ack(&frame, 200)?;
                Ok(message)
            }
            Err(err) => {
                let _ = self.send_event_ack(&frame, 500);
                Err(err)
            }
        }
    }

    fn send_event_ack(&mut self, original: &PbFrame, code: u16) -> io::Result<()> {
        let mut headers = original.headers.clone();
        headers.push(PbHeader::new("biz_rt", "0"));

        let payload = format!("{{\"code\":{code}}}");
        let ack = PbFrame {
            seq_id: original.seq_id,
            log_id: original.log_id,
            service: original.service,
            method: original.method,
            headers,
            payload_encoding: original.payload_encoding.clone(),
            payload_type: original.payload_type.clone(),
            payload: payload.into_bytes(),
            log_id_new: original.log_id_new.clone(),
        };
        self.write_frame(&ack)
    }

    fn write_frame(&mut self, frame: &PbFrame) -> io::Result<()> {
        let payload = encode_frame(frame);
        let Some(socket) = self.socket.as_mut() else {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "feishu ws is not connected",
            ));
        };

        socket
            .send(Message::Binary(payload))
            .map_err(|err| io::Error::other(format!("feishu ws send failed: {err}")))
    }
}

#[derive(Clone, Debug)]
struct PbHeader {
    key: String,
    value: String,
}

impl PbHeader {
    fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }
}

#[derive(Clone, Debug)]
struct PbFrame {
    seq_id: u64,
    log_id: u64,
    service: i32,
    method: i32,
    headers: Vec<PbHeader>,
    payload_encoding: Option<String>,
    payload_type: Option<String>,
    payload: Vec<u8>,
    log_id_new: Option<String>,
}

struct PartialEventPayload {
    parts: Vec<Option<Vec<u8>>>,
    created_at: Instant,
}

#[derive(Deserialize)]
struct FeishuWsEndpointResponse {
    code: i64,
    msg: String,
    data: Option<FeishuWsEndpointData>,
}

#[derive(Deserialize)]
struct FeishuWsEndpointData {
    #[serde(rename = "URL")]
    url: String,
    #[serde(rename = "ClientConfig")]
    client_config: FeishuWsClientConfig,
}

#[derive(Deserialize)]
struct FeishuWsClientConfig {
    #[serde(rename = "ReconnectInterval")]
    reconnect_interval: u64,
    #[serde(rename = "PingInterval")]
    ping_interval: u64,
}

#[derive(Deserialize)]
struct WsPongPayload {
    #[serde(rename = "ReconnectInterval")]
    reconnect_interval: u64,
    #[serde(rename = "PingInterval")]
    ping_interval: u64,
}

fn fetch_tenant_access_token(
    api_base: &str,
    app_id: &str,
    app_secret: &str,
) -> io::Result<(String, u64)> {
    if app_id.is_empty() || app_secret.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "feishu app_id/app_secret is not configured",
        ));
    }

    let url = format!("{api_base}/auth/v3/tenant_access_token/internal");
    let payload = serde_json::json!({
        "app_id": app_id,
        "app_secret": app_secret,
    })
    .to_string();
    let response = run_curl_json_post(
        &url,
        &["Content-Type: application/json; charset=utf-8".to_string()],
        &payload,
        "feishu tenant_access_token",
    )?;
    if response.http_code >= 400 {
        return Err(io::Error::other(format!(
            "curl feishu tenant_access_token failed: http={} {}",
            response.http_code,
            describe_feishu_response(&response.body)
        )));
    }
    ensure_feishu_success(&response.body, "feishu tenant_access_token")?;

    let token = parse_string_field(&response.body, "\"tenant_access_token\"").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "feishu tenant_access_token missing in response",
        )
    })?;
    let expires_in = parse_u64_field(&response.body, "\"expire\"")
        .or_else(|| parse_u64_field(&response.body, "\"expires_in\""));
    Ok((token, expires_in.unwrap_or(7200)))
}

fn fetch_ws_connect_config(
    api_domain: &str,
    app_id: &str,
    app_secret: &str,
) -> io::Result<FeishuWsEndpointData> {
    if app_id.trim().is_empty() || app_secret.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "feishu long connection requires app_id/app_secret",
        ));
    }

    let url = format!("{api_domain}{FEISHU_WS_CONFIG_PATH}");
    let payload = serde_json::json!({
        "AppID": app_id,
        "AppSecret": app_secret,
    })
    .to_string();
    let curl_response = run_curl_json_post(
        &url,
        &["Content-Type: application/json; charset=utf-8".to_string()],
        &payload,
        "feishu ws endpoint",
    )?;
    if curl_response.http_code >= 400 {
        return Err(io::Error::other(format!(
            "curl feishu ws endpoint failed: http={} {}",
            curl_response.http_code,
            describe_feishu_response(&curl_response.body)
        )));
    }

    let response: FeishuWsEndpointResponse =
        serde_json::from_str(&curl_response.body).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid feishu ws endpoint response: {err}"),
            )
        })?;
    if response.code != 0 {
        return Err(io::Error::other(format!(
            "feishu ws endpoint rejected: code={} msg={}",
            response.code, response.msg
        )));
    }
    response.data.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "feishu ws endpoint response missing data",
        )
    })
}

fn api_domain_from_api_base(api_base: &str) -> String {
    api_base
        .trim_end_matches('/')
        .trim_end_matches("/open-apis")
        .trim_end_matches('/')
        .to_string()
}

fn set_socket_timeouts(socket: &mut WebSocket<MaybeTlsStream<TcpStream>>) -> io::Result<()> {
    match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => {
            stream.set_read_timeout(Some(FEISHU_WS_READ_TIMEOUT))?;
            stream.set_write_timeout(Some(FEISHU_WS_WRITE_TIMEOUT))?;
        }
        MaybeTlsStream::NativeTls(stream) => {
            let tcp = tls_tcp_stream(stream);
            tcp.set_read_timeout(Some(FEISHU_WS_READ_TIMEOUT))?;
            tcp.set_write_timeout(Some(FEISHU_WS_WRITE_TIMEOUT))?;
        }
        _ => {}
    }
    Ok(())
}

fn tls_tcp_stream(stream: &mut TlsStream<TcpStream>) -> &mut TcpStream {
    stream.get_mut()
}

fn parse_query_param(url: &str, key: &str) -> Option<String> {
    let (_, query) = url.split_once('?')?;
    for item in query.split('&') {
        let (item_key, value) = item.split_once('=')?;
        if item_key == key {
            return Some(value.to_string());
        }
    }
    None
}

struct CurlJsonResponse {
    http_code: u16,
    body: String,
}

fn build_send_message_payload(chat_id: &str, text: &str) -> io::Result<String> {
    let content = serde_json::json!({
        "text": text,
    })
    .to_string();
    let payload = serde_json::json!({
        "receive_id": chat_id,
        "msg_type": "text",
        "content": content,
        "uuid": new_message_uuid(),
    })
    .to_string();
    Ok(payload)
}

fn run_curl_json_post(
    url: &str,
    headers: &[String],
    payload: &str,
    context: &str,
) -> io::Result<CurlJsonResponse> {
    let mut command = Command::new("curl");
    command.arg("-sS").arg("-X").arg("POST").arg(url);
    for header in headers {
        command.arg("-H").arg(header);
    }
    command
        .arg("-d")
        .arg(payload)
        .arg("-w")
        .arg(format!("\n{CURL_HTTP_CODE_MARKER}%{{http_code}}"));

    let output = command.output()?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!(
            "curl {context} failed: {}",
            detail.trim()
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let (body, code) = stdout.rsplit_once(CURL_HTTP_CODE_MARKER).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("curl {context} missing HTTP status marker"),
        )
    })?;
    let http_code = code.trim().parse::<u16>().map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("curl {context} returned invalid HTTP status: {err}"),
        )
    })?;
    Ok(CurlJsonResponse {
        http_code,
        body: body.trim_end_matches('\n').to_string(),
    })
}

fn ensure_feishu_success(body: &str, context: &str) -> io::Result<()> {
    let Some(code) = parse_i64_field(body, "\"code\"") else {
        return Ok(());
    };
    if code == 0 {
        return Ok(());
    }

    let msg = parse_string_field(body, "\"msg\"")
        .or_else(|| parse_string_field(body, "\"message\""))
        .unwrap_or_else(|| "unknown error".to_string());
    Err(io::Error::other(format!(
        "{context} rejected: code={code} msg={msg}"
    )))
}

fn describe_feishu_response(body: &str) -> String {
    if body.trim().is_empty() {
        return "empty response body".to_string();
    }
    let code = parse_i64_field(body, "\"code\"");
    let msg =
        parse_string_field(body, "\"msg\"").or_else(|| parse_string_field(body, "\"message\""));
    match (code, msg) {
        (Some(code), Some(msg)) => format!("code={code} msg={msg}"),
        (Some(code), None) => format!("code={code} body={body}"),
        (None, Some(msg)) => format!("msg={msg} body={body}"),
        (None, None) => format!("body={body}"),
    }
}

fn new_message_uuid() -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("botty-{}-{nanos}", std::process::id())
}

fn merge_event_payload(
    cache: &mut HashMap<String, PartialEventPayload>,
    frame: &PbFrame,
) -> io::Result<Option<Vec<u8>>> {
    cache.retain(|_, value| value.created_at.elapsed() < FEISHU_WS_CACHE_TTL);

    let message_id = header_value(&frame.headers, "message_id").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "feishu ws frame missing message_id",
        )
    })?;
    let total = header_value(&frame.headers, "sum")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    let seq = header_value(&frame.headers, "seq")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);

    if total == 1 {
        return Ok(Some(frame.payload.clone()));
    }

    let entry = cache
        .entry(message_id.to_string())
        .or_insert_with(|| PartialEventPayload {
            parts: vec![None; total],
            created_at: Instant::now(),
        });

    if entry.parts.len() != total {
        entry.parts = vec![None; total];
        entry.created_at = Instant::now();
    }

    if seq >= entry.parts.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "feishu ws frame seq out of range",
        ));
    }

    entry.parts[seq] = Some(frame.payload.clone());
    if entry.parts.iter().any(|part| part.is_none()) {
        return Ok(None);
    }

    let mut merged = Vec::new();
    for part in entry.parts.iter().filter_map(|part| part.as_ref()) {
        merged.extend_from_slice(part);
    }
    cache.remove(message_id);
    Ok(Some(merged))
}

fn parse_long_conn_message(payload: &[u8]) -> io::Result<Option<FeishuInboundMessage>> {
    let value: Value = serde_json::from_slice(payload).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid feishu event payload: {err}"),
        )
    })?;

    let event_type = value
        .pointer("/header/event_type")
        .and_then(Value::as_str)
        .or_else(|| value.get("event_type").and_then(Value::as_str))
        .unwrap_or_default();
    if event_type != FEISHU_WS_EVENT_TYPE {
        return Ok(None);
    }

    let event = match value.get("event") {
        Some(event) => event,
        None => return Ok(None),
    };

    if event
        .pointer("/sender/sender_type")
        .and_then(Value::as_str)
        .map(|sender_type| sender_type != "user")
        .unwrap_or(false)
    {
        return Ok(None);
    }

    let message = event.get("message").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "feishu event payload missing message",
        )
    })?;

    let message_type = message
        .get("message_type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if message_type != "text" && message_type != "image" {
        return Ok(None);
    }

    let message_id = required_string(message, "message_id")?;
    let chat_id = required_string(message, "chat_id")?;
    let content = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let text = if message_type == "text" {
        parse_message_text(content)
    } else {
        String::new()
    };
    let image_keys = if message_type == "image" {
        parse_message_image_keys(content)
    } else {
        Vec::new()
    };
    if text.trim().is_empty() && image_keys.is_empty() {
        return Ok(None);
    }

    let user_id = event
        .pointer("/sender/sender_id/open_id")
        .and_then(Value::as_str)
        .or_else(|| {
            event
                .pointer("/sender/sender_id/user_id")
                .and_then(Value::as_str)
        })
        .or_else(|| {
            event
                .pointer("/sender/sender_id/union_id")
                .and_then(Value::as_str)
        })
        .unwrap_or_default()
        .to_string();

    Ok(Some(FeishuInboundMessage {
        message_id,
        chat_id,
        user_id,
        text,
        image_keys,
    }))
}

fn required_string(object: &Value, key: &str) -> io::Result<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(|value| value.to_string())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("feishu event payload missing {key}"),
            )
        })
}

fn parse_message_text(content: &str) -> String {
    serde_json::from_str::<Value>(content)
        .ok()
        .and_then(|value| {
            value
                .get("text")
                .and_then(Value::as_str)
                .map(|text| text.to_string())
        })
        .unwrap_or_else(|| content.to_string())
}

fn parse_message_image_keys(content: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let Ok(value) = serde_json::from_str::<Value>(content) else {
        return keys;
    };
    for pointer in ["/image_key", "/file_key", "/key"] {
        if let Some(key) = value.pointer(pointer).and_then(Value::as_str) {
            let key = key.trim();
            if !key.is_empty() && !keys.iter().any(|item| item == key) {
                keys.push(key.to_string());
            }
        }
    }
    keys
}

fn temp_image_path(source: &str, message_id: &str, ext: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("mylittlebotty-chatbot-images");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(format!(
        "{source}-{}.{ext}",
        message_id.replace('/', "-").replace(':', "-")
    ))
}

fn header_value<'a>(headers: &'a [PbHeader], key: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|header| header.key == key)
        .map(|header| header.value.as_str())
}

fn decode_frame(bytes: &[u8]) -> io::Result<PbFrame> {
    let mut cursor = 0usize;
    let mut frame = PbFrame {
        seq_id: 0,
        log_id: 0,
        service: 0,
        method: 0,
        headers: Vec::new(),
        payload_encoding: None,
        payload_type: None,
        payload: Vec::new(),
        log_id_new: None,
    };

    while cursor < bytes.len() {
        let tag = read_varint(bytes, &mut cursor)?;
        let field = tag >> 3;
        let wire_type = (tag & 0x07) as u8;
        match field {
            1 => frame.seq_id = read_varint(bytes, &mut cursor)?,
            2 => frame.log_id = read_varint(bytes, &mut cursor)?,
            3 => frame.service = read_varint(bytes, &mut cursor)? as i32,
            4 => frame.method = read_varint(bytes, &mut cursor)? as i32,
            5 => {
                let nested = read_length_delimited(bytes, &mut cursor)?;
                frame.headers.push(decode_header(nested)?);
            }
            6 => frame.payload_encoding = Some(read_string(bytes, &mut cursor)?),
            7 => frame.payload_type = Some(read_string(bytes, &mut cursor)?),
            8 => frame.payload = read_length_delimited(bytes, &mut cursor)?.to_vec(),
            9 => frame.log_id_new = Some(read_string(bytes, &mut cursor)?),
            _ => skip_field(bytes, &mut cursor, wire_type)?,
        }
    }

    Ok(frame)
}

fn decode_header(bytes: &[u8]) -> io::Result<PbHeader> {
    let mut cursor = 0usize;
    let mut key = None;
    let mut value = None;

    while cursor < bytes.len() {
        let tag = read_varint(bytes, &mut cursor)?;
        let field = tag >> 3;
        let wire_type = (tag & 0x07) as u8;
        match field {
            1 => key = Some(read_string(bytes, &mut cursor)?),
            2 => value = Some(read_string(bytes, &mut cursor)?),
            _ => skip_field(bytes, &mut cursor, wire_type)?,
        }
    }

    Ok(PbHeader {
        key: key.unwrap_or_default(),
        value: value.unwrap_or_default(),
    })
}

fn encode_frame(frame: &PbFrame) -> Vec<u8> {
    let mut out = Vec::new();
    write_varint_field(&mut out, 1, frame.seq_id);
    write_varint_field(&mut out, 2, frame.log_id);
    write_varint_field(&mut out, 3, frame.service as u64);
    write_varint_field(&mut out, 4, frame.method as u64);
    for header in &frame.headers {
        let encoded = encode_header(header);
        write_bytes_field(&mut out, 5, &encoded);
    }
    if let Some(value) = &frame.payload_encoding {
        write_string_field(&mut out, 6, value);
    }
    if let Some(value) = &frame.payload_type {
        write_string_field(&mut out, 7, value);
    }
    if !frame.payload.is_empty() {
        write_bytes_field(&mut out, 8, &frame.payload);
    }
    if let Some(value) = &frame.log_id_new {
        write_string_field(&mut out, 9, value);
    }
    out
}

fn encode_header(header: &PbHeader) -> Vec<u8> {
    let mut out = Vec::new();
    write_string_field(&mut out, 1, &header.key);
    write_string_field(&mut out, 2, &header.value);
    out
}

fn write_varint_field(out: &mut Vec<u8>, field: u64, value: u64) {
    write_varint(out, field << 3);
    write_varint(out, value);
}

fn write_string_field(out: &mut Vec<u8>, field: u64, value: &str) {
    write_bytes_field(out, field, value.as_bytes());
}

fn write_bytes_field(out: &mut Vec<u8>, field: u64, value: &[u8]) {
    write_varint(out, (field << 3) | 2);
    write_varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

fn write_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(((value as u8) & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn read_varint(bytes: &[u8], cursor: &mut usize) -> io::Result<u64> {
    let mut shift = 0u32;
    let mut value = 0u64;

    while *cursor < bytes.len() {
        let byte = bytes[*cursor];
        *cursor += 1;
        value |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
        if shift >= 64 {
            break;
        }
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid protobuf varint",
    ))
}

fn read_length_delimited<'a>(bytes: &'a [u8], cursor: &mut usize) -> io::Result<&'a [u8]> {
    let len = read_varint(bytes, cursor)? as usize;
    if bytes.len().saturating_sub(*cursor) < len {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "truncated protobuf field",
        ));
    }
    let start = *cursor;
    *cursor += len;
    Ok(&bytes[start..start + len])
}

fn read_string(bytes: &[u8], cursor: &mut usize) -> io::Result<String> {
    let raw = read_length_delimited(bytes, cursor)?;
    String::from_utf8(raw.to_vec()).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid utf-8 in protobuf string: {err}"),
        )
    })
}

fn skip_field(bytes: &[u8], cursor: &mut usize, wire_type: u8) -> io::Result<()> {
    match wire_type {
        0 => {
            let _ = read_varint(bytes, cursor)?;
            Ok(())
        }
        2 => {
            let _ = read_length_delimited(bytes, cursor)?;
            Ok(())
        }
        1 => {
            if bytes.len().saturating_sub(*cursor) < 8 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "truncated fixed64 field",
                ));
            }
            *cursor += 8;
            Ok(())
        }
        5 => {
            if bytes.len().saturating_sub(*cursor) < 4 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "truncated fixed32 field",
                ));
            }
            *cursor += 4;
            Ok(())
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported protobuf wire type: {wire_type}"),
        )),
    }
}

fn parse_u64_field(chunk: &str, field_name: &str) -> Option<u64> {
    let idx = chunk.find(field_name)?;
    let after = &chunk[idx + field_name.len()..];
    let colon = after.find(':')?;
    let rest = after[colon + 1..].trim_start();
    let mut end = 0usize;
    for (i, ch) in rest.char_indices() {
        if ch.is_ascii_digit() {
            end = i + ch.len_utf8();
            continue;
        }
        break;
    }
    if end == 0 {
        return None;
    }
    rest[..end].parse::<u64>().ok()
}

fn parse_i64_field(chunk: &str, field_name: &str) -> Option<i64> {
    let idx = chunk.find(field_name)?;
    let after = &chunk[idx + field_name.len()..];
    let colon = after.find(':')?;
    let rest = after[colon + 1..].trim_start();
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
            _ => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protobuf_frame_roundtrip_preserves_fields() {
        let frame = PbFrame {
            seq_id: 12,
            log_id: 34,
            service: 56,
            method: FEISHU_WS_FRAME_DATA,
            headers: vec![
                PbHeader::new("type", FEISHU_WS_MESSAGE_TYPE_EVENT),
                PbHeader::new("message_id", "msg-1"),
            ],
            payload_encoding: Some("json".to_string()),
            payload_type: Some("application/json".to_string()),
            payload: br#"{"ok":true}"#.to_vec(),
            log_id_new: Some("log-new".to_string()),
        };

        let encoded = encode_frame(&frame);
        let decoded = decode_frame(&encoded).expect("decode frame");

        assert_eq!(decoded.seq_id, frame.seq_id);
        assert_eq!(decoded.log_id, frame.log_id);
        assert_eq!(decoded.service, frame.service);
        assert_eq!(decoded.method, frame.method);
        assert_eq!(decoded.payload_encoding, frame.payload_encoding);
        assert_eq!(decoded.payload_type, frame.payload_type);
        assert_eq!(decoded.payload, frame.payload);
        assert_eq!(decoded.log_id_new, frame.log_id_new);
        assert_eq!(decoded.headers.len(), 2);
        assert_eq!(decoded.headers[0].key, "type");
        assert_eq!(decoded.headers[0].value, FEISHU_WS_MESSAGE_TYPE_EVENT);
    }

    #[test]
    fn parse_im_message_receive_event() {
        let payload = br#"{
          "schema":"2.0",
          "header":{"event_type":"im.message.receive_v1"},
          "event":{
            "sender":{"sender_type":"user","sender_id":{"open_id":"ou_test"}},
            "message":{
              "message_id":"om_123",
              "chat_id":"oc_456",
              "chat_type":"group",
              "message_type":"text",
              "content":"{\"text\":\"hello botty\"}"
            }
          }
        }"#;

        let message = parse_long_conn_message(payload)
            .expect("parse event")
            .expect("inbound message");

        assert_eq!(message.message_id, "om_123");
        assert_eq!(message.chat_id, "oc_456");
        assert_eq!(message.user_id, "ou_test");
        assert_eq!(message.text, "hello botty");
    }

    #[test]
    fn trims_open_api_suffix_from_domain() {
        assert_eq!(
            api_domain_from_api_base("https://open.feishu.cn/open-apis"),
            "https://open.feishu.cn"
        );
        assert_eq!(
            api_domain_from_api_base("https://open.feishu.cn/open-apis/"),
            "https://open.feishu.cn"
        );
    }

    #[test]
    fn send_message_payload_serializes_control_characters() {
        let payload =
            build_send_message_payload("oc_123", "hi\tbotty\n\"quoted\"").expect("build payload");
        let value: Value = serde_json::from_str(&payload).expect("parse outer payload");

        assert_eq!(
            value.get("receive_id").and_then(Value::as_str),
            Some("oc_123")
        );
        assert_eq!(value.get("msg_type").and_then(Value::as_str), Some("text"));
        assert!(value.get("uuid").and_then(Value::as_str).is_some());

        let content = value
            .get("content")
            .and_then(Value::as_str)
            .expect("content string");
        let inner: Value = serde_json::from_str(content).expect("parse inner content");
        assert_eq!(
            inner.get("text").and_then(Value::as_str),
            Some("hi\tbotty\n\"quoted\"")
        );
    }
}
