import { useStore } from '../store';
import { api } from '../ipc';

interface Row {
  hostId: string;
  hostName: string;
  connected: boolean;
  port: number;
  pinned: boolean;
  live: boolean;
}

export function DashboardView() {
  const hosts = useStore((s) => s.hosts);
  const sessions = useStore((s) => s.sessions);
  const focusedHostId = useStore((s) => s.focusedHostId);
  const focusHost = useStore((s) => s.focusHost);
  const setView = useStore((s) => s.setView);

  // Single-host mode when a host is focused (the per-host dashboard tab);
  // otherwise aggregate every session (the independent global dashboard).
  const scoped = focusedHostId != null && sessions[focusedHostId] != null;

  const groups = Object.values(sessions)
    .filter((sess) => !scoped || sess.hostId === focusedHostId)
    .map((sess) => {
      const host = hosts.find((h) => h.id === sess.hostId);
      const connected = sess.conn.state === 'connected';
      const rows: Row[] = sess.forwards.map((f) => ({
        hostId: sess.hostId,
        hostName: host?.name ?? sess.hostId,
        connected,
        port: f.port,
        pinned: f.pinned,
        live: f.live,
      }));
      return { hostId: sess.hostId, hostName: host?.name ?? sess.hostId, connected, host, rows };
    })
    .sort((a, b) => a.hostName.localeCompare(b.hostName));

  const totalTunnels = groups.reduce((n, g) => n + g.rows.length, 0);
  const totalActive = groups.reduce((n, g) => n + g.rows.filter((r) => r.live).length, 0);
  const scopedName = scoped ? (groups[0]?.hostName ?? '') : '';

  return (
    <div className="view">
      <div className="ports-head">
        <div className="flex-1">
          <div className="ports-title">{scoped ? `${scopedName} · tunnels` : 'Tunnels dashboard'}</div>
          <div className="ports-desc">
            {scoped
              ? `Every forward on this host. ${totalTunnels} tunnel${totalTunnels === 1 ? '' : 's'} · ${totalActive} active.`
              : `Every forward across all connected hosts. ${totalTunnels} tunnel${totalTunnels === 1 ? '' : 's'} · ${totalActive} active.`}
          </div>
        </div>
      </div>
      <div className="ports-body">
        {groups.length === 0 && (
          <div className="pane-msg">
            {scoped
              ? "No tunnels on this host yet — open one from the ports tab."
              : 'No active sessions. Connect a host to see its tunnels here.'}
          </div>
        )}
        {groups.map((g) => (
          <div key={g.hostId} className="dash-group">
            <div className="dash-group-head">
              <span className={`conn-dot ${g.connected ? 'online' : 'reconnecting'}`} />
              <button
                className="dash-host"
                onClick={() => {
                  focusHost(g.hostId);
                  setView('ports');
                }}
              >
                {g.hostName}
              </button>
              <span className="dash-addr">
                {g.host ? `${g.host.username}@${g.host.hostname}` : ''}
              </span>
              <span className="flex-1" />
              <span className="dash-count">
                {g.rows.length} tunnel{g.rows.length === 1 ? '' : 's'}
              </span>
            </div>
            {g.rows.length === 0 && (
              <div className="dash-empty">No tunnels — open one from this host's ports tab.</div>
            )}
            {g.rows
              .sort((a, b) => a.port - b.port)
              .map((r) => (
                <div key={r.port} className="dash-row">
                  <span className={`pdot${r.live ? ' live' : ' waiting'}`} />
                  <span className="dash-port">{r.port}</span>
                  <span className="dash-tunnel">localhost:{r.port}</span>
                  {r.pinned && <span className="dash-pin">⚲ pinned</span>}
                  <span className={`dash-state${r.live ? ' live' : ''}`}>
                    {r.live ? 'active' : 'waiting'}
                  </span>
                  <span className="flex-1" />
                  <button
                    className="unpin-btn"
                    onClick={() => api.forwardSet(r.hostId, r.port, false, false).catch(() => {})}
                  >
                    stop
                  </button>
                </div>
              ))}
          </div>
        ))}
      </div>
    </div>
  );
}
