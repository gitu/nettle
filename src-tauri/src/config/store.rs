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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::ConnectionSet;
    use uuid::Uuid;

    fn tmp_store() -> (ConfigStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (ConfigStore::new(dir.path().to_path_buf()), dir)
    }

    #[tokio::test]
    async fn missing_files_yield_defaults() {
        let (store, _dir) = tmp_store();
        assert!(store.load_hosts().await.is_empty());
        let state = store.load_state().await;
        assert!(state.settings.keep_connections);
        assert!(state.sets.is_empty());
    }

    #[tokio::test]
    async fn hosts_persist_across_loads() {
        let (store, _dir) = tmp_store();
        let host = HostConfig {
            id: Uuid::new_v4(),
            name: "web".into(),
            hostname: "example.com".into(),
            port: 22,
            username: "deploy".into(),
            key_path: None,
        };
        store.save_hosts(vec![host.clone()]).await.unwrap();
        let loaded = store.load_hosts().await;
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, host.id);
        assert_eq!(loaded[0].hostname, "example.com");
    }

    #[tokio::test]
    async fn update_state_is_read_modify_write() {
        let (store, _dir) = tmp_store();
        let host = Uuid::new_v4();
        store
            .update_state(|s| {
                s.settings.keep_connections = false;
                s.sets.push(ConnectionSet {
                    id: Uuid::new_v4(),
                    name: "dev".into(),
                    host_ids: vec![host],
                });
            })
            .await
            .unwrap();
        // A second update must see the first one's writes.
        store
            .update_state(|s| {
                s.pinned_forwards.push(crate::config::model::PinnedForward {
                    host_id: host,
                    port: 3000,
                    local_port: 0,
                });
            })
            .await
            .unwrap();

        let state = store.load_state().await;
        assert!(!state.settings.keep_connections);
        assert_eq!(state.sets.len(), 1);
        assert_eq!(state.sets[0].name, "dev");
        assert_eq!(state.pinned_forwards.len(), 1);
        assert_eq!(state.pinned_forwards[0].port, 3000);
    }
}
