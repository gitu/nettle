use std::sync::Arc;
use std::time::Duration;

use russh::ChannelMsg;
use tauri::ipc::{Channel, InvokeResponseBody};
use tokio::sync::mpsc;

use crate::ssh::session::SessionCmd;
use crate::ssh::{ConnectionEpoch, EpochRx};
use crate::state::UiBridge;

const FLUSH_BYTES: usize = 8 * 1024;
const FLUSH_MS: u64 = 8;

#[derive(Debug)]
enum TermCmd {
    Write(Vec<u8>),
    Resize(u32, u32),
    Close,
}

pub struct TerminalHandle {
    tx: mpsc::UnboundedSender<TermCmd>,
}

impl TerminalHandle {
    pub fn write(&self, data: Vec<u8>) {
        let _ = self.tx.send(TermCmd::Write(data));
    }
    pub fn resize(&self, cols: u32, rows: u32) {
        let _ = self.tx.send(TermCmd::Resize(cols, rows));
    }
    pub fn close(&self) {
        let _ = self.tx.send(TermCmd::Close);
    }
}

/// Spawn the terminal task: opens a PTY shell on the current epoch and streams
/// output to the frontend. Survives reconnects by reopening the shell on each
/// new epoch, announcing what happened inline.
pub fn open(
    ui: std::sync::Arc<UiBridge>,
    mut epoch_rx: EpochRx,
    session_cmd: mpsc::UnboundedSender<SessionCmd>,
    cols: u32,
    rows: u32,
    on_data: Channel<InvokeResponseBody>,
) -> TerminalHandle {
    let (tx, mut rx) = mpsc::unbounded_channel::<TermCmd>();

    tokio::spawn(async move {
        let mut cols = cols;
        let mut rows = rows;
        let mut first_shell = true;

        'epochs: loop {
            // Wait for a live epoch.
            let epoch: Arc<ConnectionEpoch> = loop {
                if let Some(e) = epoch_rx.borrow().clone() {
                    break e;
                }
                if epoch_rx.changed().await.is_err() {
                    return; // session torn down
                }
            };

            if !first_shell {
                notice(&on_data, &format!("reconnected → {} · shell reopened", epoch.resolved_ip));
            }

            let channel = match open_shell(&epoch, cols, rows).await {
                Ok(c) => c,
                Err(_) => {
                    let _ = session_cmd.send(SessionCmd::SuspectDead(epoch.id));
                    // Wait for the epoch to change before retrying.
                    if epoch_rx.changed().await.is_err() {
                        return;
                    }
                    continue 'epochs;
                }
            };
            first_shell = false;
            let mut channel = channel;

            let mut pending: Vec<u8> = Vec::with_capacity(FLUSH_BYTES);
            loop {
                let flush_in = if pending.is_empty() {
                    Duration::from_secs(3600)
                } else {
                    Duration::from_millis(FLUSH_MS)
                };
                tokio::select! {
                    _ = epoch.cancel.cancelled() => {
                        flush(&on_data, &mut pending);
                        notice(&on_data, "link dropped — reconnecting…");
                        // Wait for the next epoch and reopen the shell.
                        if epoch_rx.changed().await.is_err() { return; }
                        continue 'epochs;
                    }
                    _ = tokio::time::sleep(flush_in) => flush(&on_data, &mut pending),
                    cmd = rx.recv() => match cmd {
                        Some(TermCmd::Write(data)) => {
                            if channel.data(&data[..]).await.is_err() {
                                let _ = session_cmd.send(SessionCmd::SuspectDead(epoch.id));
                            }
                        }
                        Some(TermCmd::Resize(c, r)) => {
                            cols = c;
                            rows = r;
                            let _ = channel.window_change(c, r, 0, 0).await;
                        }
                        Some(TermCmd::Close) | None => {
                            flush(&on_data, &mut pending);
                            let _ = channel.close().await;
                            return;
                        }
                    },
                    msg = channel.wait() => match msg {
                        Some(ChannelMsg::Data { ref data }) => {
                            pending.extend_from_slice(data);
                            if pending.len() >= FLUSH_BYTES {
                                flush(&on_data, &mut pending);
                            }
                        }
                        Some(ChannelMsg::ExtendedData { ref data, .. }) => {
                            pending.extend_from_slice(data);
                            if pending.len() >= FLUSH_BYTES {
                                flush(&on_data, &mut pending);
                            }
                        }
                        Some(ChannelMsg::ExitStatus { .. }) | Some(ChannelMsg::Close) | None => {
                            // Shell ended while the connection is alive (e.g. `exit`).
                            flush(&on_data, &mut pending);
                            ui.emit_term_closed();
                            return;
                        }
                        Some(_) => {}
                    },
                }
            }
        }
    });

    TerminalHandle { tx }
}

async fn open_shell(
    epoch: &ConnectionEpoch,
    cols: u32,
    rows: u32,
) -> Result<russh::Channel<russh::client::Msg>, russh::Error> {
    let channel = epoch.handle.channel_open_session().await?;
    channel
        .request_pty(true, "xterm-256color", cols, rows, 0, 0, &[])
        .await?;
    channel.request_shell(true).await?;
    Ok(channel)
}

fn flush(on_data: &Channel<InvokeResponseBody>, pending: &mut Vec<u8>) {
    if !pending.is_empty() {
        let _ = on_data.send(InvokeResponseBody::Raw(std::mem::take(pending)));
    }
}

fn notice(on_data: &Channel<InvokeResponseBody>, msg: &str) {
    let styled = format!("\r\n\x1b[2;35m[nettle] {msg}\x1b[0m\r\n");
    let _ = on_data.send(InvokeResponseBody::Raw(styled.into_bytes()));
}
