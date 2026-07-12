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
    pub pinned_forwards: Vec<PinnedForward>,
    #[serde(default)]
    pub ignored_ports: Vec<HostPort>,
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub sets: Vec<ConnectionSet>,
    #[serde(default)]
    pub web: WebConfig,
}

/// The optional local HTTP control server. Off by default; when enabled it
/// serves a token-authorized web panel + JSON API for browsing and moving files
/// on connected hosts. The `token` is a shared secret embedded in the link the
/// app hands out — never sent anywhere except to this local server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_web_port")]
    pub port: u16,
    /// Bind to 0.0.0.0 (reachable from the LAN) instead of 127.0.0.1.
    #[serde(default)]
    pub lan: bool,
    /// Shared secret; empty until the server is first enabled.
    #[serde(default)]
    pub token: String,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: default_web_port(),
            lan: false,
            token: String::new(),
        }
    }
}

fn default_web_port() -> u16 {
    8760
}

/// A fresh 128-bit hex token for the control server link.
pub fn new_web_token() -> String {
    Uuid::new_v4().simple().to_string()
}

/// A pinned local→remote tunnel that is re-created on every connect. Older
/// state files stored only `{hostId, port}`; those deserialize with
/// `localPort` defaulting to 0, which `local_port()` reads as "same as remote".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinnedForward {
    pub host_id: Uuid,
    pub port: u16,
    #[serde(default)]
    pub local_port: u16,
}

impl PinnedForward {
    /// The local bind port, falling back to the remote port for legacy entries.
    pub fn local_port(&self) -> u16 {
        if self.local_port == 0 {
            self.port
        } else {
            self.local_port
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_default_keeps_connections() {
        assert!(Settings::default().keep_connections);
    }

    #[test]
    fn settings_missing_field_defaults_to_true() {
        // A settings blob written before the field existed must still
        // deserialize with keepConnections defaulting to on.
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert!(s.keep_connections);
    }

    #[test]
    fn settings_round_trip_preserves_false() {
        let json = serde_json::to_string(&Settings {
            keep_connections: false,
        })
        .unwrap();
        assert!(json.contains("keepConnections"));
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert!(!back.keep_connections);
    }

    #[test]
    fn legacy_state_without_settings_or_sets_deserializes() {
        // v0.1.x state.json had only pinnedForwards/ignoredPorts.
        let legacy = r#"{ "pinnedForwards": [], "ignoredPorts": [] }"#;
        let state: StateFile = serde_json::from_str(legacy).unwrap();
        assert!(state.settings.keep_connections);
        assert!(state.sets.is_empty());
    }

    #[test]
    fn state_file_round_trips_sets_and_settings() {
        let host = Uuid::new_v4();
        let state = StateFile {
            pinned_forwards: vec![PinnedForward {
                host_id: host,
                port: 8080,
                local_port: 9090,
            }],
            ignored_ports: vec![],
            settings: Settings {
                keep_connections: false,
            },
            sets: vec![ConnectionSet {
                id: Uuid::new_v4(),
                name: "dev".into(),
                host_ids: vec![host],
            }],
            web: WebConfig::default(),
        };
        let json = serde_json::to_vec(&state).unwrap();
        let back: StateFile = serde_json::from_slice(&json).unwrap();
        assert_eq!(back.pinned_forwards.len(), 1);
        assert_eq!(back.pinned_forwards[0].port, 8080);
        assert_eq!(back.pinned_forwards[0].local_port(), 9090);
        assert!(!back.settings.keep_connections);
        assert_eq!(back.sets.len(), 1);
        assert_eq!(back.sets[0].name, "dev");
        assert_eq!(back.sets[0].host_ids, vec![host]);
    }

    #[test]
    fn legacy_pinned_forward_without_local_port_maps_to_remote() {
        // v0.2.0 state files stored pinned forwards as {hostId, port} with no
        // localPort; local_port() must fall back to the remote port.
        let host = Uuid::new_v4();
        let json = format!(r#"{{ "hostId": "{host}", "port": 3000 }}"#);
        let pin: PinnedForward = serde_json::from_str(&json).unwrap();
        assert_eq!(pin.local_port, 0);
        assert_eq!(pin.local_port(), 3000);
    }

    #[test]
    fn web_config_defaults_are_off_and_localhost() {
        let w = WebConfig::default();
        assert!(!w.enabled);
        assert!(!w.lan);
        assert_eq!(w.port, 8760);
        assert!(w.token.is_empty());
    }

    #[test]
    fn legacy_state_without_web_defaults_to_disabled() {
        // State files written before the control server existed must load with
        // the server off.
        let legacy = r#"{ "pinnedForwards": [], "ignoredPorts": [], "sets": [] }"#;
        let state: StateFile = serde_json::from_str(legacy).unwrap();
        assert!(!state.web.enabled);
        assert_eq!(state.web.port, 8760);
    }

    #[test]
    fn web_token_is_hex_and_unique() {
        let a = new_web_token();
        let b = new_web_token();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn custom_local_port_is_preserved() {
        let pin = PinnedForward {
            host_id: Uuid::new_v4(),
            port: 8080,
            local_port: 9090,
        };
        assert_eq!(pin.local_port(), 9090);
    }

    #[test]
    fn connection_set_missing_id_defaults_to_nil() {
        // A set authored without an id should still parse (nil UUID) rather
        // than failing the whole state load.
        let json = r#"{ "name": "prod", "hostIds": [] }"#;
        let set: ConnectionSet = serde_json::from_str(json).unwrap();
        assert_eq!(set.id, Uuid::nil());
        assert_eq!(set.name, "prod");
    }
}
