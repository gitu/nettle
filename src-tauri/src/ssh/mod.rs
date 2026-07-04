pub mod auth;
pub mod dns;
pub mod handler;
pub mod session;

use std::net::IpAddr;
use std::sync::Arc;

use russh::client;
use russh::ChannelMsg;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::error::Result;
use handler::ClientHandler;

/// One successful connection. Recreated (with a bumped id) after every reconnect.
/// Subsystems must race every long-lived await against `cancel`.
pub struct ConnectionEpoch {
    pub id: u64,
    pub handle: client::Handle<ClientHandler>,
    pub cancel: CancellationToken,
    pub resolved_ip: IpAddr,
    pub connected_at_ms: u64,
}

pub type EpochRx = watch::Receiver<Option<Arc<ConnectionEpoch>>>;
pub type EpochTx = watch::Sender<Option<Arc<ConnectionEpoch>>>;

/// Grab the current epoch if connected.
pub fn current_epoch(rx: &EpochRx) -> Option<Arc<ConnectionEpoch>> {
    rx.borrow().clone()
}

/// Run a command over a fresh exec channel and capture stdout + exit code.
pub async fn exec_capture(
    handle: &client::Handle<ClientHandler>,
    cmd: &str,
) -> Result<(String, Option<u32>)> {
    let mut channel = handle.channel_open_session().await?;
    channel.exec(true, cmd).await?;
    let mut out = Vec::new();
    let mut exit = None;
    while let Some(msg) = channel.wait().await {
        match msg {
            ChannelMsg::Data { ref data } => out.extend_from_slice(data),
            ChannelMsg::ExitStatus { exit_status } => exit = Some(exit_status),
            ChannelMsg::Close => break,
            _ => {}
        }
    }
    Ok((String::from_utf8_lossy(&out).into_owned(), exit))
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
