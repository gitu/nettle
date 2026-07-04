import { useEffect, useState } from 'react';
import { useStore } from '../store';
import { api } from '../ipc';
import { fmtUptime } from '../util';

export function Sidebar() {
  const hosts = useStore((s) => s.hosts);
  const sets = useStore((s) => s.sets);
  const sessions = useStore((s) => s.sessions);
  const focusedHostId = useStore((s) => s.focusedHostId);
  const connect = useStore((s) => s.connect);
  const focusHost = useStore((s) => s.focusHost);
  const refreshSets = useStore((s) => s.refreshSets);

  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);

  const focused = focusedHostId ? sessions[focusedHostId] : null;
  const focusedHost = hosts.find((h) => h.id === focusedHostId) ?? null;
  const conn = focused?.conn;
  const isSession = conn?.state === 'connected' || conn?.state === 'reconnecting';
  const forwards = focused?.forwards ?? [];
  const liveTunnels = forwards.filter((f) => f.live).length;
  const pinnedCount = forwards.filter((f) => f.pinned).length;
  const activeTransfers = focused
    ? Object.values(focused.transfers).filter((t) => t.status === 'running' || t.status === 'queued')
        .length
    : 0;

  const footDotClass =
    conn?.state === 'connected'
      ? 'online'
      : conn?.state === 'reconnecting' || conn?.state === 'connecting' || conn?.state === 'authenticating'
        ? 'reconnecting'
        : focused?.connError
          ? 'failed'
          : 'off';
  const footLabel =
    conn?.state === 'connected'
      ? 'connected'
      : conn?.state === 'reconnecting'
        ? 'reconnecting…'
        : conn?.state === 'connecting'
          ? 'connecting…'
          : conn?.state === 'authenticating'
            ? 'authenticating…'
            : focused?.connError
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
          const sess = sessions[h.id];
          const state = sess?.conn.state;
          const isFocused = h.id === focusedHostId;
          const dot =
            state === 'connected'
              ? 'on'
              : state === 'reconnecting' || state === 'connecting' || state === 'authenticating'
                ? 'busy'
                : '';
          return (
            <div
              key={h.id}
              className={`host-item${isFocused ? ' active' : ''}`}
              onClick={() => (sess ? focusHost(h.id) : connect(h.id))}
            >
              <span className={`host-dot ${dot}`} />
              <div className="host-meta">
                <div className="host-name">{h.name}</div>
                <div className="host-addr">
                  {h.username}@{h.hostname}
                </div>
              </div>
              {sess ? (
                <span className="host-badge">{state === 'connected' ? 'live' : '…'}</span>
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

        <div className="sidebar-subhead">
          <span className="sidebar-label">SETS</span>
          <button className="add-host" onClick={() => useStore.setState({ editSet: 'new' })}>
            +
          </button>
        </div>
        {sets.length === 0 && (
          <div className="pane-msg" style={{ padding: '4px 9px' }}>
            Group hosts to connect together.
          </div>
        )}
        {sets.map((set) => (
          <div key={set.id} className="set-item">
            <button
              className="set-launch"
              title="Connect all hosts in this set"
              onClick={() => api.connectSet(set.id).catch(() => {})}
            >
              ▸
            </button>
            <div className="host-meta" onClick={() => api.connectSet(set.id).catch(() => {})}>
              <div className="host-name">{set.name}</div>
              <div className="host-addr">
                {set.hostIds.length} host{set.hostIds.length === 1 ? '' : 's'}
              </div>
            </div>
            <button
              className="host-edit"
              onClick={(e) => {
                e.stopPropagation();
                useStore.setState({ editSet: set });
                refreshSets();
              }}
            >
              edit
            </button>
          </div>
        ))}
      </div>

      <div className="sidebar-foot">
        <div className="foot-row">
          <span className={`conn-dot ${footDotClass}`} />
          <span
            className={`conn-label${
              conn?.state === 'reconnecting' ? ' warn' : focused?.connError ? ' err' : ''
            }`}
          >
            {focusedHost ? footLabel : 'no session'}
          </span>
          <span className="flex-1" />
          <span className="conn-ip">{conn?.state === 'connected' ? `→ ${conn.ip}` : ''}</span>
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
        {isSession && focusedHost && (
          <div className="foot-row">
            <span className="foot-note">
              {focusedHost.username}@{focusedHost.hostname}
              {focusedHost.port !== 22 ? `:${focusedHost.port}` : ''}
            </span>
            <span className="flex-1" />
            {conn?.state === 'connected' && conn.epoch > 1 && (
              <span className="foot-note">link #{conn.epoch}</span>
            )}
          </div>
        )}
        {isSession && (
          <div className="foot-row">
            <span className={`foot-note${liveTunnels > 0 ? ' acc' : ''}`}>
              ⚲{' '}
              {forwards.length === 0
                ? 'no tunnels'
                : `${forwards.length} tunnel${forwards.length === 1 ? '' : 's'}${
                    pinnedCount > 0 ? ` · ${pinnedCount} pinned` : ''
                  }`}
            </span>
            <span className="flex-1" />
            {activeTransfers > 0 && <span className="foot-note">⇅ {activeTransfers}</span>}
            <span className="foot-uptime">
              {conn?.state === 'connected' ? `up ${fmtUptime(conn.sinceMs, now)}` : ''}
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
        {focused?.connError && (
          <div className="foot-row">
            <span className="foot-note" style={{ color: 'var(--red)', whiteSpace: 'normal' }}>
              {focused.connError}
            </span>
          </div>
        )}
      </div>
    </div>
  );
}
