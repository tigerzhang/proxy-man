use proxy_man_core::config::{
    CleanDnsSettings, ConfigStore, CustomSettings, GostSettings, HealthCheckConfig, OvertlsSettings,
    PersistedRuntime, ProbeType, RestartPolicy, RuntimeStatus, ServiceInstance, ServiceSettings,
    ServiceType,
};
use proxy_man_core::drivers::get_driver;
use proxy_man_core::supervisor::{find_listening_pids, process_is_alive, ServiceManager};
use std::time::Duration;
use tempfile::tempdir;

#[test]
fn test_overtls_driver_generates_valid_config() {
    let dir = tempdir().unwrap();
    let instance = ServiceInstance {
        id: "test-overtls".to_string(),
        name: "Test Node".to_string(),
        service_type: ServiceType::Overtls,
        enabled: true,
        listen_host: "127.0.0.1".to_string(),
        listen_port: 1088,
        bin_path: None,
        work_dir: None,
        restart_policy: RestartPolicy::OnFailure,
        max_restart_retries: 5,
        restart_backoff_secs: 2,
        health_check: HealthCheckConfig::default(),
        env_vars: std::collections::HashMap::new(),
        settings: ServiceSettings::Overtls(OvertlsSettings {
            remarks: "Test Remarks".to_string(),
            server_host: "1.2.3.4".to_string(),
            server_port: 443,
            password: "mypassword".to_string(),
            tunnel_path: "/test-path/".to_string(),
            client_id: Some("uuid-1234".to_string()),
            server_domain: Some("example.com".to_string()),
            disable_tls: false,
            cafile: None,
            raw_json: None,
            method: "none".to_string(),
        }),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    let driver = get_driver(ServiceType::Overtls);
    driver.prepare_config(&instance, dir.path()).unwrap();

    let conf_file = dir.path().join("config.json");
    assert!(conf_file.exists());

    let content = std::fs::read_to_string(&conf_file).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();

    assert_eq!(parsed["password"], "mypassword");
    assert_eq!(parsed["tunnel_path"], "/test-path/");
    assert_eq!(parsed["client_settings"]["listen_port"], 1088);
    assert_eq!(parsed["client_settings"]["server_domain"], "example.com");
}

#[test]
fn test_clean_dns_driver_generates_yaml() {
    let dir = tempdir().unwrap();
    let instance = ServiceInstance {
        id: "test-dns".to_string(),
        name: "Test DNS".to_string(),
        service_type: ServiceType::CleanDns,
        enabled: true,
        listen_host: "127.0.0.1".to_string(),
        listen_port: 5353,
        bin_path: None,
        work_dir: None,
        restart_policy: RestartPolicy::OnFailure,
        max_restart_retries: 5,
        restart_backoff_secs: 2,
        health_check: HealthCheckConfig::default(),
        env_vars: std::collections::HashMap::new(),
        settings: ServiceSettings::CleanDns(CleanDnsSettings {
            bind: "10.0.0.1:5454".to_string(),
            api_port: 4001,
            upstream_dns: vec!["8.8.8.8".to_string()],
            socks5_proxy: None,
            config_path: None,
            raw_yaml: None,
        }),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    let driver = get_driver(ServiceType::CleanDns);
    driver.prepare_config(&instance, dir.path()).unwrap();

    let conf_file = dir.path().join("config.yaml");
    assert!(conf_file.exists());

    let content = std::fs::read_to_string(&conf_file).unwrap();
    assert!(
        content.contains("bind: \"10.0.0.1:5454\""),
        "generated YAML must use DNS Bind Address, not listen_host:listen_port: {content}"
    );
    assert!(
        content.contains("api_port: 4001"),
        "generated YAML must use Clean-DNS API Port: {content}"
    );
    assert!(!content.contains("127.0.0.1:5353"));

    let cmd = driver.build_command(&instance, dir.path()).unwrap();
    let std_cmd = cmd.as_std();
    let args: Vec<String> = std_cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args[0], "run");
    assert_eq!(args[1], "-c");
    assert_eq!(args[2], conf_file.to_string_lossy());
    assert_eq!(std_cmd.get_current_dir(), Some(dir.path()));
}

fn clean_dns_instance(config_path: Option<String>) -> ServiceInstance {
    ServiceInstance {
        id: "test-dns".to_string(),
        name: "Test DNS".to_string(),
        service_type: ServiceType::CleanDns,
        enabled: true,
        listen_host: "127.0.0.1".to_string(),
        listen_port: 5353,
        bin_path: None,
        work_dir: None,
        restart_policy: RestartPolicy::OnFailure,
        max_restart_retries: 5,
        restart_backoff_secs: 2,
        health_check: HealthCheckConfig::default(),
        env_vars: std::collections::HashMap::new(),
        settings: ServiceSettings::CleanDns(CleanDnsSettings {
            bind: "127.0.0.1:5353".to_string(),
            api_port: 3002,
            upstream_dns: vec![],
            socks5_proxy: None,
            config_path,
            raw_yaml: None,
        }),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

#[test]
fn test_clean_dns_driver_uses_external_config_path() {
    let dir = tempdir().unwrap();
    let generated_dir = dir.path().join("generated");
    std::fs::create_dir(&generated_dir).unwrap();

    let config_file = dir.path().join("my-config.yaml");
    let original = r#"bind: "127.0.0.1:53"
api_port: 13002
entry: main
plugins:
  - tag: main
    type: sequence
    args:
      - exec: $upstream
"#;
    std::fs::write(&config_file, original).unwrap();

    let instance = clean_dns_instance(Some(config_file.to_string_lossy().into_owned()));
    let driver = get_driver(ServiceType::CleanDns);

    driver.prepare_config(&instance, &generated_dir).unwrap();

    let runtime_config = generated_dir.join("config.yaml");
    assert!(
        runtime_config.exists(),
        "external config_path should still write a runtime config with dashboard bind/api_port"
    );
    let content = std::fs::read_to_string(&runtime_config).unwrap();
    let bind_line = content
        .lines()
        .find(|l| l.trim_start().starts_with("bind:"))
        .unwrap_or("");
    let api_line = content
        .lines()
        .find(|l| l.trim_start().starts_with("api_port:"))
        .unwrap_or("");
    assert!(
        bind_line.contains("127.0.0.1:5353"),
        "dashboard DNS Bind Address must overlay YAML bind: {content}"
    );
    assert!(
        !bind_line.contains("127.0.0.1:53\"") && !bind_line.ends_with("127.0.0.1:53"),
        "original YAML bind must not win over dashboard: {content}"
    );
    assert!(
        api_line.contains("3002"),
        "dashboard Clean-DNS API Port must overlay YAML api_port: {content}"
    );
    assert!(
        !api_line.contains("13002"),
        "original YAML api_port must not win over dashboard: {content}"
    );
    assert!(
        content.contains("exec: $upstream"),
        "plugin/routing YAML must be preserved: {content}"
    );
    assert_eq!(
        std::fs::read_to_string(&config_file).unwrap(),
        original,
        "the user's config file must not be mutated"
    );

    let cmd = driver.build_command(&instance, &generated_dir).unwrap();
    let std_cmd = cmd.as_std();
    let args: Vec<String> = std_cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args,
        vec!["run", "-c", runtime_config.to_string_lossy().as_ref()]
    );
    assert_eq!(std_cmd.get_current_dir(), Some(dir.path()));
}

#[test]
fn test_clean_dns_driver_rejects_missing_config_path() {
    let dir = tempdir().unwrap();
    let missing = dir.path().join("does-not-exist.yaml");
    let instance = clean_dns_instance(Some(missing.to_string_lossy().into_owned()));
    let driver = get_driver(ServiceType::CleanDns);

    let err = driver
        .prepare_config(&instance, dir.path())
        .unwrap_err()
        .to_string();
    assert!(err.contains("config file not found"), "unexpected error: {err}");
}

#[tokio::test]
async fn test_config_store_syncs_listen_from_clean_dns_bind() {
    let temp = tempdir().unwrap();
    let store = ConfigStore::new(Some(temp.path().to_path_buf())).unwrap();
    let mut instance = clean_dns_instance(None);
    instance.listen_host = "0.0.0.0".to_string();
    instance.listen_port = 5353;
    if let ServiceSettings::CleanDns(ref mut s) = instance.settings {
        s.bind = "127.0.0.1:5454".to_string();
    }

    store.add(instance).await.unwrap();
    let stored = store.get("test-dns").await.unwrap();
    assert_eq!(stored.listen_host, "127.0.0.1");
    assert_eq!(stored.listen_port, 5454);
}

#[tokio::test]
async fn test_config_store_crud() {
    let temp = tempdir().unwrap();
    let store = ConfigStore::new(Some(temp.path().to_path_buf())).unwrap();

    let instance = ServiceInstance {
        id: "my-gost".to_string(),
        name: "My Gost".to_string(),
        service_type: ServiceType::Gost,
        enabled: true,
        listen_host: "127.0.0.1".to_string(),
        listen_port: 8080,
        bin_path: None,
        work_dir: None,
        restart_policy: RestartPolicy::Always,
        max_restart_retries: 3,
        restart_backoff_secs: 1,
        health_check: HealthCheckConfig {
            enabled: true,
            check_interval_secs: 5,
            timeout_secs: 2,
            consecutive_failures_threshold: 2,
            probe_type: ProbeType::Http,
            test_target: None,
        },
        env_vars: std::collections::HashMap::new(),
        settings: ServiceSettings::Gost(GostSettings {
            listen_spec: "http://:8080".to_string(),
            forward_spec: Some("socks5://127.0.0.1:1080".to_string()),
            extra_args: vec![],
            raw_config: None,
        }),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    // Add
    store.add(instance.clone()).await.unwrap();
    let list = store.list().await;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, "my-gost");

    // Get
    let fetched = store.get("my-gost").await.unwrap();
    assert_eq!(fetched.name, "My Gost");

    // Remove
    let removed = store.remove("my-gost").await.unwrap();
    assert!(removed.is_some());
    assert_eq!(store.list().await.len(), 0);
}

fn sleep_instance(id: &str, listen_port: u16) -> ServiceInstance {
    ServiceInstance {
        id: id.to_string(),
        name: id.to_string(),
        service_type: ServiceType::Custom,
        enabled: true,
        listen_host: "127.0.0.1".to_string(),
        listen_port,
        bin_path: None,
        work_dir: None,
        restart_policy: RestartPolicy::Never,
        max_restart_retries: 0,
        restart_backoff_secs: 1,
        health_check: HealthCheckConfig {
            enabled: false,
            check_interval_secs: 10,
            timeout_secs: 3,
            consecutive_failures_threshold: 3,
            probe_type: ProbeType::None,
            test_target: None,
        },
        env_vars: std::collections::HashMap::new(),
        settings: ServiceSettings::Custom(CustomSettings {
            command: "sleep".to_string(),
            args: vec!["30".to_string()],
        }),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

#[cfg(unix)]
struct KillOnDrop(u32);

#[cfg(unix)]
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = std::process::Command::new("kill")
            .arg("-KILL")
            .arg(self.0.to_string())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

#[tokio::test]
async fn test_runtime_persist_roundtrip() {
    let temp = tempdir().unwrap();
    let store = ConfigStore::new(Some(temp.path().to_path_buf())).unwrap();
    store
        .set_runtime(
            "dns",
            PersistedRuntime {
                pid: 4242,
                owns_process_group: true,
                listen_host: "127.0.0.1".to_string(),
                listen_port: 5353,
                exe: "/bin/sleep".to_string(),
            },
        )
        .await
        .unwrap();
    drop(store);

    let store = ConfigStore::new(Some(temp.path().to_path_buf())).unwrap();
    let rt = store.get_runtime("dns").await.expect("runtime record");
    assert_eq!(rt.pid, 4242);
    assert!(rt.owns_process_group);
    assert_eq!(rt.listen_port, 5353);
    assert_eq!(rt.exe, "/bin/sleep");
}

#[cfg(unix)]
#[tokio::test]
async fn test_adopts_process_after_manager_restart() {
    let temp = tempdir().unwrap();
    let store = ConfigStore::new(Some(temp.path().to_path_buf())).unwrap();
    store.add(sleep_instance("adopt-me", 61901)).await.unwrap();

    let mgr1 = ServiceManager::new(store.clone());
    mgr1.start_service("adopt-me").await.unwrap();
    let pid = mgr1.get_state("adopt-me").await.pid.expect("pid");
    let _guard = KillOnDrop(pid);
    assert!(process_is_alive(pid));
    assert_eq!(mgr1.get_state("adopt-me").await.status, RuntimeStatus::Running);

    drop(mgr1);
    assert!(
        process_is_alive(pid),
        "child must survive ServiceManager drop"
    );

    let store2 = ConfigStore::new(Some(temp.path().to_path_buf())).unwrap();
    let persisted = store2.get_runtime("adopt-me").await.expect("persisted pid");
    assert_eq!(persisted.pid, pid);

    let mgr2 = ServiceManager::new(store2);
    mgr2.start_service("adopt-me").await.unwrap();
    let state2 = mgr2.get_state("adopt-me").await;
    assert_eq!(state2.pid, Some(pid), "should adopt the leftover PID");
    assert_eq!(state2.status, RuntimeStatus::Running);

    mgr2.stop_service("adopt-me").await.unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(!process_is_alive(pid), "stop after adopt should kill the process");
}

#[cfg(unix)]
#[tokio::test]
async fn test_stale_pid_spawns_new_process() {
    let temp = tempdir().unwrap();
    let store = ConfigStore::new(Some(temp.path().to_path_buf())).unwrap();
    store.add(sleep_instance("stale-pid", 61902)).await.unwrap();
    store
        .set_runtime(
            "stale-pid",
            PersistedRuntime {
                pid: 2_000_000_000,
                owns_process_group: true,
                listen_host: "127.0.0.1".to_string(),
                listen_port: 61902,
                exe: "sleep".to_string(),
            },
        )
        .await
        .unwrap();

    let mgr = ServiceManager::new(store);
    mgr.start_service("stale-pid").await.unwrap();
    let pid = mgr.get_state("stale-pid").await.pid.expect("pid");
    let _guard = KillOnDrop(pid);
    assert_ne!(pid, 2_000_000_000);
    assert!(process_is_alive(pid));

    mgr.stop_service("stale-pid").await.unwrap();
}

#[cfg(unix)]
#[test]
fn test_find_listening_pids_own_socket() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let pids = find_listening_pids("127.0.0.1", port);
    assert!(
        pids.contains(&std::process::id()),
        "expected current pid in {pids:?} for port {port}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_start_refuses_foreign_listener() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    if find_listening_pids("127.0.0.1", port).is_empty() {
        eprintln!("skip: lsof did not report the test listener on port {port}");
        return;
    }

    let temp = tempdir().unwrap();
    let store = ConfigStore::new(Some(temp.path().to_path_buf())).unwrap();
    store.add(sleep_instance("port-busy", port)).await.unwrap();
    let mgr = ServiceManager::new(store);
    let err = mgr
        .start_service("port-busy")
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("already in use"),
        "expected port-busy error, got: {err}"
    );
}

#[tokio::test]
async fn test_loop_restart_with_zero_max_retries() {
    let temp = tempdir().unwrap();
    let store = ConfigStore::new(Some(temp.path().to_path_buf())).unwrap();

    let mut instance = sleep_instance("loop-svc", 61990);
    instance.restart_policy = RestartPolicy::OnFailure;
    instance.max_restart_retries = 0; // 0 = unlimited loop restart
    instance.restart_backoff_secs = 1;
    store.add(instance).await.unwrap();

    let mgr = ServiceManager::new(store);

    // Call trigger_auto_restart when restart_count is already high
    {
        let mut states = mgr.get_state("loop-svc").await;
        states.restart_count = 10;
        // manually put it in state
        let _ = mgr.trigger_auto_restart("loop-svc", Some(1), Some("test error".to_string())).await;
    }

    let state = mgr.get_state("loop-svc").await;
    assert_eq!(state.status, RuntimeStatus::BackoffWaiting);
    assert_eq!(state.restart_count, 1); // was 0 initially in store state, incremented to 1
}

#[tokio::test]
async fn test_restart_policy_always_loops_indefinitely() {
    let temp = tempdir().unwrap();
    let store = ConfigStore::new(Some(temp.path().to_path_buf())).unwrap();

    let mut instance = sleep_instance("always-svc", 61991);
    instance.restart_policy = RestartPolicy::Always;
    instance.max_restart_retries = 3; // Even with 3, Always keeps looping
    instance.restart_backoff_secs = 1;
    store.add(instance).await.unwrap();

    let mgr = ServiceManager::new(store);

    // Simulate 3 failures
    for _ in 0..5 {
        mgr.trigger_auto_restart("always-svc", Some(1), Some("crash".to_string())).await;
    }

    let state = mgr.get_state("always-svc").await;
    assert_eq!(state.status, RuntimeStatus::BackoffWaiting);
    assert_eq!(state.restart_count, 5);
}

#[tokio::test]
async fn test_restart_policy_on_failure_respects_max_retries() {
    let temp = tempdir().unwrap();
    let store = ConfigStore::new(Some(temp.path().to_path_buf())).unwrap();

    let mut instance = sleep_instance("limited-svc", 61992);
    instance.restart_policy = RestartPolicy::OnFailure;
    instance.max_restart_retries = 2; // Limited retries
    instance.restart_backoff_secs = 1;
    store.add(instance).await.unwrap();

    let mgr = ServiceManager::new(store);

    // 1st retry
    mgr.trigger_auto_restart("limited-svc", Some(1), None).await;
    let s1 = mgr.get_state("limited-svc").await;
    assert_eq!(s1.status, RuntimeStatus::BackoffWaiting);
    assert_eq!(s1.restart_count, 1);

    // 2nd retry
    mgr.trigger_auto_restart("limited-svc", Some(1), None).await;
    let s2 = mgr.get_state("limited-svc").await;
    assert_eq!(s2.status, RuntimeStatus::BackoffWaiting);
    assert_eq!(s2.restart_count, 2);

    // 3rd failure exceeds max retries
    mgr.trigger_auto_restart("limited-svc", Some(1), None).await;
    let s3 = mgr.get_state("limited-svc").await;
    assert_eq!(s3.status, RuntimeStatus::Crashed);
}

