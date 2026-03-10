use crate::infra::app_browser::{
    AppBrowser, AppBrowserStatus, BrowserLaunchConfig, SnapshotResult,
};
use crate::io as botty_io;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TRANSCRIPT_READ_MAX_BYTES: usize = 12 * 1024;
const SINGLETON_BROWSER_SESSION_ID: &str = "browser-singleton";
const DEFAULT_BROWSER_USER_DATA_DIR: &str = "app/browser/user_dir";

static BROWSER_SESSION_REGISTRY: OnceLock<Mutex<HashMap<String, Arc<BrowserSession>>>> =
    OnceLock::new();

pub fn handle_browser_skill_request(input_json: &str) -> io::Result<String> {
    let request: BrowserSkillRequest = serde_json::from_str(input_json).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("parse browser skill input failed: {err}"),
        )
    })?;

    match request.action.as_str() {
        "start" => {
            let session = singleton_browser_session(request.headless)?;
            Ok(render_status(&session, true)?)
        }
        "list" => {
            let registry = registry()
                .lock()
                .map_err(|_| io::Error::other("browser session registry lock poisoned"))?;
            if registry.is_empty() {
                return Ok("No active browser sessions.".to_string());
            }
            let mut lines = Vec::new();
            for session in registry.values() {
                lines.push(render_summary_line(session));
            }
            lines.sort();
            Ok(lines.join("\n"))
        }
        "status" => {
            let session = session_for_lookup(request.session_id.as_deref())?;
            Ok(render_status(&session, true)?)
        }
        "transcript" => {
            let session = session_for_lookup(request.session_id.as_deref())?;
            Ok(read_transcript_tail(session.browser.transcript_path())?)
        }
        "navigate" => {
            let session = session_for_action(request.session_id.as_deref(), request.headless)?;
            let url = required_field(request.url.as_deref(), "url")?;
            let result = session.browser.navigate(url)?;
            render_json_result("navigate", &session, result)
        }
        "snapshot" => {
            let session = session_for_action(request.session_id.as_deref(), request.headless)?;
            let result = session.browser.snapshot()?;
            Ok(render_snapshot(&session, &result))
        }
        "click" => {
            let session = session_for_action(request.session_id.as_deref(), request.headless)?;
            let locator = required_field(request.locator.as_deref(), "locator")?;
            let result = session.browser.click(locator)?;
            render_json_result("click", &session, result)
        }
        "fill" => {
            let session = session_for_action(request.session_id.as_deref(), request.headless)?;
            let locator = required_field(request.locator.as_deref(), "locator")?;
            let text = request.text.unwrap_or_default();
            let result = session.browser.fill(locator, &text)?;
            render_json_result("fill", &session, result)
        }
        "eval" => {
            let session = session_for_action(request.session_id.as_deref(), request.headless)?;
            let script = required_field(request.script.as_deref(), "script")?;
            let result = session.browser.eval(script)?;
            render_json_result("eval", &session, result)
        }
        "wait_for" => {
            let session = session_for_action(request.session_id.as_deref(), request.headless)?;
            let timeout_seconds = request.timeout_seconds.unwrap_or(30).max(1);
            if let Some(locator) = request
                .locator
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                let result = session
                    .browser
                    .wait_for(locator, Duration::from_secs(timeout_seconds))?;
                render_json_result("wait_for", &session, result)
            } else {
                thread::sleep(Duration::from_secs(timeout_seconds));
                let snapshot = session.browser.snapshot()?;
                Ok(render_snapshot(&session, &snapshot))
            }
        }
        "screenshot" => {
            let session = session_for_action(request.session_id.as_deref(), request.headless)?;
            let output_path = request
                .output_path
                .map(PathBuf::from)
                .unwrap_or_else(|| session_dir(&session.id).join("screenshot.png"));
            let path = session.browser.capture_screenshot(&output_path)?;
            let payload = json!({
                "saved_to": path.display().to_string()
            });
            let result = render_json_result("screenshot", &session, payload)?;
            Ok(format!(
                "{result}\nattachment={}|photo|{}|Browser screenshot",
                "__botty_attachment__",
                path.display()
            ))
        }
        "close" | "terminate" => {
            let session_id = request
                .session_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(SINGLETON_BROWSER_SESSION_ID);
            let session = get_browser_session(session_id)?;
            let _ = session.browser.terminate();
            remove_browser_session(session_id)?;
            Ok(format!("browser_state=closed\nsession_id={session_id}"))
        }
        other => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsupported browser action: {other}"),
        )),
    }
}

