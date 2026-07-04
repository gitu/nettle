//! End-to-end backend tests against a real sshd.
//!
//! These are gated behind the `NETTLE_E2E` env var and expect a disposable
//! ssh server (password auth) to be reachable, e.g.:
//!
//! ```sh
//! docker run -d --name nettle-sshd -p 2222:2222 \
//!   -e PASSWORD_ACCESS=true -e USER_PASSWORD=nettletest -e USER_NAME=deploy \
//!   lscr.io/linuxserver/openssh-server:latest
//! # the image ships with AllowTcpForwarding no — the tunnel test needs it on:
//! docker exec nettle-sshd sed -i \
//!   's/^AllowTcpForwarding no/AllowTcpForwarding yes/' /config/sshd/sshd_config
//! docker exec nettle-sshd sh -c 'kill -HUP $(cat /config/sshd.pid)'
//! NETTLE_E2E=1 cargo test --test e2e
//! ```
//!
//! Override the target with NETTLE_E2E_HOST / NETTLE_E2E_PORT /
//! NETTLE_E2E_USER / NETTLE_E2E_PASSWORD.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, watch};
use uuid::Uuid;

use nettle_lib::config::{ConfigStore, HostConfig};
use nettle_lib::ipc::types::{TransferDirection, TransferStatus};
use nettle_lib::ports::forwards::ForwardManager;
use nettle_lib::sftp::browse::SftpBrowser;
use nettle_lib::sftp::transfers::TransferManager;
use nettle_lib::ssh::session::{SessionActor, SessionCmd};
use nettle_lib::ssh::{exec_capture, ConnectionEpoch, EpochRx};
use nettle_lib::state::{EventSink, UiBridge};

struct TestSink {
    tx: mpsc::UnboundedSender<(String, serde_json::Value)>,
}

impl EventSink for TestSink {
    fn emit_json(&self, event: &str, payload: serde_json::Value) {
        let _ = self.tx.send((event.to_string(), payload));
    }
}

struct Harness {
    ui: Arc<UiBridge>,
    events: mpsc::UnboundedReceiver<(String, serde_json::Value)>,
    cmd_tx: mpsc::UnboundedSender<SessionCmd>,
    epoch_rx: EpochRx,
    store: ConfigStore,
    host: HostConfig,
    _tmp: tempfile::TempDir,
}

fn enabled() -> bool {
    if std::env::var("NETTLE_E2E").is_ok() {
        return true;
    }
    eprintln!("skipping: set NETTLE_E2E=1 (needs a test sshd, see file header)");
    false
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

async fn connect() -> Harness {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = ConfigStore::new(tmp.path().to_path_buf());

    let (tx, events) = mpsc::unbounded_channel();
    let ui = UiBridge::new(Box::new(TestSink { tx }));

    let host = HostConfig {
        id: Uuid::new_v4(),
        name: "e2e".into(),
        hostname: env_or("NETTLE_E2E_HOST", "localhost"),
        port: env_or("NETTLE_E2E_PORT", "2222").parse().unwrap(),
        username: env_or("NETTLE_E2E_USER", "deploy"),
        key_path: None,
    };
    let password = env_or("NETTLE_E2E_PASSWORD", "nettletest");

    // Auto-answer prompts: accept the host key (TOFU) and supply the password.
    let responder_ui = ui.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(150)).await;
            responder_ui.prompts.answer_host_key(true);
            responder_ui.prompts.answer_secret(Some(password.clone()));
        }
    });

    let (cmd_tx, epoch_rx, _task) =
        SessionActor::spawn(ui.clone(), host.clone(), store.known_hosts_path());

    Harness {
        ui,
        events,
        cmd_tx,
        epoch_rx,
        store,
        host,
        _tmp: tmp,
    }
}

async fn wait_epoch(rx: &mut EpochRx, min_id: u64) -> Arc<ConnectionEpoch> {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Some(e) = rx.borrow().clone() {
                if e.id >= min_id {
                    return e;
                }
            }
            rx.changed().await.expect("session actor died");
        }
    })
    .await
    .expect("timed out waiting for connection epoch")
}

