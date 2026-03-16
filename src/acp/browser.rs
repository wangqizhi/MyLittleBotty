use crate::botty_brain::BottyBrain;
use crate::infra::app_browser::{
    AppBrowser, AppBrowserStatus, BrowserLaunchConfig, SnapshotResult,
};
use crate::io as botty_io;
use crate::llm_provider::{ProviderMessage, ProviderResponse};
use crate::prompt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TRANSCRIPT_READ_MAX_BYTES: usize = 12 * 1024;
const TRACE_TEXT_MAX_CHARS: usize = 400;
const TRACE_ARRAY_MAX_ITEMS: usize = 20;
const TRACE_JSON_MAX_DEPTH: usize = 6;
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
            let trace_input = json!({ "url": url });
            let result = match session.browser.navigate(url) {
                Ok(result) => {
                    let _ =
                        trace_action_success(&session, "navigate", trace_input.clone(), &result);
                    result
                }
                Err(err) => {
                    let _ = trace_action_error(&session, "navigate", trace_input, &err);
                    return Err(err);
                }
            };
            render_json_result("navigate", &session, result)
        }
        "snapshot" => {
            let session = session_for_action(request.session_id.as_deref(), request.headless)?;
            let result = match session.browser.snapshot() {
                Ok(result) => {
                    let _ = trace_action_success(
                        &session,
                        "snapshot",
                        json!({}),
                        &snapshot_payload(&result),
                    );
                    result
                }
                Err(err) => {
                    let _ = trace_action_error(&session, "snapshot", json!({}), &err);
                    return Err(err);
                }
            };
            Ok(render_snapshot(&session, &result))
        }
        "click" => {
            let session = session_for_action(request.session_id.as_deref(), request.headless)?;
            let locator = required_field(request.locator.as_deref(), "locator")?;
            let trace_input = json!({ "locator": locator });
            let result = match session.browser.click(locator) {
                Ok(result) => {
                    let _ = trace_action_success(&session, "click", trace_input.clone(), &result);
                    result
                }
                Err(err) => {
                    let _ = trace_action_error(&session, "click", trace_input, &err);
                    return Err(err);
                }
            };
            render_json_result("click", &session, result)
        }
        "focus" => {
            let session = session_for_action(request.session_id.as_deref(), request.headless)?;
            let locator = required_field(request.locator.as_deref(), "locator")?;
            let trace_input = json!({ "locator": locator });
            let result = match session.browser.focus(locator) {
                Ok(result) => {
                    let _ = trace_action_success(&session, "focus", trace_input.clone(), &result);
                    result
                }
                Err(err) => {
                    let _ = trace_action_error(&session, "focus", trace_input, &err);
                    return Err(err);
                }
            };
            render_json_result("focus", &session, result)
        }
        "fill" => {
            let session = session_for_action(request.session_id.as_deref(), request.headless)?;
            let locator = required_field(request.locator.as_deref(), "locator")?;
            let text = request.text.unwrap_or_default();
            let trace_input = json!({ "locator": locator, "text": text.clone() });
            let result = match session.browser.fill(locator, &text) {
                Ok(result) => {
                    let _ = trace_action_success(&session, "fill", trace_input.clone(), &result);
                    result
                }
                Err(err) => {
                    let _ = trace_action_error(&session, "fill", trace_input, &err);
                    return Err(err);
                }
            };
            render_json_result("fill", &session, result)
        }
        "press" => {
            let session = session_for_action(request.session_id.as_deref(), request.headless)?;
            let key = required_field(request.key.as_deref(), "key")?;
            let trace_input = json!({ "key": key });
            let result = match session.browser.press(key) {
                Ok(result) => {
                    let _ = trace_action_success(&session, "press", trace_input.clone(), &result);
                    result
                }
                Err(err) => {
                    let _ = trace_action_error(&session, "press", trace_input, &err);
                    return Err(err);
                }
            };
            render_json_result("press", &session, result)
        }
        "eval" => {
            let session = session_for_action(request.session_id.as_deref(), request.headless)?;
            let script = required_field(request.script.as_deref(), "script")?;
            let trace_input = json!({ "script": script });
            let result = match session.browser.eval(script) {
                Ok(result) => {
                    let _ = trace_action_success(&session, "eval", trace_input.clone(), &result);
                    result
                }
                Err(err) => {
                    let _ = trace_action_error(&session, "eval", trace_input, &err);
                    return Err(err);
                }
            };
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
                let trace_input = json!({
                    "locator": locator,
                    "timeout_seconds": timeout_seconds,
                });
                let result = match session
                    .browser
                    .wait_for(locator, Duration::from_secs(timeout_seconds))
                {
                    Ok(result) => {
                        let _ = trace_action_success(
                            &session,
                            "wait_for",
                            trace_input.clone(),
                            &result,
                        );
                        result
                    }
                    Err(err) => {
                        let _ = trace_action_error(&session, "wait_for", trace_input, &err);
                        return Err(err);
                    }
                };
                render_json_result("wait_for", &session, result)
            } else {
                thread::sleep(Duration::from_secs(timeout_seconds));
                let trace_input = json!({ "timeout_seconds": timeout_seconds });
                let snapshot = match session.browser.snapshot() {
                    Ok(snapshot) => {
                        let _ = trace_action_success(
                            &session,
                            "wait_for",
                            trace_input.clone(),
                            &snapshot_payload(&snapshot),
                        );
                        snapshot
                    }
                    Err(err) => {
                        let _ = trace_action_error(&session, "wait_for", trace_input, &err);
                        return Err(err);
                    }
                };
                Ok(render_snapshot(&session, &snapshot))
            }
        }
        "screenshot" => {
            let session = session_for_action(request.session_id.as_deref(), request.headless)?;
            let output_path = request
                .output_path
                .map(PathBuf::from)
                .unwrap_or_else(|| session_dir(&session.id).join("screenshot.png"));
            let trace_input = json!({ "output_path": output_path.display().to_string() });
            let path = match session.browser.capture_screenshot(&output_path) {
                Ok(path) => {
                    let payload = json!({ "saved_to": path.display().to_string() });
                    let _ =
                        trace_action_success(&session, "screenshot", trace_input.clone(), &payload);
                    path
                }
                Err(err) => {
                    let _ = trace_action_error(&session, "screenshot", trace_input, &err);
                    return Err(err);
                }
            };
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
        "complete_task" => {
            let session = session_for_action(request.session_id.as_deref(), request.headless)?;
            let task = required_field(request.task.as_deref(), "task")?;
            complete_browser_task(&session, task, request.outcome.as_deref())
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
    key: Option<String>,
    script: Option<String>,
    output_path: Option<String>,
    task: Option<String>,
    outcome: Option<String>,
    timeout_seconds: Option<u64>,
    headless: Option<bool>,
}

#[derive(Clone, Serialize, Deserialize)]
struct BrowserActionTraceEntry {
    timestamp_ms: u64,
    session_id: String,
    action: String,
    input: Value,
    success: bool,
    result: Option<Value>,
    error: Option<String>,
    page_url: String,
    page_title: String,
    ready_state: String,
}

#[derive(Serialize, Deserialize)]
struct BrowserProcedureRecord {
    version: u32,
    created_at_ms: u64,
    session_id: String,
    task: String,
    outcome: Option<String>,
    domain: String,
    page_url: String,
    page_title: String,
    trace_path: String,
    trace_entries: usize,
    markdown_path: String,
    sop_markdown: String,
}

pub(crate) struct BrowserSession {
    pub(crate) id: String,
    chrome_command: String,
    work_dir: PathBuf,
    started_at_ms: u64,
    pub(crate) browser: AppBrowser,
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
    #[serde(default)]
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

pub(crate) fn singleton_browser_session(
    headless_override: Option<bool>,
) -> io::Result<Arc<BrowserSession>> {
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
    let _ = trace_action_success(
        &session,
        "session_start",
        json!({ "headless": headless_override.unwrap_or(settings.headless) }),
        &json!({ "ok": true }),
    );
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
        format!("trace_path={}", trace_file_path(&session.id).display()),
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
    let payload = snapshot_payload(snapshot);
    format!(
        "browser_action=snapshot\nsession_id={}\nresult={}",
        session.id,
        serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string())
    )
}

fn snapshot_payload(snapshot: &SnapshotResult) -> Value {
    json!({
        "title": snapshot.title,
        "url": snapshot.url,
        "readyState": snapshot.ready_state,
        "textExcerpt": snapshot.text_excerpt,
        "items": snapshot.items,
    })
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

fn trace_action_success(
    session: &BrowserSession,
    action: &str,
    input: Value,
    result: &Value,
) -> io::Result<()> {
    let status = session.browser.status();
    write_trace_entry(
        session,
        BrowserActionTraceEntry {
            timestamp_ms: now_ms(),
            session_id: session.id.clone(),
            action: action.to_string(),
            input: truncate_json_value(&input, 0),
            success: true,
            result: Some(truncate_json_value(result, 0)),
            error: None,
            page_url: sanitize_inline_value(&status.page_url),
            page_title: sanitize_inline_value(&status.page_title),
            ready_state: sanitize_inline_value(&status.ready_state),
        },
    )
}

fn trace_action_error(
    session: &BrowserSession,
    action: &str,
    input: Value,
    err: &io::Error,
) -> io::Result<()> {
    let status = session.browser.status();
    write_trace_entry(
        session,
        BrowserActionTraceEntry {
            timestamp_ms: now_ms(),
            session_id: session.id.clone(),
            action: action.to_string(),
            input: truncate_json_value(&input, 0),
            success: false,
            result: None,
            error: Some(sanitize_inline_value(&err.to_string())),
            page_url: sanitize_inline_value(&status.page_url),
            page_title: sanitize_inline_value(&status.page_title),
            ready_state: sanitize_inline_value(&status.ready_state),
        },
    )
}

fn write_trace_entry(session: &BrowserSession, entry: BrowserActionTraceEntry) -> io::Result<()> {
    let path = trace_file_path(&session.id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let line = serde_json::to_string(&entry)
        .map_err(|err| io::Error::other(format!("serialize browser trace entry failed: {err}")))?;
    writeln!(file, "{line}")
}

fn complete_browser_task(
    session: &BrowserSession,
    task: &str,
    outcome: Option<&str>,
) -> io::Result<String> {
    let trace_entries = read_trace_entries(&session.id)?;
    let task_trace_entries = current_task_trace_entries(&trace_entries);
    if task_trace_entries.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no browser action trace found for the current task in this session",
        ));
    }

    let status = session.browser.status();
    let domain = extract_domain(&status.page_url).unwrap_or_else(|| "unknown-domain".to_string());
    let created_at_ms = now_ms();
    let task_slug = slugify(task);
    let file_stem = format!("{created_at_ms}-{task_slug}");
    let directory = browser_procedure_root_dir().join(slugify(&domain));
    fs::create_dir_all(&directory)?;

    let sop_markdown =
        generate_browser_procedure_sop(session, task, outcome, &status, &task_trace_entries)?;
    let markdown_path = directory.join(format!("{file_stem}.md"));
    let json_path = directory.join(format!("{file_stem}.json"));

    fs::write(&markdown_path, ensure_trailing_newline(&sop_markdown))?;

    let record = BrowserProcedureRecord {
        version: 1,
        created_at_ms,
        session_id: session.id.clone(),
        task: task.trim().to_string(),
        outcome: outcome
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        domain: domain.clone(),
        page_url: status.page_url.clone(),
        page_title: status.page_title.clone(),
        trace_path: trace_file_path(&session.id).display().to_string(),
        trace_entries: task_trace_entries.len(),
        markdown_path: markdown_path.display().to_string(),
        sop_markdown: sop_markdown.clone(),
    };
    let json_body = serde_json::to_string_pretty(&record).map_err(|err| {
        io::Error::other(format!("serialize browser procedure record failed: {err}"))
    })?;
    fs::write(&json_path, ensure_trailing_newline(&json_body))?;

    let trace_input = json!({
        "task": task.trim(),
        "outcome": outcome.unwrap_or_default(),
    });
    let trace_result = json!({
        "saved_to": json_path.display().to_string(),
        "markdown_path": markdown_path.display().to_string(),
        "trace_entries": task_trace_entries.len(),
        "domain": domain,
    });
    let _ = trace_action_success(session, "complete_task", trace_input, &trace_result);

    Ok(format!(
        "browser_action=complete_task\nsession_id={}\nresult={}\nprocedure_path={}\nprocedure_markdown_path={}",
        session.id,
        serde_json::to_string_pretty(&trace_result)
            .unwrap_or_else(|_| trace_result.to_string()),
        json_path.display(),
        markdown_path.display()
    ))
}

