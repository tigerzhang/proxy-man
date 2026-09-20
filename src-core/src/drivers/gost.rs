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
}
