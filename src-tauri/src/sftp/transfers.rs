use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use russh_sftp::protocol::OpenFlags;
use tauri::ipc::Channel;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::error::{NettleError, Result};
use crate::ipc::types::{TransferDirection, TransferMeta, TransferProgress, TransferStatus};
use crate::sftp::open_sftp;
use crate::ssh::{current_epoch, EpochRx};

const CHUNK: usize = 256 * 1024;
const PROGRESS_EVERY: Duration = Duration::from_millis(100);
const MAX_CONCURRENT: usize = 3;

struct Entry {
    meta: TransferMeta,
    cancel: CancellationToken,
}

pub struct TransferManager {
    ui: Arc<crate::state::UiBridge>,
    epoch_rx: EpochRx,
    semaphore: Arc<Semaphore>,
    entries: StdMutex<HashMap<Uuid, Entry>>,
}

impl TransferManager {
    pub fn new(ui: Arc<crate::state::UiBridge>, epoch_rx: EpochRx) -> Arc<Self> {
        Arc::new(Self {
            ui,
            epoch_rx,
            semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT)),
            entries: StdMutex::new(HashMap::new()),
        })
    }

    pub fn list(&self) -> Vec<TransferMeta> {
        self.entries
            .lock()
            .unwrap()
            .values()
            .map(|e| e.meta.clone())
            .collect()
    }

    pub fn cancel(&self, id: Uuid) {
        if let Some(entry) = self.entries.lock().unwrap().get(&id) {
            entry.cancel.cancel();
        }
    }

    pub fn clear_finished(&self) {
        self.entries.lock().unwrap().retain(|_, e| {
            matches!(
                e.meta.status,
                TransferStatus::Queued | TransferStatus::Running
            )
        });
    }

    fn update<F: FnOnce(&mut TransferMeta)>(&self, id: Uuid, f: F) {
        let meta = {
            let mut entries = self.entries.lock().unwrap();
            let Some(entry) = entries.get_mut(&id) else {
                return;
            };
            f(&mut entry.meta);
            entry.meta.clone()
        };
        self.ui.emit_transfer(&meta);
    }

    pub fn start(
        self: &Arc<Self>,
        direction: TransferDirection,
        remote_path: String,
        local_path: String,
        on_progress: Channel<TransferProgress>,
    ) -> Uuid {
        let id = Uuid::new_v4();
        let name = remote_path
            .rsplit('/')
            .next()
            .unwrap_or(&remote_path)
            .to_string();
        let cancel = CancellationToken::new();
        let meta = TransferMeta {
            id,
            name,
            direction,
            status: TransferStatus::Queued,
            total: None,
            bytes: 0,
            error: None,
        };
        self.ui.emit_transfer(&meta);
        self.entries.lock().unwrap().insert(
            id,
            Entry {
                meta,
                cancel: cancel.clone(),
            },
        );

        let mgr = self.clone();
        tokio::spawn(async move {
            let _permit = match mgr.semaphore.clone().acquire_owned().await {
                Ok(p) => p,
                Err(_) => return,
            };
            let result = tokio::select! {
                _ = cancel.cancelled() => Err(NettleError::Msg("cancelled".into())),
                r = mgr.run_transfer(id, direction, &remote_path, &local_path, &on_progress, &cancel) => r,
            };
            match result {
                Ok(()) => mgr.update(id, |m| m.status = TransferStatus::Done),
                Err(e) if cancel.is_cancelled() => {
                    if direction == TransferDirection::Down {
                        let _ = tokio::fs::remove_file(&local_path).await;
                    }
                    let _ = e;
                    mgr.update(id, |m| m.status = TransferStatus::Cancelled);
                }
                Err(e) => mgr.update(id, |m| {
                    m.status = TransferStatus::Failed;
                    m.error = Some(e.to_string());
                }),
            }
        });
        id
    }

    async fn run_transfer(
        &self,
        id: Uuid,
        direction: TransferDirection,
        remote_path: &str,
        local_path: &str,
        on_progress: &Channel<TransferProgress>,
        cancel: &CancellationToken,
    ) -> Result<()> {
        let epoch = current_epoch(&self.epoch_rx).ok_or(NettleError::NotConnected)?;
        let epoch_cancel = epoch.cancel.clone();
        // Own SFTP session per transfer: no head-of-line blocking.
        let sftp = open_sftp(&epoch).await?;

        let total = match direction {
            TransferDirection::Down => sftp.metadata(remote_path).await.ok().and_then(|m| m.size),
            TransferDirection::Up => tokio::fs::metadata(local_path).await.ok().map(|m| m.len()),
        };
        self.update(id, |m| {
            m.status = TransferStatus::Running;
            m.total = total;
        });

        let mut bytes: u64 = 0;
        let mut last_emit = Instant::now();
        let mut last_bytes: u64 = 0;
        let mut buf = vec![0u8; CHUNK];

        macro_rules! pump {
            ($reader:expr, $writer:expr) => {
                loop {
                    let n = tokio::select! {
                        _ = cancel.cancelled() => return Err(NettleError::Msg("cancelled".into())),
                        _ = epoch_cancel.cancelled() => return Err(NettleError::Msg("connection lost".into())),
                        r = $reader.read(&mut buf) => r?,
                    };
                    if n == 0 {
                        break;
                    }
                    tokio::select! {
                        _ = cancel.cancelled() => return Err(NettleError::Msg("cancelled".into())),
                        _ = epoch_cancel.cancelled() => return Err(NettleError::Msg("connection lost".into())),
                        r = $writer.write_all(&buf[..n]) => r?,
                    }
                    bytes += n as u64;
                    let now = Instant::now();
                    let dt = now.duration_since(last_emit);
                    if dt >= PROGRESS_EVERY {
                        let rate = ((bytes - last_bytes) as f64 / dt.as_secs_f64()) as u64;
                        let _ = on_progress.send(TransferProgress {
                            id,
                            bytes,
                            total,
                            bytes_per_sec: rate,
                        });
                        last_emit = now;
                        last_bytes = bytes;
                    }
                }
            };
        }

        match direction {
            TransferDirection::Down => {
                let mut remote = sftp.open(remote_path).await?;
                let mut local = tokio::fs::File::create(local_path).await?;
                pump!(remote, local);
                local.flush().await?;
            }
            TransferDirection::Up => {
                let mut local = tokio::fs::File::open(local_path).await?;
                let mut remote = sftp
                    .open_with_flags(
                        remote_path,
                        OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
                    )
                    .await?;
                pump!(local, remote);
                remote.flush().await?;
            }
        }

        let _ = on_progress.send(TransferProgress {
            id,
            bytes,
            total,
            bytes_per_sec: 0,
        });
        self.update(id, |m| m.bytes = bytes);
        Ok(())
    }
}