fn generate_browser_procedure_sop(
    session: &BrowserSession,
    task: &str,
    outcome: Option<&str>,
    status: &AppBrowserStatus,
    trace_entries: &[BrowserActionTraceEntry],
) -> io::Result<String> {
    let brain = BottyBrain::from_setup()?;
    let domain = extract_domain(&status.page_url).unwrap_or_else(|| "unknown-domain".to_string());
    let outcome = outcome
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("(not provided)");
    let trace_text = format_trace_for_prompt(trace_entries);
    let user_prompt = format!(
        "Create a reusable browser SOP from the successful task trace.\n\nTask: {task}\nOutcome: {outcome}\nDomain: {domain}\nCurrent page title: {}\nCurrent page URL: {}\nSession id: {}\nTrace entries: {}\n\nWrite Markdown with these sections in order:\n# Task\n# Preconditions\n# Stable Cues\n# Steps\n# Success Signals\n# Fallbacks\n\nKeep it concise and actionable.\nUse concrete locators, page cues, and URLs from the trace when stable.\nIf a locator looks fragile, say so in Fallbacks instead of overcommitting.\n\nObserved trace:\n```text\n{trace_text}\n```",
        status.page_title.trim(),
        status.page_url.trim(),
        session.id,
        trace_entries.len(),
    );
    let response = brain.think(
        prompt::BROWSER_PROCEDURE_SYSTEM_PROMPT,
        &[ProviderMessage::UserText(user_prompt)],
        &[],
    )?;
    match response {
        ProviderResponse::Text(reply) => Ok(reply.text.trim().to_string()),
        ProviderResponse::ToolUse(_) => Err(io::Error::other(
            "browser procedure summary unexpectedly returned a tool call",
        )),
    }
}

