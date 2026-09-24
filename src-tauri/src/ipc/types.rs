use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
#[serde(
    rename_all = "camelCase",
    tag = "state",
    rename_all_fields = "camelCase"
)]
pub enum ConnState {
    Disconnected {
        host_id: Uuid,
    },
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
    /// Name of the docker container publishing this port, if any.
    pub container: Option<String>,
    /// Working directory of the listening process (Linux, via /proc), if known.
    /// Distinguishes e.g. two `node` dev servers by project directory.
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortsChanged {
    pub host_id: Uuid,
    pub all: Vec<RemotePort>,
    pub added: Vec<RemotePort>,
    pub removed: Vec<u16>,
    pub is_baseline: bool,
    pub unsupported: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub host_id: Uuid,
    pub conn: ConnState,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostForward {
    pub host_id: Uuid,
    pub host_name: String,
    pub forward: ForwardInfo,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardsChanged {
    pub host_id: Uuid,
    pub forwards: Vec<ForwardInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardInfo {
    /// remote port being tunneled
    pub port: u16,
    /// local port the tunnel is bound to (usually the same as `port`)
    pub local_port: u16,
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
    pub host_id: Uuid,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// One line of the in-app activity log ("what has nettle been doing").
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEntry {
    pub seq: u64,
    pub ts_ms: u64,
    pub level: LogLevel,
    /// None for app-global events (e.g. web-control server).
    pub host_id: Option<Uuid>,
    /// short slug: "conn" | "forward" | "scan" | "kill" | …
    pub category: String,
    pub message: String,
}

/// Runtime connection statistics for one host (since the app started, across
/// reconnects and manual disconnect/connect cycles).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnStats {
    pub host_id: Uuid,
    /// When these counters started (first time the host was touched).
    pub since_ms: u64,
    pub connected: bool,
    /// Fresh sessions (epoch 1) that came up.
    pub connects: u32,
    /// Links re-established after a drop (epoch > 1).
    pub reconnects: u32,
    /// Times an established link died underneath us.
    pub link_drops: u32,
    /// Failed connect / reconnect attempts.
    pub connect_failures: u32,
    pub connected_since_ms: Option<u64>,
    /// Connected time of links that already ended. The UI adds the live link
    /// (`now - connected_since_ms`) so the total ticks without new events.
    pub prior_uptime_ms: u64,
    pub last_ip: Option<String>,
    pub last_drop_at_ms: Option<u64>,
    pub last_drop_reason: Option<String>,
    /// Most recent connect/link error; cleared when a link comes up.
    pub last_error: Option<String>,
    pub tunnel_conns_total: u64,
    pub tunnel_conns_active: u64,
    /// direct-tcpip opens the remote refused.
    pub tunnel_refused: u64,
    /// Local connections dropped because the remote port never came up.
    pub tunnel_wait_timeouts: u64,
    /// local → remote bytes through tunnels
    pub tunnel_bytes_up: u64,
    /// remote → local bytes through tunnels
    pub tunnel_bytes_down: u64,
    pub scans: u64,
    pub scan_failures: u64,
    /// Duration of the most recent port scan (round trip over SSH).
    pub last_scan_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthRequest {
    /// "password" | "keyPassphrase"
    pub kind: String,
    pub username: String,
    pub host: String,
}
