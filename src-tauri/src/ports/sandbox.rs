//! Docker Sandboxes (`sbx`) on the remote host.
//!
//! A sandbox is a microVM with its own network namespace and docker daemon, so
//! its ports never show up in the host's `ss` output. nettle reaches inside it
//! through the `sbx` CLI on the host: `sbx ls` lists sandboxes, `sbx exec`
//! scans their listening ports, and every tunnelled connection is an
//! `sbx exec -i <name> socat - TCP:…` bridge on an SSH exec channel. That
//! needs nothing published on the host (`sbx ports`), reaches services bound
//! to the sandbox's loopback, and lets the same local port follow whichever
//! sandbox the user switches to.
//!
//! A sandbox can also *advertise* ports (`/etc/nettle/ports` for defaults,
//! `~/.config/nettle/ports` for what the agent adds, one `PORT [label]` per
//! line). Advertised ports are forwarded to the same local port as soon as
//! they listen, unless that local port is already taken.
//!
//! Background services run through `sbx-svc` in the sandbox (sbx-forge's
//! services kit); their state comes with the scan and nettle can start, stop
//! and tail them. A service's port counts as advertised.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::config::ConfigStore;
use crate::ipc::types::{
    AdvertisedPort, LogLevel, RemotePort, SandboxInfo, SandboxService, SandboxesChanged,
};
use crate::ports::docker;
use crate::ports::forwards::{ForwardManager, Target};
use crate::ports::parse;
use crate::ssh::{exec_capture, ConnectionEpoch, EpochRx};
use crate::state::UiBridge;

const SCAN_INTERVAL: Duration = Duration::from_secs(5);
/// How often to look for `sbx` again on a host that doesn't have it.
const UNAVAILABLE_RECHECK: Duration = Duration::from_secs(60);

/// Non-interactive SSH sessions often lack the dirs `sbx` gets installed to.
const SBX_PATH: &str = "PATH=\"$PATH:/usr/local/bin:/opt/homebrew/bin:$HOME/.local/bin\"";

/// Listening ports per running sandbox: name → (port → bind address).
pub type SandboxPorts = HashMap<String, HashMap<u16, String>>;

/// Sandbox names allow letters, digits, `.`, `_` and `-`. Anything else is
/// refused before it gets near a shell command line.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

#[derive(Deserialize)]
struct LsEntry {
    name: String,
    #[serde(default)]
    agent: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    workspaces: Vec<String>,
}

