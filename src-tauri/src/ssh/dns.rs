use std::net::SocketAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::{NettleError, Result};

/// Resolve fresh on every call — the whole point of Nettle's reconnect is that
/// the remote may come back on a different address.
pub async fn resolve(hostname: &str, port: u16) -> Result<Vec<SocketAddr>> {
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((hostname, port))
        .await
        .map_err(|_| NettleError::Dns(hostname.to_string()))?
        .collect();
    if addrs.is_empty() {
        return Err(NettleError::Dns(hostname.to_string()));
    }
    Ok(addrs)
}

/// Capped exponential backoff with cheap deterministic-ish jitter (no rand dep).
pub fn backoff_delay(attempt: u32) -> Duration {
    const STEPS: [u64; 6] = [1, 2, 4, 8, 15, 30];
    let base = STEPS[(attempt.saturating_sub(1) as usize).min(STEPS.len() - 1)];
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let jitter_ms = nanos % (base * 200); // up to 20% of base
    Duration::from_millis(base * 1000 + jitter_ms)
}
