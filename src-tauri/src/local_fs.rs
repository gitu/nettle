use std::time::UNIX_EPOCH;

use crate::error::Result;
use crate::ipc::types::{DirListing, FileEntry};
use crate::sftp::browse::sort_entries;

/// List a local directory. On Windows an empty path lists drive roots.
pub async fn list(path: &str) -> Result<DirListing> {
    #[cfg(windows)]
    if path.is_empty() {
        return Ok(drive_roots());
    }

    let path = if path.is_empty() { "/" } else { path };
    let mut entries = Vec::new();
    let mut dir = tokio::fs::read_dir(path).await?;
    while let Some(entry) = dir.next_entry().await? {
        let name = entry.file_name().to_string_lossy().into_owned();
        let meta = match entry.metadata().await {
            Ok(m) => m,
            Err(_) => continue,
        };
        let kind = if meta.is_dir() {
            "dir"
        } else if meta.file_type().is_symlink() {
            "link"
        } else {
            "file"
        };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        entries.push(FileEntry {
            name,
            kind: kind.to_string(),
            size: (!meta.is_dir()).then(|| meta.len()),
            mtime,
        });
    }
    sort_entries(&mut entries);
    Ok(DirListing {
        path: path.to_string(),
        entries,
    })
}

#[cfg(windows)]
fn drive_roots() -> DirListing {
    let mut entries = Vec::new();
    for letter in b'A'..=b'Z' {
        let root = format!("{}:\\", letter as char);
        if std::fs::metadata(&root).is_ok() {
            entries.push(FileEntry {
                name: root,
                kind: "dir".to_string(),
                size: None,
                mtime: None,
            });
        }
    }
    DirListing {
        path: String::new(),
        entries,
    }
}
