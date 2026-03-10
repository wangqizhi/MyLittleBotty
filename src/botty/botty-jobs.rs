use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const EXTERNAL_INPUT_KIND: &str = "external_input";
const DELEGATED_TASK_KIND: &str = "delegated_task";
const JOB_IDLE_TIMEOUT_SECS: u64 = 10;
const DEFAULT_MAX_HOPS: u32 = 8;
const DEFAULT_MAX_TRACE_STEPS: u32 = 24;
static MESSAGE_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Waiting,
    Done,
    Failed,
}

impl JobState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueueJob {
    pub message_id: String,
    pub trace_id: String,
    pub parent_message_id: Option<String>,
    pub from_role: String,
    pub to_role: String,
    pub kind: String,
    pub source: String,
    pub user_id: String,
    pub target: Option<String>,
    pub payload: String,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
    pub started_at_ms: Option<u128>,
    pub completed_at_ms: Option<u128>,
    pub state: JobState,
    pub hop_count: u32,
    pub max_hops: u32,
    pub trace_step: u32,
    pub max_trace_steps: u32,
    pub result_text: Option<String>,
    pub last_error: Option<String>,
    pub worker_pid: Option<i32>,
    pub continuation_payload: Option<String>,
    pub awaiting_message_id: Option<String>,
    pub continuation_result: Option<String>,
    pub pending_user_notice: Option<String>,
}

#[derive(Clone, Debug)]
pub struct JobCounts {
    pub queued: usize,
    pub running: usize,
    pub waiting: usize,
    pub done: usize,
    pub failed: usize,
}

#[derive(Clone, Debug)]
pub struct RoleSnapshot {
    pub role: String,
    pub worker_pid: Option<i32>,
    pub current_job: Option<QueueJob>,
    pub queued_jobs: Vec<QueueJob>,
    pub counts: JobCounts,
    pub recent_error: Option<String>,
}

pub fn idle_timeout() -> Duration {
    Duration::from_secs(JOB_IDLE_TIMEOUT_SECS)
}

pub fn new_external_job(
    source: &str,
    user_id: &str,
    target: Option<String>,
    payload: &str,
) -> QueueJob {
    let message_id = new_message_id();
    let now = now_ms();
    QueueJob {
        trace_id: message_id.clone(),
        message_id,
        parent_message_id: None,
        from_role: "input".to_string(),
        to_role: "leader".to_string(),
        kind: EXTERNAL_INPUT_KIND.to_string(),
        source: source.to_string(),
        user_id: user_id.to_string(),
        target,
        payload: payload.to_string(),
        created_at_ms: now,
        updated_at_ms: now,
        started_at_ms: None,
        completed_at_ms: None,
        state: JobState::Queued,
        hop_count: 0,
        max_hops: DEFAULT_MAX_HOPS,
        trace_step: 0,
        max_trace_steps: DEFAULT_MAX_TRACE_STEPS,
        result_text: None,
        last_error: None,
        worker_pid: None,
        continuation_payload: None,
        awaiting_message_id: None,
        continuation_result: None,
        pending_user_notice: None,
    }
}

pub fn new_delegated_job(
    parent: &QueueJob,
    role: &str,
    payload: &str,
    source: &str,
    user_id: &str,
) -> io::Result<QueueJob> {
    if parent.hop_count >= parent.max_hops {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "delegation hop limit exceeded for trace {} ({}/{})",
                parent.trace_id, parent.hop_count, parent.max_hops
            ),
        ));
    }
    if parent.trace_step >= parent.max_trace_steps {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "trace step limit exceeded for trace {} ({}/{})",
                parent.trace_id, parent.trace_step, parent.max_trace_steps
            ),
        ));
    }

    let now = now_ms();
    Ok(QueueJob {
        message_id: new_message_id(),
        trace_id: parent.trace_id.clone(),
        parent_message_id: Some(parent.message_id.clone()),
        from_role: parent.to_role.clone(),
        to_role: role.to_string(),
        kind: DELEGATED_TASK_KIND.to_string(),
        source: source.to_string(),
        user_id: user_id.to_string(),
        target: parent.target.clone(),
        payload: payload.to_string(),
        created_at_ms: now,
        updated_at_ms: now,
        started_at_ms: None,
        completed_at_ms: None,
        state: JobState::Queued,
        hop_count: parent.hop_count + 1,
        max_hops: parent.max_hops,
        trace_step: parent.trace_step + 1,
        max_trace_steps: parent.max_trace_steps,
        result_text: None,
        last_error: None,
        worker_pid: None,
        continuation_payload: None,
        awaiting_message_id: None,
        continuation_result: None,
        pending_user_notice: None,
    })
}

pub fn jobs_root(root: &Path) -> PathBuf {
    root.join("run").join("jobs")
}

pub fn ensure_role_dirs(root: &Path, role: &str) -> io::Result<()> {
    for state in ["queued", "running", "waiting", "done", "failed"] {
        fs::create_dir_all(role_dir(root, role).join(state))?;
    }
    Ok(())
}

pub fn enqueue_job(root: &Path, job: &QueueJob) -> io::Result<PathBuf> {
    ensure_role_dirs(root, &job.to_role)?;
    let path = job_path(root, &job.to_role, JobState::Queued, &job.message_id);
    write_job(&path, job)?;
    Ok(path)
}

pub fn load_job(
    root: &Path,
    role: &str,
    state: JobState,
    message_id: &str,
) -> io::Result<QueueJob> {
    read_job(&job_path(root, role, state, message_id))
}

