use crate::botty_jobs::{
    self, enqueue_job, idle_timeout, job_age, list_roles, load_job, new_delegated_job,
    new_external_job, read_worker_pid, role_snapshot, update_job_state, wait_job_terminal,
    write_worker_state, JobState, QueueJob,
};
use serde_json::{self, Value};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::env;
use std::ffi::CString;
use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::io::BufRead;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::Write;
use std::os::unix::net::UnixListener;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

const DOWNLOAD_URL: &str = env!("BOTTY_DOWNLOAD_URL");
const LATEST_RELEASE_API_URL: &str = env!("BOTTY_LATEST_RELEASE_API_URL");
const INSTALL_SCRIPT_URL: &str = env!("BOTTY_INSTALL_SCRIPT_URL");
const CURL_MAX_TIME_SECONDS: &str = "60";
const GUY_DEFAULT_ROLE: &str = "leader";
const CHAT_META_PREFIX: &str = "__botty_meta__";
const CONTROL_PREFIX: &str = "__botty_control__";

pub fn start_daemon() -> io::Result<()> {
    if is_boss_running()? {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Botty-Boss is already running",
        ));
    }

    let exe = env::current_exe()?;
    let log_dir = botty_root_dir().join("log");
    fs::create_dir_all(&log_dir)?;
    let log_path = log_dir.join(format!("boss{}.log", runtime_suffix()));

    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    let err_file = log_file.try_clone()?;

    let mut cmd = Command::new(&exe);
    cmd.arg0(boss_process_name())
        .arg("--boss-daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(err_file));

    unsafe {
        cmd.pre_exec(|| {
            // Detach from current session so this process runs as a daemon.
            let rc = libc::setsid();
            if rc == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }

    cmd.spawn()?;

    Ok(())
}

fn botty_root_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".mylittlebotty")
}

fn boss_pid_file() -> PathBuf {
    botty_root_dir()
        .join("run")
        .join(format!("boss{}.pid", runtime_suffix()))
}

pub fn chat_socket_path() -> PathBuf {
    botty_root_dir()
        .join("run")
        .join(format!("chat{}.sock", runtime_suffix()))
}

fn interrupt_flag_file() -> PathBuf {
    botty_root_dir()
        .join("run")
        .join(format!("interrupt-current{}.flag", runtime_suffix()))
}

pub fn ensure_chat_ready() -> io::Result<()> {
    if !is_boss_running()? {
        start_daemon()?;
    }
    wait_for_chat_socket(Duration::from_secs(5))
}

pub fn load_guy_env_map() -> io::Result<Vec<(String, String)>> {
    read_guy_env_entries(&guy_env_config_file())
}

pub fn save_guy_env_map(entries: &[(String, String)]) -> io::Result<()> {
    let path = guy_env_config_file();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut sorted = entries.to_vec();
    sorted.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));

    let mut content = String::new();
    for (key, value) in sorted {
        content.push_str(&key);
        content.push('=');
        content.push_str(&value);
        content.push('\n');
    }
    fs::write(path, content)
}

pub fn is_boss_running() -> io::Result<bool> {
    let pid_file = boss_pid_file();
    let Some(pid) = read_pid_file(&pid_file)? else {
        return Ok(false);
    };

    if is_process_alive(pid) {
        return Ok(true);
    }

    let _ = fs::remove_file(pid_file);
    Ok(false)
}

pub fn stop_all() -> io::Result<()> {
    println!("{}", stop_all_report()?);
    Ok(())
}

pub fn restart_all() -> io::Result<()> {
    for line in restart_all_report()? {
        println!("{line}");
    }
    Ok(())
}

pub fn stop_all_report() -> io::Result<String> {
    let mut targets = Vec::new();
    let pid_path = boss_pid_file();
    if let Some(pid) = read_pid_file(&pid_path)? {
        if is_process_alive(pid) {
            targets.extend(find_descendant_pids(pid)?);
        }
        targets.push(pid);
    }

    targets.extend(find_pids_by_process_name(boss_process_name())?);
    targets.extend(find_pids_by_process_name(guy_process_name())?);
    targets.extend(find_pids_by_process_name(crond_process_name())?);
    for spec in input_process_specs() {
        let name = spec.process_name();
        targets.extend(find_pids_by_process_name(&name)?);
    }
    targets.sort_unstable();
    targets.dedup();

    if targets.is_empty() {
        let _ = fs::remove_file(pid_path);
        return Ok("No Botty processes running".to_string());
    }

    for &pid in &targets {
        let _ = send_signal(pid, libc::SIGTERM);
    }
    std::thread::sleep(std::time::Duration::from_millis(800));

    let mut forced = 0usize;
    for &pid in &targets {
        if is_process_alive(pid) {
            let _ = send_signal(pid, libc::SIGKILL);
            forced += 1;
        }
    }

    let _ = fs::remove_file(pid_path);
    let _ = fs::remove_file(chat_socket_path());
    let _ = fs::remove_file(guy_role_config_file());
    let _ = fs::remove_file(crond_pid_file());
    let _ = fs::remove_dir_all(botty_jobs::jobs_root(&botty_root_dir()));
    if forced == 0 {
        Ok("Stopped Botty-Boss, Botty-Guy, and Botty-crond".to_string())
    } else {
        Ok(format!(
            "Stopped Botty-Boss, Botty-Guy, and Botty-crond (force killed {forced})"
        ))
    }
}

pub fn restart_all_report() -> io::Result<Vec<String>> {
    let mut lines = vec![stop_all_report()?];
    start_daemon()?;
    wait_for_chat_socket(Duration::from_secs(5))?;
    lines.push("Botty-Boss restarted".to_string());
    Ok(lines)
}

pub fn print_status() -> io::Result<()> {
    let snapshot = collect_status_snapshot()?;
    println!("Boss running: {}", snapshot.boss_running());
    println!("Boss pids: {}", format_pid_list(&snapshot.boss_pids));
    println!("Guy process count: {}", snapshot.guy_pids.len());
    println!("Guy pids: {}", format_pid_list(&snapshot.guy_pids));
    println!("Crond process count: {}", snapshot.crond_pids.len());
    println!("Crond pids: {}", format_pid_list(&snapshot.crond_pids));
    match load_setup_config() {
        Ok(config) => {
            for spec in input_process_specs() {
                let state = (spec.state)(&config);
                let process_name = spec.process_name();
                let pids = snapshot
                    .input_processes
                    .iter()
                    .find(|entry| entry.process_name == process_name)
                    .map(|entry| entry.pids.as_slice())
                    .unwrap_or(&[]);
                println!(
                    "{} configured: {} ({})",
                    process_name,
                    if state.enabled { "ready" } else { "disabled" },
                    state.reason
                );
                println!("{} pids: {}", process_name, format_pid_list(pids));
            }
        }
        Err(err) => println!("Input process config unavailable: {err}"),
    }
    Ok(())
}

pub fn print_watchjobs() -> io::Result<()> {
    print!("{}", render_watchjobs()?);
    Ok(())
}

