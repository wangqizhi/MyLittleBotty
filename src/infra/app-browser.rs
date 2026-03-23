use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, Message, WebSocket};

const OUTPUT_TAIL_MAX_BYTES: usize = 16 * 1024;
const DEBUG_ENDPOINT_WAIT_TIMEOUT: Duration = Duration::from_secs(15);
const PAGE_READY_TIMEOUT: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(250);

pub struct AppBrowser {
    transcript_path: PathBuf,
    remote_debugging_port: u16,
    user_data_dir: PathBuf,
    output_tail: Arc<Mutex<String>>,
    last_output_ms: Arc<AtomicU64>,
    child: Mutex<Option<Child>>,
    exit_code: Arc<Mutex<Option<i32>>>,
    target_id: String,
    cdp: Mutex<CdpSession>,
}

pub struct AppBrowserStatus {
    pub exited: bool,
    pub exit_code: Option<i32>,
    pub last_output_ms: u64,
    pub transcript_tail: String,
    pub remote_debugging_port: u16,
    pub user_data_dir: PathBuf,
    pub page_title: String,
    pub page_url: String,
    pub ready_state: String,
}

pub struct BrowserLaunchConfig {
    pub chrome_command: String,
    pub work_dir: PathBuf,
    pub transcript_path: PathBuf,
    pub user_data_dir: PathBuf,
    pub headless: bool,
    pub max_page_tabs: usize,
}

#[derive(Clone)]
pub struct SnapshotResult {
    pub title: String,
    pub url: String,
    pub ready_state: String,
    pub text_excerpt: String,
    pub items: Value,
}

impl AppBrowser {
    pub fn spawn(config: BrowserLaunchConfig) -> io::Result<Self> {
        if let Some(parent) = config.transcript_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::create_dir_all(&config.user_data_dir)?;

        let remote_debugging_port = reserve_local_port()?;
        let mut command = Command::new(&config.chrome_command);
        command
            .current_dir(&config.work_dir)
            .arg(format!("--remote-debugging-port={remote_debugging_port}"))
            .arg(format!(
                "--user-data-dir={}",
                config.user_data_dir.display()
            ))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--disable-default-apps")
            .arg("--disable-popup-blocking")
            .arg("--disable-background-networking")
            .arg("--disable-sync")
            .arg("--new-window");
        if config.headless {
            command.arg("--headless=new");
        }
        command
            .arg("about:blank")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command.spawn().map_err(|err| {
            io::Error::new(
                err.kind(),
                format!(
                    "spawn chrome failed with `{}`: {err}",
                    config.chrome_command
                ),
            )
        })?;

        let output_tail = Arc::new(Mutex::new(String::new()));
        let last_output_ms = Arc::new(AtomicU64::new(now_ms()));
        let exit_code = Arc::new(Mutex::new(None));

        if let Some(stdout) = child.stdout.take() {
            spawn_reader_thread(
                stdout,
                config.transcript_path.clone(),
                Arc::clone(&output_tail),
                Arc::clone(&last_output_ms),
            );
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_reader_thread(
                stderr,
                config.transcript_path.clone(),
                Arc::clone(&output_tail),
                Arc::clone(&last_output_ms),
            );
        }

        let startup_result = (|| -> io::Result<(TargetDescriptor, CdpSession)> {
            wait_for_debug_endpoint(remote_debugging_port)?;
            prune_page_targets(remote_debugging_port, None, config.max_page_tabs)?;
            let target = create_page_target(remote_debugging_port)?;
            let (websocket, _) =
                connect(target.web_socket_debugger_url.as_str()).map_err(ws_err)?;
            let mut cdp = CdpSession::new(websocket);
            cdp.command("Page.enable", json!({}))?;
            cdp.command("Runtime.enable", json!({}))?;
            wait_until_ready(&mut cdp, PAGE_READY_TIMEOUT)?;
            Ok((target, cdp))
        })();

        let (target, cdp) = match startup_result {
            Ok(value) => value,
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(err);
            }
        };