/// Parse `sbx ls --json`, dropping names we wouldn't dare put on a command line.
pub fn parse_ls(output: &str) -> Option<Vec<SandboxInfo>> {
    let entries: Vec<LsEntry> = serde_json::from_str(output.trim()).ok()?;
    let mut out: Vec<SandboxInfo> = entries
        .into_iter()
        .filter(|e| valid_name(&e.name))
        .map(|e| SandboxInfo {
            name: e.name,
            agent: e.agent,
            status: e.status,
            workspaces: e.workspaces,
            ports: Vec::new(),
            advertised: Vec::new(),
            services: Vec::new(),
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Some(out)
}

/// Runs inside a sandbox: listening sockets (`ss` when the image has it,
/// otherwise procfs), the advertised ports (defaults first, so the agent's
/// file wins), its `sbx-svc` services and the containers its docker daemon
/// publishes. Must not contain
/// single quotes — it is wrapped in `sh -c '…'`.
const INNER_SCAN: &str = "if command -v ss >/dev/null 2>&1; then echo @@SS; ss -tlnp 2>/dev/null; \
else echo @@PROC; cat /proc/net/tcp /proc/net/tcp6 2>/dev/null; fi; \
echo @@ADV; cat /etc/nettle/ports ~/.config/nettle/ports 2>/dev/null; echo; \
echo @@SVC; sbx-svc ls --tsv 2>/dev/null; \
echo @@DOCKER; docker ps --format \"{{.Names}}|{{.Ports}}\" 2>/dev/null";

/// One host command scanning every given (running) sandbox in turn. Never
/// pass stopped sandboxes: `sbx exec` would boot them.
pub fn scan_command(names: &[&str]) -> String {
    let mut cmd = format!("{SBX_PATH}; ");
    for name in names.iter().filter(|n| valid_name(n)) {
        cmd.push_str(&format!(
            "echo '@@SBX {name}'; sbx exec {name} sh -c '{INNER_SCAN}' </dev/null 2>/dev/null; "
        ));
    }
    cmd.push_str("true");
    cmd
}

/// One sandbox's scan: listening ports, advertised ports, services.
#[derive(Debug, Default)]
pub struct SandboxScan {
    pub ports: Vec<RemotePort>,
    pub advertised: Vec<AdvertisedPort>,
    pub services: Vec<SandboxService>,
}

/// Split the combined scan output back into per-sandbox results.
pub fn parse_scan(output: &str) -> HashMap<String, SandboxScan> {
    #[derive(Clone, Copy)]
    enum Section {
        None,
        Ss,
        Proc,
        Adv,
        Svc,
        Docker,
    }
    #[derive(Default)]
    struct Raw {
        ss: String,
        proc_net: String,
        adv: String,
        svc: String,
        docker_ps: String,
    }
    let mut out = HashMap::new();
    let mut flush = |name: &Option<String>, raw: &Raw| {
        let Some(name) = name else { return };
        let mut rows = parse::parse_ss(&raw.ss);
        rows.extend(parse::parse_proc_net(&raw.proc_net));
        let mut rows = parse::dedupe_by_port(rows);
        docker::annotate(&mut rows, &docker::parse_docker_ps(&raw.docker_ps));
        let services = parse_services(&raw.svc);
        let mut advertised = parse_advertised(&raw.adv);
        for svc in &services {
            if let Some(port) = svc.port {
                if !advertised.iter().any(|a| a.port == port) {
                    advertised.push(AdvertisedPort {
                        port,
                        label: Some(svc.name.clone()),
                    });
                }
            }
        }
        advertised.sort_by_key(|a| a.port);
        let scan = SandboxScan {
            ports: rows,
            advertised,
            services,
        };
        out.insert(name.clone(), scan);
    };

    let mut name: Option<String> = None;
    let mut section = Section::None;
    let mut raw = Raw::default();
    for line in output.lines() {
        if let Some(next) = line.strip_prefix("@@SBX ") {
            flush(&name, &raw);
            raw = Raw::default();
            name = Some(next.trim().to_string());
            section = Section::None;
            continue;
        }
        match line.trim() {
            "@@SS" => section = Section::Ss,
            "@@PROC" => section = Section::Proc,
            "@@ADV" => section = Section::Adv,
            "@@SVC" => section = Section::Svc,
            "@@DOCKER" => section = Section::Docker,
            _ => {
                let buf = match section {
                    Section::Ss => &mut raw.ss,
                    Section::Proc => &mut raw.proc_net,
                    Section::Adv => &mut raw.adv,
                    Section::Svc => &mut raw.svc,
                    Section::Docker => &mut raw.docker_ps,
                    Section::None => continue,
                };
                buf.push_str(line);
                buf.push('\n');
            }
        }
    }
    flush(&name, &raw);
    out
}

/// Parse advertised ports: `PORT [label]` per line, `#` comments. A later
/// line for the same port wins (the agent's file overrides the defaults).
pub fn parse_advertised(text: &str) -> Vec<AdvertisedPort> {
    let mut by_port: HashMap<u16, Option<String>> = HashMap::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let mut parts = line.splitn(2, char::is_whitespace);
        let Some(port) = parts.next().and_then(|p| p.parse::<u16>().ok()) else {
            continue;
        };
        if port == 0 {
            continue;
        }
        let label: String = parts
            .next()
            .unwrap_or("")
            .trim()
            .chars()
            .filter(|c| !c.is_control())
            .take(60)
            .collect();
        by_port.insert(port, (!label.is_empty()).then_some(label));
    }
    let mut out: Vec<AdvertisedPort> = by_port
        .into_iter()
        .map(|(port, label)| AdvertisedPort { port, label })
        .collect();
    out.sort_by_key(|a| a.port);
    out
}

/// Parse `sbx-svc ls --tsv`: NAME, running|stopped, PORT or -, yes|no, COMMAND.
pub fn parse_services(text: &str) -> Vec<SandboxService> {
    text.lines()
        .filter_map(|line| {
            let mut f = line.splitn(5, '\t');
            let name = f.next()?.trim();
            if !valid_name(name) {
                return None;
            }
            let running = f.next()? == "running";
            let port = f.next()?.parse::<u16>().ok().filter(|p| *p != 0);
            let autostart = f.next()? == "yes";
            let command = f.next().unwrap_or("").to_string();
            Some(SandboxService {
                name: name.to_string(),
                running,
                port,
                autostart,
                command,
            })
        })
        .collect()
}

/// `sbx-svc` actions nettle runs in a sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ServiceAction {
    Start,
    Stop,
    Restart,
}

