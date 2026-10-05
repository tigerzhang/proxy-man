use super::{find_executable, ServiceDriver};
use crate::config::{HealthCheckConfig, ProbeType, ServiceInstance, ServiceSettings};
use anyhow::{Context, Result};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use tokio::process::Command;

pub struct OvertlsDriver;

impl ServiceDriver for OvertlsDriver {
    fn prepare_config(&self, instance: &ServiceInstance, config_dir: &Path) -> Result<()> {
        let config_file = config_dir.join("config.json");

        if let ServiceSettings::Overtls(ref s) = instance.settings {
            if let Some(ref raw) = s.raw_json {
                fs::write(&config_file, raw)?;
                return Ok(());
            }

            let server_domain = s
                .server_domain
                .clone()
                .filter(|d| !d.is_empty())
                .unwrap_or_else(|| s.server_host.clone());

            let doc = json!({
                "remarks": if s.remarks.is_empty() { &instance.name } else { &s.remarks },
                "method": s.method,
                "password": s.password,
                "tunnel_path": s.tunnel_path,
                "client_settings": {
                    "disable_tls": s.disable_tls,
                    "client_id": s.client_id.clone().unwrap_or_default(),
                    "server_host": s.server_host,
                    "server_port": s.server_port,
                    "server_domain": server_domain,
                    "cafile": s.cafile.clone().unwrap_or_default(),
                    "listen_host": instance.listen_host,
                    "listen_port": instance.listen_port,
                }
            });

            let content = serde_json::to_string_pretty(&doc)?;
            fs::write(&config_file, content)
                .with_context(|| format!("Failed to write overtls config to {:?}", config_file))?;
        }
        Ok(())
    }

    fn build_command(&self, instance: &ServiceInstance, config_dir: &Path) -> Result<Command> {
        let bin_path = if let Some(ref custom_bin) = instance.bin_path {
            PathBuf::from(custom_bin)
        } else {
            find_executable(
                "overtls",
                &[
                    "/Users/zhanghu/vpn/overtls/target/release/overtls",
                    "/Users/zhanghu/vpn/overtls/target/debug/overtls",
                    "/usr/local/bin/overtls",
                    "/opt/homebrew/bin/overtls",
                ],
            )
        };

        let config_file = config_dir.join("config.json");
        let mut cmd = Command::new(bin_path);
        cmd.arg("-r")
            .arg("client")
            .arg("-c")
            .arg(config_file.to_string_lossy().as_ref());

        if let Some(ref work_dir) = instance.work_dir {
            cmd.current_dir(work_dir);
        } else {
            cmd.current_dir(config_dir);
        }

        for (k, v) in &instance.env_vars {
            cmd.env(k, v);
        }

        Ok(cmd)
    }

    fn default_health_check(&self, _instance: &ServiceInstance) -> HealthCheckConfig {
        HealthCheckConfig {
            enabled: true,
            check_interval_secs: 10,
            timeout_secs: 3,
            consecutive_failures_threshold: 3,
            probe_type: ProbeType::Socks5,
            test_target: None,
        }
    }

    fn get_target_ports(&self, instance: &ServiceInstance, config_dir: &Path) -> Vec<super::TargetPort> {
        let config_file = config_dir.join("config.json");
        if config_file.is_file() {
            if let Ok(content) = fs::read_to_string(&config_file) {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(client_settings) = json.get("client_settings") {
                        let host = client_settings
                            .get("listen_host")
                            .and_then(|v| v.as_str())
                            .unwrap_or(&instance.listen_host)
                            .to_string();
                        let port = client_settings
                            .get("listen_port")
                            .and_then(|v| v.as_u64())
                            .map(|p| p as u16)
                            .unwrap_or(instance.listen_port);
                        if port > 0 {
                            return vec![super::TargetPort {
                                host,
                                port,
                                is_udp: false,
                            }];
                        }
                    }
                }
            }
        }

        if instance.listen_port > 0 {
            vec![super::TargetPort {
                host: instance.listen_host.clone(),
                port: instance.listen_port,
                is_udp: false,
            }]
        } else {
            Vec::new()
        }
    }
}