fn format_trace_for_prompt(entries: &[BrowserActionTraceEntry]) -> String {
    let mut lines = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let input = serde_json::to_string(&entry.input).unwrap_or_else(|_| "{}".to_string());
        let result = entry
            .result
            .as_ref()
            .map(|value| serde_json::to_string(value).unwrap_or_else(|_| "null".to_string()))
            .unwrap_or_else(|| "null".to_string());
        let error = entry.error.as_deref().unwrap_or("-");
        lines.push(format!(
            "{}. action={} success={} page_title={} page_url={} ready_state={} input={} result={} error={}",
            index + 1,
            entry.action,
            entry.success,
            sanitize_inline_value(&entry.page_title),
            sanitize_inline_value(&entry.page_url),
            sanitize_inline_value(&entry.ready_state),
            input,
            result,
            sanitize_inline_value(error),
        ));
    }
    lines.join("\n")
}

fn read_trace_entries(session_id: &str) -> io::Result<Vec<BrowserActionTraceEntry>> {
    let path = trace_file_path(session_id);
    let content = fs::read_to_string(&path)?;
    let mut entries = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let entry = serde_json::from_str::<BrowserActionTraceEntry>(trimmed).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("parse browser trace entry failed: {err}"),
            )
        })?;
        entries.push(entry);
    }
    Ok(entries)
}

