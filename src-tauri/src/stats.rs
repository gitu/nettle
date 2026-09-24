//! Per-host connection statistics for the app's runtime.
//!
//! Counters live outside the session lifecycle (on the `UiBridge`), so they
//! survive reconnects and manual disconnect/connect cycles and give an honest
//! picture of how a host has behaved since the app started: how often the link
//! dropped and why, how much traffic went through its tunnels, how the port
//! scanner is doing.
//!
//! Hot paths (tunnel bytes) bump atomics without taking a lock; the rare
//! lifecycle events go through a small mutex. A `dirty` flag lets a low-rate
//! ticker push snapshots to the UI only when something actually changed.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use uuid::Uuid;

use crate::ipc::types::ConnStats;
use crate::ssh::now_ms;

#[derive(Default)]
struct Meta {
    connects: u32,
    reconnects: u32,
    link_drops: u32,
    connect_failures: u32,
    connected_since_ms: Option<u64>,
    /// Connected time of links that already ended.
    connected_total_ms: u64,
    last_ip: Option<String>,
    last_drop_at_ms: Option<u64>,
    last_drop_reason: Option<String>,
    last_error: Option<String>,
}

pub struct HostStats {
    host_id: Uuid,
    since_ms: u64,
    // ---- tunnels (hot path, lock-free) ----
    pub tunnel_bytes_up: AtomicU64,
    pub tunnel_bytes_down: AtomicU64,
    pub tunnel_conns_total: AtomicU64,
    pub tunnel_conns_active: AtomicU64,
    pub tunnel_refused: AtomicU64,
    pub tunnel_wait_timeouts: AtomicU64,
    // ---- port scanner ----
    pub scans: AtomicU64,
    pub scan_failures: AtomicU64,
    pub last_scan_ms: AtomicU64,
    dirty: AtomicBool,
    meta: StdMutex<Meta>,
}

impl HostStats {
    fn new(host_id: Uuid) -> Self {
        Self {
            host_id,
            since_ms: now_ms(),
            tunnel_bytes_up: AtomicU64::new(0),
            tunnel_bytes_down: AtomicU64::new(0),
            tunnel_conns_total: AtomicU64::new(0),
            tunnel_conns_active: AtomicU64::new(0),
            tunnel_refused: AtomicU64::new(0),
            tunnel_wait_timeouts: AtomicU64::new(0),
            scans: AtomicU64::new(0),
            scan_failures: AtomicU64::new(0),
            last_scan_ms: AtomicU64::new(0),
            dirty: AtomicBool::new(false),
            meta: StdMutex::new(Meta::default()),
        }
    }

