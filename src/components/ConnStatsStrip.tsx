import { useEffect, useState } from 'react';
import type { ConnStats } from '../ipc';
import { useStore } from '../store';
import { fmtBytes, fmtClock, fmtDuration, fmtDurationShort } from '../util';

/** A 1 Hz clock so live durations tick; idle (no timer) while `active` is false. */
export function useNow(active: boolean): number {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [active]);
  return now;
}

/** Connected time of the current link, or 0 when the host is down. */
export function liveUptimeMs(stats: ConnStats, now: number): number {
  return stats.connected && stats.connectedSinceMs != null
    ? Math.max(0, now - stats.connectedSinceMs)
    : 0;
}

interface Chip {
  label: string;
  value: string;
  title?: string;
  warn?: boolean;
}

/** Pure: the chips a stats strip shows for one host. Exported for tests. */
export function statsChips(stats: ConnStats, now: number): Chip[] {
  const live = liveUptimeMs(stats, now);
  const links = stats.connects + stats.reconnects;
  const chips: Chip[] = [
    {
      label: 'link',
      value: stats.connected ? fmtDuration(live) : 'down',
      title: stats.lastIp ? `current link · ${stats.lastIp}` : 'current link',
    },
    {
      label: 'total',
      value: fmtDurationShort(stats.priorUptimeMs + live),
      title: `connected time since the app started · ${links} link${links === 1 ? '' : 's'}`,
    },
    {
      label: 'drops',
      value: String(stats.linkDrops),
      title: `${stats.linkDrops} link drop${stats.linkDrops === 1 ? '' : 's'} · ${stats.reconnects} reconnect${
        stats.reconnects === 1 ? '' : 's'
      } · ${stats.connectFailures} failed attempt${stats.connectFailures === 1 ? '' : 's'}`,
      warn: stats.linkDrops > 0 || stats.connectFailures > 0,
    },
    {
      label: 'conns',
      value: `${stats.tunnelConnsActive} / ${stats.tunnelConnsTotal}`,
      title: `tunnel connections: active / total · ${stats.tunnelRefused} refused by the remote · ${
        stats.tunnelWaitTimeouts
      } gave up waiting for the remote port`,
      warn: stats.tunnelRefused > 0,
    },
    { label: '↑', value: fmtBytes(stats.tunnelBytesUp), title: 'bytes sent through tunnels' },
    { label: '↓', value: fmtBytes(stats.tunnelBytesDown), title: 'bytes received through tunnels' },
    {
      label: 'scan',
      value: stats.scans > 0 ? `${stats.lastScanMs} ms` : '—',
      title: `port scans: ${stats.scans} ok · ${stats.scanFailures} failed · last round trip ${
        stats.lastScanMs
      } ms`,
      warn: stats.scanFailures > 0,
    },
  ];
  return chips;
}

/** Per-host connection statistics for the dashboard group header. */
export function ConnStatsStrip({ hostId }: { hostId: string }) {
  const stats = useStore((s) => s.stats[hostId] ?? null);
  const now = useNow(stats?.connected ?? false);
  if (!stats) return null;
  const chips = statsChips(stats, now);
  const note = stats.lastDropReason
    ? `last drop ${stats.lastDropAtMs ? fmtClock(stats.lastDropAtMs) : ''} — ${stats.lastDropReason}`
    : stats.lastError
      ? `last error — ${stats.lastError}`
      : null;
  return (
    <div className="dash-stats" title="connection statistics since the app started">
      {chips.map((c) => (
        <span key={c.label} className={`dash-stat${c.warn ? ' warn' : ''}`} title={c.title}>
          <span className="dash-stat-label">{c.label}</span>
          <span className="dash-stat-value">{c.value}</span>
        </span>
      ))}
      {note && <span className="dash-stats-note">{note}</span>}
    </div>
  );
}
