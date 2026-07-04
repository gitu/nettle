use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "state", rename_all_fields = "camelCase")]
pub enum ConnState {
    Disconnected,
    Connecting {
        host_id: Uuid,
    },
    Authenticating {
        host_id: Uuid,
    },
    Connected {
        host_id: Uuid,
        ip: String,
        since_ms: u64,
        epoch: u64,
    },
    Reconnecting {
        host_id: Uuid,
        attempt: u32,
        next_retry_ms: Option<u64>,
    },
    Failed {
        host_id: Uuid,
        error: String,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    /// "dir" | "file" | "link"
    pub kind: String,
    pub size: Option<u64>,
    pub mtime: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirListing {
    pub path: String,
    pub entries: Vec<FileEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePort {
    pub port: u16,
    pub bind: String,
    pub process: Option<String>,
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortsChanged {
    pub all: Vec<RemotePort>,
    pub added: Vec<RemotePort>,
    pub removed: Vec<u16>,
    pub is_baseline: bool,
    pub unsupported: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardInfo {
    pub port: u16,
    pub pinned: bool,
    /// remote process is currently listening
    pub live: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TransferDirection {
    Down,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TransferStatus {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferMeta {
    pub id: Uuid,
    pub name: String,
    pub direction: TransferDirection,
    pub status: TransferStatus,
    pub total: Option<u64>,
    pub bytes: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferProgress {
    pub id: Uuid,
    pub bytes: u64,
    pub total: Option<u64>,
    pub bytes_per_sec: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostKeyPrompt {
    pub host: String,
    pub port: u16,
    pub key_type: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthRequest {
    /// "password" | "keyPassphrase"
    pub kind: String,
    pub username: String,
    pub host: String,
}
