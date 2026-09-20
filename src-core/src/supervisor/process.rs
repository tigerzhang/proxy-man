use super::adopt::process_is_alive;
use anyhow::{Context, Result};
use chrono::Utc;
use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncSeekExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing::{error, info, warn};

const MAX_LOG_BUFFER_LINES: usize = 1000;
const LOG_TAIL_POLL: Duration = Duration::from_millis(250);

pub enum PollExit {
    Alive,
    Exited { code: Option<i32> },
}

pub struct ProcessHandle {
    pub id: String,
    pub child: Option<Child>,
    pub pid: u32,
    pub owns_process_group: bool,
    pub log_sender: broadcast::Sender<String>,
    pub recent_logs: Arc<Mutex<VecDeque<String>>>,
    log_task: Option<JoinHandle<()>>,
}

impl ProcessHandle {
    pub fn spawn(id: String, mut cmd: Command, log_file: PathBuf) -> Result<Self> {
        write_log_banner(&log_file, &format!("--- service '{id}' starting ---"));

        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)
            .with_context(|| format!("Failed to open log file {:?}", log_file))?;
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::from(log.try_clone()?));
        cmd.stderr(Stdio::from(log));
        cmd.kill_on_drop(false);

        #[cfg(unix)]
        {
            cmd.process_group(0);
        }

        let child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn process for service '{}'", id))?;

        let pid = child.id().unwrap_or(0);
        info!("Spawned service '{}' with PID {}", id, pid);
        write_log_banner(&log_file, &format!("--- service '{id}' started pid={pid} ---"));

        Ok(Self::from_parts(
            id,
            Some(child),
            pid,
            true,
            log_file,
        ))
    }

    pub fn adopt(id: String, pid: u32, owns_process_group: bool, log_file: PathBuf) -> Self {
        info!(
            "Adopting leftover process for service '{}' (PID {})",
            id, pid
        );
        write_log_banner(
            &log_file,
            &format!("--- service '{id}' adopted pid={pid} ---"),
        );
        Self::from_parts(id, None, pid, owns_process_group, log_file)
    }

    fn from_parts(
        id: String,
        child: Option<Child>,
        pid: u32,
        owns_process_group: bool,
        log_file: PathBuf,
    ) -> Self {
        let (recent_logs, offset) = load_recent_log_lines(&log_file, MAX_LOG_BUFFER_LINES);
        let recent_logs = Arc::new(Mutex::new(recent_logs));
        let (log_sender, _) = broadcast::channel::<String>(256);
        let log_task = Some(spawn_log_tailer(
            log_file,
            offset,
            log_sender.clone(),
            Arc::clone(&recent_logs),
        ));

        Self {
            id,
            child,
            pid,
            owns_process_group,
            log_sender,
            recent_logs,
            log_task,
        }
    }

    pub fn poll_exit(&mut self) -> Result<PollExit> {
        if let Some(child) = &mut self.child {
            match child.try_wait() {
                Ok(Some(status)) => Ok(PollExit::Exited {
                    code: status.code(),
                }),
                Ok(None) => Ok(PollExit::Alive),
                Err(err) => {
                    error!("Error checking child process for '{}': {}", self.id, err);
                    if process_is_alive(self.pid) {
                        Ok(PollExit::Alive)
                    } else {
                        Ok(PollExit::Exited { code: None })
                    }
                }
            }
        } else if process_is_alive(self.pid) {
            Ok(PollExit::Alive)
        } else {
            Ok(PollExit::Exited { code: None })
        }
    }

    pub fn is_running(&mut self) -> bool {
        matches!(self.poll_exit(), Ok(PollExit::Alive))
    }

    pub async fn stop(&mut self, timeout_duration: Duration) -> Result<()> {
        info!("Stopping service '{}' (PID {})", self.id, self.pid);
        stop_pid(self.pid, self.owns_process_group, timeout_duration).await?;
        if let Some(child) = &mut self.child {
            if let Ok(None) = child.try_wait() {
                let _ = child.kill().await;
                let _ = child.wait().await;
            }
        }
        Ok(())
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        if let Some(task) = self.log_task.take() {
            task.abort();
        }
    }
}