/// Run a service action in the sandbox, then list its services again.
pub fn service_command(sandbox: &str, service: &str, action: ServiceAction) -> String {
    let verb = match action {
        ServiceAction::Start => "start",
        ServiceAction::Stop => "stop",
        ServiceAction::Restart => "restart",
    };
    exec_in_command(
        sandbox,
        &format!("sbx-svc {verb} {service} >&2; echo @@SVC; sbx-svc ls --tsv"),
    )
}

/// The last `lines` lines of a service's log.
pub fn service_logs_command(sandbox: &str, service: &str, lines: u32) -> String {
    exec_in_command(sandbox, &format!("sbx-svc logs {service} -n {lines} 2>&1"))
}

/// Forward newly listening advertised ports to the same local port. Each
/// (sandbox, port) is tried once while it stays advertised, so a forward the
/// user stops stays stopped; a local port that already tunnels elsewhere is
/// left alone (the UI offers "switch here").
async fn auto_forward(
    ui: &UiBridge,
    host_id: uuid::Uuid,
    store: &ConfigStore,
    forwards: &Arc<ForwardManager>,
    state: &SandboxesChanged,
    tried: &mut HashSet<(String, u16)>,
) {
    let advertised: HashSet<(String, u16)> = state
        .sandboxes
        .iter()
        .flat_map(|s| s.advertised.iter().map(|a| (s.name.clone(), a.port)))
        .collect();
    tried.retain(|key| advertised.contains(key));

    let candidates: Vec<(&SandboxInfo, &AdvertisedPort)> = state
        .sandboxes
        .iter()
        .flat_map(|s| s.advertised.iter().map(move |a| (s, a)))
        .filter(|(s, a)| {
            s.ports.iter().any(|p| p.port == a.port) && !tried.contains(&(s.name.clone(), a.port))
        })
        .collect();
    if candidates.is_empty() || !store.load_state().await.settings.auto_forward_advertised {
        return;
    }
    for (sb, adv) in candidates {
        tried.insert((sb.name.clone(), adv.port));
        let target = Target {
            sandbox: Some(sb.name.clone()),
            port: adv.port,
        };
        if forwards.has_target(&target) {
            continue;
        }
        let what = adv
            .label
            .as_deref()
            .map(|l| format!(" ({l})"))
            .unwrap_or_default();
        if let Some(holder) = forwards.local_target(adv.port) {
            ui.log(
                LogLevel::Info,
                Some(host_id),
                "forward",
                format!(
                    "{target}{what} is advertised, but localhost:{} tunnels to {holder}; switch it from the ports tab",
                    adv.port
                ),
            );
            continue;
        }
        // Bind failures are logged by the forward manager.
        if forwards
            .set_target(target.clone(), adv.port, true, false)
            .await
            .is_ok()
        {
            ui.log(
                LogLevel::Info,
                Some(host_id),
                "forward",
                format!("auto-forwarded advertised {target}{what}"),
            );
        }
    }
}

/// Loopback address to dial inside the sandbox for a port bound to `bind`:
/// v6-only listeners need `::1`, everything else answers on 127.0.0.1.
fn loopback_for(bind: Option<&str>) -> &'static str {
    match bind {
        Some("::1") => "[::1]",
        _ => "127.0.0.1",
    }
}