    /// Mark the snapshot stale so the next ticker pass pushes it to the UI.
    pub fn touch(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Clear and return the dirty flag.
    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::Relaxed)
    }

    /// A link came up. `epoch` 1 is a fresh session; anything higher is a
    /// reconnect of an existing one.
    pub fn on_connected(&self, ip: &str, epoch: u64) {
        let mut m = self.meta.lock().unwrap();
        if epoch > 1 {
            m.reconnects += 1;
        } else {
            m.connects += 1;
        }
        m.connected_since_ms = Some(now_ms());
        m.last_ip = Some(ip.to_string());
        m.last_error = None;
        drop(m);
        self.touch();
    }

    /// The link died underneath us (the actor will reconnect).
    pub fn on_link_lost(&self, reason: &str) {
        let mut m = self.meta.lock().unwrap();
        m.link_drops += 1;
        Self::fold_uptime(&mut m);
        m.last_drop_at_ms = Some(now_ms());
        m.last_drop_reason = Some(reason.to_string());
        m.last_error = Some(reason.to_string());
        drop(m);
        self.touch();
    }

    /// The session ended on purpose (user disconnect / teardown). Only folds
    /// the uptime if a link was actually up.
    pub fn on_disconnected(&self) {
        let mut m = self.meta.lock().unwrap();
        if m.connected_since_ms.is_some() {
            Self::fold_uptime(&mut m);
            m.last_drop_at_ms = Some(now_ms());
            m.last_drop_reason = Some("disconnected".to_string());
        }
        drop(m);
        self.touch();
    }

    /// A connect (or reconnect) attempt failed.
    pub fn on_connect_failed(&self, error: &str) {
        let mut m = self.meta.lock().unwrap();
        m.connect_failures += 1;
        m.last_error = Some(error.to_string());
        drop(m);
        self.touch();
    }

    fn fold_uptime(m: &mut Meta) {
        if let Some(since) = m.connected_since_ms.take() {
            m.connected_total_ms += now_ms().saturating_sub(since);
        }
    }

    pub fn add_tunnel_bytes(&self, up: u64, down: u64) {
        if up > 0 {
            self.tunnel_bytes_up.fetch_add(up, Ordering::Relaxed);
        }
        if down > 0 {
            self.tunnel_bytes_down.fetch_add(down, Ordering::Relaxed);
        }
        self.touch();
    }

    pub fn snapshot(&self) -> ConnStats {
        let m = self.meta.lock().unwrap();
        ConnStats {
            host_id: self.host_id,
            since_ms: self.since_ms,
            connected: m.connected_since_ms.is_some(),
            connects: m.connects,
            reconnects: m.reconnects,
            link_drops: m.link_drops,
            connect_failures: m.connect_failures,
            connected_since_ms: m.connected_since_ms,
            prior_uptime_ms: m.connected_total_ms,
            last_ip: m.last_ip.clone(),
            last_drop_at_ms: m.last_drop_at_ms,
            last_drop_reason: m.last_drop_reason.clone(),
            last_error: m.last_error.clone(),
            tunnel_conns_total: self.tunnel_conns_total.load(Ordering::Relaxed),
            tunnel_conns_active: self.tunnel_conns_active.load(Ordering::Relaxed),
            tunnel_refused: self.tunnel_refused.load(Ordering::Relaxed),
            tunnel_wait_timeouts: self.tunnel_wait_timeouts.load(Ordering::Relaxed),
            tunnel_bytes_up: self.tunnel_bytes_up.load(Ordering::Relaxed),
            tunnel_bytes_down: self.tunnel_bytes_down.load(Ordering::Relaxed),
            scans: self.scans.load(Ordering::Relaxed),
            scan_failures: self.scan_failures.load(Ordering::Relaxed),
            last_scan_ms: self.last_scan_ms.load(Ordering::Relaxed),
        }
    }
}

/// Holds one active-connection slot on a host's tunnel stats; releasing it
/// (on drop) decrements the counter, so an early return can't leak a slot.
pub struct ActiveTunnelGuard(Arc<HostStats>);

impl ActiveTunnelGuard {
    pub fn acquire(stats: Arc<HostStats>) -> Self {
        stats.tunnel_conns_total.fetch_add(1, Ordering::Relaxed);
        stats.tunnel_conns_active.fetch_add(1, Ordering::Relaxed);
        stats.touch();
        Self(stats)
    }
}

impl Drop for ActiveTunnelGuard {
    fn drop(&mut self) {
        self.0.tunnel_conns_active.fetch_sub(1, Ordering::Relaxed);
        self.0.touch();
    }
}

/// All hosts' stats, created on first touch.
#[derive(Default)]
pub struct StatsHub {
    inner: StdMutex<HashMap<Uuid, Arc<HostStats>>>,
}

impl StatsHub {
    pub fn get(&self, host_id: Uuid) -> Arc<HostStats> {
        self.inner
            .lock()
            .unwrap()
            .entry(host_id)
            .or_insert_with(|| Arc::new(HostStats::new(host_id)))
            .clone()
    }

    pub fn snapshot(&self, host_id: Uuid) -> Option<ConnStats> {
        self.inner
            .lock()
            .unwrap()
            .get(&host_id)
            .map(|s| s.snapshot())
    }

