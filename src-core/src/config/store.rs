use super::service::{PersistedRuntime, ServiceInstance};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};

#[derive(Clone)]
pub struct ConfigStore {
    base_dir: PathBuf,
    services_file: PathBuf,
    runtime_file: PathBuf,
    configs_dir: PathBuf,
    logs_dir: PathBuf,
    instances: Arc<RwLock<Vec<ServiceInstance>>>,
    runtime: Arc<RwLock<HashMap<String, PersistedRuntime>>>,
}

impl ConfigStore {
    pub fn new(custom_base_dir: Option<PathBuf>) -> Result<Self> {
        let base_dir = if let Some(dir) = custom_base_dir {
            dir
        } else {
            let user_config = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
            user_config.join("proxy-man")
        };

        let services_file = base_dir.join("services.json");
        let runtime_file = base_dir.join("runtime.json");
        let configs_dir = base_dir.join("configs");
        let logs_dir = base_dir.join("logs");

        fs::create_dir_all(&base_dir)
            .with_context(|| format!("Failed to create base dir {:?}", base_dir))?;
        fs::create_dir_all(&configs_dir)
            .with_context(|| format!("Failed to create configs dir {:?}", configs_dir))?;
        fs::create_dir_all(&logs_dir)
            .with_context(|| format!("Failed to create logs dir {:?}", logs_dir))?;

        let mut instances = Vec::new();
        if services_file.exists() {
            let data = fs::read_to_string(&services_file)
                .with_context(|| format!("Failed to read {:?}", services_file))?;
            match serde_json::from_str::<Vec<ServiceInstance>>(&data) {
                Ok(mut parsed) => {
                    for instance in &mut parsed {
                        instance.apply_driver_listen_overrides();
                    }
                    info!("Loaded {} services from {:?}", parsed.len(), services_file);
                    instances = parsed;
                }
                Err(err) => {
                    error!(
                        "Failed to parse services.json: {}. Backing up corrupt file.",
                        err
                    );
                    let backup = services_file.with_extension("corrupt.json");
                    let _ = fs::copy(&services_file, &backup);
                }
            }
        }

        let mut runtime = HashMap::new();
        if runtime_file.exists() {
            match fs::read_to_string(&runtime_file) {
                Ok(data) => match serde_json::from_str::<HashMap<String, PersistedRuntime>>(&data) {
                    Ok(parsed) => {
                        info!(
                            "Loaded {} persisted process records from {:?}",
                            parsed.len(),
                            runtime_file
                        );
                        runtime = parsed;
                    }
                    Err(err) => {
                        error!(
                            "Failed to parse runtime.json: {}. Starting with empty runtime state.",
                            err
                        );
                    }
                },
                Err(err) => {
                    error!("Failed to read {:?}: {}", runtime_file, err);
                }
            }
        }

        Ok(Self {
            base_dir,
            services_file,
            runtime_file,
            configs_dir,
            logs_dir,
            instances: Arc::new(RwLock::new(instances)),
            runtime: Arc::new(RwLock::new(runtime)),
        })
    }

    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    pub fn configs_dir(&self) -> &Path {
        &self.configs_dir
    }

    pub fn logs_dir(&self) -> &Path {
        &self.logs_dir
    }

    pub fn get_service_config_dir(&self, id: &str) -> PathBuf {
        let dir = self.configs_dir.join(id);
        let _ = fs::create_dir_all(&dir);
        dir
    }

    pub fn get_log_file_path(&self, id: &str) -> PathBuf {
        self.logs_dir.join(format!("{}.log", id))
    }

    pub async fn list(&self) -> Vec<ServiceInstance> {
        self.instances.read().await.clone()
    }

    pub async fn get(&self, id: &str) -> Option<ServiceInstance> {
        let lock = self.instances.read().await;
        lock.iter().find(|s| s.id == id).cloned()
    }

    pub async fn save_all(&self) -> Result<()> {
        let lock = self.instances.read().await;
        let json = serde_json::to_string_pretty(&*lock)?;
        let tmp_file = self.services_file.with_extension("tmp");
        fs::write(&tmp_file, json)?;
        fs::rename(&tmp_file, &self.services_file)?;
        Ok(())
    }

    pub async fn add(&self, mut instance: ServiceInstance) -> Result<()> {
        instance.apply_driver_listen_overrides();
        let mut lock = self.instances.write().await;
        if lock.iter().any(|s| s.id == instance.id) {
            anyhow::bail!("Service with ID '{}' already exists", instance.id);
        }
        lock.push(instance);
        drop(lock);
        self.save_all().await
    }

    pub async fn update(&self, mut instance: ServiceInstance) -> Result<()> {
        instance.apply_driver_listen_overrides();
        let mut lock = self.instances.write().await;
        if let Some(pos) = lock.iter().position(|s| s.id == instance.id) {
            lock[pos] = instance;
            drop(lock);
            self.save_all().await
        } else {
            anyhow::bail!("Service with ID '{}' not found", instance.id);
        }
    }

    pub async fn remove(&self, id: &str) -> Result<Option<ServiceInstance>> {
        let mut lock = self.instances.write().await;
        if let Some(pos) = lock.iter().position(|s| s.id == id) {
            let removed = lock.remove(pos);
            drop(lock);
            self.save_all().await?;
            let conf_dir = self.configs_dir.join(id);
            if conf_dir.exists() {
                let _ = fs::remove_dir_all(&conf_dir);
            }
            let _ = self.clear_runtime(id).await;
            Ok(Some(removed))
        } else {
            Ok(None)
        }
    }

    pub async fn get_runtime(&self, id: &str) -> Option<PersistedRuntime> {
        self.runtime.read().await.get(id).cloned()
    }

    pub async fn set_runtime(&self, id: &str, record: PersistedRuntime) -> Result<()> {
        let mut lock = self.runtime.write().await;
        lock.insert(id.to_string(), record);
        drop(lock);
        self.save_runtime().await
    }

    pub async fn clear_runtime(&self, id: &str) -> Result<()> {
        let mut lock = self.runtime.write().await;
        if lock.remove(id).is_none() {
            return Ok(());
        }
        drop(lock);
        self.save_runtime().await
    }

    async fn save_runtime(&self) -> Result<()> {
        let lock = self.runtime.read().await;
        let json = serde_json::to_string_pretty(&*lock)?;
        let tmp_file = self.runtime_file.with_extension("tmp");
        fs::write(&tmp_file, json)?;
        fs::rename(&tmp_file, &self.runtime_file)?;
        Ok(())
    }
}
