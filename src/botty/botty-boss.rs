use serde_json::{self, json, Value};
use std::cmp::Ordering;
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
use std::time::Duration;
use std::time::Instant;

const DOWNLOAD_URL: &str = env!("BOTTY_DOWNLOAD_URL");
const LATEST_RELEASE_API_URL: &str = env!("BOTTY_LATEST_RELEASE_API_URL");
const INSTALL_SCRIPT_URL: &str = env!("BOTTY_INSTALL_SCRIPT_URL");
const CURL_MAX_TIME_SECONDS: &str = "60";
const GUY_DEFAULT_ROLE: &str = "leader";
const CHAT_META_PREFIX: &str = "__botty_meta__";

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
    Ok(())
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
}

impl StatusSnapshot {
    fn boss_running(&self) -> bool {
        !self.boss_pids.is_empty()
    }
}

fn collect_status_snapshot() -> io::Result<StatusSnapshot> {
    let mut boss_pids = Vec::new();
    let mut guy_pids = Vec::new();
    let mut crond_pids = find_pids_by_process_name(crond_process_name())?;

    if let Some(boss_pid) = read_pid_file(&boss_pid_file())? {
        if is_process_alive(boss_pid) {
            boss_pids.push(boss_pid);
            let descendants = find_descendant_pids(boss_pid)?;
            let mut candidates = find_pids_by_process_name(guy_process_name())?;
            candidates.retain(|pid| descendants.contains(pid) && is_process_alive(*pid));
            guy_pids = candidates;
            crond_pids.retain(|pid| descendants.contains(pid) && is_process_alive(*pid));
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

pub fn run_supervisor() {
    let _pid_guard = match acquire_boss_pid_guard() {
        Ok(Some(guard)) => guard,
        Ok(None) => {
            eprintln!("Botty-Boss is already running, exiting duplicate supervisor");
            return;
        }
        Err(err) => {
            eprintln!("Botty-Boss failed to acquire pid file: {err}");
            return;
        }
    };

    set_process_name(boss_process_name());
    println!("Botty-Boss supervisor is running");

    let _socket_guard = match bind_chat_socket() {
        Ok(guard) => guard,
        Err(err) => {
            eprintln!("Botty-Boss failed to bind chat socket: {err}");
            return;
        }
    };

    let (chat_tx, chat_rx) = mpsc::channel::<QueuedChatRequest>();
    let _chat_worker = thread::spawn(move || run_chat_worker(chat_rx));
    let config = load_setup_config().unwrap_or_default();
    let _input_bridges = spawn_enabled_input_processes(&config);
    let _crond_bridge = spawn_crond_process();

    loop {
        match _socket_guard.listener.accept() {
            Ok((stream, _)) => {
                let chat_tx = chat_tx.clone();
                thread::spawn(move || {
                    if let Err(err) = handle_chat_client(stream, chat_tx) {
                        eprintln!("Botty-Boss failed to handle chat session: {err}");
                    }
                });
            }
            Err(err) => eprintln!("Botty-Boss accept error: {err}"),
        }
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
    fn spawn() -> io::Result<Self> {
        let exe = env::current_exe()?;
        let mut child = Command::new(exe)
            .arg0(guy_process_name())
            .arg("--guy")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;

        let child_pid = i32::try_from(child.id())
            .map_err(|_| io::Error::other("failed to convert guy pid to i32"))?;
        persist_guy_role(child_pid, GUY_DEFAULT_ROLE)?;

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
            eprintln!("Botty-Boss failed to get current executable path for Botty-crond: {err}");
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
            eprintln!("Botty-Boss failed to run {process_name}: {err}");
            None
        }
    }
}

struct InputProcessSpec {
    name: &'static str,
    arg: &'static str,
    enabled: fn(&SetupConfig) -> bool,
}

impl InputProcessSpec {
    fn process_name(&self) -> String {
        format!("{}{}", self.name, runtime_suffix())
    }
}

fn input_process_specs() -> [InputProcessSpec; 2] {
    [
        InputProcessSpec {
            name: "Botty-input-telegram",
            arg: "--input-telegram",
            enabled: |config| config.telegram_enabled && !config.telegram_apikey.is_empty(),
        },
        InputProcessSpec {
            name: "Botty-input-feishu",
            arg: "--input-feishu",
            enabled: |config| {
                config.feishu_enabled
                    && !config.feishu_apikey.is_empty()
                    && !config.feishu_chat_id.is_empty()
            },
        },
    ]
}

fn spawn_enabled_input_processes(config: &SetupConfig) -> Vec<InputProcessBridge> {
    let mut bridges = Vec::new();
    let exe = match env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            eprintln!("Botty-Boss failed to get current executable path: {err}");
            return bridges;
        }
    };

    for spec in input_process_specs() {
        if !(spec.enabled)(config) {
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
            Err(err) => eprintln!("Botty-Boss failed to run {process_name}: {err}"),
        }
    }

    bridges
}

fn guy_role_config_file() -> PathBuf {
    botty_root_dir()
        .join("config")
        .join(format!("guy-role-map{}.conf", runtime_suffix()))
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

impl Drop for GuyBridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn handle_chat_client(stream: UnixStream, chat_tx: Sender<QueuedChatRequest>) -> io::Result<()> {
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

        let _ = persist_chat_message(
            "user",
            &incoming.source,
            &incoming.user_id,
            &incoming.message,
        );

        let (reply_tx, reply_rx) = mpsc::channel();
        chat_tx
            .send(QueuedChatRequest { incoming, reply_tx })
            .map_err(|_| {
                io::Error::new(io::ErrorKind::BrokenPipe, "chat worker is not available")
            })?;
        let response = reply_rx.recv().map_err(|_| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "chat worker closed reply channel",
            )
        })??;

        writeln!(writer, "{}", encode_ipc_line(&response)?)?;
        writer.flush()?;
    }
}