        Ok(Self {
            transcript_path: config.transcript_path,
            remote_debugging_port,
            user_data_dir: config.user_data_dir,
            output_tail,
            last_output_ms,
            child: Mutex::new(Some(child)),
            exit_code,
            target_id: target.id,
            cdp: Mutex::new(cdp),
        })
    }

    pub fn attach(
        transcript_path: PathBuf,
        user_data_dir: PathBuf,
        remote_debugging_port: u16,
        target_id: Option<&str>,
        max_page_tabs: usize,
    ) -> io::Result<Self> {
        if let Some(parent) = transcript_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::create_dir_all(&user_data_dir)?;
        wait_for_debug_endpoint(remote_debugging_port)?;
        prune_page_targets(remote_debugging_port, target_id, max_page_tabs)?;
        let target = find_or_create_page_target(remote_debugging_port, target_id)?;
        let (websocket, _) = connect(target.web_socket_debugger_url.as_str()).map_err(ws_err)?;
        let mut cdp = CdpSession::new(websocket);
        cdp.command("Page.enable", json!({}))?;
        cdp.command("Runtime.enable", json!({}))?;
        wait_until_ready(&mut cdp, PAGE_READY_TIMEOUT)?;

        Ok(Self {
            transcript_path,
            remote_debugging_port,
            user_data_dir,
            output_tail: Arc::new(Mutex::new(String::new())),
            last_output_ms: Arc::new(AtomicU64::new(now_ms())),
            child: Mutex::new(None),
            exit_code: Arc::new(Mutex::new(None)),
            target_id: target.id,
            cdp: Mutex::new(cdp),
        })
    }

    pub fn navigate(&self, url: &str) -> io::Result<Value> {
        let mut cdp = self.lock_cdp()?;
        cdp.command("Page.navigate", json!({ "url": url.trim() }))?;
        wait_until_ready(&mut cdp, PAGE_READY_TIMEOUT)?;
        page_state_json(&mut cdp)
    }

    pub fn snapshot(&self) -> io::Result<SnapshotResult> {
        let mut cdp = self.lock_cdp()?;
        wait_until_ready(&mut cdp, PAGE_READY_TIMEOUT)?;
        read_snapshot(&mut cdp)
    }

    pub fn click(&self, locator: &str) -> io::Result<Value> {
        let mut cdp = self.lock_cdp()?;
        let value = cdp.evaluate_value(prepare_interaction_script(locator, false))?;
        dispatch_mouse_click(&mut cdp, &value)?;
        wait_until_ready(&mut cdp, PAGE_READY_TIMEOUT)?;
        Ok(value)
    }

    pub fn focus(&self, locator: &str) -> io::Result<Value> {
        let mut cdp = self.lock_cdp()?;
        let mut value = cdp.evaluate_value(prepare_interaction_script(locator, true))?;
        if !value
            .get("focused")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            dispatch_mouse_click(&mut cdp, &value)?;
            value = cdp.evaluate_value(check_focus_script(locator))?;
        }
        Ok(value)
    }

    pub fn fill(&self, locator: &str, text: &str) -> io::Result<Value> {
        let mut cdp = self.lock_cdp()?;
        let mut value = cdp.evaluate_value(prepare_interaction_script(locator, true))?;
        if !value
            .get("focused")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            dispatch_mouse_click(&mut cdp, &value)?;
            value = cdp.evaluate_value(check_focus_script(locator))?;
        }
        if !value
            .get("focused")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err(io::Error::other(format!(
                "locator `{locator}` did not receive focus before typing"
            )));
        }
        cdp.command(
            "Input.dispatchKeyEvent",
            json!({
                "type": "rawKeyDown",
                "key": "End",
                "code": "End",
                "windowsVirtualKeyCode": 35,
                "nativeVirtualKeyCode": 35,
            }),
        )?;
        cdp.command(
            "Input.dispatchKeyEvent",
            json!({
                "type": "keyUp",
                "key": "End",
                "code": "End",
                "windowsVirtualKeyCode": 35,
                "nativeVirtualKeyCode": 35,
            }),
        )?;
        cdp.command("Input.insertText", json!({ "text": text }))?;
        wait_until_ready(&mut cdp, PAGE_READY_TIMEOUT)?;
        Ok(value)
    }

    pub fn press(&self, key: &str) -> io::Result<Value> {
        let mut cdp = self.lock_cdp()?;
        dispatch_key_press(&mut cdp, key)?;
        Ok(json!({
            "ok": true,
            "key": key,
        }))
    }

    pub fn eval(&self, script: &str) -> io::Result<Value> {
        let mut cdp = self.lock_cdp()?;
        cdp.evaluate_value(script.to_string())
    }

    pub fn wait_for(&self, locator: &str, timeout: Duration) -> io::Result<Value> {
        let mut cdp = self.lock_cdp()?;
        wait_for_locator(&mut cdp, locator, timeout)
    }

    pub fn capture_screenshot(&self, output_path: &Path) -> io::Result<PathBuf> {
        let mut cdp = self.lock_cdp()?;
        let result = cdp.command(
            "Page.captureScreenshot",
            json!({
                "format": "png",
                "captureBeyondViewport": true
            }),
        )?;
        let data = result
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| io::Error::other("missing screenshot data from CDP"))?;
        let bytes = BASE64_STANDARD
            .decode(data)
            .map_err(|err| io::Error::other(format!("decode screenshot data failed: {err}")))?;
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(output_path, bytes)?;
        Ok(output_path.to_path_buf())
    }

    pub fn terminate(&self) -> io::Result<()> {
        if let Ok(mut cdp) = self.cdp.lock() {
            let _ = cdp.command("Browser.close", json!({}));
            let _ = cdp.close();
        }
        let mut child = self
            .child
            .lock()
            .map_err(|_| io::Error::other("browser child lock poisoned"))?;
        if let Some(process) = child.as_mut() {
            process
                .kill()
                .map_err(|err| io::Error::other(format!("kill chrome failed: {err}")))?;
            let _ = process.wait();
            *child = None;
            return Ok(());
        }
        Ok(())
    }

    pub fn status(&self) -> AppBrowserStatus {
        let mut exited = false;
        if let Ok(mut child) = self.child.lock() {
            if let Some(child) = child.as_mut() {
                if let Ok(Some(status)) = child.try_wait() {
                    exited = true;
                    if let Ok(mut exit_code) = self.exit_code.lock() {
                        *exit_code = status.code();
                    }
                }
            }
        }

        let transcript_tail = self
            .output_tail
            .lock()
            .map(|tail| tail.clone())
            .unwrap_or_default();
        let exit_code = self.exit_code.lock().map(|code| *code).unwrap_or(None);

        let page_state = self
            .cdp
            .lock()
            .ok()
            .and_then(|mut cdp| page_state_json(&mut cdp).ok())
            .unwrap_or_else(|| {
                json!({
                    "title": "",
                    "url": "",
                    "readyState": ""
                })
            });

        AppBrowserStatus {
            exited,
            exit_code,
            last_output_ms: self.last_output_ms.load(Ordering::SeqCst),
            transcript_tail,
            remote_debugging_port: self.remote_debugging_port,
            user_data_dir: self.user_data_dir.clone(),
            page_title: page_state
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            page_url: page_state
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            ready_state: page_state
                .get("readyState")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }
    }

    pub fn transcript_path(&self) -> &Path {
        &self.transcript_path
    }

    pub fn target_id(&self) -> &str {
        &self.target_id
    }

    fn lock_cdp(&self) -> io::Result<std::sync::MutexGuard<'_, CdpSession>> {
        self.cdp
            .lock()
            .map_err(|_| io::Error::other("browser cdp lock poisoned"))
    }
}

