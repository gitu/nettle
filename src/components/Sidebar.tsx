import { useEffect, useState } from 'react';
import { useStore } from '../store';
import { fmtUptime } from '../util';

export function Sidebar() {
  const hosts = useStore((s) => s.hosts);
  const conn = useStore((s) => s.conn);
  const activeHostId = useStore((s) => s.activeHostId);
  const connError = useStore((s) => s.connError);
  const connect = useStore((s) => s.connect);
  const forwards = useStore((s) => s.forwards);
  const transfers = useStore((s) => s.transfers);

  const isSession = conn.state === 'connected' || conn.state === 'reconnecting';
  const activeHost = hosts.find((h) => h.id === activeHostId) ?? null;
  const liveTunnels = forwards.filter((f) => f.live).length;
  const pinnedCount = forwards.filter((f) => f.pinned).length;
  const tunnelLabel =
    `${forwards.length} tunnel${forwards.length === 1 ? '' : 's'}` +
    (pinnedCount > 0 ? ` · ${pinnedCount} pinned` : '');
  const activeTransfers = Object.values(transfers).filter(
    (t) => t.status === 'running' || t.status === 'queued',
  ).length;

  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);

  const dotClass =
    conn.state === 'connected'
      ? 'online'
      : conn.state === 'reconnecting' || conn.state === 'connecting' || conn.state === 'authenticating'
        ? conn.state
        : connError
          ? 'failed'
          : 'off';

  const label =
    conn.state === 'connected'
      ? 'connected'
      : conn.state === 'reconnecting'
        ? 'reconnecting…'
        : conn.state === 'connecting'
          ? 'connecting…'
          : conn.state === 'authenticating'
            ? 'authenticating…'
            : connError
              ? 'failed'
              : 'offline';

  return (
    <div className="sidebar">
      <div className="sidebar-head">
        <span className="sidebar-label">HOSTS</span>
        <button className="add-host" onClick={() => useStore.setState({ editHost: 'new' })}>
          +
        </button>
      </div>
      <div className="host-list">
        {hosts.map((h) => {
          const isActive = h.id === activeHostId;
          const dot = isActive
            ? conn.state === 'connected'
              ? 'on'
              : 'busy'
            : '';
          return (
            <div
              key={h.id}
              className={`host-item${isActive ? ' active' : ''}`}
              onClick={() => {
                if (!isActive) connect(h.id);
              }}
            >
              <span className={`host-dot ${dot}`} />
              <div className="host-meta">
                <div className="host-name">{h.name}</div>
                <div className="host-addr">
                  {h.username}@{h.hostname}
                </div>
              </div>
              {isActive && conn.state === 'connected' ? (
                <span className="host-badge">live</span>
              ) : (
                <button
                  className="host-edit"
                  onClick={(e) => {
                    e.stopPropagation();
                    useStore.setState({ editHost: h });
                  }}
                >
                  edit
                </button>
              )}
            </div>
          );
        })}
        {hosts.length === 0 && (
          <div className="pane-msg">No hosts yet — hit + to add your first server.</div>
        )}
      </div>
      <div className="sidebar-foot">
        <div className="foot-row">
          <span className={`conn-dot ${dotClass}`} />
          <span
            className={`conn-label${
              conn.state === 'reconnecting' ? ' warn' : connError ? ' err' : ''
            }`}
          >
            {label}
          </span>
          <span className="flex-1" />
          <span className="conn-ip">
            {conn.state === 'connected' ? `→ ${conn.ip}` : ''}
          </span>
          {!isSession && (
            <button
              className="gear-btn"
              title="About nettle"
              onClick={() => useStore.setState({ aboutOpen: true })}
            >
              ⚙
            </button>
          )}
        </div>
        {isSession && activeHost && (
          <div className="foot-row">
            <span className="foot-note">
              {activeHost.username}@{activeHost.hostname}
              {activeHost.port !== 22 ? `:${activeHost.port}` : ''}
            </span>
            <span className="flex-1" />
            {conn.state === 'connected' && conn.epoch > 1 && (
              <span className="foot-note">link #{conn.epoch}</span>
            )}
          </div>
        )}
        {isSession && (
          <div className="foot-row">
            <span className={`foot-note${liveTunnels > 0 ? ' acc' : ''}`}>
              ⚲ {forwards.length === 0 ? 'no tunnels' : tunnelLabel}
            </span>
            <span className="flex-1" />
            {activeTransfers > 0 && (
              <span className="foot-note">⇅ {activeTransfers}</span>
            )}
            <span className="foot-uptime">
              {conn.state === 'connected' ? `up ${fmtUptime(conn.sinceMs, now)}` : ''}
            </span>
            <button
              className="gear-btn"
              title="About nettle"
              onClick={() => useStore.setState({ aboutOpen: true })}
            >
              ⚙
            </button>
          </div>
        )}
        {connError && (
          <div className="foot-row">
            <span className="foot-note" style={{ color: 'var(--red)', whiteSpace: 'normal' }}>
              {connError}
            </span>
          </div>
        )}
      </div>
    </div>
  );
}
