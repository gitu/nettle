use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostConfig {
    pub id: Uuid,
    pub name: String,
    pub hostname: String,
    pub port: u16,
    pub username: String,
    /// Optional path to a private key file; ssh-agent and password are always tried.
    #[serde(default)]
    pub key_path: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostsFile {
    pub version: u32,
    pub hosts: Vec<HostConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostPort {
    pub host_id: Uuid,
    pub port: u16,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateFile {
    #[serde(default)]
    pub pinned_forwards: Vec<HostPort>,
    #[serde(default)]
    pub ignored_ports: Vec<HostPort>,
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub sets: Vec<ConnectionSet>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionSet {
    #[serde(default)]
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub host_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Keep background sessions (and their tunnels/terminals) alive when
    /// switching to another host.
    #[serde(default = "default_true")]
    pub keep_connections: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            keep_connections: true,
        }
    }
}

fn default_true() -> bool {
    true
}