struct QueuedChatRequest {
    incoming: IncomingChatMessage,
    reply_tx: Sender<io::Result<String>>,
}

struct AssistantReply {
    text: String,
    thinking: Option<String>,
}

fn run_chat_worker(chat_rx: Receiver<QueuedChatRequest>) {
    let mut guy_bridge = match GuyBridge::spawn() {
        Ok(bridge) => bridge,
        Err(err) => {
            eprintln!("Botty-Boss failed to run Botty-Guy: {err}");
            return;
        }
    };

    while let Ok(request) = chat_rx.recv() {
        let leader_message = leader_message_for_source(&request.incoming);
        let response = match guy_bridge.ask(&leader_message) {
            Ok(response) => Ok(response),
            Err(_) => {
                if take_interrupt_flag() {
                    let _ = request.reply_tx.send(Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "Request interrupted.",
                    )));
                    if let Ok(bridge) = GuyBridge::spawn() {
                        guy_bridge = bridge;
                    }
                    continue;
                }
                match GuyBridge::spawn().and_then(|bridge| {
                    guy_bridge = bridge;
                    guy_bridge.ask(&leader_message)
                }) {
                    Ok(response) => Ok(response),
                    Err(err) => Err(err),
                }
            }
        };

        if let Ok(reply) = &response {
            let _ = persist_chat_message(
                "assistant",
                &request.incoming.source,
                &request.incoming.user_id,
                &format_assistant_memory_message(reply),
            );
        }

        let _ = request.reply_tx.send(response.map(|reply| reply.text));
    }
}

const CHAT_MEMORY_MAX_BYTES: u64 = 200 * 1024;

struct IncomingChatMessage {
    source: String,
    user_id: String,
    message: String,
}

fn leader_message_for_source(incoming: &IncomingChatMessage) -> String {
    let prefix = format!("{}: ", incoming.source);
    if incoming.message.starts_with(&prefix) {
        incoming.message.clone()
    } else {
        format!("{prefix}{}", incoming.message)
    }
}

fn take_interrupt_flag() -> bool {
    let path = interrupt_flag_file();
    if !path.exists() {
        return false;
    }
    let _ = fs::remove_file(path);
    true
}

fn parse_chat_meta_message(raw: &str) -> IncomingChatMessage {
    let mut incoming = IncomingChatMessage {
        source: "unknown".to_string(),
        user_id: "unknown".to_string(),
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

fn format_assistant_memory_message(reply: &AssistantReply) -> String {
    match reply.thinking.as_deref().map(str::trim) {
        Some(thinking) if !thinking.is_empty() => {
            format!("|{{'thinking': {}}} | {}", json!(thinking), reply.text)
        }
        _ => reply.text.clone(),
    }
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
    Ok(AssistantReply { text, thinking })
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
    feishu_apikey: String,
    feishu_chat_id: String,
}

impl Default for SetupConfig {
    fn default() -> Self {
        Self {
            ai_provider_debug: false,
            telegram_enabled: true,
            telegram_apikey: String::new(),
            feishu_enabled: false,
            feishu_apikey: String::new(),
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
            "chatbot.feishu.apikey" => config.feishu_apikey = value.to_string(),
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
