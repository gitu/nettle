use crate::ipc::types::RemotePort;

/// Parse `ss -tlnp` output. Handles: header row, `*:80` / `[::]:22` /
/// `127.0.0.53%lo:53` bind forms, and the process column being entirely
/// absent when run without privileges.
pub fn parse_ss(output: &str) -> Vec<RemotePort> {
    let mut ports = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("State") || line.starts_with("Netid") {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        // State Recv-Q Send-Q Local:Port Peer:Port [Process]
        if fields.len() < 5 {
            continue;
        }
        // Some ss builds prepend a Netid column ("tcp LISTEN 0 ...").
        let local_idx = if fields[0].eq_ignore_ascii_case("tcp") {
            4
        } else {
            3
        };
        let Some(local) = fields.get(local_idx) else {
            continue;
        };
        let Some((bind, port)) = split_bind(local) else {
            continue;
        };
        let (process, pid) = parse_ss_process(line);
        ports.push(RemotePort {
            port,
            bind,
            process,
            pid,
        });
    }
    ports
}

/// Extract `("name",pid=1234` from a `users:((...))` column, if present.
fn parse_ss_process(line: &str) -> (Option<String>, Option<u32>) {
    let Some(start) = line.find("users:((\"") else {
        return (None, None);
    };
    let rest = &line[start + 9..];
    let Some(name_end) = rest.find('"') else {
        return (None, None);
    };
    let name = &rest[..name_end];
    let pid = rest[name_end..].split("pid=").nth(1).and_then(|s| {
        s.chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .ok()
    });
    (Some(name.to_string()), pid)
}

/// Parse `netstat -tlnp` output (`1234/name` process format, `-` when unknown).
pub fn parse_netstat(output: &str) -> Vec<RemotePort> {
    let mut ports = Vec::new();
    for line in output.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 6 || !fields[0].starts_with("tcp") {
            continue;
        }
        // proto recvq sendq local foreign state [pid/name]
        if fields[5] != "LISTEN" {
            continue;
        }
        let Some((bind, port)) = split_bind(fields[3]) else {
            continue;
        };
        let (process, pid) = match fields.get(6) {
            Some(&"-") | None => (None, None),
            Some(pp) => match pp.split_once('/') {
                Some((pid, name)) => (Some(name.to_string()), pid.parse().ok()),
                None => (None, None),
            },
        };
        ports.push(RemotePort {
            port,
            bind,
            process,
            pid,
        });
    }
    ports
}

/// Parse concatenated /proc/net/tcp + /proc/net/tcp6 (state 0A = LISTEN).
/// No process names available at this level.
pub fn parse_proc_net(output: &str) -> Vec<RemotePort> {
    let mut ports = Vec::new();
    for line in output.lines() {
        // data rows look like "0: 0100007F:1F90 00000000:0000 0A ..."
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 4 || fields[3] != "0A" {
            continue;
        }
        let Some((addr_hex, port_hex)) = fields[1].split_once(':') else {
            continue;
        };
        let Ok(port) = u16::from_str_radix(port_hex, 16) else {
            continue;
        };
        let bind = decode_proc_addr(addr_hex);
        ports.push(RemotePort {
            port,
            bind,
            process: None,
            pid: None,
        });
    }
    ports
}

fn decode_proc_addr(hex: &str) -> String {
    if hex.len() == 8 {
        // IPv4, little-endian
        if let Ok(v) = u32::from_str_radix(hex, 16) {
            let b = v.to_le_bytes();
            return format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3]);
        }
    } else if hex.len() == 32 {
        if hex.chars().all(|c| c == '0') {
            return "::".to_string();
        }
        if hex == "00000000000000000000000001000000" {
            return "::1".to_string();
        }
        return "v6".to_string();
    }
    hex.to_string()
}

/// Split "addr:port", tolerating `[::]:22`, `*:80`, `0.0.0.0:22`, `127.0.0.53%lo:53`.
fn split_bind(local: &str) -> Option<(String, u16)> {
    let (addr, port) = local.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    let mut addr = addr.trim_start_matches('[').trim_end_matches(']');
    if let Some((base, _iface)) = addr.split_once('%') {
        addr = base;
    }
    let bind = match addr {
        "*" | "" => "0.0.0.0".to_string(),
        a => a.to_string(),
    };
    Some((bind, port))
}

