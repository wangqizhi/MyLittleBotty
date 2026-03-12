use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use std::fs::{self, OpenOptions};
use std::io;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

const OUTPUT_TAIL_MAX_BYTES: usize = 16 * 1024;
const CODEX_TRUST_PROMPT_MARKERS: [&str; 3] = [
    "Do you trust the contents of this directory?",
    "Working with untrusted contents comes with higher risk of prompt injection.",
    "Press enter to continue",
];
const CODEX_TRUST_MAX_AUTO_ATTEMPTS: u8 = 3;

pub struct AppTerminal {
    transcript_path: PathBuf,
    output_tail: Arc<Mutex<String>>,
    last_output_ms: Arc<AtomicU64>,
    exited: Arc<AtomicBool>,
    exit_code: Arc<Mutex<Option<i32>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Mutex<Box<dyn portable_pty::Child + Send>>,
}

pub struct AppTerminalStatus {
    pub exited: bool,
    pub exit_code: Option<i32>,
    pub last_output_ms: u64,
    pub transcript_tail: String,
}

impl AppTerminal {
    pub fn spawn(
        program: &str,
        args: &[String],
        work_dir: &Path,
        transcript_path: PathBuf,
        envs: &[(String, String)],
    ) -> io::Result<Self> {
        if let Some(parent) = transcript_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(PtySize {
                rows: 30,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|err| io::Error::other(format!("open pty failed: {err}")))?;

        let mut command = CommandBuilder::new(program);
        for arg in args {
            command.arg(arg);
        }
        command.cwd(work_dir);
        command.env("TERM", "xterm-256color");
        for (key, value) in envs {
            command.env(key, value);
        }

        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|err| io::Error::other(format!("spawn terminal command failed: {err}")))?;
        drop(pair.slave);
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|err| io::Error::other(format!("clone pty reader failed: {err}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|err| io::Error::other(format!("take pty writer failed: {err}")))?;

        let output_tail = Arc::new(Mutex::new(String::new()));
        let last_output_ms = Arc::new(AtomicU64::new(now_ms()));
        let exited = Arc::new(AtomicBool::new(false));
        let exit_code = Arc::new(Mutex::new(None));
        let writer = Arc::new(Mutex::new(writer));

        spawn_reader_thread(
            reader,
            transcript_path.clone(),
            Arc::clone(&writer),
            Arc::clone(&output_tail),
            Arc::clone(&last_output_ms),
        );
        Ok(Self {
            transcript_path,
            output_tail,
            last_output_ms,
            exited,
            exit_code,
            writer,
            child: Mutex::new(child),
        })
    }

    pub fn send_line(&self, text: &str) -> io::Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| io::Error::other("terminal writer lock poisoned"))?;
        writer.write_all(text.as_bytes())?;
        writer.flush()?;
        // Codex's TUI reliably submits only when Enter is sent as a distinct
        // follow-up keypress instead of being coalesced with pasted text.
        thread::sleep(Duration::from_millis(120));
        writer.write_all(b"\r")?;
        writer.flush()
    }

    pub fn send_ctrl_c(&self) -> io::Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| io::Error::other("terminal writer lock poisoned"))?;
        writer.write_all(&[3])?;
        writer.flush()
    }

    pub fn terminate(&self) -> io::Result<()> {
        let mut child = self
            .child
            .lock()
            .map_err(|_| io::Error::other("terminal child lock poisoned"))?;
        child
            .kill()
            .map_err(|err| io::Error::other(format!("kill terminal child failed: {err}")))
    }

    pub fn status(&self) -> AppTerminalStatus {
        if let Ok(mut child) = self.child.lock() {
            if let Ok(Some(status)) = child.try_wait() {
                self.exited.store(true, Ordering::SeqCst);
                if let Ok(mut exit_code) = self.exit_code.lock() {
                    *exit_code = Some(i32::try_from(status.exit_code()).unwrap_or(i32::MAX));
                }
            }
        }
        let transcript_tail = self
            .output_tail
            .lock()
            .map(|tail| tail.clone())
            .unwrap_or_default();
        let exit_code = self.exit_code.lock().map(|code| *code).unwrap_or(None);
        AppTerminalStatus {
            exited: self.exited.load(Ordering::SeqCst),
            exit_code,
            last_output_ms: self.last_output_ms.load(Ordering::SeqCst),
            transcript_tail,
        }
    }

    pub fn transcript_path(&self) -> &Path {
        &self.transcript_path
    }
}

fn spawn_reader_thread(
    mut reader: Box<dyn Read + Send>,
    transcript_path: PathBuf,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
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
        let mut codex_trust_attempts = 0u8;

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
                if codex_trust_attempts < CODEX_TRUST_MAX_AUTO_ATTEMPTS
                    && should_accept_codex_trust_prompt(tail.as_str())
                {
                    if let Ok(mut writer) = writer.lock() {
                        let payload = if codex_trust_attempts == 0 {
                            b"\r".as_slice()
                        } else {
                            b"1\r".as_slice()
                        };
                        let _ = writer.write_all(payload);
                        let _ = writer.flush();
                        codex_trust_attempts += 1;
                    }
                }
            }
        }
    });
}

fn should_accept_codex_trust_prompt(transcript_tail: &str) -> bool {
    CODEX_TRUST_PROMPT_MARKERS
        .iter()
        .all(|marker| transcript_tail.contains(marker))
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

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