struct TargetDescriptor {
    id: String,
    web_socket_debugger_url: String,
}

struct CdpSession {
    websocket: WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: u64,
}

impl CdpSession {
    fn new(websocket: WebSocket<MaybeTlsStream<TcpStream>>) -> Self {
        Self {
            websocket,
            next_id: 1,
        }
    }

    fn command(&mut self, method: &str, params: Value) -> io::Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let payload = json!({
            "id": id,
            "method": method,
            "params": params,
        });
        self.websocket
            .send(Message::Text(payload.to_string()))
            .map_err(ws_err)?;

        loop {
            let message = self.websocket.read().map_err(ws_err)?;
            let text = match message {
                Message::Text(text) => text,
                Message::Binary(bytes) => String::from_utf8_lossy(&bytes).to_string(),
                Message::Ping(payload) => {
                    let _ = self.websocket.send(Message::Pong(payload));
                    continue;
                }
                Message::Pong(_) => continue,
                Message::Close(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "browser websocket closed",
                    ));
                }
                _ => continue,
            };
            let value: Value = serde_json::from_str(&text).map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("parse browser websocket message failed: {err}"),
                )
            })?;
            if value.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = value.get("error") {
                return Err(io::Error::other(format!(
                    "cdp command `{method}` failed: {error}"
                )));
            }
            return Ok(value.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    fn evaluate_value(&mut self, expression: String) -> io::Result<Value> {
        let result = self.command(
            "Runtime.evaluate",
            json!({
                "expression": expression,
                "awaitPromise": true,
                "returnByValue": true,
            }),
        )?;
        if let Some(details) = result.get("exceptionDetails") {
            return Err(io::Error::other(format!(
                "page script evaluation failed: {details}"
            )));
        }
        Ok(result
            .get("result")
            .and_then(|value| value.get("value"))
            .cloned()
            .unwrap_or(Value::Null))
    }

    fn close(&mut self) -> io::Result<()> {
        self.websocket.close(None).map_err(ws_err)
    }
}

