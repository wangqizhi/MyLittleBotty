use std::env;
use std::cmp::Ordering;
use std::ffi::CString;
use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};

const DOWNLOAD_URL: &str = env!("BOTTY_DOWNLOAD_URL");
const LATEST_RELEASE_API_URL: &str = env!("BOTTY_LATEST_RELEASE_API_URL");

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
    let mut targets = Vec::new();
    let pid_path = boss_pid_file();
    if let Some(pid) = read_pid_file(&pid_path)? {
        targets.push(pid);
    }

    targets.extend(find_pids_by_pattern(boss_process_name())?);
    targets.extend(find_pids_by_pattern(guy_process_name())?);
    targets.sort_unstable();
    targets.dedup();

    if targets.is_empty() {
        println!("No Botty processes running");
        let _ = fs::remove_file(pid_path);
        return Ok(());
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
    if forced == 0 {
        println!("Stopped Botty-Boss and Botty-Guy");
    } else {
        println!("Stopped Botty-Boss and Botty-Guy (force killed {forced})");
    }
    Ok(())
}

pub fn print_status() -> io::Result<()> {
    let snapshot = collect_status_snapshot()?;
    println!("Boss running: {}", snapshot.boss_running());
    println!("Boss pids: {}", format_pid_list(&snapshot.boss_pids));
    println!("Guy process count: {}", snapshot.guy_pids.len());
    println!("Guy pids: {}", format_pid_list(&snapshot.guy_pids));
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
    if snapshot.boss_running() || !snapshot.guy_pids.is_empty() {
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

fn find_pids_by_pattern(pattern: &str) -> io::Result<Vec<i32>> {
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

struct StatusSnapshot {
    boss_pids: Vec<i32>,
    guy_pids: Vec<i32>,
}

impl StatusSnapshot {
    fn boss_running(&self) -> bool {
        !self.boss_pids.is_empty()
    }
}

fn collect_status_snapshot() -> io::Result<StatusSnapshot> {
    let mut boss_pids = Vec::new();
    let mut guy_pids = Vec::new();

    if let Some(boss_pid) = read_pid_file(&boss_pid_file())? {
        if is_process_alive(boss_pid) {
            boss_pids.push(boss_pid);
            guy_pids = find_descendant_pids(boss_pid)?;
            guy_pids.retain(|pid| is_process_alive(*pid));
        } else {
            let _ = fs::remove_file(boss_pid_file());
        }
    }

    boss_pids.sort_unstable();
    boss_pids.dedup();
    guy_pids.sort_unstable();
    guy_pids.dedup();

    Ok(StatusSnapshot { boss_pids, guy_pids })
}

fn fetch_latest_release_tag() -> io::Result<String> {
    let output = Command::new("curl")
        .arg("-fsSL")
        .arg("-H")
        .arg("Accept: application/vnd.github+json")
        .arg("-H")
        .arg("User-Agent: mylittlebotty-updater")
        .arg(LATEST_RELEASE_API_URL)
        .output()?;

    if !output.status.success() {
        return Err(io::Error::other("failed to request latest release"));
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
        return Err(io::Error::other("failed to download release asset"));
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

    loop {
        match spawn_guy().and_then(|mut child| child.wait()) {
            Ok(status) => report_exit(status),
            Err(err) => eprintln!("Botty-Boss failed to run Botty-Guy: {err}"),
        }

        std::thread::sleep(std::time::Duration::from_secs(1));
        println!("Botty-Boss restarting Botty-Guy...");
    }
}

fn spawn_guy() -> io::Result<std::process::Child> {
    let exe = env::current_exe()?;

    Command::new(exe)
        .arg0(guy_process_name())
        .arg("--guy")
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
}

fn runtime_suffix() -> &'static str {
    if cfg!(debug_assertions) {
        "-dev"
    } else {
        ""
    }
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

fn report_exit(status: ExitStatus) {
    if let Some(code) = status.code() {
        eprintln!("Botty-Guy exited with code {code}");
    } else {
        eprintln!("Botty-Guy was terminated by signal");
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
