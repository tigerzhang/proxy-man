use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServiceType {
    Overtls,
    #[serde(alias = "overtls_chain")]
    OvertlsChain,
    #[serde(alias = "clean_dns")]
    CleanDns,
    Gost,
    Custom,
}

impl std::fmt::Display for ServiceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceType::Overtls => write!(f, "overtls"),
            ServiceType::OvertlsChain => write!(f, "overtls-chain"),
            ServiceType::CleanDns => write!(f, "clean-dns"),
            ServiceType::Gost => write!(f, "gost"),
            ServiceType::Custom => write!(f, "custom"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartPolicy {
    Always,
    OnFailure,
    Never,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        RestartPolicy::OnFailure
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeType {
    Socks5,
    Http,
    Dns,
    Tcp,
    None,
}

impl Default for ProbeType {
    fn default() -> Self {
        ProbeType::Tcp
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_probe_interval")]
    pub check_interval_secs: u64,
    #[serde(default = "default_probe_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_failures_threshold")]
    pub consecutive_failures_threshold: u32,
    #[serde(default)]
    pub probe_type: ProbeType,
    #[serde(default)]
    pub test_target: Option<String>,
}

fn default_true() -> bool {
    true
}

fn default_probe_interval() -> u64 {
    10
}

fn default_probe_timeout() -> u64 {
    3
}

fn default_failures_threshold() -> u32 {
    3
}

impl Default for HealthCheckConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            check_interval_secs: 10,
            timeout_secs: 3,
            consecutive_failures_threshold: 3,
            probe_type: ProbeType::Tcp,
            test_target: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OvertlsSettings {
    #[serde(default)]
    pub remarks: String,
    #[serde(default = "default_overtls_method")]
    pub method: String,
    #[serde(default)]
    pub password: String,
    #[serde(default = "default_tunnel_path")]
    pub tunnel_path: String,
    #[serde(default)]
    pub server_host: String,
    #[serde(default = "default_https_port")]
    pub server_port: u16,
    #[serde(default)]
    pub server_domain: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub disable_tls: bool,
    #[serde(default)]
    pub cafile: Option<String>,
    #[serde(default)]
    pub raw_json: Option<String>,
}

fn default_overtls_method() -> String {
    "none".to_string()
}

fn default_tunnel_path() -> String {
    "/secret-tunnel-path/".to_string()
}

fn default_https_port() -> u16 {
    443
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CleanDnsSettings {
    #[serde(default = "default_dns_bind")]
    pub bind: String,
    #[serde(default = "default_dns_api_port")]
    pub api_port: u16,
    #[serde(default)]
    pub upstream_dns: Vec<String>,
    #[serde(default)]
    pub socks5_proxy: Option<String>,
    /// Existing clean-dns YAML passed as `-c`. When set, ProxyMan does not generate config.yaml.
    #[serde(default)]
    pub config_path: Option<String>,
    #[serde(default)]
    pub raw_yaml: Option<String>,
}

fn default_dns_bind() -> String {
    "127.0.0.1:5353".to_string()
}

fn default_dns_api_port() -> u16 {
    3002
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GostSettings {
    #[serde(default)]
    pub listen_spec: String, // e.g. "http://:8080"
    #[serde(default)]
    pub forward_spec: Option<String>, // e.g. "socks5://127.0.0.1:1080"
    #[serde(default)]
    pub extra_args: Vec<String>,
    #[serde(default)]
    pub raw_config: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CustomSettings {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "settings")]
pub enum ServiceSettings {
    #[serde(rename = "overtls")]
    Overtls(OvertlsSettings),
    #[serde(rename = "overtls-chain")]
    OvertlsChain(OvertlsSettings),
    #[serde(rename = "clean-dns")]
    CleanDns(CleanDnsSettings),
    #[serde(rename = "gost")]
    Gost(GostSettings),
    #[serde(rename = "custom")]
    Custom(CustomSettings),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceInstance {
    pub id: String,
    pub name: String,
    pub service_type: ServiceType,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_localhost")]
    pub listen_host: String,
    pub listen_port: u16,
    #[serde(default)]
    pub bin_path: Option<String>,
    #[serde(default)]
    pub work_dir: Option<String>,
    #[serde(default)]
    pub restart_policy: RestartPolicy,
    #[serde(default = "default_max_restart_retries")]
    pub max_restart_retries: u32,
    #[serde(default = "default_restart_backoff")]
    pub restart_backoff_secs: u64,
    #[serde(default)]
    pub health_check: HealthCheckConfig,
    #[serde(default)]
    pub env_vars: HashMap<String, String>,
    pub settings: ServiceSettings,
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
    #[serde(default = "Utc::now")]
    pub updated_at: DateTime<Utc>,
}

/// Parse `host:port` / `[ipv6]:port` into components. Returns None when empty or invalid.
pub fn parse_host_port(addr: &str) -> Option<(String, u16)> {
    let addr = addr.trim();
    if addr.is_empty() {
        return None;
    }
    if let Ok(sock) = addr.parse::<SocketAddr>() {
        return Some((sock.ip().to_string(), sock.port()));
    }
    let (host, port) = addr.rsplit_once(':')?;
    let host = host.trim().trim_matches(['[', ']']);
    if host.is_empty() {
        return None;
    }
    let port: u16 = port.parse().ok()?;
    Some((host.to_string(), port))
}

impl ServiceInstance {
    /// Keep `listen_host` / `listen_port` aligned with driver-specific bind settings.
    /// For clean-dns, DNS Bind Address is the source of truth for health checks and adoption.
    pub fn apply_driver_listen_overrides(&mut self) {
        if let ServiceSettings::CleanDns(ref s) = self.settings {
            if let Some((host, port)) = parse_host_port(&s.bind) {
                self.listen_host = host;
                self.listen_port = port;
            }
        }
    }
}

fn default_localhost() -> String {
    "127.0.0.1".to_string()
}

fn default_max_restart_retries() -> u32 {
    5
}

fn default_restart_backoff() -> u64 {
    2
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    Stopped,
    Starting,
    Running,
    Degraded,
    Crashed,
    BackoffWaiting,
}

impl std::fmt::Display for RuntimeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeStatus::Stopped => write!(f, "stopped"),
            RuntimeStatus::Starting => write!(f, "starting"),
            RuntimeStatus::Running => write!(f, "running"),
            RuntimeStatus::Degraded => write!(f, "degraded"),
            RuntimeStatus::Crashed => write!(f, "crashed"),
            RuntimeStatus::BackoffWaiting => write!(f, "backoff_waiting"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceRuntimeState {
    pub id: String,
    pub status: RuntimeStatus,
    pub pid: Option<u32>,
    pub uptime_secs: Option<u64>,
    pub restart_count: u32,
    pub last_restart_time: Option<DateTime<Utc>>,
    pub last_exit_code: Option<i32>,
    pub last_error: Option<String>,
    pub latency_ms: Option<u64>,
    pub last_probe_time: Option<DateTime<Utc>>,
    pub memory_bytes: Option<u64>,
}

/// On-disk record of a process ProxyMan started, used to adopt it after a restart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedRuntime {
    pub pid: u32,
    /// True when ProxyMan spawned this process in its own process group.
    /// Stop may then signal the group; otherwise only this PID is signaled.
    #[serde(default)]
    pub owns_process_group: bool,
    #[serde(default)]
    pub listen_host: String,
    #[serde(default)]
    pub listen_port: u16,
    #[serde(default)]
    pub exe: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_type_serializes_as_kebab_case() {
        assert_eq!(
            serde_json::to_string(&ServiceType::CleanDns).unwrap(),
            r#""clean-dns""#
        );
        assert_eq!(
            serde_json::to_string(&ServiceType::OvertlsChain).unwrap(),
            r#""overtls-chain""#
        );
        assert_eq!(
            serde_json::to_string(&ServiceType::Overtls).unwrap(),
            r#""overtls""#
        );
    }

    #[test]
    fn service_type_accepts_kebab_and_legacy_snake_case() {
        assert_eq!(
            serde_json::from_str::<ServiceType>(r#""clean-dns""#).unwrap(),
            ServiceType::CleanDns
        );
        assert_eq!(
            serde_json::from_str::<ServiceType>(r#""clean_dns""#).unwrap(),
            ServiceType::CleanDns
        );
        assert_eq!(
            serde_json::from_str::<ServiceType>(r#""overtls-chain""#).unwrap(),
            ServiceType::OvertlsChain
        );
        assert_eq!(
            serde_json::from_str::<ServiceType>(r#""overtls_chain""#).unwrap(),
            ServiceType::OvertlsChain
        );
    }

    #[test]
    fn service_instance_accepts_dashboard_clean_dns_payload() {
        let json = r#"{
            "id": "local-dns",
            "name": "Clean DNS Server",
            "service_type": "clean-dns",
            "listen_port": 5353,
            "settings": { "type": "clean-dns", "settings": { "bind": "127.0.0.1:5353" } }
        }"#;
        let instance: ServiceInstance = serde_json::from_str(json).unwrap();
        assert_eq!(instance.service_type, ServiceType::CleanDns);
        assert!(matches!(instance.settings, ServiceSettings::CleanDns(_)));
    }

    #[test]
    fn service_instance_accepts_legacy_snake_case_service_type() {
        let json = r#"{
            "id": "local-dns",
            "name": "Clean DNS Server",
            "service_type": "clean_dns",
            "listen_port": 5353,
            "settings": { "type": "clean-dns", "settings": {} }
        }"#;
        let instance: ServiceInstance = serde_json::from_str(json).unwrap();
        assert_eq!(instance.service_type, ServiceType::CleanDns);
    }

    #[test]
    fn service_instance_accepts_clean_dns_config_path() {
        let json = r#"{
            "id": "local-dns",
            "name": "Clean DNS Server",
            "service_type": "clean-dns",
            "listen_port": 53,
            "settings": {
                "type": "clean-dns",
                "settings": { "config_path": "/Users/zhanghu/vpn/clean-dns/config.yaml" }
            }
        }"#;
        let instance: ServiceInstance = serde_json::from_str(json).unwrap();
        match instance.settings {
            ServiceSettings::CleanDns(s) => {
                assert_eq!(
                    s.config_path.as_deref(),
                    Some("/Users/zhanghu/vpn/clean-dns/config.yaml")
                );
            }
            other => panic!("unexpected settings: {other:?}"),
        }
    }

    #[test]
    fn parse_host_port_accepts_ipv4_ipv6_and_hostname() {
        assert_eq!(
            parse_host_port("127.0.0.1:5353"),
            Some(("127.0.0.1".to_string(), 5353))
        );
        assert_eq!(
            parse_host_port("[::1]:5353"),
            Some(("::1".to_string(), 5353))
        );
        assert_eq!(
            parse_host_port("localhost:53"),
            Some(("localhost".to_string(), 53))
        );
        assert_eq!(parse_host_port(""), None);
        assert_eq!(parse_host_port("not-an-address"), None);
    }

    #[test]
    fn apply_driver_listen_overrides_uses_clean_dns_bind() {
        let json = r#"{
            "id": "local-dns",
            "name": "Clean DNS Server",
            "service_type": "clean-dns",
            "listen_host": "0.0.0.0",
            "listen_port": 5353,
            "settings": {
                "type": "clean-dns",
                "settings": { "bind": "127.0.0.1:5454", "api_port": 4001 }
            }
        }"#;
        let mut instance: ServiceInstance = serde_json::from_str(json).unwrap();
        instance.apply_driver_listen_overrides();
        assert_eq!(instance.listen_host, "127.0.0.1");
        assert_eq!(instance.listen_port, 5454);
    }
}
