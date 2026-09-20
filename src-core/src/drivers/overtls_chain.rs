use super::{find_executable, ServiceDriver};
use crate::config::{HealthCheckConfig, ProbeType, ServiceInstance, ServiceSettings};
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use tokio::process::Command;

pub struct OvertlsChainDriver;

impl ServiceDriver for OvertlsChainDriver {
    fn prepare_config(&self, instance: &ServiceInstance, config_dir: &Path) -> Result<()> {
        let config_file = config_dir.join("chain_config.json");

        if let ServiceSettings::OvertlsChain(ref s) = instance.settings {
            if let Some(ref raw) = s.raw_json {
                fs::write(&config_file, raw)?;
                return Ok(());
            }

            // If empty, generate standard template
            let doc = serde_json::json!({
                "remarks": &instance.name,
                "tunnel_path": s.tunnel_path,
                "password": s.password,
                "client_settings": {
                    "listen_host": instance.listen_host,
                    "listen_port": instance.listen_port,
                    "server_host": s.server_host,
                    "server_port": s.server_port,
                }
            });
            let content = serde_json::to_string_pretty(&doc)?;
            fs::write(&config_file, content).with_context(|| {
                format!("Failed to write overtls-chain config to {:?}", config_file)
            })?;
        }
        Ok(())
    }

    fn build_command(&self, instance: &ServiceInstance, config_dir: &Path) -> Result<Command> {
        let bin_path = if let Some(ref custom_bin) = instance.bin_path {
            PathBuf::from(custom_bin)
        } else {
            find_executable(
                "overtls-chain",
                &[
                    "/Users/zhanghu/vpn/overtls-chain/target/release/overtls-chain",
                    "/Users/zhanghu/vpn/overtls-chain/target/debug/overtls-chain",
                    "/usr/local/bin/overtls-chain",
                    "/opt/homebrew/bin/overtls-chain",
                ],
            )
        };

        let config_file = config_dir.join("chain_config.json");
        let mut cmd = Command::new(bin_path);
        cmd.arg("-c").arg(config_file.to_string_lossy().as_ref());

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
}
