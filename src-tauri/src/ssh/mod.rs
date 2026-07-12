pub mod auth;
pub mod dns;
pub mod handler;
pub mod session;

use std::net::IpAddr;
use std::sync::Arc;

use russh::client;
use russh::ChannelMsg;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
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

/// Best-effort detection of what a remote loopback port is serving. Opens a
/// direct-tcpip channel to `127.0.0.1:port`, sends a plaintext HTTP request and
/// inspects the first response bytes: an HTTPS server rejects the cleartext
/// request with a TLS record (handshake 0x16 / alert 0x15, version 0x03xx),
/// while an HTTP server answers `HTTP/…`. Defaults to `"http"` when ambiguous.
pub async fn probe_http_scheme(
    handle: &client::Handle<ClientHandler>,
    port: u16,
) -> Result<&'static str> {
    let channel = handle
        .channel_open_direct_tcpip("127.0.0.1", port as u32, "127.0.0.1", 0)
        .await?;
    let mut stream = channel.into_stream();
    stream
        .write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await?;

    let mut buf = [0u8; 16];
    let n = match tokio::time::timeout(std::time::Duration::from_secs(3), stream.read(&mut buf))
        .await
    {
        Ok(Ok(n)) => n,
        _ => 0,
    };

    // TLS record header: content type handshake/alert + version major 0x03.
    if n >= 2 && (buf[0] == 0x16 || buf[0] == 0x15) && buf[1] == 0x03 {
        return Ok("https");
    }
    Ok("http")
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