pub fn render_watchjobs() -> io::Result<String> {
    let root = botty_root_dir();
    let mut roles = list_roles(&root)?;
    if !roles.iter().any(|role| role == GUY_DEFAULT_ROLE) {
        roles.insert(0, GUY_DEFAULT_ROLE.to_string());
    }
    let mut output = String::new();
    if roles.is_empty() {
        output.push_str("No job queues found\n");
        return Ok(output);
    }

    let now = botty_jobs::now_ms();
    for role in roles {
        let worker_pid = read_worker_pid(&root, &role)?;
        let snapshot = role_snapshot(&root, &role, worker_pid)?;
        output.push_str(&format!(
            "[role={}] queued={} running={} waiting={} done={} failed={} worker_pid={}",
            snapshot.role,
            snapshot.counts.queued,
            snapshot.counts.running,
            snapshot.counts.waiting,
            snapshot.counts.done,
            snapshot.counts.failed,
            snapshot
                .worker_pid
                .map(|pid| pid.to_string())
                .unwrap_or_else(|| "-".to_string())
        ));
        output.push('\n');
        if let Some(job) = snapshot.current_job {
            output.push_str(&format!(
                "  current message_id={} trace_id={} age={} from={} kind={}",
                job.message_id,
                job.trace_id,
                botty_jobs::format_duration(job_age(now, &job)),
                job.from_role,
                job.kind
            ));
            output.push('\n');
        }
        for job in snapshot.queued_jobs.iter().take(5) {
            output.push_str(&format!(
                "  queued  message_id={} trace_id={} wait={} from={} kind={}",
                job.message_id,
                job.trace_id,
                botty_jobs::format_duration(job_age(now, job)),
                job.from_role,
                job.kind
            ));
            output.push('\n');
        }
        if let Some(error) = snapshot.recent_error {
            output.push_str(&format!("  recent_error {error}\n"));
        }
    }
    Ok(output)
}

pub fn render_watchapp(name: &str) -> io::Result<String> {
    match name {
        "terminal" => render_watch_terminal_app(),
        other => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsupported app name: {other}. currently only `terminal` is supported"),
        )),
    }
}

pub fn interrupt_active_request() -> io::Result<()> {
    let flag = interrupt_flag_file();
    if let Some(parent) = flag.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&flag, b"1")?;

    if let Some(pid) = active_guy_pid()? {
        send_signal(pid, libc::SIGINT)?;
    }
    Ok(())
}

pub fn update_self() -> io::Result<()> {
    let current_version = env!("CARGO_PKG_VERSION");
    let latest_tag = fetch_latest_release_tag()?;
    let latest_version = normalize_release_version(&latest_tag);

    if compare_versions(&latest_version, current_version) != Ordering::Greater {
        println!("Already up-to-date: {current_version}");
        return Ok(());
    }

    println!("New version available: {current_version} -> {latest_version}");
    if !confirm("Continue to upgrade? [y/N]: ")? {
        println!("Update cancelled");
        return Ok(());
    }

    let snapshot = collect_status_snapshot()?;
    let should_restart_after_update = snapshot.boss_running() || !snapshot.guy_pids.is_empty();
    if should_restart_after_update {
        println!("Detected running processes:");
        println!("Boss pids: {}", format_pid_list(&snapshot.boss_pids));
        println!("Guy pids: {}", format_pid_list(&snapshot.guy_pids));
        if !confirm("Stop them before upgrade? [y/N]: ")? {
            println!("Update cancelled");
            return Ok(());
        }
        stop_all()?;
    }

    download_and_replace_binary()?;
    println!("Updated mylittlebotty to {latest_version}");
    if should_restart_after_update {
        start_daemon()?;
        wait_for_chat_socket(Duration::from_secs(5))?;
        println!("Botty-Boss restarted");
    }
    Ok(())
}

struct BossPidGuard {
    path: PathBuf,
}

impl Drop for BossPidGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn acquire_boss_pid_guard() -> io::Result<Option<BossPidGuard>> {
    let pid_path = boss_pid_file();
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

    Ok(Some(BossPidGuard { path: pid_path }))
}

fn read_pid_file(path: &PathBuf) -> io::Result<Option<i32>> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };

    let pid = content.trim().parse::<i32>().ok();
    Ok(pid)
}

fn active_guy_pid() -> io::Result<Option<i32>> {
    let entries = read_guy_role_entries(&guy_role_config_file())?;
    for (pid, role) in entries {
        if role == GUY_DEFAULT_ROLE && is_process_alive(pid) {
            return Ok(Some(pid));
        }
    }
    Ok(None)
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

fn send_signal(pid: i32, signal: i32) -> io::Result<()> {
    if pid <= 0 {
        return Ok(());
    }

    let rc = unsafe { libc::kill(pid, signal) };
    if rc == 0 {
        return Ok(());
    }

    let err = io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::ESRCH) => Ok(()),
        _ => Err(err),
    }
}

fn find_pids_by_process_name(name: &str) -> io::Result<Vec<i32>> {
    let escaped = regex_escape(name);
    let pattern = format!("^{escaped}([[:space:]]|$)");
    let output = Command::new("pgrep").arg("-f").arg(pattern).output()?;
    if !output.status.success() {
        return Ok(Vec::new());
    }

    let mut pids = Vec::new();
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Ok(pid) = line.trim().parse::<i32>() {
            pids.push(pid);
        }
    }
    Ok(pids)
}

fn regex_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '.' | '^' | '$' | '|' | '(' | ')' | '[' | ']' | '{' | '}' | '*' | '+' | '?' | '\\' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }
    escaped
}

struct StatusSnapshot {
    boss_pids: Vec<i32>,
    guy_pids: Vec<i32>,
    crond_pids: Vec<i32>,
    input_processes: Vec<NamedPidList>,
}

impl StatusSnapshot {
    fn boss_running(&self) -> bool {
        !self.boss_pids.is_empty()
    }
}

struct NamedPidList {
    process_name: String,
    pids: Vec<i32>,
}

fn collect_status_snapshot() -> io::Result<StatusSnapshot> {
    let mut boss_pids = Vec::new();
    let mut guy_pids = Vec::new();
    let mut crond_pids = find_pids_by_process_name(crond_process_name())?;
    let mut input_processes = input_process_specs()
        .into_iter()
        .map(|spec| NamedPidList {
            process_name: spec.process_name(),
            pids: Vec::new(),
        })
        .collect::<Vec<_>>();

    if let Some(boss_pid) = read_pid_file(&boss_pid_file())? {
        if is_process_alive(boss_pid) {
            boss_pids.push(boss_pid);
            let descendants = find_descendant_pids(boss_pid)?;
            let mut candidates = find_pids_by_process_name(guy_process_name())?;
            candidates.retain(|pid| descendants.contains(pid) && is_process_alive(*pid));
            guy_pids = candidates;
            crond_pids.retain(|pid| descendants.contains(pid) && is_process_alive(*pid));
            for entry in &mut input_processes {
                let mut candidates = find_pids_by_process_name(&entry.process_name)?;
                candidates.retain(|pid| descendants.contains(pid) && is_process_alive(*pid));
                candidates.sort_unstable();
                candidates.dedup();
                entry.pids = candidates;
            }
        } else {
            let _ = fs::remove_file(boss_pid_file());
        }
    }

    boss_pids.sort_unstable();
    boss_pids.dedup();
    guy_pids.sort_unstable();
    guy_pids.dedup();
    crond_pids.sort_unstable();
    crond_pids.dedup();

    Ok(StatusSnapshot {
        boss_pids,
        guy_pids,
        crond_pids,
        input_processes,
    })
}

fn fetch_latest_release_tag() -> io::Result<String> {
    let output = Command::new("curl")
        .arg("-fsSL")
        .arg("--max-time")
        .arg(CURL_MAX_TIME_SECONDS)
        .arg("-H")
        .arg("Accept: application/vnd.github+json")
        .arg("-H")
        .arg("User-Agent: mylittlebotty-updater")
        .arg(LATEST_RELEASE_API_URL)
        .output()?;

    if !output.status.success() {
        return Err(curl_failure_error(
            "failed to request latest release",
            &output,
        ));
    }

    let body = String::from_utf8_lossy(&output.stdout);
    extract_json_string(&body, "tag_name")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "tag_name not found in response"))
}

