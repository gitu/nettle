import { useEffect, useState } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { useStore } from '../store';
import { api, type ForwardInfo, type RemotePort } from '../ipc';

/** Forward the port if needed, sniff http vs https over the SSH link, then open
 *  the local tunnel in the default browser. */
async function openPortInBrowser(hostId: string, port: number, fwd: ForwardInfo | null) {
  const scheme = await api.probePortScheme(hostId, port).catch(() => 'http' as const);
  let localPort = fwd?.localPort ?? port;
  if (!fwd) {
    await useStore.getState().setForward(hostId, port, true, false);
    localPort = port;
  }
  await openUrl(`${scheme}://localhost:${localPort}`).catch(() => {});
}

/** Two-step kill control for the process behind a remote port: first click asks
 *  for confirmation, then offers a graceful SIGTERM or a hard SIGKILL. */
function KillButton({ hostId, port, pid }: { hostId: string; port: number; pid: number | null }) {
  const [confirming, setConfirming] = useState(false);
  const killRemote = useStore((s) => s.killRemote);

  // Arm/disarm only — auto-reset so a stray click doesn't leave a live kill UI.
  useEffect(() => {
    if (!confirming) return;
    const t = setTimeout(() => setConfirming(false), 6000);
    return () => clearTimeout(t);
  }, [confirming]);

  if (!confirming) {
    return (
      <button
        className="kill-btn"
        title="kill the remote process to free this port"
        onClick={() => setConfirming(true)}
      >
        ✕ kill
      </button>
    );
  }
  const fire = (force: boolean) => {
    setConfirming(false);
    killRemote(hostId, port, pid, force);
  };
  return (
    <span className="kill-confirm">
      <button className="kill-btn danger" title="send SIGTERM (graceful)" onClick={() => fire(false)}>
        term
      </button>
      <button className="kill-btn danger" title="send SIGKILL (force)" onClick={() => fire(true)}>
        -9
      </button>
      <button className="kill-btn" title="cancel" onClick={() => setConfirming(false)}>
        ✕
      </button>
    </span>
  );
}

/** The local-tunnel cell: shows the bind target and lets you retarget the
 *  local port inline when a forward is active. */
function LocalTunnel({
  hostId,
  port,
  fwd,
}: {
  hostId: string;
  port: number;
  fwd: ForwardInfo | null;
}) {
  const [editing, setEditing] = useState(false);
  const [value, setValue] = useState('');
  const setForward = useStore((s) => s.setForward);

  if (!fwd) return <span className="plocal off">not forwarded</span>;

  const commit = () => {
    const next = parseInt(value, 10);
    setEditing(false);
    if (Number.isInteger(next) && next > 0 && next < 65536 && next !== fwd.localPort) {
      setForward(hostId, port, true, fwd.pinned, next);
    }
  };

  if (editing) {
    return (
      <span className="plocal">
        → localhost:
        <input
          className="port-input"
          type="number"
          min={1}
          max={65535}
          autoFocus
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') commit();
            if (e.key === 'Escape') setEditing(false);
          }}
          onBlur={commit}
        />
      </span>
    );
  }

  return (
    <button
      className="plocal linkish"
      title="click to change the local port"
      onClick={() => {
        setValue(String(fwd.localPort));
        setEditing(true);
      }}
    >
      → localhost:{fwd.localPort}
      {fwd.localPort !== port && <span className="premap"> (remap)</span>}
    </button>
  );
}

/** Manually forward an arbitrary remote port (the scanner may not see it, or
 *  you may want it bound to a specific local port). */