#[derive(Deserialize)]
struct BrowserSkillRequest {
    action: String,
    session_id: Option<String>,
    url: Option<String>,
    locator: Option<String>,
    text: Option<String>,
    script: Option<String>,
    output_path: Option<String>,
    timeout_seconds: Option<u64>,
    headless: Option<bool>,
}

struct BrowserSession {
    id: String,
    chrome_command: String,
    work_dir: PathBuf,
    started_at_ms: u64,
    browser: AppBrowser,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SessionMeta {
    session_id: String,
    state: String,
    started_at_ms: u64,
    updated_at_ms: u64,
    work_dir: String,
    transcript_path: String,
    remote_debugging_port: u16,
    user_data_dir: String,
    chrome_command: String,
    target_id: Option<String>,
}

struct BrowserSettings {
    chrome_command: Option<String>,
    headless: bool,
    user_data_dir: Option<PathBuf>,
}

fn session_for_action(
    session_id: Option<&str>,
    headless: Option<bool>,
) -> io::Result<Arc<BrowserSession>> {
    match session_id {
        Some(session_id) => get_browser_session(session_id),
        None => singleton_browser_session(headless),
    }
}

fn singleton_browser_session(headless_override: Option<bool>) -> io::Result<Arc<BrowserSession>> {
    if let Some(session) = registry()
        .lock()
        .map_err(|_| io::Error::other("browser session registry lock poisoned"))?
        .get(SINGLETON_BROWSER_SESSION_ID)
        .cloned()
    {
        if !session.browser.status().exited {
            return Ok(session);
        }
    }

    if let Some(session) = restore_singleton_browser_session()? {
        registry()
            .lock()
            .map_err(|_| io::Error::other("browser session registry lock poisoned"))?
            .insert(
                SINGLETON_BROWSER_SESSION_ID.to_string(),
                Arc::clone(&session),
            );
        return Ok(session);
    }

    if let Some(session) = latest_running_session()? {
        if session.id == SINGLETON_BROWSER_SESSION_ID {
            return Ok(session);
        }
    }
    close_all_browser_sessions()?;
    start_browser_session_with_id(SINGLETON_BROWSER_SESSION_ID, headless_override)
}

fn start_browser_session_with_id(
    session_id: &str,
    headless_override: Option<bool>,
) -> io::Result<Arc<BrowserSession>> {
    let settings = load_browser_settings()?;
    let chrome_command = resolve_chrome_command(settings.chrome_command.as_deref())?;
    let work_dir = botty_io::effective_work_dir()?;
    let browser = AppBrowser::spawn(BrowserLaunchConfig {
        chrome_command: chrome_command.clone(),
        work_dir: work_dir.clone(),
        transcript_path: session_dir(session_id).join("transcript.log"),
        user_data_dir: settings
            .user_data_dir
            .clone()
            .unwrap_or_else(default_browser_user_data_dir),
        headless: headless_override.unwrap_or(settings.headless),
    })?;
    let session = Arc::new(BrowserSession {
        id: session_id.to_string(),
        chrome_command,
        work_dir,
        started_at_ms: now_ms(),
        browser,
    });
    write_session_meta(&session, "running")?;
    registry()
        .lock()
        .map_err(|_| io::Error::other("browser session registry lock poisoned"))?
        .insert(session_id.to_string(), Arc::clone(&session));
    Ok(session)
}

fn session_for_lookup(session_id: Option<&str>) -> io::Result<Arc<BrowserSession>> {
    match session_id.map(str::trim).filter(|value| !value.is_empty()) {
        Some(session_id) => get_browser_session(session_id),
        None => singleton_browser_session(None),
    }
}

fn get_browser_session(session_id: &str) -> io::Result<Arc<BrowserSession>> {
    registry()
        .lock()
        .map_err(|_| io::Error::other("browser session registry lock poisoned"))?
        .get(session_id)
        .cloned()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("browser session not found in current worker: {session_id}"),
            )
        })
}

fn remove_browser_session(session_id: &str) -> io::Result<()> {
    if let Some(session) = registry()
        .lock()
        .map_err(|_| io::Error::other("browser session registry lock poisoned"))?
        .remove(session_id)
    {
        let _ = write_session_meta(&session, "closed");
    }
    Ok(())
}