fn download_and_replace_binary() -> io::Result<()> {
    let exe = env::current_exe()?;
    let tmp_path = exe.with_extension("download");

    let output = Command::new("curl")
        .arg("-fsSL")
        .arg("--max-time")
        .arg(CURL_MAX_TIME_SECONDS)
        .arg("--retry")
        .arg("3")
        .arg("--retry-delay")
        .arg("1")
        .arg("-o")
        .arg(&tmp_path)
        .arg(DOWNLOAD_URL)
        .output()?;

    if !output.status.success() {
        let _ = fs::remove_file(&tmp_path);
        return Err(curl_failure_error(
            "failed to download release asset",
            &output,
        ));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o755);
        fs::set_permissions(&tmp_path, perms)?;
    }

    fs::rename(&tmp_path, &exe)?;
    Ok(())
}

fn curl_failure_error(context: &str, output: &std::process::Output) -> io::Error {
    let timeout = output.status.code() == Some(28);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.trim();

    let reason = if timeout {
        format!("{context}: timeout after {CURL_MAX_TIME_SECONDS}s, unable to connect")
    } else if detail.is_empty() {
        context.to_string()
    } else {
        format!("{context}: {detail}")
    };

    io::Error::other(format!(
        "{reason}\nPlease run installer:\ncurl -LsSf {INSTALL_SCRIPT_URL} | bash && source ~/.zshrc"
    ))
}

fn extract_json_string(body: &str, key: &str) -> Option<String> {
    let quoted_key = format!("\"{key}\"");
    let key_pos = body.find(&quoted_key)?;
    let after_key = &body[key_pos + quoted_key.len()..];
    let colon_pos = after_key.find(':')?;
    let mut value = after_key[colon_pos + 1..].trim_start();
    if !value.starts_with('"') {
        return None;
    }
    value = &value[1..];
    let end = value.find('"')?;
    Some(value[..end].to_string())
}

fn normalize_release_version(tag: &str) -> String {
    tag.trim_start_matches('v').to_string()
}

fn compare_versions(a: &str, b: &str) -> Ordering {
    let pa = parse_version_parts(a);
    let pb = parse_version_parts(b);
    let max_len = pa.len().max(pb.len());

    for i in 0..max_len {
        let va = *pa.get(i).unwrap_or(&0);
        let vb = *pb.get(i).unwrap_or(&0);
        match va.cmp(&vb) {
            Ordering::Equal => {}
            non_eq => return non_eq,
        }
    }
    Ordering::Equal
}

fn parse_version_parts(version: &str) -> Vec<u64> {
    version
        .split('.')
        .map(|part| {
            let numeric: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
            numeric.parse::<u64>().unwrap_or(0)
        })
        .collect()
}

fn confirm(prompt: &str) -> io::Result<bool> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let answer = input.trim().to_ascii_lowercase();
    Ok(matches!(answer.as_str(), "y" | "yes"))
}