fn reserve_local_port() -> io::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

fn wait_for_debug_endpoint(port: u16) -> io::Result<()> {
    let start = Instant::now();
    loop {
        if create_http_connection(port).is_ok() {
            return Ok(());
        }
        if start.elapsed() >= DEBUG_ENDPOINT_WAIT_TIMEOUT {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("chrome remote debugging endpoint not ready on port {port}"),
            ));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn create_page_target(port: u16) -> io::Result<TargetDescriptor> {
    let url = format!("http://127.0.0.1:{port}/json/new?about:blank");
    let output = Command::new("curl")
        .arg("--noproxy")
        .arg("*")
        .arg("-fsS")
        .arg("-X")
        .arg("PUT")
        .arg(url)
        .output()?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!(
            "chrome target creation failed: {}",
            detail.trim()
        )));
    }
    let body = String::from_utf8_lossy(&output.stdout);
    let value: Value = serde_json::from_str(&body).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("parse chrome target response failed: {err}"),
        )
    })?;
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| io::Error::other("chrome target response missing id"))?;
    let websocket = value
        .get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .ok_or_else(|| io::Error::other("chrome target response missing websocket url"))?;
    Ok(TargetDescriptor {
        id: id.to_string(),
        web_socket_debugger_url: websocket.to_string(),
    })
}

fn close_page_target(port: u16, target_id: &str) -> io::Result<()> {
    let url = format!("http://127.0.0.1:{port}/json/close/{target_id}");
    let output = Command::new("curl")
        .arg("--noproxy")
        .arg("*")
        .arg("-fsS")
        .arg(url)
        .output()?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!(
            "chrome target close failed: {}",
            detail.trim()
        )));
    }
    Ok(())
}

fn find_or_create_page_target(port: u16, target_id: Option<&str>) -> io::Result<TargetDescriptor> {
    if let Some(target_id) = target_id {
        if let Some(target) = find_page_target(port, target_id)? {
            return Ok(target);
        }
    }
    if let Some(target) = find_first_page_target(port)? {
        return Ok(target);
    }
    create_page_target(port)
}

fn find_page_target(port: u16, target_id: &str) -> io::Result<Option<TargetDescriptor>> {
    let list = list_targets(port)?;
    for value in list {
        let Some(id) = value.get("id").and_then(Value::as_str) else {
            continue;
        };
        if id != target_id {
            continue;
        }
        let Some(websocket) = value.get("webSocketDebuggerUrl").and_then(Value::as_str) else {
            continue;
        };
        return Ok(Some(TargetDescriptor {
            id: id.to_string(),
            web_socket_debugger_url: websocket.to_string(),
        }));
    }
    Ok(None)
}

fn find_first_page_target(port: u16) -> io::Result<Option<TargetDescriptor>> {
    let list = list_targets(port)?;
    for value in list {
        let Some(kind) = value.get("type").and_then(Value::as_str) else {
            continue;
        };
        if kind != "page" {
            continue;
        }
        let Some(id) = value.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(websocket) = value.get("webSocketDebuggerUrl").and_then(Value::as_str) else {
            continue;
        };
        return Ok(Some(TargetDescriptor {
            id: id.to_string(),
            web_socket_debugger_url: websocket.to_string(),
        }));
    }
    Ok(None)
}

fn list_targets(port: u16) -> io::Result<Vec<Value>> {
    let url = format!("http://127.0.0.1:{port}/json/list");
    let output = Command::new("curl")
        .arg("--noproxy")
        .arg("*")
        .arg("-fsS")
        .arg(url)
        .output()?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!(
            "chrome target list failed: {}",
            detail.trim()
        )));
    }
    let body = String::from_utf8_lossy(&output.stdout);
    let values: Value = serde_json::from_str(&body).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("parse chrome target list failed: {err}"),
        )
    })?;
    let Some(list) = values.as_array() else {
        return Ok(Vec::new());
    };
    Ok(list.clone())
}