/// Collapse rows to one per port (a dual-stack service shows v4+v6 rows);
/// prefer the row that has a process name, then the v4 bind.
pub fn dedupe_by_port(mut ports: Vec<RemotePort>) -> Vec<RemotePort> {
    ports.sort_by(|a, b| {
        a.port
            .cmp(&b.port)
            .then_with(|| b.process.is_some().cmp(&a.process.is_some()))
            .then_with(|| a.bind.contains(':').cmp(&b.bind.contains(':')))
    });
    ports.dedup_by(|a, b| a.port == b.port);
    ports
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ss_with_process() {
        let out = "State      Recv-Q Send-Q Local Address:Port  Peer Address:Port Process\n\
LISTEN     0      511          0.0.0.0:5173       0.0.0.0:*     users:((\"node\",pid=1234,fd=23))\n\
LISTEN     0      128            [::]:22             [::]:*     users:((\"sshd\",pid=800,fd=3),(\"sshd\",pid=801,fd=3))\n\
LISTEN     0      4096   127.0.0.53%lo:53         0.0.0.0:*     users:((\"systemd-resolve\",pid=500,fd=14))\n";
        let ports = parse_ss(out);
        assert_eq!(ports.len(), 3);
        assert_eq!(ports[0].port, 5173);
        assert_eq!(ports[0].process.as_deref(), Some("node"));
        assert_eq!(ports[0].pid, Some(1234));
        assert_eq!(ports[1].bind, "::");
        assert_eq!(ports[2].port, 53);
        assert_eq!(ports[2].bind, "127.0.0.53");
    }

    #[test]
    fn ss_without_process_column() {
        let out = "State  Recv-Q Send-Q  Local Address:Port  Peer Address:Port\n\
LISTEN 0      128     0.0.0.0:8080        0.0.0.0:*\n\
LISTEN 0      128     *:3000              *:*\n";
        let ports = parse_ss(out);
        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0].process, None);
        assert_eq!(ports[1].port, 3000);
        assert_eq!(ports[1].bind, "0.0.0.0");
    }

    #[test]
    fn netstat_output() {
        let out = "Active Internet connections (only servers)\n\
Proto Recv-Q Send-Q Local Address           Foreign Address         State       PID/Program name\n\
tcp        0      0 127.0.0.1:5432          0.0.0.0:*               LISTEN      900/postgres\n\
tcp6       0      0 :::8080                 :::*                    LISTEN      -\n";
        let ports = parse_netstat(out);
        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0].port, 5432);
        assert_eq!(ports[0].process.as_deref(), Some("postgres"));
        assert_eq!(ports[0].pid, Some(900));
        assert_eq!(ports[1].port, 8080);
        assert_eq!(ports[1].process, None);
    }

    #[test]
    fn proc_net_tcp() {
        let out = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 12345 1 0000000000000000 100 0 0 10 0\n\
   1: 00000000000000000000000000000000:0016 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 999 1 0000000000000000 100 0 0 10 0\n";
        let ports = parse_proc_net(out);
        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0].port, 8080);
        assert_eq!(ports[0].bind, "127.0.0.1");
        assert_eq!(ports[1].port, 22);
        assert_eq!(ports[1].bind, "::");
    }

    #[test]
    fn dedupe_dual_stack() {
        let ports = vec![
            RemotePort {
                port: 8080,
                bind: "::".into(),
                process: None,
                pid: None,
            },
            RemotePort {
                port: 8080,
                bind: "0.0.0.0".into(),
                process: Some("node".into()),
                pid: Some(1),
            },
            RemotePort {
                port: 22,
                bind: "0.0.0.0".into(),
                process: None,
                pid: None,
            },
        ];
        let deduped = dedupe_by_port(ports);
        assert_eq!(deduped.len(), 2);
        let p8080 = deduped.iter().find(|p| p.port == 8080).unwrap();
        assert_eq!(p8080.process.as_deref(), Some("node"));
    }
}