fn format_pid_list(pids: &[i32]) -> String {
    if pids.is_empty() {
        return "-".to_string();
    }

    pids.iter()
        .map(|pid| pid.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn find_descendant_pids(root_pid: i32) -> io::Result<Vec<i32>> {
    let mut all = Vec::new();
    let mut queue = vec![root_pid];

    while let Some(parent) = queue.pop() {
        let children = find_child_pids(parent)?;
        for child in children {
            all.push(child);
            queue.push(child);
        }
    }

    Ok(all)
}

fn find_child_pids(parent_pid: i32) -> io::Result<Vec<i32>> {
    let output = Command::new("pgrep")
        .arg("-P")
        .arg(parent_pid.to_string())
        .output()?;
    if !output.status.success() {
        return Ok(Vec::new());
    }

    let mut pids = Vec::new();
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Ok(pid) = line.trim().parse::<i32>() {
            pids.push(pid);
        }
    }
    Ok(pids)
}

enum DispatcherCommand {
    EnsureRole(String),
}

pub fn run_supervisor() {
    let _pid_guard = match acquire_boss_pid_guard() {
        Ok(Some(guard)) => guard,
        Ok(None) => {
            boss_log_error("Botty-Boss is already running, exiting duplicate supervisor");
            return;
        }
        Err(err) => {
            boss_log_error(&format!("Botty-Boss failed to acquire pid file: {err}"));
            return;
        }
    };

    set_process_name(boss_process_name());
    boss_log_info("Botty-Boss supervisor is running");

    let _socket_guard = match bind_chat_socket() {
        Ok(guard) => guard,
        Err(err) => {
            boss_log_error(&format!("Botty-Boss failed to bind chat socket: {err}"));
            return;
        }
    };

    let root = botty_root_dir();
    let jobs_root = botty_jobs::jobs_root(&root);
    let _ = fs::create_dir_all(&jobs_root);
    let (dispatch_tx, dispatch_rx) = mpsc::channel::<DispatcherCommand>();
    let dispatch_root = root.clone();
    let dispatch_loop_tx = dispatch_tx.clone();
    let _dispatcher =
        thread::spawn(move || run_dispatcher(dispatch_root, dispatch_rx, dispatch_loop_tx));
    let _ = dispatch_tx.send(DispatcherCommand::EnsureRole(GUY_DEFAULT_ROLE.to_string()));
    let config = load_setup_config().unwrap_or_default();
    let _input_bridges = spawn_enabled_input_processes(&config);
    let _crond_bridge = spawn_crond_process();

    loop {
        match _socket_guard.listener.accept() {
            Ok((stream, _)) => {
                let dispatch_tx = dispatch_tx.clone();
                thread::spawn(move || {
                    if let Err(err) = handle_chat_client(stream, dispatch_tx) {
                        boss_log_error(&format!("Botty-Boss failed to handle chat session: {err}"));
                    }
                });
            }
            Err(err) => boss_log_error(&format!("Botty-Boss accept error: {err}")),
        }
    }
}

fn run_dispatcher(
    root: PathBuf,
    dispatch_rx: Receiver<DispatcherCommand>,
    dispatch_tx: Sender<DispatcherCommand>,
) {
    let mut active_roles: HashMap<String, thread::JoinHandle<()>> = HashMap::new();
    while let Ok(command) = dispatch_rx.recv() {
        match command {
            DispatcherCommand::EnsureRole(role) => {
                let dead = active_roles
                    .get(&role)
                    .map(thread::JoinHandle::is_finished)
                    .unwrap_or(false);
                if dead {
                    let _ = active_roles.remove(&role);
                }
                if active_roles.contains_key(&role) {
                    continue;
                }
                let role_root = root.clone();
                let role_name = role.clone();
                let role_dispatch_tx = dispatch_tx.clone();
                let handle = thread::spawn(move || {
                    run_role_processor(role_root, role_name, role_dispatch_tx)
                });
                active_roles.insert(role, handle);
            }
        }
    }
}

fn run_role_processor(root: PathBuf, role: String, dispatch_tx: Sender<DispatcherCommand>) {
    let mut bridge: Option<GuyBridge> = None;
    let mut last_active = Instant::now();

    loop {
        if role == GUY_DEFAULT_ROLE && bridge.is_none() {
            match GuyBridge::spawn(&role) {
                Ok(live) => {
                    let pid = i32::try_from(live.child.id()).ok();
                    let _ = write_worker_state(&root, &role, pid, None, None);
                    bridge = Some(live);
                }
                Err(err) => {
                    boss_log_error(&format!(
                        "Botty-Boss failed to eagerly start worker for role {role}: {err}"
                    ));
                    let _ = write_worker_state(
                        &root,
                        &role,
                        None,
                        None,
                        Some("failed to eagerly start leader worker"),
                    );
                    thread::sleep(Duration::from_millis(300));
                    continue;
                }
            }
        }

        let next_job = match botty_jobs::next_queued_job(&root, &role) {
            Ok(job) => job,
            Err(err) => {
                boss_log_error(&format!(
                    "Botty-Boss failed to read queue for role {role}: {err}"
                ));
                thread::sleep(Duration::from_millis(300));
                continue;
            }
        };

        let Some(mut job) = next_job else {
            if role != GUY_DEFAULT_ROLE && last_active.elapsed() >= idle_timeout() {
                if let Some(live) = bridge.take() {
                    let _ = write_worker_state(&root, &role, None, None, None);
                    drop(live);
                }
                return;
            }
            thread::sleep(Duration::from_millis(200));
            continue;
        };

        if bridge.is_none() {
            match GuyBridge::spawn(&role) {
                Ok(live) => {
                    let pid = i32::try_from(live.child.id()).ok();
                    let _ = write_worker_state(&root, &role, pid, None, None);
                    bridge = Some(live);
                }
                Err(err) => {
                    job.last_error = Some(format!("spawn worker failed: {err}"));
                    job.completed_at_ms = Some(botty_jobs::now_ms());
                    let _ = update_job_state(
                        &root,
                        &role,
                        JobState::Queued,
                        JobState::Failed,
                        &mut job,
                    );
                    let _ = write_worker_state(&root, &role, None, None, job.last_error.as_deref());
                    continue;
                }
            }
        }

        job.started_at_ms = Some(botty_jobs::now_ms());
        let pid = bridge
            .as_ref()
            .and_then(|live| i32::try_from(live.child.id()).ok());
        job.worker_pid = pid;
        let _ = update_job_state(&root, &role, JobState::Queued, JobState::Running, &mut job);
        let _ = write_worker_state(&root, &role, pid, Some(&job.message_id), None);

        if let Some(live) = bridge.as_mut() {
            let _ = live.set_env("BOTTY_CURRENT_JOB_ID", &job.message_id);
            let _ = live.set_env("BOTTY_CURRENT_JOB_SOURCE", &job.source);
            let _ = live.set_env("BOTTY_CURRENT_JOB_USER_ID", &job.user_id);
            let _ = live.set_env(
                "BOTTY_CURRENT_JOB_TARGET",
                job.target.as_deref().unwrap_or_default(),
            );
        }

        let request_message = if let Some(control) = build_resume_control_message(&job) {
            control
        } else {
            job.payload.clone()
        };
        let _ = append_guy_role_log(
            &role,
            "request",
            &format!(
                "job_id={} payload={}",
                job.message_id,
                sanitize_log_value(&request_message)
            ),
        );
        let response = bridge
            .as_mut()
            .expect("worker bridge must exist")
            .ask(&request_message);
        match response {
            Ok(reply) => {
                let still_running = match is_job_still_running(&root, &role, &job.message_id) {
                    Ok(value) => value,
                    Err(err) => {
                        boss_log_error(&format!(
                            "Botty-Boss failed to verify running state for role {role} job {}: {err}",
                            job.message_id
                        ));
                        let _ = write_worker_state(
                            &root,
                            &role,
                            None,
                            None,
                            Some("failed to verify running state"),
                        );
                        bridge = None;
                        last_active = Instant::now();
                        continue;
                    }
                };
                if !still_running {
                    let _ = append_guy_role_log(
                        &role,
                        "stale_response",
                        &format!(
                            "job_id={} ignored because job is no longer running",
                            job.message_id
                        ),
                    );
                    let _ = write_worker_state(&root, &role, pid, None, None);
                    last_active = Instant::now();
                    continue;
                }
                let _ = append_guy_role_log(
                    &role,
                    "response",
                    &format!(
                        "job_id={} text={} control={}",
                        job.message_id,
                        sanitize_log_value(&reply.text),
                        reply
                            .control
                            .as_ref()
                            .map(|control| sanitize_log_value(&format!("{control:?}")))
                            .unwrap_or_else(|| "-".to_string())
                    ),
                );
                if let Some(crate::botty_body::AssistantControl::AwaitDelegation {
                    child_message_id,
                    continuation_payload,
                    handoff_message,
                    ..
                }) = reply.control
                {
                    job.continuation_payload = Some(continuation_payload);
                    job.awaiting_message_id = Some(child_message_id);
                    job.continuation_result = None;
                    job.pending_user_notice =
                        handoff_message.filter(|text| !text.trim().is_empty());
                    job.result_text = None;
                    job.completed_at_ms = None;
                    let _ = update_job_state(
                        &root,
                        &role,
                        JobState::Running,
                        JobState::Waiting,
                        &mut job,
                    );
                    let _ = write_worker_state(&root, &role, pid, None, None);
                } else {
                    job.result_text = Some(reply.text.clone());
                    job.completed_at_ms = Some(botty_jobs::now_ms());
                    let _ =
                        update_job_state(&root, &role, JobState::Running, JobState::Done, &mut job);
                    let _ = write_worker_state(&root, &role, pid, None, None);
                    let _ = try_resume_parent_job(&root, &job, &dispatch_tx);
                }
            }
            Err(err) => {
                let still_running = match is_job_still_running(&root, &role, &job.message_id) {
                    Ok(value) => value,
                    Err(state_err) => {
                        boss_log_error(&format!(
                            "Botty-Boss failed to verify running state for role {role} job {}: {state_err}",
                            job.message_id
                        ));
                        let _ = write_worker_state(
                            &root,
                            &role,
                            None,
                            None,
                            Some("failed to verify running state"),
                        );
                        bridge = None;
                        last_active = Instant::now();
                        continue;
                    }
                };
                if !still_running {
                    let _ = append_guy_role_log(
                        &role,
                        "stale_error",
                        &format!(
                            "job_id={} ignored because job is no longer running: {}",
                            job.message_id,
                            sanitize_log_value(&err.to_string())
                        ),
                    );
                    let _ = write_worker_state(&root, &role, None, None, None);
                    bridge = None;
                    last_active = Instant::now();
                    continue;
                }
                let _ = append_guy_role_log(
                    &role,
                    "error",
                    &format!(
                        "job_id={} {}",
                        job.message_id,
                        sanitize_log_value(&err.to_string())
                    ),
                );
                job.last_error = Some(err.to_string());
                job.completed_at_ms = Some(botty_jobs::now_ms());
                let _ =
                    update_job_state(&root, &role, JobState::Running, JobState::Failed, &mut job);
                let _ = write_worker_state(&root, &role, None, None, job.last_error.as_deref());
                let _ = try_resume_parent_job(&root, &job, &dispatch_tx);
                bridge = None;
            }
        }
        last_active = Instant::now();
    }
}

fn build_resume_control_message(job: &crate::botty_jobs::QueueJob) -> Option<String> {
    let continuation_payload = job.continuation_payload.as_ref()?;
    let tool_result = job.continuation_result.as_ref()?;
    if job.awaiting_message_id.is_some() {
        return None;
    }
    Some(format!(
        "{CONTROL_PREFIX}resume|{}",
        serde_json::json!({
            "continuation_payload": continuation_payload,
            "tool_result": tool_result,
        })
    ))
}

fn try_resume_parent_job(
    root: &PathBuf,
    child_job: &crate::botty_jobs::QueueJob,
    dispatch_tx: &Sender<DispatcherCommand>,
) -> io::Result<()> {
    let Some(parent_message_id) = child_job.parent_message_id.as_deref() else {
        return Ok(());
    };

    let mut parent = match load_job(
        root,
        &child_job.from_role,
        JobState::Waiting,
        parent_message_id,
    ) {
        Ok(job) => job,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };

    if parent.awaiting_message_id.as_deref() != Some(child_job.message_id.as_str()) {
        return Ok(());
    }

    let tool_result = match child_job.state {
        JobState::Done => child_job.result_text.clone().unwrap_or_default(),
        JobState::Failed => child_job
            .last_error
            .clone()
            .unwrap_or_else(|| format!("delegated job failed for role {}", child_job.to_role)),
        _ => return Ok(()),
    };

    parent.awaiting_message_id = None;
    parent.continuation_result = Some(tool_result);
    parent.pending_user_notice = None;
    parent.completed_at_ms = None;
    update_job_state(
        root,
        &child_job.from_role,
        JobState::Waiting,
        JobState::Queued,
        &mut parent,
    )?;
    dispatch_tx
        .send(DispatcherCommand::EnsureRole(child_job.from_role.clone()))
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "dispatcher is not available"))?;
    Ok(())
}

