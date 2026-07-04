use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum NettleError {
    #[error("ssh error: {0}")]
    Ssh(#[from] russh::Error),
    #[error("sftp error: {0}")]
    Sftp(#[from] russh_sftp::client::error::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("key error: {0}")]
    Keys(#[from] russh::keys::Error),
    #[error("authentication failed: no accepted method")]
    AuthFailed,
    #[error("authentication cancelled")]
    AuthCancelled,
    #[error("could not resolve {0}")]
    Dns(String),
    #[error("connection timed out")]
    Timeout,
    #[error("not connected")]
    NotConnected,
    #[error("{0}")]
    Msg(String),
}

impl NettleError {
    pub fn code(&self) -> &'static str {
        match self {
            NettleError::Ssh(_) => "ssh",
            NettleError::Sftp(_) => "sftp",
            NettleError::Io(_) => "io",
            NettleError::Json(_) => "json",
            NettleError::Keys(_) => "keys",
            NettleError::AuthFailed => "auth_failed",
            NettleError::AuthCancelled => "auth_cancelled",
            NettleError::Dns(_) => "dns",
            NettleError::Timeout => "timeout",
            NettleError::NotConnected => "not_connected",
            NettleError::Msg(_) => "error",
        }
    }
}

impl Serialize for NettleError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("NettleError", 2)?;
        s.serialize_field("code", self.code())?;
        s.serialize_field("message", &self.to_string())?;
        s.end()
    }
}

pub type Result<T, E = NettleError> = std::result::Result<T, E>;