/// Host command bridging the SSH exec channel's stdio to `port` inside the
/// sandbox. socat lingers 30 s after one side's EOF (matching the tunnel's
/// half-close linger); nc is the fallback for images without socat.
pub fn bridge_command(name: &str, port: u16, bind: Option<&str>) -> String {
    let addr = loopback_for(bind);
    let nc_addr = addr.trim_start_matches('[').trim_end_matches(']');
    format!(
        "{SBX_PATH}; exec sbx exec -i {name} sh -c 'if command -v socat >/dev/null 2>&1; \
then exec socat -t 30 - TCP:{addr}:{port}; fi; exec nc {nc_addr} {port}' 2>/dev/null"
    )
}

/// Run `cmd` (a shell command line without single quotes) inside a sandbox.
pub fn exec_in_command(name: &str, cmd: &str) -> String {
    format!("{SBX_PATH}; sbx exec {name} sh -c '{cmd}' </dev/null")
}

/// Watch the host's sandboxes: `sbx ls` plus a port scan of every running one,
/// published as `sandboxes-changed` and as the live-port map the forward
/// manager waits on.
pub fn spawn(
    ui: Arc<UiBridge>,
    host_id: uuid::Uuid,
    store: ConfigStore,
    mut epoch_rx: EpochRx,
    forwards: Arc<ForwardManager>,
    live_tx: watch::Sender<SandboxPorts>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        // Outlives epochs: a reconnect doesn't re-forward what the user stopped.
        let mut tried: HashSet<(String, u16)> = HashSet::new();
        loop {
            let epoch: Arc<ConnectionEpoch> = loop {
                if let Some(e) = epoch_rx.borrow().clone() {
                    break e;
                }
                if epoch_rx.changed().await.is_err() {
                    return;
                }
            };

            let mut prev: Option<SandboxesChanged> = None;
            loop {
                let state = tokio::select! {
                    _ = epoch.cancel.cancelled() => break,
                    r = scan_once(&epoch, host_id) => r,
                };
                // Exec failures mean the link is going away; the host scanner
                // reports that, we just retry on the next tick or epoch.
                let Ok(state) = state else {
                    tokio::select! {
                        _ = epoch.cancel.cancelled() => break,
                        _ = tokio::time::sleep(SCAN_INTERVAL) => {}
                    }
                    continue;
                };

                let was_available = prev.as_ref().map(|p| p.available);
                if was_available != Some(state.available) && state.available {
                    ui.log(
                        LogLevel::Info,
                        Some(host_id),
                        "sandbox",
                        format!("sbx found on host: {} sandbox(es)", state.sandboxes.len()),
                    );
                }
                if state.error.is_some() && prev.as_ref().map(|p| &p.error) != Some(&state.error) {
                    ui.log(
                        LogLevel::Warn,
                        Some(host_id),
                        "sandbox",
                        state.error.clone().unwrap_or_default(),
                    );
                }
                let interval = if state.available {
                    SCAN_INTERVAL
                } else {
                    UNAVAILABLE_RECHECK
                };

                if prev.as_ref() != Some(&state) {
                    let live: SandboxPorts = state
                        .sandboxes
                        .iter()
                        .map(|s| {
                            let ports = s.ports.iter().map(|p| (p.port, p.bind.clone())).collect();
                            (s.name.clone(), ports)
                        })
                        .collect();
                    // A failed `sbx ls` keeps the last known ports rather
                    // than flapping every tunnel to "waiting".
                    if state.error.is_none() {
                        let _ = live_tx.send(live);
                    }
                    ui.emit_sandboxes(&state);
                    forwards.on_ports_update(&HashSet::new());
                    if state.error.is_none() {
                        auto_forward(&ui, host_id, &store, &forwards, &state, &mut tried).await;
                    }
                    prev = Some(state);
                }

                tokio::select! {
                    _ = epoch.cancel.cancelled() => break,
                    _ = tokio::time::sleep(interval) => {}
                }
            }

            if epoch_rx.changed().await.is_err() {
                return;
            }
        }
    })
}

