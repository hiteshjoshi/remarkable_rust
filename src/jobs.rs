//! Background job tracking.
//!
//! When the CLI is invoked with `--background`, the parent process re-spawns
//! itself with `RR_JOB_ID` set and the appropriate stdio detached to log
//! files. The child writes a PID file, runs the command, then writes a
//! status file with the final exit state. This module owns the on-disk
//! layout and the parent-side "spawn detached" plumbing.
//!
//! Layout (under `state_dir()`):
//!
//! ```text
//! jobs/
//!   <id>.pid       <- child PID while running
//!   <id>.log       <- combined stdout+stderr
//!   <id>.meta.json <- {kind, args, started_at, status, exit_code}
//! ```

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

const ENV_JOB_ID: &str = "RR_JOB_ID";

/// Status of a background job.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

/// On-disk job metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobMeta {
    pub id: String,
    pub kind: String,
    pub args: Vec<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub status: JobStatus,
    pub exit_code: Option<i32>,
    pub pid: Option<u32>,
}

/// In-flight job handle returned by [`spawn_detached`].
#[derive(Debug)]
pub struct JobHandle {
    pub id: String,
    pub pid: u32,
    pub log_path: PathBuf,
}

/// Resolve the directory holding job state. Honors `XDG_STATE_HOME`.
pub fn state_dir() -> Result<PathBuf> {
    if let Ok(custom) = std::env::var("XDG_STATE_HOME") {
        if !custom.is_empty() {
            return Ok(PathBuf::from(custom).join("rr").join("jobs"));
        }
    }
    let base = dirs::data_local_dir()
        .or_else(dirs::cache_dir)
        .ok_or_else(|| Error::Config("could not determine state directory".into()))?;
    Ok(base.join("rr").join("jobs"))
}

fn ensure_state_dir() -> Result<PathBuf> {
    let dir = state_dir()?;
    fs::create_dir_all(&dir).map_err(|source| Error::Io {
        path: dir.clone(),
        source,
    })?;
    Ok(dir)
}

fn log_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.log"))
}

fn pid_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.pid"))
}

fn meta_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.meta.json"))
}

/// If the current process was spawned as a background job, return the id.
pub fn current_job_id() -> Option<String> {
    std::env::var(ENV_JOB_ID).ok().filter(|s| !s.is_empty())
}

/// Mark the *current* process as the running incarnation of the given job:
/// write the pid file and the initial `running` meta. Call this from the
/// child immediately on entry.
pub fn child_init(id: &str, kind: &str, args: &[String]) -> Result<()> {
    let dir = ensure_state_dir()?;
    let pid = std::process::id();
    fs::write(pid_path(&dir, id), pid.to_string()).map_err(|source| Error::Io {
        path: pid_path(&dir, id),
        source,
    })?;
    let meta = JobMeta {
        id: id.to_string(),
        kind: kind.to_string(),
        args: args.to_vec(),
        started_at: Utc::now(),
        finished_at: None,
        status: JobStatus::Running,
        exit_code: None,
        pid: Some(pid),
    };
    write_meta(&dir, &meta)
}

/// Finalise the job's meta and remove the pid file.
pub fn child_finalise(id: &str, status: JobStatus, exit_code: Option<i32>) -> Result<()> {
    let dir = ensure_state_dir()?;
    let mut meta = read_meta(&dir, id)?;
    meta.status = status;
    meta.exit_code = exit_code;
    meta.finished_at = Some(Utc::now());
    write_meta(&dir, &meta)?;
    let pid = pid_path(&dir, id);
    if pid.exists() {
        let _ = fs::remove_file(pid);
    }
    Ok(())
}

fn write_meta(dir: &Path, meta: &JobMeta) -> Result<()> {
    let path = meta_path(dir, &meta.id);
    let json = serde_json::to_string_pretty(meta)
        .map_err(|e| Error::Other(format!("encode meta: {e}")))?;
    fs::write(&path, json).map_err(|source| Error::Io { path, source })
}

fn read_meta(dir: &Path, id: &str) -> Result<JobMeta> {
    let path = meta_path(dir, id);
    let text = fs::read_to_string(&path).map_err(|source| Error::Io {
        path: path.clone(),
        source,
    })?;
    serde_json::from_str(&text).map_err(|e| Error::InvalidResponse(format!("meta json: {e}")))
}

/// List jobs (newest first). Reaps stale `Running` jobs whose PIDs are gone.
pub fn list() -> Result<Vec<JobMeta>> {
    let dir = match state_dir() {
        Ok(d) => d,
        Err(_) => return Ok(Vec::new()),
    };
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let entries = fs::read_dir(&dir).map_err(|source| Error::Io {
        path: dir.clone(),
        source,
    })?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(mut meta) = serde_json::from_str::<JobMeta>(&text) else {
            continue;
        };
        if meta.status == JobStatus::Running && !is_pid_alive(meta.pid) {
            // Reap zombie meta: process gone but never finalised.
            meta.status = JobStatus::Failed;
            meta.exit_code = meta.exit_code.or(Some(-1));
            meta.finished_at = Some(Utc::now());
            let _ = write_meta(&dir, &meta);
            let _ = fs::remove_file(pid_path(&dir, &meta.id));
        }
        out.push(meta);
    }
    out.sort_by_key(|m| std::cmp::Reverse(m.started_at));
    Ok(out)
}