fn prune_page_targets(
    port: u16,
    preferred_target_id: Option<&str>,
    max_page_tabs: usize,
) -> io::Result<()> {
    if max_page_tabs == 0 {
        return Ok(());
    }

    let mut pages: Vec<Value> = list_targets(port)?
        .into_iter()
        .filter(|value| value.get("type").and_then(Value::as_str) == Some("page"))
        .collect();
    if pages.len() <= max_page_tabs {
        return Ok(());
    }

    pages.sort_by_key(|value| {
        value
            .get("id")
            .and_then(Value::as_str)
            .map(|id| id != preferred_target_id.unwrap_or_default())
            .unwrap_or(true)
    });

    let overflow = pages.len().saturating_sub(max_page_tabs);
    for value in pages.into_iter().take(overflow) {
        let Some(target_id) = value.get("id").and_then(Value::as_str) else {
            continue;
        };
        if Some(target_id) == preferred_target_id {
            continue;
        }
        close_page_target(port, target_id)?;
    }
    Ok(())
}

fn page_state_json(cdp: &mut CdpSession) -> io::Result<Value> {
    cdp.evaluate_value(
        r#"(function () {
            return {
                title: document.title || "",
                url: location.href || "",
                readyState: document.readyState || ""
            };
        })()"#
            .to_string(),
    )
}

fn read_snapshot(cdp: &mut CdpSession) -> io::Result<SnapshotResult> {
    let value = cdp.evaluate_value(
        r#"(function () {
            function visible(el) {
                if (!el) return false;
                const style = window.getComputedStyle(el);
                if (!style || style.visibility === "hidden" || style.display === "none") return false;
                const rect = el.getBoundingClientRect();
                return rect.width > 0 && rect.height > 0;
            }

            const nodes = Array.from(document.querySelectorAll("a, button, input, textarea, select, [role='button'], [onclick]"));
            let seq = 1;
            const items = [];
            for (const el of nodes) {
                if (!visible(el)) continue;
                let ref = el.getAttribute("data-botty-ref");
                if (!ref) {
                    ref = `e${seq++}`;
                    el.setAttribute("data-botty-ref", ref);
                }
                items.push({
                    ref: `@${ref}`,
                    tag: (el.tagName || "").toLowerCase(),
                    type: el.getAttribute("type") || "",
                    text: (el.innerText || el.value || el.getAttribute("aria-label") || el.getAttribute("placeholder") || "").trim().slice(0, 200),
                    href: el.getAttribute("href") || "",
                    placeholder: el.getAttribute("placeholder") || "",
                    name: el.getAttribute("name") || ""
                });
            }

            const bodyText = (document.body && document.body.innerText ? document.body.innerText : "").replace(/\s+/g, " ").trim().slice(0, 4000);
            return {
                title: document.title || "",
                url: location.href || "",
                readyState: document.readyState || "",
                textExcerpt: bodyText,
                items
            };
        })()"#
            .to_string(),
    )?;

    Ok(SnapshotResult {
        title: value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        url: value
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        ready_state: value
            .get("readyState")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        text_excerpt: value
            .get("textExcerpt")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        items: value.get("items").cloned().unwrap_or_else(|| json!([])),
    })
}

fn wait_until_ready(cdp: &mut CdpSession, timeout: Duration) -> io::Result<()> {
    let start = Instant::now();
    loop {
        let state = page_state_json(cdp)?;
        let ready_state = state
            .get("readyState")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if ready_state == "complete" || ready_state == "interactive" {
            return Ok(());
        }
        if start.elapsed() >= timeout {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "browser page did not become ready in time",
            ));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn wait_for_locator(cdp: &mut CdpSession, locator: &str, timeout: Duration) -> io::Result<Value> {
    let start = Instant::now();
    let escaped = serde_json::to_string(locator)
        .map_err(|err| io::Error::other(format!("encode locator failed: {err}")))?;
    let script = format!(
        r#"(function () {{
            const locator = {escaped};
            {locator_resolver}
            const el = resolve(locator);
            if (!el) return {{ found: false }};
            const rect = el.getBoundingClientRect();
            return {{
                found: rect.width > 0 && rect.height > 0,
                tag: (el.tagName || "").toLowerCase(),
                text: (el.innerText || el.value || "").trim().slice(0, 200)
            }};
        }})()"#,
        locator_resolver = locator_resolver_script()
    );

    loop {
        let value = cdp.evaluate_value(script.clone())?;
        if value.get("found").and_then(Value::as_bool).unwrap_or(false) {
            return Ok(value);
        }
        if start.elapsed() >= timeout {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("timed out waiting for locator `{locator}`"),
            ));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn prepare_interaction_script(locator: &str, focus: bool) -> String {
    let locator = serde_json::to_string(locator).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"(function () {{
            const locator = {locator};
            const shouldFocus = {focus};
            {locator_resolver}
            const el = resolve(locator);
            if (!el) {{
                throw new Error(`locator not found: ${{locator}}`);
            }}
            el.scrollIntoView({{ block: "center", inline: "center" }});
            if (shouldFocus && typeof el.focus === "function") {{
                el.focus({{ preventScroll: true }});
            }}

            if (typeof el.select === "function") {{
                el.select();
            }} else if (
                "value" in el &&
                typeof el.value === "string" &&
                typeof el.setSelectionRange === "function"
            ) {{
                el.setSelectionRange(0, el.value.length);
            }} else if (el.isContentEditable) {{
                const selection = window.getSelection();
                const range = document.createRange();
                range.selectNodeContents(el);
                selection.removeAllRanges();
                selection.addRange(range);
            }}

            const rect = el.getBoundingClientRect();
            const active = document.activeElement;
            const focused = active === el || el.contains(active);
            return {{
                ok: true,
                locator,
                tag: (el.tagName || "").toLowerCase(),
                text: (el.innerText || el.value || "").trim().slice(0, 200),
                focused,
                x: rect.left + rect.width / 2,
                y: rect.top + rect.height / 2,
                width: rect.width,
                height: rect.height
            }};
        }})()"#,
        focus = if focus { "true" } else { "false" },
        locator_resolver = locator_resolver_script()
    )
}

