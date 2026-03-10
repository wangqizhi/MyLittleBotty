use crate::infra::app_terminal::AppTerminal;
use crate::io as botty_io;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TRANSCRIPT_READ_MAX_BYTES: usize = 12 * 1024;
const EXECUTE_TASK_POLL_SECONDS: u64 = 10;
const COMPLETED_MARKER: &str = "BOTTY_TERMINAL_STATUS: COMPLETED";
const NEED_USER_MARKER: &str = "BOTTY_TERMINAL_STATUS: NEED_USER_INPUT";

static SESSION_REGISTRY: OnceLock<Mutex<HashMap<String, Arc<AgentSession>>>> = OnceLock::new();

pub fn handle_skill_request(input_json: &str) -> io::Result<String> {
    let request: TerminalSkillRequest = serde_json::from_str(input_json).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("parse terminal skill input failed: {err}"),
        )
    })?;

    match request.action.as_str() {
        "execute_task" => {
            let prompt = required_field(request.prompt.as_deref(), "prompt")?;
            let session = start_session(request.provider.as_deref())?;
            wait_for_output(2);
            session.send(&build_terminal_task_prompt(prompt))?;
            monitor_session_until_terminal_state(&session)
        }
        "continue_session" => {
            let prompt = required_field(request.prompt.as_deref(), "prompt")?;
            let session_id = required_field(request.session_id.as_deref(), "session_id")?;
            let session = get_session(session_id)?;
            wait_for_output(1);
            session.send(prompt)?;
            monitor_session_until_terminal_state(&session)
        }
        "status" => {
            let session_id = required_field(request.session_id.as_deref(), "session_id")?;
            let session = get_session(session_id)?;
            if let Some(wait_seconds) = request.wait_seconds {
                wait_for_output(wait_seconds);
            }
            Ok(render_session_status(&session, true)?)
        }
        "transcript" => {
            let session_id = required_field(request.session_id.as_deref(), "session_id")?;
            let session = get_session(session_id)?;
            if let Some(wait_seconds) = request.wait_seconds {
                wait_for_output(wait_seconds);
            }
            Ok(read_transcript_tail(session.terminal.transcript_path())?)
        }
        "interrupt" => {
            let session_id = required_field(request.session_id.as_deref(), "session_id")?;
            let session = get_session(session_id)?;
            session.interrupt()?;
            wait_for_output(1);
            Ok(render_session_status(&session, true)?)
        }
        "terminate" => {
            let session_id = required_field(request.session_id.as_deref(), "session_id")?;
            let session = get_session(session_id)?;
            session.terminate()?;
            Ok(render_session_status(&session, true)?)
        }
        "restart" => {
            let provider = request.provider.as_deref();
            if let Some(session_id) = request.session_id.as_deref() {
                if let Ok(session) = get_session(session_id) {
                    let _ = session.terminate();
                    registry()
                        .lock()
                        .map_err(|_| io::Error::other("session registry lock poisoned"))?
                        .remove(session_id);
                }
            }
            let session = start_session(provider)?;
            if let Some(prompt) = request.prompt.as_deref() {
                if !prompt.trim().is_empty() {
                    session.send(prompt)?;
                    wait_for_output(request.wait_seconds.unwrap_or(2));
                }
            }
            Ok(render_session_status(&session, true)?)
        }
        "list" => {
            let registry = registry()
                .lock()
                .map_err(|_| io::Error::other("session registry lock poisoned"))?;
            if registry.is_empty() {
                return Ok("No active agent sessions.".to_string());
            }
            let mut lines = Vec::new();
            for session in registry.values() {
                lines.push(render_session_summary_line(session));
            }
            lines.sort();
            Ok(lines.join("\n"))
        }
        other => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsupported terminal action: {other}"),
        )),
    }
}

#[derive(Deserialize)]
struct TerminalSkillRequest {
    action: String,
    session_id: Option<String>,
    prompt: Option<String>,
    provider: Option<String>,
    wait_seconds: Option<u64>,
}

struct AgentSession {
    id: String,
    provider: String,
    command_line: String,
    work_dir: PathBuf,
    started_at_ms: u64,
    terminal: AppTerminal,
}

#[derive(Serialize, Deserialize)]
struct SessionMeta {
    session_id: String,
    provider: String,
    state: String,
    started_at_ms: u64,
    updated_at_ms: u64,
    work_dir: String,
    transcript_path: String,
}

impl AgentSession {
    fn send(&self, prompt: &str) -> io::Result<()> {
        self.terminal.send_line(prompt)
    }

    fn interrupt(&self) -> io::Result<()> {
        self.terminal.send_ctrl_c()
    }

    fn terminate(&self) -> io::Result<()> {
        self.terminal.terminate()
    }
}

enum TerminalTaskState {
    Running,
    NeedsUserInput(String),
    Completed(String),
    Exited(String),
}

struct AgentRuntimeConfig {
    provider: String,
    program: String,
    args: Vec<String>,
    envs: Vec<(String, String)>,
    work_dir: PathBuf,
}