fn is_job_still_running(root: &PathBuf, role: &str, message_id: &str) -> io::Result<bool> {
    match load_job(root, role, JobState::Running, message_id) {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

struct ChatSocketGuard {
    path: PathBuf,
    listener: UnixListener,
}

impl Drop for ChatSocketGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn bind_chat_socket() -> io::Result<ChatSocketGuard> {
    let path = chat_socket_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.exists() {
        let _ = fs::remove_file(&path);
    }
    let listener = UnixListener::bind(&path)?;
    Ok(ChatSocketGuard { path, listener })
}

struct GuyBridge {
    child: std::process::Child,
    stdin: BufWriter<std::process::ChildStdin>,
    stdout: BufReader<std::process::ChildStdout>,
}

impl GuyBridge {
    fn spawn(role: &str) -> io::Result<Self> {
        let exe = env::current_exe()?;
        let mut cmd = Command::new(exe);
        cmd.arg0(guy_process_name())
            .arg("--guy")
            .env("BOTTY_GUY_ROLE", role)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        for (key, value) in load_guy_env_map()? {
            cmd.env(key, value);
        }

        let mut child = cmd.spawn()?;

        let child_pid = i32::try_from(child.id())
            .map_err(|_| io::Error::other("failed to convert guy pid to i32"))?;
        persist_guy_role(child_pid, role)?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("failed to capture guy stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("failed to capture guy stdout"))?;

        Ok(Self {
            child,
            stdin: BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
        })
    }

    fn ask(&mut self, message: &str) -> io::Result<AssistantReply> {
        if self.child.try_wait()?.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "Botty-Guy has exited",
            ));
        }

        writeln!(self.stdin, "{}", encode_ipc_line(message)?)?;
        self.stdin.flush()?;

        let mut response = String::new();
        let bytes = self.stdout.read_line(&mut response)?;
        if bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "Botty-Guy connection closed",
            ));
        }

        let decoded = decode_ipc_line(response.trim_end())?;
        decode_assistant_reply(&decoded)
    }

    fn set_env(&mut self, key: &str, value: &str) -> io::Result<()> {
        let control = format!("{CONTROL_PREFIX}set-env|{key}|{value}");
        self.ask(&control).map(|_| ())
    }
}

struct InputProcessBridge {
    child: std::process::Child,
}

impl Drop for InputProcessBridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn_crond_process() -> Option<InputProcessBridge> {
    let exe = match env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            boss_log_error(&format!(
                "Botty-Boss failed to get current executable path for Botty-crond: {err}"
            ));
            return None;
        }
    };

    let process_name = if cfg!(debug_assertions) {
        "Botty-crond-dev"
    } else {
        "Botty-crond"
    };

    match Command::new(&exe)
        .arg0(process_name)
        .arg("--crond")
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(child) => Some(InputProcessBridge { child }),
        Err(err) => {
            boss_log_error(&format!("Botty-Boss failed to run {process_name}: {err}"));
            None
        }
    }
}

struct InputProcessSpec {
    name: &'static str,
    arg: &'static str,
    state: fn(&SetupConfig) -> InputProcessState,
}

struct InputProcessState {
    enabled: bool,
    reason: String,
}

impl InputProcessSpec {
    fn process_name(&self) -> String {
        format!("{}{}", self.name, runtime_suffix())
    }
}

fn enabled_input_process(reason: impl Into<String>) -> InputProcessState {
    InputProcessState {
        enabled: true,
        reason: reason.into(),
    }
}

fn disabled_input_process(reason: impl Into<String>) -> InputProcessState {
    InputProcessState {
        enabled: false,
        reason: reason.into(),
    }
}

fn telegram_input_process_state(config: &SetupConfig) -> InputProcessState {
    if !config.telegram_enabled {
        return disabled_input_process("chatbot.telegram.enabled=false");
    }
    if config.telegram_apikey.trim().is_empty() {
        return disabled_input_process("chatbot.telegram.apikey is empty");
    }
    enabled_input_process("configured")
}

fn feishu_input_process_state(config: &SetupConfig) -> InputProcessState {
    if !config.feishu_enabled {
        return disabled_input_process("chatbot.feishu.enabled=false");
    }
    if config.feishu_app_id.trim().is_empty() || config.feishu_app_secret.trim().is_empty() {
        return disabled_input_process(
            "chatbot.feishu.app_id/app_secret is incomplete for long connection",
        );
    }
    enabled_input_process("configured")
}

fn input_process_specs() -> [InputProcessSpec; 2] {
    [
        InputProcessSpec {
            name: "Botty-input-telegram",
            arg: "--input-telegram",
            state: telegram_input_process_state,
        },
        InputProcessSpec {
            name: "Botty-input-feishu",
            arg: "--input-feishu",
            state: feishu_input_process_state,
        },
    ]
}

fn spawn_enabled_input_processes(config: &SetupConfig) -> Vec<InputProcessBridge> {
    let mut bridges = Vec::new();
    let exe = match env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            boss_log_error(&format!(
                "Botty-Boss failed to get current executable path: {err}"
            ));
            return bridges;
        }
    };

    for spec in input_process_specs() {
        let state = (spec.state)(config);
        if !state.enabled {
            boss_log_info(&format!(
                "Botty-Boss skipped {}: {}",
                spec.process_name(),
                state.reason
            ));
            continue;
        }

        let process_name = spec.process_name();
        let child = Command::new(&exe)
            .arg0(&process_name)
            .arg(spec.arg)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn();

        match child {
            Ok(child) => bridges.push(InputProcessBridge { child }),
            Err(err) => boss_log_error(&format!("Botty-Boss failed to run {process_name}: {err}")),
        }
    }

    bridges
}

fn guy_role_config_file() -> PathBuf {
    botty_root_dir()
        .join("config")
        .join(format!("guy-role-map{}.conf", runtime_suffix()))
}

fn guy_env_config_file() -> PathBuf {
    botty_root_dir()
        .join("config")
        .join(format!("guy-env{}.conf", runtime_suffix()))
}

fn guy_role_log_file(role: &str) -> PathBuf {
    botty_root_dir().join("log").join(format!(
        "guy-role-{}{}.log",
        sanitize_role_for_filename(role),
        runtime_suffix()
    ))
}

