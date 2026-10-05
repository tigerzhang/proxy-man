use super::adopt::{
    find_adoptable_pid_proto, process_exe_matches, process_is_alive, AdoptLookup,
};
use super::probe::{run_probe, ProbeResult};
use super::process::{stop_pid, PollExit, ProcessHandle};
use crate::config::{
    ConfigStore, PersistedRuntime, RestartPolicy, RuntimeStatus, ServiceInstance, ServiceRuntimeState,
};
use crate::drivers::get_driver;
use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use sysinfo::{Pid, ProcessesToUpdate, System};
use tokio::sync::{broadcast, Mutex, RwLock};
use tokio::time::sleep;
use tracing::{error, info, warn};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StartOutcome {
    Spawned {
        pid: u32,
    },
    Adopted {
        pid: u32,
        host: String,
        port: u16,
        is_udp: bool,
        exe: String,
    },
    AlreadyRunning {
        pid: u32,
    },
}

struct AdoptedTarget {
    pid: u32,
    host: String,
    port: u16,
    is_udp: bool,
}

#[derive(Clone)]
pub struct ServiceManager {
    store: ConfigStore,
    handles: Arc<RwLock<HashMap<String, ProcessHandle>>>,
    states: Arc<RwLock<HashMap<String, ServiceRuntimeState>>>,
    consecutive_failures: Arc<RwLock<HashMap<String, u32>>>,
    event_sender: broadcast::Sender<ServiceRuntimeState>,
    system_info: Arc<Mutex<System>>,
}

impl ServiceManager {
    pub fn new(store: ConfigStore) -> Self {
        let (event_sender, _) = broadcast::channel::<ServiceRuntimeState>(128);
        Self {
            store,
            handles: Arc::new(RwLock::new(HashMap::new())),
            states: Arc::new(RwLock::new(HashMap::new())),
            consecutive_failures: Arc::new(RwLock::new(HashMap::new())),
            event_sender,
            system_info: Arc::new(Mutex::new(System::new_all())),
        }
    }

