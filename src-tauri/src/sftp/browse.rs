use std::sync::Arc;

use russh_sftp::client::SftpSession;
use tokio::sync::Mutex;

use crate::error::{NettleError, Result};
use crate::ipc::types::{DirListing, FileEntry};
use crate::sftp::open_sftp;
use crate::ssh::{current_epoch, EpochRx};

/// Directory browsing over one long-lived SFTP session, reopened per epoch.
pub struct SftpBrowser {
    epoch_rx: EpochRx,
    cached: Mutex<Option<(u64, Arc<SftpSession>)>>,
}

impl SftpBrowser {
    pub fn new(epoch_rx: EpochRx) -> Self {
        Self {
            epoch_rx,
            cached: Mutex::new(None),
        }
    }

    async fn session(&self) -> Result<Arc<SftpSession>> {
        let epoch = current_epoch(&self.epoch_rx).ok_or(NettleError::NotConnected)?;
        let mut cached = self.cached.lock().await;
        if let Some((id, sftp)) = cached.as_ref() {
            if *id == epoch.id {
                return Ok(sftp.clone());
            }
        }
        let sftp = Arc::new(open_sftp(&epoch).await?);
        *cached = Some((epoch.id, sftp.clone()));
        Ok(sftp)
    }

    pub async fn home(&self) -> Result<String> {
        let sftp = self.session().await?;
        Ok(sftp.canonicalize(".").await?)
    }

    /// Read a whole remote file into memory. Used by the web-control server to
    /// stream a download; large files are buffered, so this is meant for the
    /// configs/logs/artifacts a control panel typically reaches for.
    pub async fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        use tokio::io::AsyncReadExt;
        let sftp = self.session().await?;
        let mut file = sftp.open(path).await?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).await?;
        Ok(buf)
    }

    /// Create/overwrite a remote file with the given bytes.
    pub async fn write_file(&self, path: &str, data: &[u8]) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        let sftp = self.session().await?;
        let mut file = sftp.create(path).await?;
        file.write_all(data).await?;
        file.flush().await?;
        file.shutdown().await?;
        Ok(())
    }

    pub async fn list(&self, path: &str) -> Result<DirListing> {
        let sftp = self.session().await?;
        let path = if path.is_empty() || path == "~" {
            sftp.canonicalize(".").await?
        } else if let Some(rest) = path.strip_prefix("~/") {
            format!("{}/{}", sftp.canonicalize(".").await?, rest)
        } else {
            path.to_string()
        };

        let mut entries: Vec<FileEntry> = Vec::new();
        for entry in sftp.read_dir(&path).await? {
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let meta = entry.metadata();
            let kind = if meta.is_dir() {
                "dir"
            } else if meta.is_symlink() {
                "link"
            } else {
                "file"
            };
            entries.push(FileEntry {
                name,
                kind: kind.to_string(),
                size: meta.size,
                mtime: meta.mtime.map(|t| t as u64),
            });
        }
        sort_entries(&mut entries);
        Ok(DirListing { path, entries })
    }
}

pub fn sort_entries(entries: &mut [FileEntry]) {
    entries.sort_by(|a, b| {
        let a_dir = a.kind == "dir";
        let b_dir = b.kind == "dir";
        b_dir
            .cmp(&a_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}