async fn scan_once(
    epoch: &ConnectionEpoch,
    host_id: uuid::Uuid,
) -> crate::error::Result<SandboxesChanged> {
    let (out, exit) = exec_capture(
        &epoch.handle,
        &format!(
            "{SBX_PATH}; command -v sbx >/dev/null 2>&1 || exit 127; sbx ls --json 2>/dev/null"
        ),
    )
    .await?;
    let mut state = SandboxesChanged {
        host_id,
        available: exit != Some(127),
        error: None,
        sandboxes: Vec::new(),
    };
    if !state.available {
        return Ok(state);
    }
    let Some(mut sandboxes) = parse_ls(&out).filter(|_| exit == Some(0)) else {
        state.error = Some(format!(
            "`sbx ls` failed on the host (exit {}) — is the sandbox daemon running and are you signed in?",
            exit.map(|e| e.to_string()).unwrap_or_else(|| "?".into())
        ));
        return Ok(state);
    };

    let running: Vec<&str> = sandboxes
        .iter()
        .filter(|s| s.status == "running")
        .map(|s| s.name.as_str())
        .collect();
    if !running.is_empty() {
        let (out, _) = exec_capture(&epoch.handle, &scan_command(&running)).await?;
        let mut scans = parse_scan(&out);
        for s in sandboxes.iter_mut() {
            let scan = scans.remove(&s.name).unwrap_or_default();
            s.ports = scan.ports;
            s.advertised = scan.advertised;
            s.services = scan.services;
        }
    }
    state.sandboxes = sandboxes;
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_validated() {
        assert!(valid_name("claude-github-sbx-forge"));
        assert!(valid_name("a.b_c-1"));
        assert!(!valid_name(""));
        assert!(!valid_name("-rf"));
        assert!(!valid_name("a b"));
        assert!(!valid_name("a';rm -rf ~;'"));
        assert!(!valid_name("$(id)"));
    }

    #[test]
    fn parses_sbx_ls_json() {
        let out = r#"[
  {"name": "claude-b", "id": "x", "agent": "claude", "status": "stopped",
   "workspaces": ["/home/u/b"], "created_at": "2026-10-25T21:17:26Z"},
  {"name": "claude-a", "agent": "claude", "status": "running", "workspaces": ["/home/u/a"]},
  {"name": "bad name", "status": "running"}
]"#;
        let list = parse_ls(out).unwrap();
        assert_eq!(list.len(), 2, "invalid names are dropped");
        assert_eq!(list[0].name, "claude-a");
        assert_eq!(list[0].status, "running");
        assert_eq!(list[1].workspaces, vec!["/home/u/b".to_string()]);
        assert!(parse_ls("not json").is_none());
        assert_eq!(parse_ls("[]").unwrap().len(), 0);
    }

    #[test]
    fn scan_command_only_includes_valid_names() {
        let cmd = scan_command(&["a", "b;id"]);
        assert!(cmd.contains("@@SBX a'"));
        assert!(cmd.contains("sbx exec a sh -c '"));
        assert!(!cmd.contains("b;id"));
        assert!(!INNER_SCAN.contains('\''), "inner script is single-quoted");
    }

    #[test]
    fn splits_scan_output_per_sandbox() {
        let out = "@@SBX a\n\
@@PROC\n\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 0100007F:1054 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 468599 1\n\
   1: 030011AC:BEE0 020011AC:0C38 01 00000000:00000000 02:00000A9A 00000000  1000        0 136332 2\n\
   2: 00000000:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1 1\n\
@@DOCKER\n\
web|0.0.0.0:8080->80/tcp\n\
@@SBX b\n\
@@SS\n\
State  Recv-Q Send-Q Local Address:Port Peer Address:Port Process\n\
LISTEN 0      511          *:5173            *:*     users:((\"node\",pid=42,fd=20))\n\
@@DOCKER\n";
        let map = parse_scan(out);
        let a = &map["a"].ports;
        assert_eq!(
            a.iter().map(|p| p.port).collect::<Vec<_>>(),
            vec![4180, 8080]
        );
        assert_eq!(a[0].bind, "127.0.0.1");
        assert_eq!(a[1].container.as_deref(), Some("web"));
        let b = &map["b"].ports;
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].port, 5173);
        assert_eq!(b[0].process.as_deref(), Some("node"));
    }

    #[test]
    fn sandbox_without_output_has_no_ports() {
        let map = parse_scan("@@SBX a\n@@SBX b\n@@PROC\n");
        assert!(map["a"].ports.is_empty());
        assert!(map["b"].ports.is_empty());
        assert!(map["b"].advertised.is_empty());
    }

    #[test]
    fn reads_advertised_ports_per_sandbox() {
        let out = "@@SBX a\n@@PROC\n@@ADV\n4180 oauth2-proxy\n3000\n\n@@DOCKER\n\
@@SBX b\n@@PROC\n@@ADV\n\n@@DOCKER\n";
        let map = parse_scan(out);
        let a = &map["a"].advertised;
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].port, 3000);
        assert_eq!(a[0].label, None);
        assert_eq!(a[1].label.as_deref(), Some("oauth2-proxy"));
        assert!(map["b"].advertised.is_empty());
    }

    #[test]
    fn reads_services_and_advertises_their_ports() {
        let out = "@@SBX a\n@@PROC\n@@ADV\n4180 oauth2-proxy\n\n@@SVC\n\
web\trunning\t5173\tyes\tnpm run dev -- --host\n\
proxy\tstopped\t4180\tno\toauth2-proxy --config x\n\
worker\tstopped\t-\tno\twhile true; do echo\tx; done\n\
bad name\trunning\t1\tyes\tx\n\
@@DOCKER\n";
        let a = &parse_scan(out)["a"];
        let names: Vec<&str> = a.services.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["web", "proxy", "worker"]);
        assert!(a.services[0].running && a.services[0].autostart);
        assert_eq!(a.services[0].port, Some(5173));
        assert_eq!(a.services[0].command, "npm run dev -- --host");
        assert_eq!(a.services[2].port, None);
        assert_eq!(a.services[2].command, "while true; do echo\tx; done");
        // service ports are advertised; an explicit advertisement keeps its label
        let adv: Vec<(u16, Option<&str>)> = a
            .advertised
            .iter()
            .map(|x| (x.port, x.label.as_deref()))
            .collect();
        assert_eq!(adv, vec![(4180, Some("oauth2-proxy")), (5173, Some("web"))]);
    }

    #[test]
    fn service_commands_quote_safely() {
        let c = service_command("sb-a", "web", ServiceAction::Restart);
        assert!(c.contains(
            "sbx exec sb-a sh -c 'sbx-svc restart web >&2; echo @@SVC; sbx-svc ls --tsv'"
        ));
        let l = service_logs_command("sb-a", "web", 300);
        assert!(l.contains("sh -c 'sbx-svc logs web -n 300 2>&1'"));
    }

    #[test]
    fn parses_advertised_lines() {
        let text = "# defaults\n4180 oauth2 proxy  # login\n8080\n\
nope\n0 zero\n70000 too big\n  5173\tvite dev\n8080 api\n";
        let adv = parse_advertised(text);
        let got: Vec<(u16, Option<&str>)> =
            adv.iter().map(|a| (a.port, a.label.as_deref())).collect();
        assert_eq!(
            got,
            vec![
                (4180, Some("oauth2 proxy")),
                (5173, Some("vite dev")),
                // the later line wins: the agent's file overrides the defaults
                (8080, Some("api")),
            ]
        );
        let long = format!("9000 {}\u{7}", "x".repeat(100));
        assert_eq!(parse_advertised(&long)[0].label.as_ref().unwrap().len(), 60);
    }

    #[test]
    fn bridge_dials_matching_loopback() {
        let v4 = bridge_command("a", 4180, Some("0.0.0.0"));
        assert!(v4.contains("exec sbx exec -i a sh -c '"));
        assert!(v4.contains("TCP:127.0.0.1:4180"));
        assert!(v4.contains("exec nc 127.0.0.1 4180'"));
        let v6 = bridge_command("a", 4180, Some("::1"));
        assert!(v6.contains("TCP:[::1]:4180"));
        assert!(v6.contains("exec nc ::1 4180'"));
    }
}