fn registry() -> &'static Mutex<HashMap<String, Arc<BrowserSession>>> {
    BROWSER_SESSION_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn latest_running_session() -> io::Result<Option<Arc<BrowserSession>>> {
    let registry = registry()
        .lock()
        .map_err(|_| io::Error::other("browser session registry lock poisoned"))?;
    let mut latest: Option<Arc<BrowserSession>> = None;
    for session in registry.values() {
        if session.browser.status().exited {
            continue;
        }
        match &latest {
            Some(current) if current.started_at_ms >= session.started_at_ms => {}
            _ => latest = Some(Arc::clone(session)),
        }
    }
    Ok(latest)
}

fn close_all_browser_sessions() -> io::Result<()> {
    let sessions: Vec<(String, Arc<BrowserSession>)> = {
        let registry = registry()
            .lock()
            .map_err(|_| io::Error::other("browser session registry lock poisoned"))?;
        registry
            .iter()
            .map(|(id, session)| (id.clone(), Arc::clone(session)))
            .collect()
    };

    for (session_id, session) in sessions {
        let _ = session.browser.terminate();
        let _ = remove_browser_session(&session_id);
    }
    Ok(())
}

fn load_browser_settings() -> io::Result<BrowserSettings> {
    let path = setup_config_file();
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };

    let mut settings = BrowserSettings {
        chrome_command: None,
        headless: false,
        user_data_dir: Some(default_browser_user_data_dir()),
    };

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
            "browser.chrome.command" => {
                if !value.is_empty() {
                    settings.chrome_command = Some(value.to_string());
                }
            }
            "browser.chrome.headless" => {
                settings.headless = matches!(value, "1" | "true" | "yes" | "on");
            }
            "browser.chrome.user_data_dir" => {
                if !value.is_empty() {
                    settings.user_data_dir = Some(resolve_config_path(value));
                }
            }
            _ => {}
        }
    }

    Ok(settings)
}

fn resolve_chrome_command(configured: Option<&str>) -> io::Result<String> {
    if let Some(value) = configured {
        if !value.trim().is_empty() {
            return Ok(value.trim().to_string());
        }
    }

    let candidates = [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "google-chrome",
        "chromium",
        "chromium-browser",
        "chrome",
    ];

    for candidate in candidates {
        if candidate.starts_with('/') {
            if Path::new(candidate).exists() {
                return Ok(candidate.to_string());
            }
            continue;
        }
        let found = Command::new("which")
            .arg(candidate)
            .output()
            .ok()
            .map(|output| output.status.success())
            .unwrap_or(false);
        if found {
            return Ok(candidate.to_string());
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "could not find a Chrome/Chromium executable; set `browser.chrome.command` in setup config",
    ))
}

fn render_status(session: &BrowserSession, include_tail: bool) -> io::Result<String> {
    let status = session.browser.status();
    let mut lines = vec![
        format!("session_id={}", session.id),
        format!("state={}", if status.exited { "exited" } else { "running" }),
        format!("started_at_ms={}", session.started_at_ms),
        format!("last_output_ms={}", status.last_output_ms),
        format!("work_dir={}", session.work_dir.display()),
        format!("chrome_command={}", session.chrome_command),
        format!("remote_debugging_port={}", status.remote_debugging_port),
        format!("target_id={}", session.browser.target_id()),
        format!("page_title={}", sanitize_inline_value(&status.page_title)),
        format!("page_url={}", sanitize_inline_value(&status.page_url)),
        format!("ready_state={}", sanitize_inline_value(&status.ready_state)),
        format!(
            "transcript_path={}",
            session.browser.transcript_path().display()
        ),
        format!("user_data_dir={}", status.user_data_dir.display()),
    ];
    if let Some(exit_code) = status.exit_code {
        lines.push(format!("exit_code={exit_code}"));
    }
    if include_tail {
        lines.push("transcript_tail:".to_string());
        lines.push(if status.transcript_tail.trim().is_empty() {
            "(empty)".to_string()
        } else {
            status.transcript_tail
        });
    }
    Ok(lines.join("\n"))
}

fn render_summary_line(session: &BrowserSession) -> String {
    let status = session.browser.status();
    format!(
        "session_id={} state={} page_title={} page_url={}",
        session.id,
        if status.exited { "exited" } else { "running" },
        sanitize_inline_value(&status.page_title),
        sanitize_inline_value(&status.page_url)
    )
}