fn check_focus_script(locator: &str) -> String {
    let locator = serde_json::to_string(locator).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"(function () {{
            const locator = {locator};
            {locator_resolver}
            const el = resolve(locator);
            if (!el) {{
                throw new Error(`locator not found: ${{locator}}`);
            }}
            const active = document.activeElement;
            return {{
                ok: true,
                locator,
                tag: (el.tagName || "").toLowerCase(),
                focused: active === el || el.contains(active),
                text: (el.innerText || el.value || "").trim().slice(0, 200)
            }};
        }})()"#,
        locator_resolver = locator_resolver_script()
    )
}

fn locator_resolver_script() -> &'static str {
    r#"
            function visible(el) {
                if (!el) return false;
                const style = window.getComputedStyle(el);
                if (!style || style.visibility === "hidden" || style.display === "none") return false;
                const rect = el.getBoundingClientRect();
                return rect.width > 0 && rect.height > 0;
            }

            function textOf(el) {
                return String(
                    el?.innerText ||
                    el?.textContent ||
                    el?.value ||
                    el?.getAttribute?.("aria-label") ||
                    el?.getAttribute?.("placeholder") ||
                    ""
                ).trim();
            }

            function normalize(value) {
                return String(value || "").replace(/\s+/g, " ").trim().toLowerCase();
            }

            function pickVisible(nodes) {
                for (const el of nodes) {
                    if (visible(el)) return el;
                }
                return nodes[0] || null;
            }

            function resolve(target) {
                if (!target) return null;
                if (target.startsWith("@")) {
                    return document.querySelector(`[data-botty-ref="${target.slice(1)}"]`);
                }
                if (target.startsWith("css=")) {
                    return document.querySelector(target.slice(4));
                }
                if (target.startsWith("placeholder=")) {
                    const expected = target.slice("placeholder=".length).trim();
                    return pickVisible(Array.from(document.querySelectorAll("input[placeholder], textarea[placeholder]")).filter((el) =>
                        normalize(el.getAttribute("placeholder")).includes(normalize(expected))
                    ));
                }
                if (target.startsWith("aria=")) {
                    const expected = target.slice("aria=".length).trim();
                    return pickVisible(Array.from(document.querySelectorAll("[aria-label]")).filter((el) =>
                        normalize(el.getAttribute("aria-label")).includes(normalize(expected))
                    ));
                }
                if (target.startsWith("label=")) {
                    const expected = normalize(target.slice("label=".length));
                    const labels = Array.from(document.querySelectorAll("label"));
                    for (const label of labels) {
                        if (!normalize(textOf(label)).includes(expected)) continue;
                        if (label.control) return label.control;
                        const nested = label.querySelector("input, textarea, select, [contenteditable='true'], [role='textbox']");
                        if (nested) return nested;
                    }
                    return null;
                }
                if (target.startsWith("text=")) {
                    const expected = normalize(target.slice("text=".length));
                    const candidates = Array.from(document.querySelectorAll(
                        "button, a, input, textarea, [contenteditable='true'], [role='button'], [role='textbox'], div, span"
                    )).filter((el) => normalize(textOf(el)).includes(expected));
                    return pickVisible(candidates);
                }
                if (target.startsWith("role=")) {
                    const raw = target.slice("role=".length);
                    const parts = raw.split("|");
                    const role = normalize(parts[0]);
                    const name = normalize(parts.slice(1).join("|"));
                    const candidates = Array.from(document.querySelectorAll(`[role="${role}"], ${role}`)).filter((el) => {
                        if (!name) return true;
                        return normalize(textOf(el)).includes(name);
                    });
                    return pickVisible(candidates);
                }
                return document.querySelector(target);
            }
    "#
}