pub async fn stop_pid(pid: u32, owns_process_group: bool, timeout_duration: Duration) -> Result<()> {
    if pid == 0 || pid > i32::MAX as u32 {
        return Ok(());
    }

    info!("Stopping PID {} (owns_process_group={})", pid, owns_process_group);
    signal_process(pid, owns_process_group, false);

    let start_wait = std::time::Instant::now();
    loop {
        if !process_is_alive(pid) {
            return Ok(());
        }
        if start_wait.elapsed() > timeout_duration {
            warn!(
                "PID {} did not stop gracefully; killing with SIGKILL",
                pid
            );
            signal_process(pid, owns_process_group, true);
            for _ in 0..20 {
                if !process_is_alive(pid) {
                    break;
                }
                sleep(Duration::from_millis(50)).await;
            }
            return Ok(());
        }
        sleep(Duration::from_millis(100)).await;
    }
}

fn signal_process(pid: u32, owns_process_group: bool, kill: bool) {
    if pid == 0 || pid > i32::MAX as u32 {
        return;
    }
    #[cfg(unix)]
    unsafe {
        let sig = if kill { libc::SIGKILL } else { libc::SIGTERM };
        if owns_process_group {
            let pgid = libc::getpgid(pid as i32);
            if pgid > 0 {
                libc::kill(-pgid, sig);
            } else {
                libc::kill(pid as i32, sig);
            }
        } else {
            libc::kill(pid as i32, sig);
        }
    }

    #[cfg(not(unix))]
    {
        let _ = kill;
        let _ = owns_process_group;
        let mut sys = sysinfo::System::new();
        let sys_pid = sysinfo::Pid::from_u32(pid);
        sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[sys_pid]), true);
        if let Some(proc) = sys.process(sys_pid) {
            proc.kill();
        }
    }
}

fn write_log_banner(path: &Path, message: &str) {
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let timestamp = Utc::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(f, "[{}] {}", timestamp, message);
    }
}

fn load_recent_log_lines(path: &Path, limit: usize) -> (VecDeque<String>, u64) {
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return (VecDeque::new(), 0),
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(256 * 1024);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return (VecDeque::new(), len);
    }
    let mut buf = Vec::new();
    if file.read_to_end(&mut buf).is_err() {
        return (VecDeque::new(), len);
    }
    let text = String::from_utf8_lossy(&buf);
    let mut lines: VecDeque<String> = text.lines().map(String::from).collect();
    if start > 0 && !lines.is_empty() {
        lines.pop_front();
    }
    while lines.len() > limit {
        lines.pop_front();
    }
    (lines, len)
}

fn spawn_log_tailer(
    path: PathBuf,
    start_offset: u64,
    log_sender: broadcast::Sender<String>,
    recent_logs: Arc<Mutex<VecDeque<String>>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut offset = start_offset;
        loop {
            if let Err(err) = tail_once(&path, &mut offset, &log_sender, &recent_logs).await {
                warn!("Log tailer for {:?} failed: {}", path, err);
            }
            sleep(LOG_TAIL_POLL).await;
        }
    })
}

async fn tail_once(
    path: &Path,
    offset: &mut u64,
    log_sender: &broadcast::Sender<String>,
    recent_logs: &Arc<Mutex<VecDeque<String>>>,
) -> std::io::Result<()> {
    let meta = match tokio::fs::metadata(path).await {
        Ok(m) => m,
        Err(_) => return Ok(()),
    };
    let len = meta.len();
    if len < *offset {
        *offset = 0;
    }
    if len == *offset {
        return Ok(());
    }

    let mut file = tokio::fs::File::open(path).await?;
    file.seek(SeekFrom::Start(*offset)).await?;
    let mut reader = BufReader::new(file);

    loop {
        let mut buf = Vec::new();
        let n = reader.read_until(b'\n', &mut buf).await?;
        if n == 0 {
            break;
        }
        if !buf.ends_with(b"\n") {
            break;
        }
        *offset += n as u64;
        let line = String::from_utf8_lossy(&buf).trim_end_matches(['\n', '\r']).to_string();
        {
            let mut logs = recent_logs.lock().await;
            if logs.len() >= MAX_LOG_BUFFER_LINES {
                logs.pop_front();
            }
            logs.push_back(line.clone());
        }
        let _ = log_sender.send(line);
    }

    Ok(())
}
