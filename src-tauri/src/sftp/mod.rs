pub mod browse;
pub mod transfers;

use russh_sftp::client::SftpSession;

use crate::error::Result;
use crate::ssh::ConnectionEpoch;

/// Open a dedicated SFTP session on its own channel (channels are multiplexed,
/// so this is cheap and avoids head-of-line blocking between subsystems).
pub async fn open_sftp(epoch: &ConnectionEpoch) -> Result<SftpSession> {
    let channel = epoch.handle.channel_open_session().await?;
    channel.request_subsystem(true, "sftp").await?;
    Ok(SftpSession::new(channel.into_stream()).await?)
}