pub fn update_job_state(
    root: &Path,
    role: &str,
    from: JobState,
    to: JobState,
    job: &mut QueueJob,
) -> io::Result<PathBuf> {
    ensure_role_dirs(root, role)?;
    let old_path = job_path(root, role, from, &job.message_id);
    let new_path = job_path(root, role, to.clone(), &job.message_id);
    job.state = to;
    job.updated_at_ms = now_ms();
    write_job(&new_path, job)?;
    if old_path != new_path {
        let _ = fs::remove_file(&old_path);
    }
    Ok(new_path)
}

pub fn list_roles(root: &Path) -> io::Result<Vec<String>> {
    let root = jobs_root(root);
    let mut roles = Vec::new();
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(roles),
        Err(err) => return Err(err),
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            roles.push(entry.file_name().to_string_lossy().to_string());
        }
    }
    roles.sort_unstable();
    Ok(roles)
}

pub fn list_jobs(root: &Path, role: &str, state: JobState) -> io::Result<Vec<QueueJob>> {
    let dir = job_state_dir(root, role, state);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut jobs = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        match read_job(&entry.path()) {
            Ok(job) => jobs.push(job),
            Err(_) => {}
        }
    }
    jobs.sort_by(|left, right| {
        left.created_at_ms
            .cmp(&right.created_at_ms)
            .then_with(|| left.message_id.cmp(&right.message_id))
    });
    Ok(jobs)
}

pub fn next_queued_job(root: &Path, role: &str) -> io::Result<Option<QueueJob>> {
    let mut jobs = list_jobs(root, role, JobState::Queued)?;
    if jobs.is_empty() {
        return Ok(None);
    }
    Ok(Some(jobs.remove(0)))
}

pub fn role_snapshot(root: &Path, role: &str, worker_pid: Option<i32>) -> io::Result<RoleSnapshot> {
    let queued_jobs = list_jobs(root, role, JobState::Queued)?;
    let mut running_jobs = list_jobs(root, role, JobState::Running)?;
    let waiting_jobs = list_jobs(root, role, JobState::Waiting)?;
    let done_jobs = list_jobs(root, role, JobState::Done)?;
    let failed_jobs = list_jobs(root, role, JobState::Failed)?;
    let current_job = if running_jobs.is_empty() {
        None
    } else {
        Some(running_jobs.remove(0))
    };
    let running_count = usize::from(current_job.is_some());
    let recent_error = failed_jobs.last().and_then(|job| job.last_error.clone());

    Ok(RoleSnapshot {
        role: role.to_string(),
        worker_pid,
        current_job,
        counts: JobCounts {
            queued: queued_jobs.len(),
            running: running_count,
            waiting: waiting_jobs.len(),
            done: done_jobs.len(),
            failed: failed_jobs.len(),
        },
        queued_jobs,
        recent_error,
    })
}

pub fn read_job(path: &Path) -> io::Result<QueueJob> {
    let content = fs::read_to_string(path)?;
    serde_json::from_str::<QueueJob>(&content).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("parse job json failed for {}: {err}", path.display()),
        )
    })
}

pub fn write_job(path: &Path, job: &QueueJob) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(job)
        .map_err(|err| io::Error::other(format!("serialize job json failed: {err}")))?;
    fs::write(path, format!("{json}\n"))
}

pub fn role_dir(root: &Path, role: &str) -> PathBuf {
    jobs_root(root).join(role)
}

pub fn worker_state_path(root: &Path, role: &str) -> PathBuf {
    role_dir(root, role).join("worker.json")
}

pub fn write_worker_state(
    root: &Path,
    role: &str,
    pid: Option<i32>,
    current_message_id: Option<&str>,
    last_error: Option<&str>,
) -> io::Result<()> {
    ensure_role_dirs(root, role)?;
    let value = serde_json::json!({
        "role": role,
        "pid": pid,
        "current_message_id": current_message_id,
        "updated_at_ms": now_ms(),
        "last_error": last_error,
    });
    fs::write(worker_state_path(root, role), format!("{value}\n"))
}

pub fn read_worker_pid(root: &Path, role: &str) -> io::Result<Option<i32>> {
    let path = worker_state_path(root, role);
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    let value: Value = serde_json::from_str(&content).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("parse worker state failed: {err}"),
        )
    })?;
    Ok(value
        .get("pid")
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok()))
}

pub fn wait_job_terminal(root: &Path, role: &str, message_id: &str) -> io::Result<QueueJob> {
    loop {
        match load_job(root, role, JobState::Done, message_id) {
            Ok(job) => return Ok(job),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        match load_job(root, role, JobState::Failed, message_id) {
            Ok(job) => return Ok(job),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

pub fn job_age(now_ms: u128, job: &QueueJob) -> Duration {
    Duration::from_millis(now_ms.saturating_sub(job.created_at_ms) as u64)
}

pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    let secs = secs % 60;
    if hours > 0 {
        format!("{hours:02}:{mins:02}:{secs:02}")
    } else {
        format!("{mins:02}:{secs:02}")
    }
}

pub fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn job_state_dir(root: &Path, role: &str, state: JobState) -> PathBuf {
    role_dir(root, role).join(state.as_str())
}

fn job_path(root: &Path, role: &str, state: JobState, message_id: &str) -> PathBuf {
    job_state_dir(root, role, state).join(format!("{message_id}.json"))
}

fn new_message_id() -> String {
    format!(
        "job-{}-{}",
        now_ms(),
        MESSAGE_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}
