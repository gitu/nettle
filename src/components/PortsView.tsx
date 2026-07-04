import { useStore } from '../store';
import { api, type ForwardInfo, type RemotePort } from '../ipc';

export function PortsView() {
  const hostId = useStore((s) => s.focusedHostId);
  const ports = useStore((s) => (hostId ? (s.sessions[hostId]?.ports ?? []) : []));
  const forwards = useStore((s) => (hostId ? (s.sessions[hostId]?.forwards ?? []) : []));
  const unsupported = useStore((s) => (hostId ? (s.sessions[hostId]?.portsUnsupported ?? false) : false));

  if (!hostId) return null;

  const fwdByPort = new Map<number, ForwardInfo>(forwards.map((f) => [f.port, f]));
  const portByNum = new Map<number, RemotePort>(ports.map((p) => [p.port, p]));

  // Union: everything the scanner sees + forwards whose remote process is gone.
  const rows: { port: number; info: RemotePort | null; fwd: ForwardInfo | null }[] = [
    ...ports.map((p) => ({ port: p.port, info: p, fwd: fwdByPort.get(p.port) ?? null })),
    ...forwards
      .filter((f) => !portByNum.has(f.port))
      .map((f) => ({ port: f.port, info: null, fwd: f })),
  ].sort((a, b) => a.port - b.port);

  const pinned = forwards.filter((f) => f.pinned);

  return (
    <div className="view">
      <div className="ports-head">
        <div className="flex-1">
          <div className="ports-title">Port forwarding</div>
          <div className="ports-desc">
            Ports listening on the remote appear here live. Toggle to tunnel to{' '}
            <b>localhost</b> — pin one to keep the tunnel and reclaim it automatically when
            the dev server restarts.
          </div>
        </div>
        <div className="scan-chip">auto-scan · 3s ↻</div>
      </div>
      <div className="ports-body">
        {unsupported && (
          <div className="pane-msg">
            Port discovery isn't supported on this remote (no ss / netstat / procfs).
          </div>
        )}
        <div className="pcols">
          <span className="pcol-port">REMOTE</span>
          <span className="pcol-proc">PROCESS</span>
          <span className="pcol-bind">BIND</span>
          <span className="pcol-local">LOCAL TUNNEL</span>
          <span className="pcol-act">ACTIONS</span>
        </div>
        {rows.length === 0 && !unsupported && (
          <div className="pane-msg">Scanning remote ports…</div>
        )}
        {rows.map(({ port, info, fwd }) => {
          const listening = info != null;
          const forwarded = fwd != null;
          return (
            <div key={port} className={`prow${forwarded ? ' fwd' : ''}`}>
              <div className="pcol-port" style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                <span className={`pdot${listening ? ' live' : ' waiting'}`} />
                <span className="pport">{port}</span>
              </div>
              <div className="pcol-proc" style={{ minWidth: 0 }}>
                <div className="pproc">{info?.process ?? (listening ? '—' : '—')}</div>
                <div className={`pstate${listening ? '' : ' waiting'}`}>
                  {listening ? 'listening' : 'waiting for process…'}
                </div>
              </div>
              <span className="pcol-bind pbind">
                {info ? `${info.bind}:${port}` : '—'}
              </span>
              <div className="pcol-local">
                <span className={`plocal${forwarded ? '' : ' off'}`}>
                  {forwarded ? `→ localhost:${port}` : 'not forwarded'}
                </span>
              </div>
              <div
                className="pcol-act"
                style={{ display: 'flex', alignItems: 'center', justifyContent: 'flex-end', gap: 8 }}
              >
                <button
                  className={`pin-btn${fwd?.pinned ? ' pinned' : ''}`}
                  title="keep across restarts"
                  onClick={() => api.forwardSet(hostId, port, true, !(fwd?.pinned ?? false))}
                >
                  ⚲
                </button>
                <button
                  className={`tgl-btn${forwarded ? ' on' : ''}`}
                  onClick={() => api.forwardSet(hostId, port, !forwarded, false)}
                >
                  {forwarded ? 'forwarded' : 'forward'}
                </button>
              </div>
            </div>
          );
        })}
        <div style={{ height: 16 }} />
        <div className="pinned-label">PINNED · SURVIVES RESTARTS</div>
        {pinned.length === 0 && (
          <div className="pinned-empty">
            Nothing pinned. Click the ⚲ on a forward to keep it alive across remote restarts.
          </div>
        )}
        {pinned.map((f) => {
          const info = portByNum.get(f.port);
          return (
            <div key={f.port} className="pinned-row">
              <span className="pinned-glyph">⚲</span>
              <span className="pinned-port">{f.port}</span>
              <span className="pinned-proc">{info?.process ?? 'waiting for process'}</span>
              <span className="pinned-tunnel">localhost:{f.port}</span>
              <span className={`pinned-live${f.live ? '' : ' waiting'}`}>
                {f.live ? 'active' : 'reconnecting'}
              </span>
              <button className="unpin-btn" onClick={() => api.forwardSet(hostId, f.port, true, false)}>
                unpin
              </button>
            </div>
          );
        })}
      </div>
    </div>
  );
}