fn start_session(provider_override: Option<&str>) -> io::Result<Arc<AgentSession>> {
    let config = load_runtime_config(provider_override)?;
    fs::create_dir_all(&config.work_dir)?;
    let session_id = format!("{}-{}", config.provider, now_ms());
    let transcript_path = sessions_root_dir().join(&session_id).join("transcript.log");
    let command_line = if config.args.is_empty() {
        config.program.clone()
    } else {
        format!("{} {}", config.program, config.args.join(" "))
    };
    let terminal = AppTerminal::spawn(
        &config.program,
        &config.args,
        &config.work_dir,
        transcript_path,
        &config.envs,
    )?;
    let session = Arc::new(AgentSession {
        id: session_id.clone(),
        provider: config.provider,
        command_line,
        work_dir: config.work_dir,
        started_at_ms: now_ms(),
        terminal,
    });
    write_session_meta(&session, "running")?;
    registry()
        .lock()
        .map_err(|_| io::Error::other("session registry lock poisoned"))?
        .insert(session_id, Arc::clone(&session));
    Ok(session)
}

fn get_session(session_id: &str) -> io::Result<Arc<AgentSession>> {
    registry()
        .lock()
        .map_err(|_| io::Error::other("session registry lock poisoned"))?
        .get(session_id)
        .cloned()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("agent session not found in current worker: {session_id}"),
            )
        })
}

fn registry() -> &'static Mutex<HashMap<String, Arc<AgentSession>>> {
    SESSION_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn load_runtime_config(provider_override: Option<&str>) -> io::Result<AgentRuntimeConfig> {
    let settings = load_agent_settings()?;
    let provider = provider_override
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| settings.provider.clone());
    let work_dir = botty_io::effective_work_dir()?;

    match provider.as_str() {
        "codex" => {
            ensure_codex_logged_in(&settings.codex_command)?;
            Ok(AgentRuntimeConfig {
                provider,
                program: settings.codex_command,
                args: vec![
                    "--no-alt-screen".to_string(),
                    "-s".to_string(),
                    "workspace-write".to_string(),
                    "-a".to_string(),
                    "never".to_string(),
                    "-C".to_string(),
                    work_dir.display().to_string(),
                ],
                envs: Vec::new(),
                work_dir,
            })
        }
        "claude" => Ok(AgentRuntimeConfig {
            provider,
            program: settings.claude_command,
            args: Vec::new(),
            envs: Vec::new(),
            work_dir,
        }),
        other => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsupported terminal provider: {other}"),
        )),
    }
}

struct AgentSettings {
    provider: String,
    codex_command: String,
    claude_command: String,
}

fn load_agent_settings() -> io::Result<AgentSettings> {
    let path = setup_config_file();
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };

    let mut settings = AgentSettings {
        provider: "codex".to_string(),
        codex_command: "codex".to_string(),
        claude_command: "claude".to_string(),
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
            "agent.provider" => settings.provider = value.to_ascii_lowercase(),
            "agent.codex.command" => {
                if !value.is_empty() {
                    settings.codex_command = value.to_string();
                }
            }
            "agent.claude.command" => {
                if !value.is_empty() {
                    settings.claude_command = value.to_string();
                }
            }
            _ => {}
        }
    }

    Ok(settings)
}