fn current_task_trace_entries(entries: &[BrowserActionTraceEntry]) -> Vec<BrowserActionTraceEntry> {
    let start_index = entries
        .iter()
        .rposition(|entry| entry.action == "complete_task" || entry.action == "session_start")
        .map(|index| index + 1)
        .unwrap_or(0);
    entries[start_index..].to_vec()
}

fn truncate_json_value(value: &Value, depth: usize) -> Value {
    if depth >= TRACE_JSON_MAX_DEPTH {
        return Value::String("[truncated-depth]".to_string());
    }

    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
        Value::String(text) => Value::String(truncate_text(text, TRACE_TEXT_MAX_CHARS)),
        Value::Array(items) => {
            let mut out = Vec::new();
            for item in items.iter().take(TRACE_ARRAY_MAX_ITEMS) {
                out.push(truncate_json_value(item, depth + 1));
            }
            if items.len() > TRACE_ARRAY_MAX_ITEMS {
                out.push(Value::String(format!(
                    "[truncated {} more items]",
                    items.len() - TRACE_ARRAY_MAX_ITEMS
                )));
            }
            Value::Array(out)
        }
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, item) in map {
                out.insert(key.clone(), truncate_json_value(item, depth + 1));
            }
            Value::Object(out)
        }
    }
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= max_chars {
            out.push_str("...[truncated]");
            return out;
        }
        out.push(ch);
    }
    out
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

fn ensure_trailing_newline(text: &str) -> String {
    if text.ends_with('\n') {
        text.to_string()
    } else {
        format!("{text}\n")
    }
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

fn trace_file_path(session_id: &str) -> PathBuf {
    session_dir(session_id).join("action-trace.jsonl")
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

fn browser_procedure_root_dir() -> PathBuf {
    botty_root_dir().join("memory").join("browser-procedures")
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

fn extract_domain(url: &str) -> Option<String> {
    let trimmed = url.trim();
    let after_scheme = trimmed
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(trimmed);
    let host = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .trim();
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host).trim();
    if host.is_empty() {
        None
    } else {
        Some(host.to_ascii_lowercase())
    }
}

fn slugify(text: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;
    for ch in text.chars() {
        let normalized = ch.to_ascii_lowercase();
        if normalized.is_ascii_alphanumeric() {
            slug.push(normalized);
            last_was_dash = false;
        } else if !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    while slug.starts_with('-') {
        slug.remove(0);
    }
    if slug.is_empty() {
        "task".to_string()
    } else {
        slug
    }
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
