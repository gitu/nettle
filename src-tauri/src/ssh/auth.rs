use std::sync::Arc;

use russh::client;
use russh::keys::agent::client::AgentClient;
use russh::keys::agent::AgentIdentity;
use russh::keys::{load_secret_key, PrivateKeyWithHashAlg};
use tauri::AppHandle;

use crate::config::HostConfig;
use crate::error::{NettleError, Result};
use crate::ipc::types::AuthRequest;
use crate::ssh::handler::ClientHandler;
use crate::state::Prompts;

/// Secrets entered by the user, kept in memory for the lifetime of the session
/// so auto-reconnect never re-prompts. Never persisted.
#[derive(Default)]
pub struct SecretCache {
    pub password: Option<String>,
    pub key_passphrase: Option<String>,
}

/// Try agent → key file → password, in that order.
/// `interactive` is false during auto-reconnect: only cached secrets are used.
pub async fn authenticate(
    handle: &mut client::Handle<ClientHandler>,
    host: &HostConfig,
    app: &AppHandle,
    prompts: &Prompts,
    cache: &mut SecretCache,
    interactive: bool,
) -> Result<()> {
    if try_agent(handle, host).await {
        return Ok(());
    }
    if let Some(done) = try_key_file(handle, host, app, prompts, cache, interactive).await? {
        if done {
            return Ok(());
        }
    }
    if try_password(handle, host, app, prompts, cache, interactive).await? {
        return Ok(());
    }
    Err(NettleError::AuthFailed)
}

async fn connect_agent() -> Option<AgentClient<impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send>> {
    #[cfg(unix)]
    {
        AgentClient::connect_env().await.ok()
    }
    #[cfg(windows)]
    {
        AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent")
            .await
            .ok()
    }
}

async fn try_agent(handle: &mut client::Handle<ClientHandler>, host: &HostConfig) -> bool {
    let Some(mut agent) = connect_agent().await else {
        return false;
    };
    let Ok(identities) = agent.request_identities().await else {
        return false;
    };
    let hash = handle
        .best_supported_rsa_hash()
        .await
        .ok()
        .flatten()
        .flatten();
    for identity in identities {
        let AgentIdentity::PublicKey { key, .. } = identity else {
            continue;
        };
        match handle
            .authenticate_publickey_with(&host.username, key, hash, &mut agent)
            .await
        {
            Ok(result) if result.success() => return true,
            _ => continue,
        }
    }
    false
}

/// Ok(Some(true)) = authenticated; Ok(Some(false)) = key tried but rejected;
/// Ok(None) = no key configured / not loadable.
async fn try_key_file(
    handle: &mut client::Handle<ClientHandler>,
    host: &HostConfig,
    app: &AppHandle,
    prompts: &Prompts,
    cache: &mut SecretCache,
    interactive: bool,
) -> Result<Option<bool>> {
    let Some(path) = host.key_path.as_deref().filter(|p| !p.is_empty()) else {
        return Ok(None);
    };
    let path = expand_tilde(path);

    let key = match load_secret_key(&path, cache.key_passphrase.as_deref()) {
        Ok(key) => key,
        Err(_) if interactive => {
            // Probably encrypted (or wrong cached passphrase) — ask the user.
            let Some(pass) = prompts
                .ask_secret(
                    app,
                    AuthRequest {
                        kind: "keyPassphrase".into(),
                        username: host.username.clone(),
                        host: host.hostname.clone(),
                    },
                )
                .await
            else {
                return Ok(None);
            };
            match load_secret_key(&path, Some(&pass)) {
                Ok(key) => {
                    cache.key_passphrase = Some(pass);
                    key
                }
                Err(_) => return Ok(None),
            }
        }
        Err(_) => return Ok(None),
    };

    let hash = handle
        .best_supported_rsa_hash()
        .await
        .ok()
        .flatten()
        .flatten();
    let result = handle
        .authenticate_publickey(
            &host.username,
            PrivateKeyWithHashAlg::new(Arc::new(key), hash),
        )
        .await?;
    Ok(Some(result.success()))
}

async fn try_password(
    handle: &mut client::Handle<ClientHandler>,
    host: &HostConfig,
    app: &AppHandle,
    prompts: &Prompts,
    cache: &mut SecretCache,
    interactive: bool,
) -> Result<bool> {
    if let Some(password) = cache.password.clone() {
        let result = handle
            .authenticate_password(&host.username, &password)
            .await?;
        if result.success() {
            return Ok(true);
        }
    }
    if !interactive {
        return Ok(false);
    }
    let Some(password) = prompts
        .ask_secret(
            app,
            AuthRequest {
                kind: "password".into(),
                username: host.username.clone(),
                host: host.hostname.clone(),
            },
        )
        .await
    else {
        return Err(NettleError::AuthCancelled);
    };
    let result = handle
        .authenticate_password(&host.username, &password)
        .await?;
    if result.success() {
        cache.password = Some(password);
        return Ok(true);
    }
    Ok(false)
}

pub fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            return format!("{}/{}", home.to_string_lossy(), rest);
        }
    }
    path.to_string()
}
