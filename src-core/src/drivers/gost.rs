use super::{find_executable, ServiceDriver};
use crate::config::{HealthCheckConfig, ProbeType, ServiceInstance, ServiceSettings};
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use tokio::process::Command;

pub struct GostDriver;

impl ServiceDriver for GostDriver {
    fn prepare_config(&self, instance: &ServiceInstance, config_dir: &Path) -> Result<()> {
        if let ServiceSettings::Gost(ref s) = instance.settings {
            if let Some(ref raw) = s.raw_config {
                let config_file = config_dir.join("gost.json");
                fs::write(&config_file, raw)?;
            }
        }
        Ok(())
    }

    fn build_command(&self, instance: &ServiceInstance, config_dir: &Path) -> Result<Command> {
        let bin_path = if let Some(ref custom_bin) = instance.bin_path {
            PathBuf::from(custom_bin)
        } else {
            find_executable(
                "gost",
                &[
                    "/opt/homebrew/bin/gost",
                    "/usr/local/bin/gost",
                    "/usr/bin/gost",
                ],
            )
        };

        let mut cmd = Command::new(bin_path);

        if let ServiceSettings::Gost(ref s) = instance.settings {
            if s.raw_config.is_some() {
                let config_file = config_dir.join("gost.json");
                cmd.arg("-C").arg(config_file.to_string_lossy().as_ref());
            } else {
                let listen_spec = if !s.listen_spec.is_empty() {
                    s.listen_spec.clone()
                } else {
                    format!("http://{}:{}", instance.listen_host, instance.listen_port)
                };
                cmd.arg("-L").arg(listen_spec);

                if let Some(ref fwd) = s.forward_spec {
                    if !fwd.is_empty() {
                        cmd.arg("-F").arg(fwd);
                    }
                }

                for arg in &s.extra_args {
                    cmd.arg(arg);
                }
            }
        }

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

    fn default_health_check(&self, instance: &ServiceInstance) -> HealthCheckConfig {
        let probe_type = if let ServiceSettings::Gost(ref s) = instance.settings {
            if s.listen_spec.starts_with("socks5://") {
                ProbeType::Socks5
            } else {
                ProbeType::Http
            }
        } else {
            ProbeType::Http
        };

        HealthCheckConfig {
            enabled: true,
            check_interval_secs: 10,
            timeout_secs: 3,
            consecutive_failures_threshold: 3,
            probe_type,
            test_target: None,
        }
    }

    fn get_target_ports(&self, instance: &ServiceInstance, config_dir: &Path) -> Vec<super::TargetPort> {
        let mut ports = Vec::new();
        let config_file = config_dir.join("gost.json");
        if config_file.is_file() {
            if let Ok(content) = fs::read_to_string(&config_file) {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(services) = json.get("services").and_then(|v| v.as_array()) {
                        for svc in services {
                            if let Some(addr) = svc.get("addr").and_then(|v| v.as_str()) {
                                if let Some((host, port)) = parse_gost_addr(addr) {
                                    ports.push(super::TargetPort {
                                        host,
                                        port,
                                        is_udp: false,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        if ports.is_empty() {
            if let ServiceSettings::Gost(ref s) = instance.settings {
                if !s.listen_spec.is_empty() {
                    if let Some((host, port, is_udp)) = parse_gost_spec(&s.listen_spec) {
                        ports.push(super::TargetPort { host, port, is_udp });
                    }
                }
            }
        }

        if ports.is_empty() && instance.listen_port > 0 {
            ports.push(super::TargetPort {
                host: instance.listen_host.clone(),
                port: instance.listen_port,
                is_udp: false,
            });
        }

        ports
    }
}

fn parse_gost_spec(spec: &str) -> Option<(String, u16, bool)> {
    let spec = spec.trim();
    let is_udp = spec.starts_with("udp://");
    let addr = if let Some((_proto, rest)) = spec.split_once("://") {
        rest
    } else {
        spec
    };
    let (host, port) = parse_gost_addr(addr)?;
    Some((host, port, is_udp))
}

fn parse_gost_addr(addr: &str) -> Option<(String, u16)> {
    let addr = addr.trim();
    if addr.is_empty() {
        return None;
    }
    if let Some((host, port_str)) = addr.rsplit_once(':') {
        let port: u16 = port_str.parse().ok()?;
        let host = host.trim().trim_matches(['[', ']']);
        let host = if host.is_empty() || host == "0.0.0.0" || host == "*" {
            "127.0.0.1".to_string()
        } else {
            host.to_string()
        };
        Some((host, port))
    } else {
        None
    }
}