fn render_session_status(session: &AgentSession, include_tail: bool) -> io::Result<String> {
    let status = session.terminal.status();
    let state = if status.exited { "exited" } else { "running" };
    let mut lines = vec![
        format!("session_id={}", session.id),
        format!("provider={}", session.provider),
        format!("state={state}"),
        format!("started_at_ms={}", session.started_at_ms),
        format!("last_output_ms={}", status.last_output_ms),
        format!("work_dir={}", session.work_dir.display()),
        format!("command={}", session.command_line),
        format!("transcript_path={}", session.terminal.transcript_path().display()),
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

fn monitor_session_until_terminal_state(session: &Arc<AgentSession>) -> io::Result<String> {
    loop {
        wait_for_output(EXECUTE_TASK_POLL_SECONDS);
        let transcript = read_transcript_tail(session.terminal.transcript_path())?;
        match analyze_terminal_transcript(session, &transcript) {
            TerminalTaskState::Running => continue,
            TerminalTaskState::NeedsUserInput(question) => {
                let _ = write_session_meta(session, "needs_user_input");
                return Ok(format!(
                    "terminal_state=needs_user_input\nsession_id={}\nquestion={}\ntranscript_path={}\ntranscript_tail:\n{}",
                    session.id,
                    sanitize_inline_value(&question),
                    session.terminal.transcript_path().display(),
                    transcript
                ));
            }
            TerminalTaskState::Completed(summary) => {
                let _ = write_session_meta(session, "completed");
                let _ = session.terminate();
                let _ = remove_session(&session.id);
                return Ok(format!(
                    "terminal_state=completed\nsummary={}\ntranscript_path={}\ntranscript_tail:\n{}",
                    sanitize_inline_value(&summary),
                    session.terminal.transcript_path().display(),
                    transcript
                ));
            }
            TerminalTaskState::Exited(reason) => {
                let _ = write_session_meta(session, "exited");
                let _ = remove_session(&session.id);
                return Ok(format!(
                    "terminal_state=exited\nreason={}\ntranscript_path={}\ntranscript_tail:\n{}",
                    sanitize_inline_value(&reason),
                    session.terminal.transcript_path().display(),
                    transcript
                ));
            }
        }
    }
}

fn analyze_terminal_transcript(session: &AgentSession, transcript: &str) -> TerminalTaskState {
    if let Some(question) = extract_marker_payload(transcript, NEED_USER_MARKER) {
        return TerminalTaskState::NeedsUserInput(question);
    }
    if let Some(summary) = extract_marker_payload(transcript, COMPLETED_MARKER) {
        return TerminalTaskState::Completed(summary);
    }

    let status = session.terminal.status();
    if status.exited {
        let reason = if transcript.trim().is_empty() {
            status
                .exit_code
                .map(|code| format!("terminal agent exited with code {code}"))
                .unwrap_or_else(|| "terminal agent exited".to_string())
        } else {
            transcript.to_string()
        };
        return TerminalTaskState::Exited(reason);
    }

    TerminalTaskState::Running
}

fn extract_marker_payload(transcript: &str, marker: &str) -> Option<String> {
    transcript
        .lines()
        .rev()
        .find_map(|line| line.split_once(marker).map(|(_, rest)| rest.trim().to_string()))
        .filter(|text| !text.is_empty())
}

fn build_terminal_task_prompt(task: &str) -> String {
    format!(
        "You are working inside the user's configured work directory only.\n\
Complete the following task end to end.\n\
Task:\n{task}\n\n\
Rules:\n\
- Do the work directly in the current workspace.\n\
- If you need a user decision or missing input, print exactly one line starting with `{NEED_USER_MARKER}` followed by the concise question.\n\
- When the task is fully complete, print exactly one line starting with `{COMPLETED_MARKER}` followed by a concise completion summary that includes the main output path.\n\
- Do not ask for confirmation if the task can be completed directly.\n"
    )
}

fn remove_session(session_id: &str) -> io::Result<()> {
    if let Some(session) = registry()
        .lock()
        .map_err(|_| io::Error::other("session registry lock poisoned"))?
        .remove(session_id)
    {
        let _ = write_session_meta(&session, "terminated");
    }
    Ok(())
}

fn sanitize_inline_value(value: &str) -> String {
    value.replace('\n', "\\n").replace('\r', "\\r")
}

fn ensure_codex_logged_in(command: &str) -> io::Result<()> {
    let output = Command::new(command)
        .arg("login")
        .arg("status")
        .output()
        .map_err(|err| {
            io::Error::new(
                err.kind(),
                format!("failed to check Codex login status with `{command} login status`: {err}"),
            )
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let combined = format!("{stdout}\n{stderr}").to_ascii_lowercase();

    if output.status.success()
        && (combined.contains("logged in")
            || combined.contains("using chatgpt")
            || combined.contains("using api key"))
    {
        return Ok(());
    }

    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!(
            "Codex CLI is not logged in. Please run `{} login` in your terminal first.",
            command
        ),
    ))
}

fn render_session_summary_line(session: &AgentSession) -> String {
    let status = session.terminal.status();
    let state = if status.exited { "exited" } else { "running" };
    format!(
        "session_id={} provider={} state={} work_dir={}",
        session.id,
        session.provider,
        state,
        session.work_dir.display()
    )
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

fn wait_for_output(seconds: u64) {
    thread::sleep(Duration::from_secs(seconds.max(1)));
}

fn required_field<'a>(value: Option<&'a str>, name: &str) -> io::Result<&'a str> {
    value
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("terminal action requires `{name}`"),
            )
        })
}

fn sessions_root_dir() -> PathBuf {
    botty_root_dir().join("app").join("terminal").join("sessions")
}

fn session_dir(session_id: &str) -> PathBuf {
    sessions_root_dir().join(session_id)
}

fn session_meta_path(session_id: &str) -> PathBuf {
    session_dir(session_id).join("session.json")
}

fn write_session_meta(session: &AgentSession, state: &str) -> io::Result<()> {
    let path = session_meta_path(&session.id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let meta = SessionMeta {
        session_id: session.id.clone(),
        provider: session.provider.clone(),
        state: state.to_string(),
        started_at_ms: session.started_at_ms,
        updated_at_ms: now_ms(),
        work_dir: session.work_dir.display().to_string(),
        transcript_path: session.terminal.transcript_path().display().to_string(),
    };
    let content = serde_json::to_string_pretty(&meta)
        .map_err(|err| io::Error::other(format!("serialize session meta failed: {err}")))?;
    fs::write(path, content)
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

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