fn dispatch_mouse_click(cdp: &mut CdpSession, value: &Value) -> io::Result<()> {
    let x = value
        .get("x")
        .and_then(Value::as_f64)
        .ok_or_else(|| io::Error::other("interaction target missing x coordinate"))?;
    let y = value
        .get("y")
        .and_then(Value::as_f64)
        .ok_or_else(|| io::Error::other("interaction target missing y coordinate"))?;
    cdp.command(
        "Input.dispatchMouseEvent",
        json!({ "type": "mouseMoved", "x": x, "y": y, "button": "none", "buttons": 0 }),
    )?;
    cdp.command(
        "Input.dispatchMouseEvent",
        json!({ "type": "mousePressed", "x": x, "y": y, "button": "left", "buttons": 1, "clickCount": 1 }),
    )?;
    cdp.command(
        "Input.dispatchMouseEvent",
        json!({ "type": "mouseReleased", "x": x, "y": y, "button": "left", "buttons": 0, "clickCount": 1 }),
    )?;
    Ok(())
}

fn dispatch_key_press(cdp: &mut CdpSession, key: &str) -> io::Result<()> {
    let payload = key_event_payload(key)?;
    cdp.command(
        "Input.dispatchKeyEvent",
        payload.get("down").cloned().unwrap_or(Value::Null),
    )?;
    if let Some(char_payload) = payload.get("char") {
        cdp.command("Input.dispatchKeyEvent", char_payload.clone())?;
    }
    cdp.command(
        "Input.dispatchKeyEvent",
        payload.get("up").cloned().unwrap_or(Value::Null),
    )?;
    Ok(())
}

