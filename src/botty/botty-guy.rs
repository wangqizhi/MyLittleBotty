use std::ffi::CString;
use std::io;
use std::io::BufRead;
use std::io::Write;

pub fn run() {
    set_process_name(guy_process_name());
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut lines = stdin.lock().lines();

    while let Some(line_result) = lines.next() {
        let line = match line_result {
            Ok(line) => line,
            Err(err) => {
                eprintln!("Botty-Guy failed to read input: {err}");
                break;
            }
        };
        let message = line.trim();
        if message.is_empty() {
            continue;
        }

        let reply = format!("收到：{message}");
        if let Err(err) = writeln!(stdout, "{reply}") {
            eprintln!("Botty-Guy failed to write output: {err}");
            break;
        }
        if let Err(err) = stdout.flush() {
            eprintln!("Botty-Guy failed to flush output: {err}");
            break;
        }
    }
}

fn guy_process_name() -> &'static str {
    if cfg!(debug_assertions) {
        "Botty-Guy-dev"
    } else {
        "Botty-Guy"
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
