use std::path::PathBuf;
use std::sync::Arc;

use russh::client;
use russh::keys::known_hosts::{check_known_hosts_path, learn_known_hosts_path};
use russh::keys::ssh_key;
use tauri::AppHandle;
use tokio::sync::mpsc;

use crate::ipc::types::HostKeyPrompt;
use crate::state::Prompts;

pub struct ClientHandler {
    pub app: AppHandle,
    pub hostname: String,
    pub port: u16,
    pub known_hosts_path: PathBuf,
    /// TOFU prompting is only allowed on user-initiated connects, never mid-reconnect.
    pub prompt_allowed: bool,
    pub prompts: Arc<Prompts>,
    pub death_tx: mpsc::UnboundedSender<String>,
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        match check_known_hosts_path(&self.hostname, self.port, key, &self.known_hosts_path) {
            Ok(true) => Ok(true),
            Ok(false) => self.prompt_unknown_key(key).await,
            Err(russh::keys::Error::KeyChanged { .. }) => {
                use tauri::Emitter;
                let _ = self.app.emit(
                    "host-key-mismatch",
                    HostKeyPrompt {
                        host: self.hostname.clone(),
                        port: self.port,
                        key_type: key.algorithm().to_string(),
                        fingerprint: key.fingerprint(ssh_key::HashAlg::Sha256).to_string(),
                    },
                );
                Ok(false)
            }
            // A missing known_hosts file surfaces as an IO error — treat as unknown host.
            Err(_) => self.prompt_unknown_key(key).await,
        }
    }

    fn disconnected(
        &mut self,
        reason: client::DisconnectReason<Self::Error>,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        let _ = self.death_tx.send(format!("{reason:?}"));
        async move {
            match reason {
                client::DisconnectReason::ReceivedDisconnect(_) => Ok(()),
                client::DisconnectReason::Error(e) => Err(e),
            }
        }
    }
}

impl ClientHandler {
    async fn prompt_unknown_key(&self, key: &ssh_key::PublicKey) -> Result<bool, russh::Error> {
        if !self.prompt_allowed {
            return Ok(false);
        }
        let fingerprint = key.fingerprint(ssh_key::HashAlg::Sha256).to_string();
        let accepted = self
            .prompts
            .ask_host_key(
                &self.app,
                HostKeyPrompt {
                    host: self.hostname.clone(),
                    port: self.port,
                    key_type: key.algorithm().to_string(),
                    fingerprint,
                },
            )
            .await;
        if accepted {
            if let Some(dir) = self.known_hosts_path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = learn_known_hosts_path(&self.hostname, self.port, key, &self.known_hosts_path);
        }
        Ok(accepted)
    }
}