fn render_snapshot(session: &BrowserSession, snapshot: &SnapshotResult) -> String {
    let payload = json!({
        "title": snapshot.title,
        "url": snapshot.url,
        "readyState": snapshot.ready_state,
        "textExcerpt": snapshot.text_excerpt,
        "items": snapshot.items,
    });
    format!(
        "browser_action=snapshot\nsession_id={}\nresult={}",
        session.id,
        serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string())
    )
}

fn render_json_result(
    action: &str,
    session: &BrowserSession,
    result: serde_json::Value,
) -> io::Result<String> {
    let payload = serde_json::to_string_pretty(&result)
        .map_err(|err| io::Error::other(format!("serialize browser result failed: {err}")))?;
    Ok(format!(
        "browser_action={action}\nsession_id={}\nresult={payload}",
        session.id
    ))
}

fn required_field<'a>(value: Option<&'a str>, name: &str) -> io::Result<&'a str> {
    value
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("browser action requires `{name}`"),
            )
        })
}

fn sanitize_inline_value(value: &str) -> String {
    value.replace('\n', "\\n").replace('\r', "\\r")
}

fn write_session_meta(session: &BrowserSession, state: &str) -> io::Result<()> {
    let status: AppBrowserStatus = session.browser.status();
    let path = session_meta_path(&session.id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let meta = SessionMeta {
        session_id: session.id.clone(),
        state: state.to_string(),
        started_at_ms: session.started_at_ms,
        updated_at_ms: now_ms(),
        work_dir: session.work_dir.display().to_string(),
        transcript_path: session.browser.transcript_path().display().to_string(),
        remote_debugging_port: status.remote_debugging_port,
        user_data_dir: status.user_data_dir.display().to_string(),
        chrome_command: session.chrome_command.clone(),
        target_id: Some(session.browser.target_id().to_string()),
    };
    let content = serde_json::to_string_pretty(&meta)
        .map_err(|err| io::Error::other(format!("serialize browser session meta failed: {err}")))?;
    fs::write(path, content)
}

fn session_dir(session_id: &str) -> PathBuf {
    sessions_root_dir().join(session_id)
}

fn sessions_root_dir() -> PathBuf {
    botty_root_dir()
        .join("app")
        .join("browser")
        .join("sessions")
}

fn session_meta_path(session_id: &str) -> PathBuf {
    session_dir(session_id).join("session.json")
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

fn resolve_config_path(value: &str) -> PathBuf {
    let trimmed = value.trim();
    if let Some(rest) = trimmed.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    let path = PathBuf::from(trimmed);
    if path.is_absolute() {
        path
    } else {
        botty_root_dir().join(path)
    }
}

fn default_browser_user_data_dir() -> PathBuf {
    botty_root_dir().join(DEFAULT_BROWSER_USER_DATA_DIR)
}

fn restore_singleton_browser_session() -> io::Result<Option<Arc<BrowserSession>>> {
    let path = session_meta_path(SINGLETON_BROWSER_SESSION_ID);
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    let meta: SessionMeta = serde_json::from_str(&content).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("parse browser session meta failed: {err}"),
        )
    })?;
    if meta.state == "closed" {
        return Ok(None);
    }

    let browser = match AppBrowser::attach(
        PathBuf::from(&meta.transcript_path),
        PathBuf::from(&meta.user_data_dir),
        meta.remote_debugging_port,
        meta.target_id.as_deref(),
    ) {
        Ok(browser) => browser,
        Err(_) => return Ok(None),
    };

    Ok(Some(Arc::new(BrowserSession {
        id: meta.session_id,
        chrome_command: meta.chrome_command,
        work_dir: PathBuf::from(meta.work_dir),
        started_at_ms: meta.started_at_ms,
        browser,
    })))
}

fn read_transcript_tail(path: &Path) -> io::Result<String> {
    let content = fs::read_to_string(path)?;
    if content.len() <= TRANSCRIPT_READ_MAX_BYTES {
        return Ok(content);
    }
    let mut start = content.len() - TRANSCRIPT_READ_MAX_BYTES;
    while !content.is_char_boundary(start) && start < content.len() {
        start += 1;
    }
    Ok(content[start..].to_string())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
