use std::env;
use std::ffi::CString;
use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};

pub fn start_daemon() -> io::Result<()> {
    let exe = env::current_exe()?;
    let log_dir = botty_root_dir().join("log");
    fs::create_dir_all(&log_dir)?;
    let log_path = log_dir.join("boss.log");

    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    let err_file = log_file.try_clone()?;

    let mut cmd = Command::new(&exe);
    cmd.arg0("Botty-Boss")
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

pub fn run_supervisor() {
    set_process_name("Botty-Boss");
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
        .arg0("Botty-Guy")
        .arg("--guy")
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
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