fn render_watch_terminal_app() -> io::Result<String> {
    let root = botty_root_dir()
        .join("app")
        .join("terminal")
        .join("sessions");
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(format!(
                "Watching app `terminal`\nroot={}\n\nNo terminal session transcripts yet.\n",
                root.display()
            ));
        }
        Err(err) => return Err(err),
    };

    let mut latest_running: Option<(u64, PathBuf)> = None;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let meta_path = entry.path().join("session.json");
        let meta_content = match fs::read_to_string(&meta_path) {
            Ok(content) => content,
            Err(_) => continue,
        };
        let meta: serde_json::Value = match serde_json::from_str(&meta_content) {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        if meta.get("state").and_then(serde_json::Value::as_str) != Some("running") {
            continue;
        }
        let updated_at_ms = meta
            .get("updated_at_ms")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let transcript_path = match meta
            .get("transcript_path")
            .and_then(serde_json::Value::as_str)
            .map(PathBuf::from)
        {
            Some(path) => path,
            None => continue,
        };
        match &latest_running {
            Some((saved_updated_at_ms, _)) if updated_at_ms <= *saved_updated_at_ms => {}
            _ => latest_running = Some((updated_at_ms, transcript_path)),
        }
    }

    let Some((_, path)) = latest_running else {
        return Ok(format!(
            "Watching app `terminal`\nroot={}\n\nNo terminal session transcripts yet.\n",
            root.display()
        ));
    };

    let body = match fs::read_to_string(&path) {
        Ok(content) => tail_text(&content, 16 * 1024),
        Err(err) => return Err(err),
    };

    Ok(format!(
        "Watching app `terminal`\nroot={}\ntranscript_path={}\n\n{}",
        root.display(),
        path.display(),
        body
    ))
}

fn append_guy_role_log(role: &str, direction: &str, message: &str) -> io::Result<()> {
    let path = guy_role_log_file(role);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let timestamp = local_time_format("%Y-%m-%d %H:%M:%S")?;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "[{timestamp}] {direction}: {message}")?;
    Ok(())
}

fn sanitize_log_value(value: &str) -> String {
    value.replace('\n', "\\n").replace('\r', "\\r")
}

fn sanitize_role_for_filename(role: &str) -> String {
    role.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn tail_text(content: &str, max_bytes: usize) -> String {
    if content.len() <= max_bytes {
        return content.to_string();
    }
    let mut start = content.len() - max_bytes;
    while !content.is_char_boundary(start) && start < content.len() {
        start += 1;
    }
    content[start..].to_string()
}

fn persist_guy_role(pid: i32, role: &str) -> io::Result<()> {
    let path = guy_role_config_file();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut entries = read_guy_role_entries(&path)?;
    entries.retain(|(saved_pid, _)| *saved_pid != pid && is_process_alive(*saved_pid));
    entries.push((pid, role.to_string()));
    entries.sort_unstable_by_key(|(saved_pid, _)| *saved_pid);

    let mut content = String::new();
    for (saved_pid, saved_role) in entries {
        content.push_str(&format!("{saved_pid}={saved_role}\n"));
    }
    fs::write(path, content)?;
    Ok(())
}

fn read_guy_role_entries(path: &PathBuf) -> io::Result<Vec<(i32, String)>> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };

    let mut entries = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let Some((pid_part, role_part)) = trimmed.split_once('=') else {
            continue;
        };

        if let Ok(saved_pid) = pid_part.trim().parse::<i32>() {
            let saved_role = role_part.trim();
            if !saved_role.is_empty() {
                entries.push((saved_pid, saved_role.to_string()));
            }
        }
    }

    Ok(entries)
}

fn read_guy_env_entries(path: &PathBuf) -> io::Result<Vec<(String, String)>> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };

    let mut entries = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let Some((key_part, value_part)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key_part.trim();
        if key.is_empty() {
            continue;
        }
        entries.push((key.to_string(), value_part.to_string()));
    }

    Ok(entries)
}

impl Drop for GuyBridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn handle_chat_client(
    stream: UnixStream,
    dispatch_tx: Sender<DispatcherCommand>,
) -> io::Result<()> {
    let read_stream = stream.try_clone()?;
    let mut reader = BufReader::new(read_stream);
    let mut writer = BufWriter::new(stream);
    let mut input = String::new();

    loop {
        input.clear();
        let bytes = reader.read_line(&mut input)?;
        if bytes == 0 {
            return Ok(());
        }

        let raw = input.trim_end();
        if raw.is_empty() {
            continue;
        }
        let decoded = decode_ipc_line(raw)?;
        let incoming = parse_chat_meta_message(&decoded);
        if incoming.message.is_empty() {
            continue;
        }

        let response = if let Some((key, value)) = parse_set_env_control(&incoming.message) {
            apply_set_env_control(&dispatch_tx, &key, &value)
        } else if let Some(control) = parse_delegate_control(&incoming.message) {
            handle_delegated_request(control, &dispatch_tx)
        } else if let Some(control) = parse_interrupt_control(&incoming.message) {
            handle_interrupt_request(control, &dispatch_tx)
        } else if let Some(control_incoming) =
            parse_enqueue_external_control(&incoming.message, &incoming)
        {
            let _ = persist_chat_message(
                "user",
                &control_incoming.source,
                &control_incoming.user_id,
                &control_incoming.message,
            );
            enqueue_external_request(control_incoming, &dispatch_tx)
        } else {
            let _ = persist_chat_message(
                "user",
                &incoming.source,
                &incoming.user_id,
                &incoming.message,
            );
            handle_external_request(incoming, &dispatch_tx)
        }?;

        writeln!(writer, "{}", encode_ipc_line(&response)?)?;
        writer.flush()?;
    }
}

struct AssistantReply {
    text: String,
    control: Option<crate::botty_body::AssistantControl>,
}

const CHAT_MEMORY_MAX_BYTES: u64 = 200 * 1024;

struct IncomingChatMessage {
    source: String,
    user_id: String,
    target: Option<String>,
    message: String,
}

struct DelegatedControlRequest {
    parent_message_id: String,
    role: String,
    payload: String,
    source: String,
    user_id: String,
}

struct InterruptControlRequest {
    message_id: Option<String>,
    role: Option<String>,
    source: String,
    user_id: String,
}

struct JobLookup {
    role: String,
    state: JobState,
    job: QueueJob,
}

fn apply_set_env_control(
    dispatch_tx: &Sender<DispatcherCommand>,
    key: &str,
    value: &str,
) -> io::Result<String> {
    let mut entries = load_guy_env_map()?;
    let mut updated = false;
    for (saved_key, saved_value) in &mut entries {
        if saved_key == key {
            *saved_value = value.to_string();
            updated = true;
            break;
        }
    }
    if !updated {
        entries.push((key.to_string(), value.to_string()));
    }
    save_guy_env_map(&entries)?;
    let _ = dispatch_tx.send(DispatcherCommand::EnsureRole(GUY_DEFAULT_ROLE.to_string()));
    Ok("ok".to_string())
}

fn handle_external_request(
    incoming: IncomingChatMessage,
    dispatch_tx: &Sender<DispatcherCommand>,
) -> io::Result<String> {
    let root = botty_root_dir();
    let leader_message = leader_message_for_source(&incoming);
    let job = new_external_job(
        &incoming.source,
        &incoming.user_id,
        incoming.target.clone(),
        &leader_message,
    );
    enqueue_job(&root, &job)?;
    dispatch_tx
        .send(DispatcherCommand::EnsureRole(GUY_DEFAULT_ROLE.to_string()))
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "dispatcher is not available"))?;

    let done_job = wait_job_terminal(&root, GUY_DEFAULT_ROLE, &job.message_id)?;
    match done_job.state {
        JobState::Done => {
            let reply = done_job.result_text.unwrap_or_default();
            let _ = persist_chat_message("assistant", &incoming.source, &incoming.user_id, &reply);
            Ok(reply)
        }
        JobState::Failed => Err(io::Error::other(
            done_job
                .last_error
                .unwrap_or_else(|| "leader job failed".to_string()),
        )),
        _ => Err(io::Error::other(
            "leader job did not reach a terminal state",
        )),
    }
}