function AddForward({ hostId }: { hostId: string }) {
  const [remote, setRemote] = useState('');
  const [local, setLocal] = useState('');
  const [error, setError] = useState<string | null>(null);
  const setForward = useStore((s) => s.setForward);

  const submit = () => {
    const rp = parseInt(remote, 10);
    if (!Number.isInteger(rp) || rp <= 0 || rp >= 65536) {
      setError('enter a remote port (1–65535)');
      return;
    }
    const lp = local.trim() === '' ? undefined : parseInt(local, 10);
    if (lp !== undefined && (!Number.isInteger(lp) || lp <= 0 || lp >= 65536)) {
      setError('local port must be 1–65535');
      return;
    }
    setForward(hostId, rp, true, false, lp);
    setRemote('');
    setLocal('');
    setError(null);
  };

  return (
    <div className="add-fwd">
      <span className="add-fwd-label">forward a port</span>
      <input
        className="port-input"
        type="number"
        min={1}
        max={65535}
        placeholder="remote"
        value={remote}
        onChange={(e) => setRemote(e.target.value)}
        onKeyDown={(e) => e.key === 'Enter' && submit()}
      />
      <span className="add-fwd-arrow">→ localhost:</span>
      <input
        className="port-input"
        type="number"
        min={1}
        max={65535}
        placeholder="same"
        value={local}
        onChange={(e) => setLocal(e.target.value)}
        onKeyDown={(e) => e.key === 'Enter' && submit()}
      />
      <button className="tgl-btn on" onClick={submit}>
        forward
      </button>
      {error && <span className="add-fwd-err">{error}</span>}
    </div>
  );
}

