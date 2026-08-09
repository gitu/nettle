import { useStore } from '../store';
import { shortDir } from '../util';
import { openPortInBrowser } from '../openPort';

interface Row {
  hostId: string;
  hostName: string;
  connected: boolean;
  port: number;
  localPort: number;
  pinned: boolean;
  live: boolean;
  process: string | null;
  container: string | null;
  cwd: string | null;
}

export function DashboardView() {
  const hosts = useStore((s) => s.hosts);
  const sessions = useStore((s) => s.sessions);
  const focusedHostId = useStore((s) => s.focusedHostId);
  const focusHost = useStore((s) => s.focusHost);
  const setView = useStore((s) => s.setView);
  const setForward = useStore((s) => s.setForward);

  // Single-host mode when a host is focused (the per-host dashboard tab);
  // otherwise aggregate every session (the independent global dashboard).
  const scoped = focusedHostId != null && sessions[focusedHostId] != null;

  const groups = Object.values(sessions)
    .filter((sess) => !scoped || sess.hostId === focusedHostId)
    .map((sess) => {
      const host = hosts.find((h) => h.id === sess.hostId);
      const connected = sess.conn.state === 'connected';
      const portInfo = new Map(sess.ports.map((p) => [p.port, p]));
      const rows: Row[] = sess.forwards.map((f) => {
        const info = portInfo.get(f.port);
        return {
          hostId: sess.hostId,
          hostName: host?.name ?? sess.hostId,
          connected,
          port: f.port,
          localPort: f.localPort,
          pinned: f.pinned,
          live: f.live,
          process: info?.process ?? null,
          container: info?.container ?? null,
          cwd: info?.cwd ?? null,
        };
      });
      return { hostId: sess.hostId, hostName: host?.name ?? sess.hostId, connected, host, rows };
    })
    .sort((a, b) => a.hostName.localeCompare(b.hostName));

  const totalTunnels = groups.reduce((n, g) => n + g.rows.length, 0);
  const totalActive = groups.reduce((n, g) => n + g.rows.filter((r) => r.live).length, 0);
  const scopedName = scoped ? (groups[0]?.hostName ?? '') : '';

  // Pinned forwards whose remote process is gone — candidates for bulk cleanup.
  const stalePins = groups.flatMap((g) => g.rows.filter((r) => r.pinned && !r.live));
  const unpinStale = () => {
    for (const r of stalePins) setForward(r.hostId, r.port, false, false);
  };

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
        {stalePins.length > 0 && (
          <button
            className="open-btn"
            title="remove every pinned tunnel whose remote process is gone"
            onClick={unpinStale}
          >
            ⌫ unpin {stalePins.length} without a process
          </button>
        )}
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
            {g.rows.length > 0 && (
              <div className="pcols dash-cols">
                <span className="dcol-port">REMOTE</span>
                <span className="dcol-proc">PROCESS</span>
                <span className="dcol-local">LOCAL TUNNEL</span>
                <span className="dcol-pin">PIN</span>
                <span className="dcol-state">STATE</span>
                <span className="dcol-act" />
              </div>
            )}
            {g.rows
              .sort((a, b) => a.port - b.port)
              .map((r) => (
                <div key={r.port} className="dash-row">
                  <span className="dcol-port dash-port-cell">
                    <span className={`pdot${r.live ? ' live' : ' waiting'}`} />
                    <span className="dash-port">{r.port}</span>
                  </span>
                  <span className="dcol-proc dash-proc">
                    {r.process ?? (r.live ? '—' : 'no process')}
                    {r.container && (
                      <span className="pcontainer" title={`docker container: ${r.container}`}>
                        ⬡ {r.container}
                      </span>
                    )}
                    {r.cwd && (
                      <span className="pcwd inline" title={r.cwd}>
                        in {shortDir(r.cwd)}
                      </span>
                    )}
                  </span>
                  <span className="dcol-local dash-tunnel">
                    localhost:{r.localPort}
                    {r.localPort !== r.port && <span className="premap"> (remap)</span>}
                  </span>
                  <span className="dcol-pin dash-pin">{r.pinned ? '⚲ pinned' : ''}</span>
                  <span className={`dcol-state dash-state${r.live ? ' live' : ''}`}>
                    {r.live ? 'active' : 'waiting'}
                  </span>
                  <span className="dcol-act dash-act">
                    <button
                      className="open-btn"
                      title="open localhost tunnel in browser"
                      onClick={() => openPortInBrowser(r.hostId, r.port, r.localPort)}
                    >
                      ↗ open
                    </button>
                    <button
                      className="unpin-btn"
                      onClick={() => setForward(r.hostId, r.port, false, false)}
                    >
                      stop
                    </button>
                  </span>
                </div>
              ))}
          </div>
        ))}
      </div>
    </div>
  );
}
