use super::{find_executable, ServiceDriver};
use crate::config::{HealthCheckConfig, ProbeType, ServiceInstance, ServiceSettings};
use anyhow::Result;
use std::path::{Path, PathBuf};
use tokio::process::Command;

pub struct CustomDriver;

impl ServiceDriver for CustomDriver {
    fn prepare_config(&self, _instance: &ServiceInstance, _config_dir: &Path) -> Result<()> {
        Ok(())
    }

    fn build_command(&self, instance: &ServiceInstance, config_dir: &Path) -> Result<Command> {
        let (command, args) = if let ServiceSettings::Custom(ref s) = instance.settings {
            (s.command.clone(), s.args.clone())
        } else {
            ("echo".to_string(), vec![])
        };

        let bin_path = if let Some(ref custom_bin) = instance.bin_path {
            PathBuf::from(custom_bin)
        } else {
            find_executable(&command, &[])
        };

        let mut cmd = Command::new(bin_path);
        for arg in args {
            cmd.arg(arg);
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

    fn default_health_check(&self, _instance: &ServiceInstance) -> HealthCheckConfig {
        HealthCheckConfig {
            enabled: true,
            check_interval_secs: 10,
            timeout_secs: 3,
            consecutive_failures_threshold: 3,
            probe_type: ProbeType::Tcp,
            test_target: None,
        }
    }
}
