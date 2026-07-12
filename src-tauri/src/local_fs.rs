use std::time::UNIX_EPOCH;

use crate::error::{NettleError, Result};
use crate::ipc::types::{DirListing, FileEntry};
use crate::sftp::browse::sort_entries;

/// Map an OS error into an actionable message when it's a permission denial.
/// On macOS, folders like Downloads/Documents/Desktop are gated by TCC, and a
/// denial surfaces as EACCES/EPERM — turn that into guidance instead of a raw
/// "Operation not permitted".
fn map_dir_error(path: &str, e: std::io::Error) -> NettleError {
    let denied = e.kind() == std::io::ErrorKind::PermissionDenied
        || matches!(e.raw_os_error(), Some(1) | Some(13)); // EPERM | EACCES
    if !denied {
        return e.into();
    }
    if cfg!(target_os = "macos") {
        NettleError::Permission(format!(
            "macOS blocked access to {path}. Grant nettle access under System \
             Settings → Privacy & Security → Files and Folders (or Full Disk \
             Access), then reopen this folder."
        ))
    } else {
        NettleError::Permission(format!("permission denied reading {path}"))
    }
}

/// List a local directory. On Windows an empty path lists drive roots.
pub async fn list(path: &str) -> Result<DirListing> {
    #[cfg(windows)]
    if path.is_empty() {
        return Ok(drive_roots());
    }

    let path = if path.is_empty() { "/" } else { path };
    let mut entries = Vec::new();
    let mut dir = tokio::fs::read_dir(path)
        .await
        .map_err(|e| map_dir_error(path, e))?;
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
            size: (!meta.is_dir()).then_some(meta.len()),
            mtime,
        });
    }
    sort_entries(&mut entries);
    Ok(DirListing {
        path: path.to_string(),
        entries,
    })
}

#[cfg(test)]
mod tests {
    use super::map_dir_error;
    use crate::error::NettleError;
    use std::io::{Error, ErrorKind};

    #[test]
    fn permission_denied_becomes_actionable() {
        let err = map_dir_error("/x/Downloads", Error::from(ErrorKind::PermissionDenied));
        assert!(matches!(err, NettleError::Permission(_)));
        assert_eq!(err.code(), "permission");
        assert!(err.to_string().contains("/x/Downloads"));
    }

    #[test]
    fn eperm_is_treated_as_permission_denied() {
        // macOS TCC denials arrive as EPERM (1), which older Rust didn't map to
        // ErrorKind::PermissionDenied.
        let err = map_dir_error("/x/Downloads", Error::from_raw_os_error(1));
        assert!(matches!(err, NettleError::Permission(_)));
    }

    #[test]
    fn other_errors_pass_through() {
        let err = map_dir_error("/x", Error::from(ErrorKind::NotFound));
        assert!(matches!(err, NettleError::Io(_)));
    }
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