export function PortsView() {
  const hostId = useStore((s) => s.focusedHostId);
  const ports = useStore((s) => (hostId ? (s.sessions[hostId]?.ports ?? []) : []));
  const forwards = useStore((s) => (hostId ? (s.sessions[hostId]?.forwards ?? []) : []));
  const unsupported = useStore((s) => (hostId ? (s.sessions[hostId]?.portsUnsupported ?? false) : false));
  const connState = useStore((s) => (hostId ? s.sessions[hostId]?.conn.state : undefined));
  const portError = useStore((s) => (hostId ? (s.sessions[hostId]?.portError ?? null) : null));
  const setForward = useStore((s) => s.setForward);
  const dismissPortError = useStore((s) => s.dismissPortError);

  if (!hostId) return null;

  // Ports can only be scanned over a live connection. Without one, the scanner
  // never runs — so distinguish "not connected" from "connected, scanning".
  const connected = connState === 'connected';

  const fwdByPort = new Map<number, ForwardInfo>(forwards.map((f) => [f.port, f]));
  const portByNum = new Map<number, RemotePort>(ports.map((p) => [p.port, p]));

  // Union: everything the scanner sees + forwards whose remote process is gone.
  // Sorted so active tunnels float to the top: pinned first, then other
  // forwards, then the remaining listening ports — each group by port number.
  const rank = (r: { fwd: ForwardInfo | null }) => (r.fwd ? (r.fwd.pinned ? 0 : 1) : 2);
  const rows: { port: number; info: RemotePort | null; fwd: ForwardInfo | null }[] = [
    ...ports.map((p) => ({ port: p.port, info: p, fwd: fwdByPort.get(p.port) ?? null })),
    ...forwards
      .filter((f) => !portByNum.has(f.port))
      .map((f) => ({ port: f.port, info: null, fwd: f })),
  ].sort((a, b) => rank(a) - rank(b) || a.port - b.port);

  const pinned = forwards.filter((f) => f.pinned);

  return (
    <div className="view">
      <div className="ports-head">
        <div className="flex-1">
          <div className="ports-title">Port forwarding</div>
          <div className="ports-desc">
            Ports listening on the remote appear here live. Toggle to tunnel to{' '}
            <b>localhost</b> — click the local target to bind a different port, or pin one to
            keep the tunnel and reclaim it automatically when the dev server restarts.
          </div>
        </div>
        <div className="scan-chip">auto-scan · 3s ↻</div>
      </div>
      <div className="ports-body">
        {portError && (
          <div className="port-err-banner">
            <span className="port-err-msg">{portError.message}</span>
            {portError.hint != null && (
              <button
                className="port-err-action"
                onClick={() =>
                  setForward(hostId, portError.port, true, portError.pinned, portError.hint!)
                }
              >
                bind to localhost:{portError.hint} instead
              </button>
            )}
            <button className="port-err-dismiss" onClick={() => dismissPortError(hostId)}>
              dismiss
            </button>
          </div>
        )}
        {unsupported && (
          <div className="pane-msg">
            Port discovery isn't supported on this remote (no ss / netstat / procfs).
          </div>
        )}
        {pinned.length > 0 && (
          <div className="pinned-block">
            <div className="pinned-label">PINNED · SURVIVES RESTARTS</div>
            {pinned.map((f) => {
              const info = portByNum.get(f.port);
              return (
                <div key={f.port} className="pinned-row">
                  <span className="pinned-glyph">⚲</span>
                  <span className="pinned-port">{f.port}</span>
                  <span className="pinned-proc">
                    {info?.process ?? 'waiting for process'}
                    {info?.container && (
                      <span className="pcontainer" title={`docker container: ${info.container}`}>
                        ⬡ {info.container}
                      </span>
                    )}
                  </span>
                  <span className="pinned-tunnel">
                    localhost:{f.localPort}
                    {f.localPort !== f.port && <span className="premap"> (remap)</span>}
                  </span>
                  <span className={`pinned-live${f.live ? '' : ' waiting'}`}>
                    {f.live ? 'active' : 'reconnecting'}
                  </span>
                  <button
                    className="open-btn"
                    title="open in browser"
                    onClick={() => openPortInBrowser(hostId, f.port, f)}
                  >
                    ↗ open
                  </button>
                  <button
                    className="unpin-btn"
                    onClick={() => setForward(hostId, f.port, true, false, f.localPort)}
                  >
                    unpin
                  </button>
                </div>
              );
            })}
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
          <div className="pane-msg">
            {connected
              ? 'Scanning remote ports…'
              : connState === 'reconnecting'
                ? 'Reconnecting — ports will refresh once the link is back.'
                : connState === 'connecting' || connState === 'authenticating'
                  ? 'Connecting…'
                  : 'Not connected — this host is unreachable or the session is down.'}
          </div>
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
                <div className="pproc">
                  {info?.process ?? '—'}
                  {info?.container && (
                    <span className="pcontainer" title={`docker container: ${info.container}`}>
                      ⬡ {info.container}
                    </span>
                  )}
                </div>
                <div className={`pstate${listening ? '' : ' waiting'}`}>
                  {listening ? 'listening' : 'waiting for process…'}
                </div>
              </div>
              <span className="pcol-bind pbind">
                {info ? `${info.bind}:${port}` : '—'}
              </span>
              <div className="pcol-local">
                <LocalTunnel hostId={hostId} port={port} fwd={fwd} />
              </div>
              <div
                className="pcol-act"
                style={{ display: 'flex', alignItems: 'center', justifyContent: 'flex-end', gap: 8 }}
              >
                {listening && <KillButton hostId={hostId} port={port} pid={info?.pid ?? null} />}
                {(listening || forwarded) && (
                  <button
                    className="open-btn"
                    title="open in browser (forwards if needed)"
                    onClick={() => openPortInBrowser(hostId, port, fwd)}
                  >
                    ↗ open
                  </button>
                )}
                <button
                  className={`pin-btn${fwd?.pinned ? ' pinned' : ''}`}
                  title={fwd?.pinned ? 'unpin' : 'pin — keep across restarts'}
                  onClick={() =>
                    setForward(hostId, port, true, !(fwd?.pinned ?? false), fwd?.localPort)
                  }
                >
                  ⚲
                </button>
                <button
                  className={`tgl-btn${forwarded ? ' on' : ''}`}
                  onClick={() =>
                    forwarded
                      ? setForward(hostId, port, false, false)
                      : setForward(hostId, port, true, false)
                  }
                >
                  {forwarded ? 'forwarded' : 'forward'}
                </button>
              </div>
            </div>
          );
        })}
        <AddForward hostId={hostId} />
      </div>
    </div>
  );
}