fn enqueue_external_request(
    incoming: IncomingChatMessage,
    dispatch_tx: &Sender<DispatcherCommand>,
) -> io::Result<String> {
    let root = botty_root_dir();
    let leader_message = leader_message_for_source(&incoming);
    let job = new_external_job(
        &incoming.source,
        &incoming.user_id,
        incoming.target,
        &leader_message,
    );
    let message_id = job.message_id.clone();
    enqueue_job(&root, &job)?;
    dispatch_tx
        .send(DispatcherCommand::EnsureRole(GUY_DEFAULT_ROLE.to_string()))
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "dispatcher is not available"))?;
    Ok(message_id)
}

fn handle_delegated_request(
    control: DelegatedControlRequest,
    dispatch_tx: &Sender<DispatcherCommand>,
) -> io::Result<String> {
    let root = botty_root_dir();
    let parent = load_job(
        &root,
        GUY_DEFAULT_ROLE,
        JobState::Running,
        &control.parent_message_id,
    )?;
    let job = new_delegated_job(
        &parent,
        &control.role,
        &control.payload,
        &control.source,
        &control.user_id,
    )?;
    enqueue_job(&root, &job)?;
    dispatch_tx
        .send(DispatcherCommand::EnsureRole(control.role.clone()))
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "dispatcher is not available"))?;
    Ok(job.message_id.clone())
}

fn handle_interrupt_request(
    control: InterruptControlRequest,
    dispatch_tx: &Sender<DispatcherCommand>,
) -> io::Result<String> {
    let root = botty_root_dir();
    let target = resolve_interrupt_target(&root, &control)?;
    interrupt_job(&root, target, dispatch_tx)
}

fn leader_message_for_source(incoming: &IncomingChatMessage) -> String {
    let prefix = format!("{}: ", incoming.source);
    if incoming.message.starts_with(&prefix) {
        incoming.message.clone()
    } else {
        format!("{prefix}{}", incoming.message)
    }
}

fn parse_set_env_control(message: &str) -> Option<(String, String)> {
    let payload = message.strip_prefix(CONTROL_PREFIX)?;
    let payload = payload.strip_prefix("set-env|")?;
    let mut parts = payload.splitn(2, '|');
    let key = parts.next()?.trim();
    let value = parts.next()?.to_string();
    if key.is_empty() {
        return None;
    }
    Some((key.to_string(), value))
}

fn parse_delegate_control(message: &str) -> Option<DelegatedControlRequest> {
    let payload = message.strip_prefix(CONTROL_PREFIX)?;
    let payload = payload.strip_prefix("delegate|")?;
    let value: Value = serde_json::from_str(payload).ok()?;
    Some(DelegatedControlRequest {
        parent_message_id: value.get("parent_message_id")?.as_str()?.to_string(),
        role: value.get("role")?.as_str()?.to_string(),
        payload: value.get("payload")?.as_str()?.to_string(),
        source: value
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or("leader")
            .to_string(),
        user_id: value
            .get("user_id")
            .and_then(Value::as_str)
            .unwrap_or("leader")
            .to_string(),
    })
}

fn parse_interrupt_control(message: &str) -> Option<InterruptControlRequest> {
    let payload = message.strip_prefix(CONTROL_PREFIX)?;
    let payload = payload.strip_prefix("interrupt|")?;
    let value: Value = serde_json::from_str(payload).ok()?;
    Some(InterruptControlRequest {
        message_id: value
            .get("message_id")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        role: value
            .get("role")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        source: value
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or("leader")
            .to_string(),
        user_id: value
            .get("user_id")
            .and_then(Value::as_str)
            .unwrap_or("leader")
            .to_string(),
    })
}

fn resolve_interrupt_target(
    root: &PathBuf,
    control: &InterruptControlRequest,
) -> io::Result<JobLookup> {
    if let Some(message_id) = control.message_id.as_deref() {
        let Some(found) = find_job_by_message_id(root, message_id)? else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("task {message_id} not found"),
            ));
        };
        if let Some(role) = control.role.as_deref() {
            if found.job.to_role != role {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "task {message_id} belongs to role {}, not {role}",
                        found.job.to_role
                    ),
                ));
            }
        }
        return Ok(found);
    }

    let mut candidates = list_active_jobs(root)?
        .into_iter()
        .filter(|item| item.job.kind == "delegated_task")
        .filter(|item| item.job.source == control.source)
        .filter(|item| item.job.user_id == control.user_id)
        .collect::<Vec<_>>();
    if let Some(role) = control.role.as_deref() {
        candidates.retain(|item| item.job.to_role == role);
    }
    candidates.sort_by(|left, right| {
        right
            .job
            .updated_at_ms
            .cmp(&left.job.updated_at_ms)
            .then_with(|| right.job.created_at_ms.cmp(&left.job.created_at_ms))
    });
    candidates.into_iter().next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "no active delegated task found to interrupt",
        )
    })
}

fn find_job_by_message_id(root: &PathBuf, message_id: &str) -> io::Result<Option<JobLookup>> {
    for role in list_roles(root)? {
        for state in all_job_states() {
            match load_job(root, &role, state.clone(), message_id) {
                Ok(job) => {
                    return Ok(Some(JobLookup { role, state, job }));
                }
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
        }
    }
    Ok(None)
}

fn list_active_jobs(root: &PathBuf) -> io::Result<Vec<JobLookup>> {
    let mut jobs = Vec::new();
    for role in list_roles(root)? {
        for state in active_job_states() {
            for job in botty_jobs::list_jobs(root, &role, state.clone())? {
                jobs.push(JobLookup {
                    role: role.clone(),
                    state: state.clone(),
                    job,
                });
            }
        }
    }
    Ok(jobs)
}

fn active_job_states() -> [JobState; 3] {
    [JobState::Queued, JobState::Running, JobState::Waiting]
}

fn all_job_states() -> [JobState; 5] {
    [
        JobState::Queued,
        JobState::Running,
        JobState::Waiting,
        JobState::Done,
        JobState::Failed,
    ]
}

fn interrupt_job(
    root: &PathBuf,
    target: JobLookup,
    dispatch_tx: &Sender<DispatcherCommand>,
) -> io::Result<String> {
    match target.state {
        JobState::Queued => {
            let message_id = target.job.message_id.clone();
            let role = target.role.clone();
            fail_job(
                root,
                role.as_str(),
                JobState::Queued,
                target.job,
                "interrupted before start".to_string(),
                dispatch_tx,
            )?;
            Ok(format!(
                "Interrupted queued task {message_id} for role {role} before it started."
            ))
        }
        JobState::Running => {
            let pid = target
                .job
                .worker_pid
                .or(read_worker_pid(root, &target.role)?)
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        format!(
                            "task {} is running but worker pid is unavailable",
                            target.job.message_id
                        ),
                    )
                })?;
            let message_id = target.job.message_id.clone();
            let role = target.role.clone();
            fail_job(
                root,
                role.as_str(),
                JobState::Running,
                target.job,
                format!("interrupted while running (pid {pid})"),
                dispatch_tx,
            )?;
            if let Err(err) = send_signal(pid, libc::SIGINT) {
                if err.raw_os_error() != Some(libc::ESRCH) {
                    return Err(err);
                }
            }
            let _ = write_worker_state(root, role.as_str(), None, None, Some("interrupted"));
            Ok(format!(
                "Sent interrupt to running task {} for role {} (pid {}).",
                message_id, role, pid
            ))
        }
        JobState::Waiting => {
            if let Some(child_message_id) = target.job.awaiting_message_id.clone() {
                if child_message_id == target.job.message_id {
                    return Err(io::Error::other(
                        "refusing to interrupt self-referential waiting job",
                    ));
                }
                let child = find_job_by_message_id(root, &child_message_id)?.ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        format!(
                            "waiting task {} references missing child task {}",
                            target.job.message_id, child_message_id
                        ),
                    )
                })?;
                return interrupt_job(root, child, dispatch_tx);
            }

            let message_id = target.job.message_id.clone();
            let role = target.role.clone();
            fail_job(
                root,
                role.as_str(),
                JobState::Waiting,
                target.job,
                "interrupted while waiting".to_string(),
                dispatch_tx,
            )?;
            Ok(format!(
                "Interrupted waiting task {message_id} for role {role}."
            ))
        }
        JobState::Done => Ok(format!(
            "Task {} for role {} is already done.",
            target.job.message_id, target.role
        )),
        JobState::Failed => Ok(format!(
            "Task {} for role {} is already failed.",
            target.job.message_id, target.role
        )),
    }
}