    pub fn snapshot_all(&self) -> Vec<ConnStats> {
        let mut all: Vec<ConnStats> = self
            .inner
            .lock()
            .unwrap()
            .values()
            .map(|s| s.snapshot())
            .collect();
        all.sort_by_key(|s| s.since_ms);
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_host_starts_at_zero() {
        let hub = StatsHub::default();
        let id = Uuid::new_v4();
        let s = hub.get(id).snapshot();
        assert_eq!(s.host_id, id);
        assert!(!s.connected);
        assert_eq!(s.connects, 0);
        assert_eq!(s.prior_uptime_ms, 0);
        assert!(s.connected_since_ms.is_none());
        assert_eq!(s.tunnel_bytes_up, 0);
        assert!(s.last_drop_reason.is_none());
    }

    #[test]
    fn hub_returns_the_same_counters_per_host() {
        let hub = StatsHub::default();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        hub.get(a).add_tunnel_bytes(10, 20);
        hub.get(a).add_tunnel_bytes(1, 2);
        let sa = hub.get(a).snapshot();
        assert_eq!((sa.tunnel_bytes_up, sa.tunnel_bytes_down), (11, 22));
        // Another host is untouched.
        assert_eq!(hub.get(b).snapshot().tunnel_bytes_up, 0);
        assert_eq!(hub.snapshot_all().len(), 2);
        assert!(hub.snapshot(Uuid::new_v4()).is_none());
    }

    #[test]
    fn connect_reconnect_and_drop_are_counted_separately() {
        let s = HostStats::new(Uuid::new_v4());
        s.on_connected("10.0.0.1", 1);
        assert!(s.snapshot().connected);
        assert_eq!(s.snapshot().last_ip.as_deref(), Some("10.0.0.1"));

        s.on_link_lost("keepalive timeout");
        let snap = s.snapshot();
        assert!(!snap.connected);
        assert_eq!(snap.link_drops, 1);
        assert_eq!(snap.last_drop_reason.as_deref(), Some("keepalive timeout"));
        assert_eq!(snap.last_error.as_deref(), Some("keepalive timeout"));
        assert!(snap.last_drop_at_ms.is_some());

        s.on_connect_failed("dns");
        assert_eq!(s.snapshot().connect_failures, 1);

        s.on_connected("10.0.0.2", 2);
        let snap = s.snapshot();
        assert_eq!(snap.connects, 1, "a reconnect is not a new connect");
        assert_eq!(snap.reconnects, 1);
        assert!(snap.last_error.is_none(), "a successful link clears the error");
        assert!(snap.connected);
    }

    #[test]
    fn user_disconnect_folds_uptime_only_when_a_link_was_up() {
        let s = HostStats::new(Uuid::new_v4());
        // Not connected: nothing to fold, no drop recorded.
        s.on_disconnected();
        assert!(s.snapshot().last_drop_reason.is_none());

        s.on_connected("::1", 1);
        assert!(s.snapshot().connected_since_ms.is_some());
        s.on_disconnected();
        let snap = s.snapshot();
        assert!(!snap.connected);
        assert!(snap.connected_since_ms.is_none(), "uptime folded into prior");
        assert_eq!(snap.link_drops, 0, "a deliberate disconnect is not a drop");
        assert_eq!(snap.last_drop_reason.as_deref(), Some("disconnected"));
    }

    #[test]
    fn active_guard_releases_its_slot_on_drop() {
        let s = Arc::new(HostStats::new(Uuid::new_v4()));
        {
            let _a = ActiveTunnelGuard::acquire(s.clone());
            let _b = ActiveTunnelGuard::acquire(s.clone());
            let snap = s.snapshot();
            assert_eq!(snap.tunnel_conns_active, 2);
            assert_eq!(snap.tunnel_conns_total, 2);
        }
        let snap = s.snapshot();
        assert_eq!(snap.tunnel_conns_active, 0);
        assert_eq!(snap.tunnel_conns_total, 2, "totals never go down");
    }

    #[test]
    fn dirty_flag_is_set_by_mutations_and_cleared_by_take() {
        let s = HostStats::new(Uuid::new_v4());
        assert!(!s.take_dirty());
        s.add_tunnel_bytes(1, 0);
        assert!(s.take_dirty());
        assert!(!s.take_dirty(), "take clears the flag");
        s.on_connected("1.2.3.4", 1);
        assert!(s.take_dirty());
    }
}
