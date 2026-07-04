import { useEffect, useState } from 'react';
import { useStore } from '../store';
import { fmtUptime } from '../util';

export function Sidebar() {
  const hosts = useStore((s) => s.hosts);
  const conn = useStore((s) => s.conn);
  const activeHostId = useStore((s) => s.activeHostId);
  const connError = useStore((s) => s.connError);
  const connect = useStore((s) => s.connect);

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
        </div>
        <div className="foot-row">
          <span className="foot-note">⟳ auto-reconnect · fresh DNS</span>
          <span className="flex-1" />
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
