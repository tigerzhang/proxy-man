pub mod clean_dns;
pub mod custom;
pub mod gost;
pub mod overtls;
pub mod overtls_chain;

use crate::config::{HealthCheckConfig, ServiceInstance, ServiceType};
use anyhow::Result;
use std::env;
use std::path::{Path, PathBuf};
use tokio::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetPort {
    pub host: String,
    pub port: u16,
    pub is_udp: bool,
}

pub trait ServiceDriver: Send + Sync {
    fn prepare_config(&self, instance: &ServiceInstance, config_dir: &Path) -> Result<()>;
    fn build_command(&self, instance: &ServiceInstance, config_dir: &Path) -> Result<Command>;
    fn default_health_check(&self, instance: &ServiceInstance) -> HealthCheckConfig;
    fn get_target_ports(&self, instance: &ServiceInstance, _config_dir: &Path) -> Vec<TargetPort> {
        if instance.listen_port > 0 {
            vec![TargetPort {
                host: instance.listen_host.clone(),
                port: instance.listen_port,
                is_udp: false,
            }]
        } else {
            Vec::new()
        }
    }
}

pub fn get_driver(service_type: ServiceType) -> Box<dyn ServiceDriver> {
    match service_type {
        ServiceType::Overtls => Box::new(overtls::OvertlsDriver),
        ServiceType::OvertlsChain => Box::new(overtls_chain::OvertlsChainDriver),
        ServiceType::CleanDns => Box::new(clean_dns::CleanDnsDriver),
        ServiceType::Gost => Box::new(gost::GostDriver),
        ServiceType::Custom => Box::new(custom::CustomDriver),
    }
}

pub fn find_executable(name: &str, preferred_paths: &[&str]) -> PathBuf {
    for p in preferred_paths {
        let path = PathBuf::from(p);
        if path.is_file() {
            return path;
        }
    }

    if let Some(paths) = env::var_os("PATH") {
        for dir in env::split_paths(&paths) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return candidate;
            }
            #[cfg(target_os = "windows")]
            {
                let exe_candidate = dir.join(format!("{}.exe", name));
                if exe_candidate.is_file() {
                    return exe_candidate;
                }
            }
        }
    }

    PathBuf::from(name)
}
