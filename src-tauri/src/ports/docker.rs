use std::collections::HashMap;

use crate::ipc::types::RemotePort;
use crate::ssh::{exec_capture, ConnectionEpoch};

/// `|` is safe as a separator: container names match [a-zA-Z0-9][a-zA-Z0-9_.-]*
/// and the Ports column only contains addresses, `->`, `/proto` and commas.
const DOCKER_PS_CMD: &str = "docker ps --format '{{.Names}}|{{.Ports}}' 2>/dev/null";

/// Ask the remote docker daemon which containers publish which host ports.
/// `Ok(None)` means docker isn't usable on this host (not installed, daemon
/// down, or no permission) — callers should stop probing for this epoch.
pub async fn scan(epoch: &ConnectionEpoch) -> crate::error::Result<Option<HashMap<u16, String>>> {
    let (out, exit) = exec_capture(&epoch.handle, DOCKER_PS_CMD).await?;
    if exit != Some(0) {
        return Ok(None);
    }
    Ok(Some(parse_docker_ps(&out)))
}

/// Parse `docker ps --format '{{.Names}}|{{.Ports}}'` into host-port → name.
/// Ports look like `0.0.0.0:8080->80/tcp, [::]:8080->80/tcp` for published
/// ports and bare `80/tcp` (no `->`) for unpublished ones, which are skipped.
pub fn parse_docker_ps(output: &str) -> HashMap<u16, String> {
    let mut map = HashMap::new();
    for line in output.lines() {
        let Some((name, ports)) = line.trim().split_once('|') else {
            continue;
        };
        for mapping in ports.split(',') {
            let Some((host, _container)) = mapping.trim().split_once("->") else {
                continue;
            };
            let Some((_addr, port)) = host.rsplit_once(':') else {
                continue;
            };
            if let Ok(port) = port.parse::<u16>() {
                map.insert(port, name.to_string());
            }
        }
    }
    map
}

/// Tag scanned ports with the docker container publishing them.
pub fn annotate(ports: &mut [RemotePort], containers: &HashMap<u16, String>) {
    for p in ports {
        p.container = containers.get(&p.port).cloned();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_published_ports() {
        let out = "web|0.0.0.0:8080->80/tcp, [::]:8080->80/tcp\n\
db|127.0.0.1:5432->5432/tcp\n\
worker|\n\
internal|6379/tcp\n";
        let map = parse_docker_ps(out);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get(&8080).map(String::as_str), Some("web"));
        assert_eq!(map.get(&5432).map(String::as_str), Some("db"));
    }

    #[test]
    fn parses_port_ranges_and_udp() {
        // Ranges expand per docker as explicit list only with -p a-b; a single
        // published range renders like `0.0.0.0:7000-7001->7000-7001/tcp` —
        // no single host port to key on, so it is skipped rather than misparsed.
        let out = "range|0.0.0.0:7000-7001->7000-7001/tcp\n\
dns|0.0.0.0:5353->53/udp\n";
        let map = parse_docker_ps(out);
        assert_eq!(map.get(&5353).map(String::as_str), Some("dns"));
        assert!(!map.contains_key(&7000));
    }

    #[test]
    fn annotates_matching_ports() {
        let mut ports = vec![
            RemotePort {
                port: 8080,
                bind: "0.0.0.0".into(),
                process: Some("docker-proxy".into()),
                pid: Some(100),
                container: None,
            },
            RemotePort {
                port: 22,
                bind: "0.0.0.0".into(),
                process: Some("sshd".into()),
                pid: Some(1),
                container: None,
            },
        ];
        let mut map = HashMap::new();
        map.insert(8080u16, "web".to_string());
        annotate(&mut ports, &map);
        assert_eq!(ports[0].container.as_deref(), Some("web"));
        assert_eq!(ports[1].container, None);
    }
}
