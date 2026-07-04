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
            pinned_forwards: vec![HostPort {
                host_id: host,
                port: 8080,
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
        };
        let json = serde_json::to_vec(&state).unwrap();
        let back: StateFile = serde_json::from_slice(&json).unwrap();
        assert_eq!(back.pinned_forwards.len(), 1);
        assert_eq!(back.pinned_forwards[0].port, 8080);
        assert!(!back.settings.keep_connections);
        assert_eq!(back.sets.len(), 1);
        assert_eq!(back.sets[0].name, "dev");
        assert_eq!(back.sets[0].host_ids, vec![host]);
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
