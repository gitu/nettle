use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;

use super::model::{HostConfig, HostsFile, StateFile};
use crate::error::Result;

/// Atomic JSON persistence rooted at the Tauri app-config directory.
#[derive(Clone)]
pub struct ConfigStore {
    dir: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl ConfigStore {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn known_hosts_path(&self) -> PathBuf {
        self.dir.join("known_hosts")
    }

    fn hosts_path(&self) -> PathBuf {
        self.dir.join("hosts.json")
    }

    fn state_path(&self) -> PathBuf {
        self.dir.join("state.json")
    }

    async fn read_json<T: serde::de::DeserializeOwned + Default>(&self, path: PathBuf) -> T {
        match tokio::fs::read(&path).await {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => T::default(),
        }
    }

    async fn write_json<T: serde::Serialize>(&self, path: PathBuf, value: &T) -> Result<()> {
        let _guard = self.lock.lock().await;
        tokio::fs::create_dir_all(&self.dir).await?;
        let tmp = path.with_extension("json.tmp");
        let data = serde_json::to_vec_pretty(value)?;
        tokio::fs::write(&tmp, data).await?;
        tokio::fs::rename(&tmp, &path).await?;
        Ok(())
    }

    pub async fn load_hosts(&self) -> Vec<HostConfig> {
        self.read_json::<HostsFile>(self.hosts_path()).await.hosts
    }

    pub async fn save_hosts(&self, hosts: Vec<HostConfig>) -> Result<()> {
        self.write_json(self.hosts_path(), &HostsFile { version: 1, hosts })
            .await
    }

    pub async fn load_state(&self) -> StateFile {
        self.read_json(self.state_path()).await
    }

    pub async fn update_state<F: FnOnce(&mut StateFile)>(&self, f: F) -> Result<()> {
        let mut state: StateFile = self.read_json(self.state_path()).await;
        f(&mut state);
        self.write_json(self.state_path(), &state).await
    }
}