fn key_event_payload(key: &str) -> io::Result<Value> {
    let normalized = key.trim();
    let payload = match normalized {
        "Enter" => json!({
            "down": { "type": "rawKeyDown", "key": "Enter", "code": "Enter", "windowsVirtualKeyCode": 13, "nativeVirtualKeyCode": 13 },
            "char": { "type": "char", "key": "Enter", "code": "Enter", "text": "\r", "unmodifiedText": "\r", "windowsVirtualKeyCode": 13, "nativeVirtualKeyCode": 13 },
            "up": { "type": "keyUp", "key": "Enter", "code": "Enter", "windowsVirtualKeyCode": 13, "nativeVirtualKeyCode": 13 }
        }),
        "Tab" => json!({
            "down": { "type": "rawKeyDown", "key": "Tab", "code": "Tab", "windowsVirtualKeyCode": 9, "nativeVirtualKeyCode": 9 },
            "up": { "type": "keyUp", "key": "Tab", "code": "Tab", "windowsVirtualKeyCode": 9, "nativeVirtualKeyCode": 9 }
        }),
        "Escape" => json!({
            "down": { "type": "rawKeyDown", "key": "Escape", "code": "Escape", "windowsVirtualKeyCode": 27, "nativeVirtualKeyCode": 27 },
            "up": { "type": "keyUp", "key": "Escape", "code": "Escape", "windowsVirtualKeyCode": 27, "nativeVirtualKeyCode": 27 }
        }),
        "Backspace" => json!({
            "down": { "type": "rawKeyDown", "key": "Backspace", "code": "Backspace", "windowsVirtualKeyCode": 8, "nativeVirtualKeyCode": 8 },
            "up": { "type": "keyUp", "key": "Backspace", "code": "Backspace", "windowsVirtualKeyCode": 8, "nativeVirtualKeyCode": 8 }
        }),
        "ArrowDown" => json!({
            "down": { "type": "rawKeyDown", "key": "ArrowDown", "code": "ArrowDown", "windowsVirtualKeyCode": 40, "nativeVirtualKeyCode": 40 },
            "up": { "type": "keyUp", "key": "ArrowDown", "code": "ArrowDown", "windowsVirtualKeyCode": 40, "nativeVirtualKeyCode": 40 }
        }),
        "ArrowUp" => json!({
            "down": { "type": "rawKeyDown", "key": "ArrowUp", "code": "ArrowUp", "windowsVirtualKeyCode": 38, "nativeVirtualKeyCode": 38 },
            "up": { "type": "keyUp", "key": "ArrowUp", "code": "ArrowUp", "windowsVirtualKeyCode": 38, "nativeVirtualKeyCode": 38 }
        }),
        "ArrowLeft" => json!({
            "down": { "type": "rawKeyDown", "key": "ArrowLeft", "code": "ArrowLeft", "windowsVirtualKeyCode": 37, "nativeVirtualKeyCode": 37 },
            "up": { "type": "keyUp", "key": "ArrowLeft", "code": "ArrowLeft", "windowsVirtualKeyCode": 37, "nativeVirtualKeyCode": 37 }
        }),
        "ArrowRight" => json!({
            "down": { "type": "rawKeyDown", "key": "ArrowRight", "code": "ArrowRight", "windowsVirtualKeyCode": 39, "nativeVirtualKeyCode": 39 },
            "up": { "type": "keyUp", "key": "ArrowRight", "code": "ArrowRight", "windowsVirtualKeyCode": 39, "nativeVirtualKeyCode": 39 }
        }),
        "Space" => json!({
            "down": { "type": "rawKeyDown", "key": " ", "code": "Space", "windowsVirtualKeyCode": 32, "nativeVirtualKeyCode": 32 },
            "char": { "type": "char", "key": " ", "code": "Space", "text": " ", "unmodifiedText": " ", "windowsVirtualKeyCode": 32, "nativeVirtualKeyCode": 32 },
            "up": { "type": "keyUp", "key": " ", "code": "Space", "windowsVirtualKeyCode": 32, "nativeVirtualKeyCode": 32 }
        }),
        _ if normalized.chars().count() == 1 => {
            let ch = normalized.chars().next().unwrap_or_default();
            let code = if ch.is_ascii_alphabetic() {
                format!("Key{}", ch.to_ascii_uppercase())
            } else if ch.is_ascii_digit() {
                format!("Digit{ch}")
            } else {
                "Unidentified".to_string()
            };
            let key_code = ch as u32;
            json!({
                "down": { "type": "rawKeyDown", "key": normalized, "code": code, "windowsVirtualKeyCode": key_code, "nativeVirtualKeyCode": key_code },
                "char": { "type": "char", "key": normalized, "code": code, "text": normalized, "unmodifiedText": normalized, "windowsVirtualKeyCode": key_code, "nativeVirtualKeyCode": key_code },
                "up": { "type": "keyUp", "key": normalized, "code": code, "windowsVirtualKeyCode": key_code, "nativeVirtualKeyCode": key_code }
            })
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unsupported key: {normalized}"),
            ))
        }
    };
    Ok(payload)
}

fn spawn_reader_thread(
    mut reader: impl Read + Send + 'static,
    transcript_path: PathBuf,
    output_tail: Arc<Mutex<String>>,
    last_output_ms: Arc<AtomicU64>,
) {
    thread::spawn(move || {
        let mut transcript = match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&transcript_path)
        {
            Ok(file) => file,
            Err(_) => return,
        };

        let mut buffer = [0u8; 4096];
        loop {
            let bytes = match reader.read(&mut buffer) {
                Ok(bytes) => bytes,
                Err(_) => break,
            };
            if bytes == 0 {
                break;
            }

            let chunk = String::from_utf8_lossy(&buffer[..bytes]).to_string();
            let _ = transcript.write_all(chunk.as_bytes());
            let _ = transcript.flush();
            last_output_ms.store(now_ms(), Ordering::SeqCst);

            if let Ok(mut tail) = output_tail.lock() {
                tail.push_str(&chunk);
                trim_tail_to_max_bytes(&mut tail, OUTPUT_TAIL_MAX_BYTES);
            }
        }
    });
}

fn trim_tail_to_max_bytes(text: &mut String, max_bytes: usize) {
    if text.len() <= max_bytes {
        return;
    }
    let mut trim_from = text.len() - max_bytes;
    while !text.is_char_boundary(trim_from) && trim_from < text.len() {
        trim_from += 1;
    }
    *text = text[trim_from..].to_string();
}

fn create_http_connection(port: u16) -> io::Result<TcpStream> {
    TcpStream::connect(("127.0.0.1", port))
}

fn ws_err(err: tungstenite::Error) -> io::Error {
    io::Error::other(format!("browser websocket failed: {err}"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