/// Read the log file for a given job (full contents).
pub fn read_log(id: &str) -> Result<String> {
    let dir = ensure_state_dir()?;
    let path = log_path(&dir, id);
    fs::read_to_string(&path).map_err(|source| Error::Io { path, source })
}

/// Cancel a running job. Returns true if a signal was sent.
pub fn cancel(id: &str) -> Result<bool> {
    let dir = ensure_state_dir()?;
    let meta = read_meta(&dir, id)?;
    let Some(pid) = meta.pid else {
        return Ok(false);
    };
    if !is_pid_alive(Some(pid)) {
        return Ok(false);
    }
    send_terminate(pid)?;
    Ok(true)
}

#[cfg(unix)]
fn send_terminate(pid: u32) -> Result<()> {
    use std::process::Command;
    let status = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .map_err(|e| Error::Job(format!("kill failed: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::Job(format!("kill exited with {status}")))
    }
}

#[cfg(not(unix))]
fn send_terminate(pid: u32) -> Result<()> {
    use std::process::Command;
    let status = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .status()
        .map_err(|e| Error::Job(format!("taskkill failed: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::Job(format!("taskkill exited with {status}")))
    }
}

#[cfg(unix)]
fn is_pid_alive(pid: Option<u32>) -> bool {
    let Some(pid) = pid else { return false };
    // signal 0 = existence check
    use std::process::Command;
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_pid_alive(_pid: Option<u32>) -> bool {
    true // best-effort; on Windows the metadata stays accurate via taskkill
}

/// Spawn `rr` again, detached, with `RR_JOB_ID` set. Stdio is redirected
/// to the per-job log file. Returns once the child is launched; the parent
/// process exits normally so the caller (shell or AI agent) is unblocked.
pub fn spawn_detached(kind: &str, child_args: &[String]) -> Result<JobHandle> {
    let id = new_id();
    let dir = ensure_state_dir()?;
    let log = log_path(&dir, &id);

    let mut log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .map_err(|source| Error::Io {
            path: log.clone(),
            source,
        })?;
    let banner = format!(
        "[rr] job {id} started at {} :: {}\n",
        Utc::now().to_rfc3339(),
        child_args.join(" ")
    );
    log_file
        .write_all(banner.as_bytes())
        .map_err(|source| Error::Io {
            path: log.clone(),
            source,
        })?;
    let log_clone = log_file.try_clone().map_err(|source| Error::Io {
        path: log.clone(),
        source,
    })?;

    // Seed metadata so `rr jobs` shows it immediately.
    let meta = JobMeta {
        id: id.clone(),
        kind: kind.to_string(),
        args: child_args.to_vec(),
        started_at: Utc::now(),
        finished_at: None,
        status: JobStatus::Running,
        exit_code: None,
        pid: None,
    };
    write_meta(&dir, &meta)?;

    let exe = std::env::current_exe().map_err(|e| Error::Job(format!("current_exe: {e}")))?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(child_args.iter().map(OsString::from));
    cmd.env(ENV_JOB_ID, &id);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_clone));

    detach(&mut cmd);

    let child = cmd
        .spawn()
        .map_err(|e| Error::Job(format!("spawn detached: {e}")))?;
    let pid = child.id();

    // Update meta with PID; the child will overwrite once it reaches child_init.
    let mut meta = meta;
    meta.pid = Some(pid);
    write_meta(&dir, &meta)?;

    // We intentionally drop the child handle so the parent doesn't wait.
    drop(child);

    Ok(JobHandle {
        id,
        pid,
        log_path: log,
    })
}

#[cfg(unix)]
fn detach(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    // setsid → new session/process group, fully detached from controlling TTY.
    // SAFETY: pre_exec runs in the child between fork and exec; setsid is async-signal-safe.
    // The closure must not allocate or touch shared state — `libc::setsid()` is fine.
    // We avoid `unsafe` by going through the safe `libc`-free `process_group(0)` alternative
    // when available. Stable Rust requires unsafe for pre_exec, but we can satisfy
    // detachment without it by using process_group(0) on Rust 1.64+:
    cmd.process_group(0);
}

#[cfg(not(unix))]
fn detach(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    // CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
}

fn new_id() -> String {
    // Short, sortable, lowercase. Format: YYYYMMDDHHMMSS-<rand6>
    let ts = Utc::now().format("%Y%m%d%H%M%S");
    let suffix: String = {
        use rand::distributions::Alphanumeric;
        use rand::Rng;
        rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(6)
            .map(char::from)
            .collect::<String>()
            .to_lowercase()
    };
    format!("{ts}-{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_has_expected_shape() {
        let id = new_id();
        let (ts, suffix) = id.split_once('-').expect("id should contain '-'");
        assert_eq!(ts.len(), 14, "timestamp portion should be YYYYMMDDHHMMSS");
        assert_eq!(suffix.len(), 6);
        assert!(suffix.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn meta_round_trips() {
        let meta = JobMeta {
            id: "20260520120000-abcdef".into(),
            kind: "upload".into(),
            args: vec!["--background".into(), "file.md".into()],
            started_at: Utc::now(),
            finished_at: None,
            status: JobStatus::Running,
            exit_code: None,
            pid: Some(1234),
        };
        let json = serde_json::to_string(&meta).unwrap();
        let back: JobMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, meta.id);
        assert_eq!(back.status, JobStatus::Running);
    }
}