    pub fn store(&self) -> &ConfigStore {
        &self.store
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<ServiceRuntimeState> {
        self.event_sender.subscribe()
    }

    pub async fn get_state(&self, id: &str) -> ServiceRuntimeState {
        let states = self.states.read().await;
        if let Some(state) = states.get(id) {
            state.clone()
        } else {
            ServiceRuntimeState {
                id: id.to_string(),
                status: RuntimeStatus::Stopped,
                pid: None,
                uptime_secs: None,
                restart_count: 0,
                last_restart_time: None,
                last_exit_code: None,
                last_error: None,
                latency_ms: None,
                last_probe_time: None,
                memory_bytes: None,
            }
        }
    }

    pub async fn list_states(&self) -> Vec<ServiceRuntimeState> {
        let instances = self.store.list().await;
        let states = self.states.read().await;

        instances
            .into_iter()
            .map(|inst| {
                if let Some(state) = states.get(&inst.id) {
                    state.clone()
                } else {
                    ServiceRuntimeState {
                        id: inst.id,
                        status: RuntimeStatus::Stopped,
                        pid: None,
                        uptime_secs: None,
                        restart_count: 0,
                        last_restart_time: None,
                        last_exit_code: None,
                        last_error: None,
                        latency_ms: None,
                        last_probe_time: None,
                        memory_bytes: None,
                    }
                }
            })
            .collect()
    }

    pub async fn start_service(&self, id: &str) -> Result<StartOutcome> {
        let instance = self
            .store
            .get(id)
            .await
            .context(format!("Service '{}' not found", id))?;

        {
            let mut handles = self.handles.write().await;
            if let Some(handle) = handles.get_mut(id) {
                if handle.is_running() {
                    info!("Service '{}' is already running (PID {})", id, handle.pid);
                    return Ok(StartOutcome::AlreadyRunning { pid: handle.pid });
                }
                handles.remove(id);
            }
        }

        let driver = get_driver(instance.service_type);
        let config_dir = self.store.get_service_config_dir(id);

        driver
            .prepare_config(&instance, &config_dir)
            .context("Failed to prepare service configuration files")?;

        let expected_exe = {
            let cmd = driver
                .build_command(&instance, &config_dir)
                .context("Failed to build command for service")?;
            PathBuf::from(cmd.as_std().get_program())
        };
        let log_file = self.store.get_log_file_path(id);
        let persisted = self.store.get_runtime(id).await;

        let target_ports = driver.get_target_ports(&instance, &config_dir);
        let mut adopted_info: Option<AdoptedTarget> = None;

        for target in &target_ports {
            match find_adoptable_pid_proto(
                persisted.as_ref(),
                &expected_exe,
                &target.host,
                target.port,
                target.is_udp,
            ) {
                AdoptLookup::Found(pid) => {
                    adopted_info = Some(AdoptedTarget {
                        pid,
                        host: target.host.clone(),
                        port: target.port,
                        is_udp: target.is_udp,
                    });
                }
                AdoptLookup::PortBusy { pid, exe } => {
                    let proto_str = if target.is_udp { "UDP" } else { "TCP" };
                    let foreign_exe = exe.as_deref().unwrap_or("unknown");
                    let port_desc = format!("{}:{} ({proto_str})", target.host, target.port);
                    let occ_desc = format!("{pid} ({foreign_exe})");
                    let exp_desc = format!("{}", expected_exe.display());
                    warn!(
                        "\n╭─ ✖ PORT CONFLICT ERROR ──────────────────────────────────────╮\n\
                         │ Service:         {id:<42} │\n\
                         │ Target Port:     {port_desc:<42} │\n\
                         │ Occupied By PID: {occ_desc:<42} │\n\
                         │ Expected Binary: {exp_desc:<42} │\n\
                         │ Cause:           Port opened by another process             │\n\
                         ╰─────────────────────────────────────────────────────────────╯"
                    );
                    anyhow::bail!(
                        "Target port {}:{} ({proto_str}) from configuration file is already in use by PID {pid} ({foreign_exe}) — not adopting (executable does not match {})",
                        target.host,
                        target.port,
                        expected_exe.display()
                    );
                }
                AdoptLookup::None => {}
            }
        }

        if let Some(adopted) = adopted_info {
            let owns_process_group = persisted
                .as_ref()
                .filter(|rt| rt.pid == adopted.pid)
                .map(|rt| rt.owns_process_group)
                .unwrap_or(false);
            let handle = ProcessHandle::adopt(
                id.to_string(),
                adopted.pid,
                owns_process_group,
                log_file,
            );
            self.handles.write().await.insert(id.to_string(), handle);
            self.persist_runtime(&instance, adopted.pid, owns_process_group, &expected_exe)
                .await;
            self.mark_running(id, adopted.pid).await;

            let proto_str = if adopted.is_udp { "UDP" } else { "TCP" };
            let pid_str = format!("{}", adopted.pid);
            let port_str = format!("{}:{} ({proto_str})", adopted.host, adopted.port);
            let exe_str = format!("{}", expected_exe.display());
            info!(
                "\n╭─ ℹ PROCESS ADOPTED ──────────────────────────────────────────╮\n\
                 │ Service:     {id:<46} │\n\
                 │ Adopted PID: {pid_str:<46} │\n\
                 │ Target Port: {port_str:<46} │\n\
                 │ Executable:  {exe_str:<46} │\n\
                 │ Status:      Attached running instance successfully          │\n\
                 ╰─────────────────────────────────────────────────────────────╯"
            );
            return Ok(StartOutcome::Adopted {
                pid: adopted.pid,
                host: adopted.host,
                port: adopted.port,
                is_udp: adopted.is_udp,
                exe: expected_exe.display().to_string(),
            });
        }

        if target_ports.is_empty() {
            if let Some(rt) = persisted.as_ref() {
                if process_is_alive(rt.pid) && process_exe_matches(rt.pid, &expected_exe) {
                    let handle = ProcessHandle::adopt(
                        id.to_string(),
                        rt.pid,
                        rt.owns_process_group,
                        log_file,
                    );
                    self.handles.write().await.insert(id.to_string(), handle);
                    self.persist_runtime(&instance, rt.pid, rt.owns_process_group, &expected_exe)
                        .await;
                    self.mark_running(id, rt.pid).await;
                    let pid_str = format!("{}", rt.pid);
                    let exe_str = format!("{}", expected_exe.display());
                    info!(
                        "\n╭─ ℹ PROCESS ADOPTED ──────────────────────────────────────────╮\n\
                         │ Service:     {id:<46} │\n\
                         │ Adopted PID: {pid_str:<46} │\n\
                         │ Executable:  {exe_str:<46} │\n\
                         │ Status:      Attached running instance successfully          │\n\
                         ╰─────────────────────────────────────────────────────────────╯"
                    );
                    return Ok(StartOutcome::Adopted {
                        pid: rt.pid,
                        host: instance.listen_host.clone(),
                        port: instance.listen_port,
                        is_udp: false,
                        exe: expected_exe.display().to_string(),
                    });
                }
            }
        }

        let cmd = driver
            .build_command(&instance, &config_dir)
            .context("Failed to build command for service")?;
        let expected_exe = PathBuf::from(cmd.as_std().get_program());

        let handle = ProcessHandle::spawn(id.to_string(), cmd, log_file)?;
        let pid = handle.pid;
        self.handles.write().await.insert(id.to_string(), handle);

        self.persist_runtime(&instance, pid, true, &expected_exe)
            .await;
        self.mark_running(id, pid).await;
        info!("Started service '{}' (PID {})", id, pid);
        Ok(StartOutcome::Spawned { pid })
    }

    pub async fn stop_service(&self, id: &str) -> Result<()> {
        let mut handles = self.handles.write().await;
        if let Some(mut handle) = handles.remove(id) {
            drop(handles);
            handle.stop(Duration::from_secs(3)).await?;
        } else {
            drop(handles);
            if let Some(rt) = self.store.get_runtime(id).await {
                stop_pid(rt.pid, rt.owns_process_group, Duration::from_secs(3)).await?;
            }
        }

        if let Err(err) = self.store.clear_runtime(id).await {
            warn!("Failed to clear persisted runtime for '{}': {}", id, err);
        }

        let mut states = self.states.write().await;
        if let Some(state) = states.get_mut(id) {
            state.status = RuntimeStatus::Stopped;
            state.pid = None;
            state.uptime_secs = None;
            state.latency_ms = None;
            state.restart_count = 0;
            let _ = self.event_sender.send(state.clone());
        }

        let mut failures = self.consecutive_failures.write().await;
        failures.remove(id);

        info!("Stopped service '{}'", id);
        Ok(())
    }

    async fn persist_runtime(
        &self,
        instance: &ServiceInstance,
        pid: u32,
        owns_process_group: bool,
        exe: &std::path::Path,
    ) {
        let record = PersistedRuntime {
            pid,
            owns_process_group,
            listen_host: instance.listen_host.clone(),
            listen_port: instance.listen_port,
            exe: exe.display().to_string(),
        };
        if let Err(err) = self.store.set_runtime(&instance.id, record).await {
            warn!(
                "Failed to persist runtime for service '{}': {}",
                instance.id, err
            );
        }
    }

    async fn mark_running(&self, id: &str, pid: u32) {
        let mut states = self.states.write().await;
        let current_restarts = states.get(id).map(|s| s.restart_count).unwrap_or(0);
        let last_restart_time = states.get(id).and_then(|s| s.last_restart_time);
        let state = ServiceRuntimeState {
            id: id.to_string(),
            status: RuntimeStatus::Running,
            pid: Some(pid),
            uptime_secs: Some(0),
            restart_count: current_restarts,
            last_restart_time,
            last_exit_code: None,
            last_error: None,
            latency_ms: None,
            last_probe_time: None,
            memory_bytes: None,
        };
        states.insert(id.to_string(), state.clone());
        let _ = self.event_sender.send(state);
    }

    pub async fn restart_service(&self, id: &str) -> Result<StartOutcome> {
        info!("Restarting service '{}'", id);
        let _ = self.stop_service(id).await;
        sleep(Duration::from_millis(200)).await;
        self.start_service(id).await
    }

    pub async fn get_logs(&self, id: &str, limit: usize) -> Vec<String> {
        let handles = self.handles.read().await;
        if let Some(handle) = handles.get(id) {
            let buf = handle.recent_logs.lock().await;
            let start = if buf.len() > limit {
                buf.len() - limit
            } else {
                0
            };
            return buf.iter().skip(start).cloned().collect();
        }

        // Fallback to reading log file on disk
        let log_file = self.store.get_log_file_path(id);
        if log_file.exists() {
            if let Ok(content) = std::fs::read_to_string(&log_file) {
                let lines: Vec<String> = content.lines().map(String::from).collect();
                let start = if lines.len() > limit {
                    lines.len() - limit
                } else {
                    0
                };
                return lines.into_iter().skip(start).collect();
            }
        }

        vec![]
    }

    pub async fn subscribe_logs(&self, id: &str) -> Option<broadcast::Receiver<String>> {
        let handles = self.handles.read().await;
        handles.get(id).map(|h| h.log_sender.subscribe())
    }

    pub async fn run_immediate_probe(&self, id: &str) -> Result<ProbeResult> {
        let instance = self
            .store
            .get(id)
            .await
            .context(format!("Service '{}' not found", id))?;

        let probe_res = run_probe(
            instance.health_check.probe_type,
            &instance.listen_host,
            instance.listen_port,
            instance.health_check.test_target.as_deref(),
            Duration::from_secs(instance.health_check.timeout_secs),
        )
        .await;

        let mut states = self.states.write().await;
        if let Some(state) = states.get_mut(id) {
            state.last_probe_time = Some(Utc::now());
            if probe_res.success {
                state.latency_ms = Some(probe_res.latency_ms);
            }
            let _ = self.event_sender.send(state.clone());
        }

        Ok(probe_res)
    }

    pub async fn trigger_auto_restart(
        &self,
        id: &str,
        exit_status: Option<i32>,
        error_msg: Option<String>,
    ) {
        let instance = match self.store.get(id).await {
            Some(inst) => inst,
            None => return,
        };

        if !instance.enabled {
            return;
        }

        let should_restart = match instance.restart_policy {
            RestartPolicy::Always => true,
            RestartPolicy::OnFailure => exit_status != Some(0),
            RestartPolicy::Never => false,
        };

        let is_infinite_loop =
            instance.restart_policy == RestartPolicy::Always || instance.max_restart_retries == 0;

        let mut states = self.states.write().await;
        let state = states.entry(id.to_string()).or_insert_with(|| ServiceRuntimeState {
            id: id.to_string(),
            status: RuntimeStatus::Crashed,
            pid: None,
            uptime_secs: None,
            restart_count: 0,
            last_restart_time: None,
            last_exit_code: exit_status,
            last_error: error_msg.clone(),
            latency_ms: None,
            last_probe_time: None,
            memory_bytes: None,
        });

        state.last_exit_code = exit_status;
        state.pid = None;
        state.uptime_secs = None;
        state.latency_ms = None;
        if let Some(ref err) = error_msg {
            state.last_error = Some(err.clone());
        }

        let can_retry =
            should_restart && (is_infinite_loop || state.restart_count < instance.max_restart_retries);

        if can_retry {
            state.restart_count = state.restart_count.saturating_add(1);
            state.last_restart_time = Some(Utc::now());
            state.status = RuntimeStatus::BackoffWaiting;
            let _ = self.event_sender.send(state.clone());

            let base_delay = if instance.restart_backoff_secs == 0 {
                2
            } else {
                instance.restart_backoff_secs
            };
            let max_delay = base_delay.max(60);
            let backoff_secs = (base_delay
                * (1 << (state.restart_count.saturating_sub(1)).min(5)))
            .min(max_delay);

            if is_infinite_loop {
                warn!(
                    "Service '{}' will restart in {}s (attempt {}, loop restart with delay)",
                    id, backoff_secs, state.restart_count
                );
            } else {
                warn!(
                    "Service '{}' will restart in {}s (attempt {}/{})",
                    id, backoff_secs, state.restart_count, instance.max_restart_retries
                );
            }
            drop(states);

            let self_clone = Arc::new(self.clone());
            let inst_id = id.to_string();
            tokio::spawn(async move {
                let mut current_delay = backoff_secs;
                loop {
                    sleep(Duration::from_secs(current_delay)).await;

                    // Abort if service was manually stopped or disabled
                    let instance = match self_clone.store.get(&inst_id).await {
                        Some(inst) if inst.enabled => inst,
                        _ => return,
                    };
                    {
                        let states = self_clone.states.read().await;
                        if let Some(s) = states.get(&inst_id) {
                            if s.status == RuntimeStatus::Stopped {
                                return;
                            }
                        }
                    }

                    match self_clone.start_service(&inst_id).await {
                        Ok(_) => {
                            info!("Auto-restart of service '{}' succeeded", inst_id);
                            return;
                        }
                        Err(e) => {
                            error!("Failed to auto-restart service '{}': {}", inst_id, e);
                            let is_infinite_loop = instance.restart_policy == RestartPolicy::Always
                                || instance.max_restart_retries == 0;

                            let mut states = self_clone.states.write().await;
                            let state = states.entry(inst_id.clone()).or_insert_with(|| ServiceRuntimeState {
                                id: inst_id.clone(),
                                status: RuntimeStatus::Crashed,
                                pid: None,
                                uptime_secs: None,
                                restart_count: 0,
                                last_restart_time: None,
                                last_exit_code: None,
                                last_error: Some(format!("Auto-restart failed: {}", e)),
                                latency_ms: None,
                                last_probe_time: None,
                                memory_bytes: None,
                            });

                            state.last_error = Some(format!("Auto-restart failed: {}", e));

                            let can_retry = is_infinite_loop || state.restart_count < instance.max_restart_retries;
                            if can_retry {
                                state.restart_count = state.restart_count.saturating_add(1);
                                state.last_restart_time = Some(Utc::now());
                                state.status = RuntimeStatus::BackoffWaiting;
                                let _ = self_clone.event_sender.send(state.clone());

                                let base_delay = if instance.restart_backoff_secs == 0 {
                                    2
                                } else {
                                    instance.restart_backoff_secs
                                };
                                let max_delay = base_delay.max(60);
                                current_delay = (base_delay
                                    * (1 << (state.restart_count.saturating_sub(1)).min(5)))
                                .min(max_delay);

                                if is_infinite_loop {
                                    warn!(
                                        "Service '{}' will retry restart in {}s (attempt {}, loop restart with delay)",
                                        inst_id, current_delay, state.restart_count
                                    );
                                } else {
                                    warn!(
                                        "Service '{}' will retry restart in {}s (attempt {}/{})",
                                        inst_id, current_delay, state.restart_count, instance.max_restart_retries
                                    );
                                }
                            } else {
                                state.status = RuntimeStatus::Crashed;
                                warn!(
                                    "Service '{}' exceeded max restarts or restart policy is Never",
                                    inst_id
                                );
                                let _ = self_clone.event_sender.send(state.clone());
                                return;
                            }
                        }
                    }
                }
            });
        } else {
            state.status = RuntimeStatus::Crashed;
            warn!(
                "Service '{}' exceeded max restarts or restart policy is Never",
                id
            );
            let _ = self.event_sender.send(state.clone());
        }
    }

    pub fn start_watchdog(self: Arc<Self>) {
        tokio::spawn(async move {
            info!("Starting ProxyMan supervisor watchdog loop");
            let mut tick_interval = tokio::time::interval(Duration::from_secs(2));

            loop {
                tick_interval.tick().await;

                let instances = self.store.list().await;
                let mut system = self.system_info.lock().await;
                system.refresh_processes(ProcessesToUpdate::All, true);

                for instance in instances {
                    let id = instance.id.clone();
                    let mut process_died = false;
                    let mut exit_status = None;
                    let mut pid_opt = None;

                    // 1. Check process liveness
                    {
                        let mut handles = self.handles.write().await;
                        if let Some(handle) = handles.get_mut(&id) {
                            pid_opt = Some(handle.pid);
                            match handle.poll_exit() {
                                Ok(PollExit::Exited { code }) => {
                                    process_died = true;
                                    exit_status = code;
                                    warn!(
                                        "Detected termination of service '{}' (PID {}) with code {:?}",
                                        id, handle.pid, exit_status
                                    );
                                }
                                Ok(PollExit::Alive) => {}
                                Err(err) => {
                                    error!("Error checking child process for '{}': {}", id, err);
                                    process_died = true;
                                }
                            }
                        }
                    }

                    if process_died {
                        let mut handles = self.handles.write().await;
                        handles.remove(&id);
                        drop(handles);
                        if let Err(err) = self.store.clear_runtime(&id).await {
                            warn!("Failed to clear persisted runtime for '{}': {}", id, err);
                        }

                        self.trigger_auto_restart(
                            &id,
                            exit_status,
                            Some("Process terminated unexpectedly".to_string()),
                        )
                        .await;
                    } else if let Some(pid) = pid_opt {
                        // 2. Process is running: update memory & uptime
                        let mut states = self.states.write().await;
                        if let Some(state) = states.get_mut(&id) {
                            state.status = RuntimeStatus::Running;
                            let sysinfo_pid = Pid::from_u32(pid);
                            if let Some(proc) = system.process(sysinfo_pid) {
                                state.memory_bytes = Some(proc.memory());
                                let run_time = proc.run_time();
                                state.uptime_secs = Some(run_time);
                                // Reset restart count once running stably for at least 30s
                                if run_time >= 30 && state.restart_count > 0 {
                                    state.restart_count = 0;
                                }
                            }
                        }
                        drop(states);

                        // 3. Health check probe
                        if instance.health_check.enabled {
                            let probe_res = run_probe(
                                instance.health_check.probe_type,
                                &instance.listen_host,
                                instance.listen_port,
                                instance.health_check.test_target.as_deref(),
                                Duration::from_secs(instance.health_check.timeout_secs),
                            )
                            .await;

                            let mut states = self.states.write().await;
                            let mut failures = self.consecutive_failures.write().await;
                            let fail_count = failures.entry(id.clone()).or_insert(0);

                            if let Some(state) = states.get_mut(&id) {
                                state.last_probe_time = Some(Utc::now());
                                if probe_res.success {
                                    state.latency_ms = Some(probe_res.latency_ms);
                                    *fail_count = 0;
                                    state.status = RuntimeStatus::Running;
                                    if state.restart_count > 0 {
                                        state.restart_count = 0;
                                    }
                                } else {
                                    *fail_count += 1;
                                    warn!(
                                        "Service '{}' probe failed (failures: {}/{})",
                                        id, *fail_count, instance.health_check.consecutive_failures_threshold
                                    );
                                    if *fail_count >= instance.health_check.consecutive_failures_threshold {
                                        state.status = RuntimeStatus::Degraded;
                                        state.last_error = probe_res.error.clone();

                                        // Restart degraded service if policy allows
                                        let is_infinite_loop = instance.restart_policy == RestartPolicy::Always
                                            || instance.max_restart_retries == 0;
                                        let can_retry = instance.restart_policy != RestartPolicy::Never
                                            && (is_infinite_loop || state.restart_count < instance.max_restart_retries);

                                        if can_retry {
                                            warn!(
                                                "Service '{}' is degraded; triggering auto-restart",
                                                id
                                            );
                                            *fail_count = 0;
                                            let self_clone = Arc::clone(&self);
                                            let inst_id = id.clone();
                                            tokio::spawn(async move {
                                                let _ = self_clone.restart_service(&inst_id).await;
                                            });
                                        }
                                    }
                                }
                                let _ = self.event_sender.send(state.clone());
                            }
                        }
                    }
                }
            }
        });
    }
}