#[tokio::test]
async fn connect_exec_disconnect() {
    if !enabled() {
        return;
    }
    let mut h = connect().await;
    let epoch = wait_epoch(&mut h.epoch_rx, 1).await;

    let (out, exit) = exec_capture(&epoch.handle, "echo nettle-ok").await.unwrap();
    assert!(out.contains("nettle-ok"), "unexpected output: {out}");
    assert_eq!(exit, Some(0));

    h.cmd_tx.send(SessionCmd::Disconnect).unwrap();
    // Expect a disconnected event to land.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline - tokio::time::Instant::now();
        let (event, payload) = tokio::time::timeout(remaining, h.events.recv())
            .await
            .expect("no disconnected event")
            .expect("event stream closed");
        if event == "connection-state" && payload["state"] == "disconnected" {
            break;
        }
    }
}

#[tokio::test]
async fn tofu_host_key_is_learned() {
    if !enabled() {
        return;
    }
    let mut h = connect().await;
    wait_epoch(&mut h.epoch_rx, 1).await;
    let known_hosts = std::fs::read_to_string(h.store.known_hosts_path())
        .expect("known_hosts file should exist after TOFU accept");
    assert!(
        known_hosts.contains(&h.host.hostname) || known_hosts.contains("ssh-"),
        "known_hosts should contain the learned key: {known_hosts}"
    );
    h.cmd_tx.send(SessionCmd::Disconnect).unwrap();
}