fn fail_job(
    root: &PathBuf,
    role: &str,
    from: JobState,
    mut job: QueueJob,
    error: String,
    dispatch_tx: &Sender<DispatcherCommand>,
) -> io::Result<()> {
    job.last_error = Some(error);
    job.completed_at_ms = Some(botty_jobs::now_ms());
    update_job_state(root, role, from, JobState::Failed, &mut job)?;
    try_resume_parent_job(root, &job, dispatch_tx)
}

fn parse_chat_meta_message(raw: &str) -> IncomingChatMessage {
    let mut incoming = IncomingChatMessage {
        source: "unknown".to_string(),
        user_id: "unknown".to_string(),
        target: None,
        message: raw.to_string(),
    };

    if !raw.starts_with(CHAT_META_PREFIX) {
        return incoming;
    }

    let mut parts = raw.splitn(4, '|');
    let prefix = parts.next();
    let source = parts.next();
    let user_id = parts.next();
    let message = parts.next();

    if prefix != Some(CHAT_META_PREFIX) || message.is_none() {
        return incoming;
    }

    if let Some(source) = source.and_then(|s| s.strip_prefix("source=")) {
        incoming.source = source.to_string();
    }
    if let Some(user_id) = user_id.and_then(|s| s.strip_prefix("user_id=")) {
        incoming.user_id = user_id.to_string();
    }
    incoming.message = message.unwrap_or_default().to_string();
    incoming
}

fn parse_enqueue_external_control(
    message: &str,
    defaults: &IncomingChatMessage,
) -> Option<IncomingChatMessage> {
    let payload = message.strip_prefix(CONTROL_PREFIX)?;
    let payload = payload.strip_prefix("enqueue-external|")?;
    let value: Value = serde_json::from_str(payload).ok()?;
    Some(IncomingChatMessage {
        source: value
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or(defaults.source.as_str())
            .to_string(),
        user_id: value
            .get("user_id")
            .and_then(Value::as_str)
            .unwrap_or(defaults.user_id.as_str())
            .to_string(),
        target: value
            .get("target")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        message: value.get("message")?.as_str()?.to_string(),
    })
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

fn boss_log_info(message: &str) {
    let _ = write_boss_log_line(&mut io::stdout(), message);
}

fn boss_log_error(message: &str) {
    let _ = write_boss_log_line(&mut io::stderr(), message);
}

fn write_boss_log_line(writer: &mut dyn Write, message: &str) -> io::Result<()> {
    let timestamp =
        local_time_format("%Y-%m-%d %H:%M:%S").unwrap_or_else(|_| "unknown-time".to_string());
    writeln!(writer, "[{timestamp}] {message}")
}

fn wait_for_chat_socket(timeout: Duration) -> io::Result<()> {
    let socket = chat_socket_path();
    let start = Instant::now();

    while start.elapsed() < timeout {
        if socket.exists() && UnixStream::connect(&socket).is_ok() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }

    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        format!("chat socket not ready: {}", socket.display()),
    ))
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

fn decode_assistant_reply(raw: &str) -> io::Result<AssistantReply> {
    let value: Value = serde_json::from_str(raw).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("decode assistant reply failed: {err}"),
        )
    })?;
    let text = value
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let thinking = value
        .get("thinking")
        .and_then(Value::as_str)
        .map(|value| value.to_string());
    let control = match value.get("control") {
        Some(Value::Null) | None => None,
        Some(control_value) => Some(
            serde_json::from_value::<crate::botty_body::AssistantControl>(control_value.clone())
                .map_err(|err| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("decode assistant control failed: {err}"),
                    )
                })?,
        ),
    };
    let _ = thinking;
    Ok(AssistantReply { text, control })
}

fn runtime_suffix() -> &'static str {
    if cfg!(debug_assertions) {
        "-dev"
    } else {
        ""
    }
}

fn setup_config_file() -> PathBuf {
    botty_root_dir()
        .join("config")
        .join(format!("setup{}.conf", runtime_suffix()))
}

fn crond_pid_file() -> PathBuf {
    botty_root_dir()
        .join("run")
        .join(format!("crond{}.pid", runtime_suffix()))
}

struct SetupConfig {
    ai_provider_debug: bool,
    telegram_enabled: bool,
    telegram_apikey: String,
    feishu_enabled: bool,
    feishu_app_id: String,
    feishu_app_secret: String,
    feishu_access_token: String,
    feishu_chat_id: String,
}

impl Default for SetupConfig {
    fn default() -> Self {
        Self {
            ai_provider_debug: false,
            telegram_enabled: true,
            telegram_apikey: String::new(),
            feishu_enabled: false,
            feishu_app_id: String::new(),
            feishu_app_secret: String::new(),
            feishu_access_token: String::new(),
            feishu_chat_id: String::new(),
        }
    }
}

fn load_setup_config() -> io::Result<SetupConfig> {
    let path = setup_config_file();
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(SetupConfig::default()),
        Err(err) => return Err(err),
    };

    let mut config = SetupConfig::default();
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
                    .map(|s| s.trim())
                    .any(|provider| provider == "telegram");
                config.feishu_enabled = value
                    .split(',')
                    .map(|s| s.trim())
                    .any(|provider| provider == "feishu");
            }
            "ai.provider.debug" => config.ai_provider_debug = parse_bool(value),
            "provider.debug" => config.ai_provider_debug = parse_bool(value),
            "chatbot.telegram.enabled" => config.telegram_enabled = parse_bool(value),
            "chatbot.telegram.apikey" => config.telegram_apikey = value.to_string(),
            "chatbot.feishu.enabled" => config.feishu_enabled = parse_bool(value),
            "chatbot.feishu.app_id" => config.feishu_app_id = value.to_string(),
            "chatbot.feishu.app_secret" => config.feishu_app_secret = value.to_string(),
            "chatbot.feishu.apikey" => config.feishu_access_token = value.to_string(),
            "chatbot.feishu.chat_id" => config.feishu_chat_id = value.to_string(),
            "chatbot.apikey" => {
                if config.telegram_apikey.is_empty() {
                    config.telegram_apikey = value.to_string();
                }
                if config.feishu_access_token.is_empty() {
                    config.feishu_access_token = value.to_string();
                }
            }
            _ => {}
        }
    }
    Ok(config)
}

fn parse_bool(value: &str) -> bool {
    matches!(value.trim(), "1" | "true" | "yes" | "on")
}

fn boss_process_name() -> &'static str {
    if cfg!(debug_assertions) {
        "Botty-Boss-dev"
    } else {
        "Botty-Boss"
    }
}

fn guy_process_name() -> &'static str {
    if cfg!(debug_assertions) {
        "Botty-Guy-dev"
    } else {
        "Botty-Guy"
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

#[allow(dead_code)]
fn _release_binary_path() -> PathBuf {
    PathBuf::from("release").join("mylittlebotty")
}