#[tokio::test]
async fn terminal_shell_roundtrip() {
    if !enabled() {
        return;
    }
    let mut h = connect().await;
    wait_epoch(&mut h.epoch_rx, 1).await;

    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let chan = tauri::ipc::Channel::new(move |body| {
        if let tauri::ipc::InvokeResponseBody::Raw(bytes) = body {
            let _ = out_tx.send(bytes);
        }
        Ok(())
    });
    let term = nettle_lib::terminal::open(
        h.ui.clone(),
        h.epoch_rx.clone(),
        h.cmd_tx.clone(),
        120,
        30,
        chan,
    );

    term.write(b"echo term-roundtrip-$((20+3))\n".to_vec());

    let mut collected = String::new();
    let found = tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(bytes) = out_rx.recv().await {
            collected.push_str(&String::from_utf8_lossy(&bytes));
            if collected.contains("term-roundtrip-23") {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    assert!(found, "terminal output missing echo result: {collected}");

    term.close();
    h.cmd_tx.send(SessionCmd::Disconnect).unwrap();
}

#[tokio::test]
async fn sftp_upload_download_roundtrip() {
    if !enabled() {
        return;
    }
    let mut h = connect().await;
    wait_epoch(&mut h.epoch_rx, 1).await;

    let browser = SftpBrowser::new(h.epoch_rx.clone());
    let home = browser.home().await.expect("sftp home");

    // Build a 1 MiB patterned payload.
    let payload: Vec<u8> = (0..1024 * 1024).map(|i| (i % 251) as u8).collect();
    let up_src = h._tmp.path().join("upload-src.bin");
    std::fs::write(&up_src, &payload).unwrap();
    let remote_path = format!("{home}/nettle-e2e.bin");
    let down_dst = h._tmp.path().join("download-dst.bin");

    let transfers = TransferManager::new(h.ui.clone(), h.epoch_rx.clone());

    let progress = tauri::ipc::Channel::new(|_| Ok(()));
    let up_id = transfers.start(
        TransferDirection::Up,
        remote_path.clone(),
        up_src.to_string_lossy().into_owned(),
        progress,
    );
    wait_transfer(&transfers, up_id).await;

    // The uploaded file must appear in the listing with the right size.
    let listing = browser.list(&home).await.unwrap();
    let entry = listing
        .entries
        .iter()
        .find(|e| e.name == "nettle-e2e.bin")
        .expect("uploaded file in remote listing");
    assert_eq!(entry.size, Some(payload.len() as u64));

    let progress = tauri::ipc::Channel::new(|_| Ok(()));
    let down_id = transfers.start(
        TransferDirection::Down,
        remote_path.clone(),
        down_dst.to_string_lossy().into_owned(),
        progress,
    );
    wait_transfer(&transfers, down_id).await;

    let roundtripped = std::fs::read(&down_dst).unwrap();
    assert_eq!(
        roundtripped, payload,
        "downloaded bytes differ from uploaded"
    );

    let epoch = wait_epoch(&mut h.epoch_rx, 1).await;
    let _ = exec_capture(&epoch.handle, &format!("rm -f {remote_path}")).await;
    h.cmd_tx.send(SessionCmd::Disconnect).unwrap();
}

async fn wait_transfer(transfers: &Arc<TransferManager>, id: Uuid) {
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let status = transfers
                .list()
                .into_iter()
                .find(|t| t.id == id)
                .map(|t| t.status);
            match status {
                Some(TransferStatus::Done) => return,
                Some(TransferStatus::Failed) | Some(TransferStatus::Cancelled) => {
                    let err = transfers
                        .list()
                        .into_iter()
                        .find(|t| t.id == id)
                        .and_then(|t| t.error);
                    panic!("transfer did not complete: {err:?}");
                }
                _ => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
    })
    .await
    .expect("transfer timed out");
}

#[tokio::test]
async fn forward_tunnel_to_remote_sshd() {
    if !enabled() {
        return;
    }
    let mut h = connect().await;
    wait_epoch(&mut h.epoch_rx, 1).await;

    let remote_port: u16 = env_or("NETTLE_E2E_REMOTE_SSH_PORT", "2222")
        .parse()
        .unwrap();
    let local_port: u16 = 43222;

    let (live_tx, live_rx) = watch::channel(HashSet::from([remote_port]));
    let forwards = ForwardManager::new(
        h.ui.clone(),
        h.host.id,
        h.store.clone(),
        h.epoch_rx.clone(),
        live_rx,
    );
    forwards
        .set_with_local(remote_port, local_port, true, false)
        .await
        .expect("bind local forward");

    // Connect through the tunnel to the remote's own sshd; expect its banner.
    let mut sock = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::net::TcpStream::connect(("127.0.0.1", local_port)),
    )
    .await
    .expect("connect timeout")
    .expect("tcp connect through tunnel");

    let mut banner = vec![0u8; 64];
    let n = tokio::time::timeout(Duration::from_secs(10), sock.read(&mut banner))
        .await
        .expect("banner read timeout")
        .expect("banner read");
    let banner = String::from_utf8_lossy(&banner[..n]);
    assert!(
        banner.starts_with("SSH-2.0"),
        "expected SSH banner through tunnel, got: {banner:?}"
    );
    let _ = sock.shutdown().await;

    drop(live_tx);
    forwards.shutdown();
    h.cmd_tx.send(SessionCmd::Disconnect).unwrap();
}

#[tokio::test]
async fn reconnect_creates_new_epoch_with_cached_auth() {
    if !enabled() {
        return;
    }
    let mut h = connect().await;
    let first = wait_epoch(&mut h.epoch_rx, 1).await;
    assert_eq!(first.id, 1);

    // Simulate a dead link; the actor must tear down and reconnect using the
    // cached password (no interactive prompt is allowed mid-reconnect).
    h.cmd_tx.send(SessionCmd::SuspectDead(first.id)).unwrap();

    let second = wait_epoch(&mut h.epoch_rx, 2).await;
    assert!(second.id >= 2, "expected a fresh epoch after reconnect");
    assert!(first.cancel.is_cancelled(), "old epoch must be cancelled");

    let (out, _) = exec_capture(&second.handle, "echo reconnected-ok")
        .await
        .expect("exec on new epoch");
    assert!(out.contains("reconnected-ok"));

    h.cmd_tx.send(SessionCmd::Disconnect).unwrap();
}
