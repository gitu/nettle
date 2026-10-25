import { useEffect, useState } from 'react';
import { useStore } from '../store';
import type { AdvertisedPort, ForwardInfo, RemotePort, SandboxInfo } from '../ipc';
import { shortDir } from '../util';
import { openPortInBrowser } from '../openPort';
import { ServicesPanel } from './ServicesPanel';

/** Two-step kill control for the process behind a remote port: first click asks
 *  for confirmation, then offers a graceful SIGTERM or a hard SIGKILL. */
function KillButton({
  hostId,
  port,
  pid,
  sandbox,
}: {
  hostId: string;
  port: number;
  pid: number | null;
  sandbox: string | null;
}) {
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
    killRemote(hostId, port, pid, force, sandbox);
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
  sandbox,
}: {
  hostId: string;
  port: number;
  fwd: ForwardInfo | null;
  sandbox: string | null;
}) {
  const [editing, setEditing] = useState(false);
  const [value, setValue] = useState('');
  const setForward = useStore((s) => s.setForward);

  if (!fwd) return <span className="plocal off">not forwarded</span>;

  const commit = () => {
    const next = parseInt(value, 10);
    setEditing(false);
    if (Number.isInteger(next) && next > 0 && next < 65536 && next !== fwd.localPort) {
      setForward(hostId, port, true, fwd.pinned, next, sandbox);
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
function AddForward({ hostId, sandbox }: { hostId: string; sandbox: string | null }) {
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
    setForward(hostId, rp, true, false, lp, sandbox);
    setRemote('');
    setLocal('');
    setError(null);
  };

  return (
    <div className="add-fwd">
      <span className="add-fwd-label">
        {sandbox ? `forward a port of ${sandbox}` : 'forward a port'}
      </span>
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

/** Host ↔ sandbox switcher: the host's Docker sandboxes (`sbx ls`), each with
 *  the local ports currently tunnelled into it. */
function SandboxBar({
  hostId,
  target,
  sandboxes,
  forwards,
}: {
  hostId: string;
  target: string | null;
  sandboxes: SandboxInfo[];
  forwards: ForwardInfo[];
}) {
  const setPortsTarget = useStore((s) => s.setPortsTarget);
  const locals = (sandbox: string | null) =>
    forwards
      .filter((f) => f.sandbox === sandbox)
      .map((f) => `:${f.localPort}`)
      .join(' ');
  const chip = (sandbox: string | null, label: string, title: string, running: boolean) => {
    const tunnels = locals(sandbox);
    return (
      <button
        key={sandbox ?? ''}
        className={`sbx-chip${target === sandbox ? ' active' : ''}`}
        title={title}
        onClick={() => setPortsTarget(hostId, sandbox)}
      >
        <span className={`pdot${running ? ' live' : ''}`} />
        {label}
        {tunnels && <span className="sbx-tunnels">→ {tunnels}</span>}
      </button>
    );
  };
  return (
    <div className="sbx-bar">
      <span className="sbx-bar-label">target</span>
      {chip(null, 'host', 'ports listening on the host itself', true)}
      {sandboxes.map((sb) =>
        chip(
          sb.name,
          sb.name,
          `Docker sandbox · ${sb.agent || 'agent'} · ${sb.status}` +
            (sb.workspaces.length ? `\n${sb.workspaces.join('\n')}` : ''),
          sb.status === 'running',
        ),
      )}
    </div>
  );
}

/** Every port appears exactly once, in one of four states. */
type RowState = 'open' | 'forwarded' | 'waiting' | 'advertised';

const STATE_LABEL: Record<RowState, string> = {
  open: 'open on host',
  forwarded: 'forwarded',
  waiting: 'waiting for process',
  advertised: 'advertised · not listening',
};

const NO_PORTS: RemotePort[] = [];
const NO_FORWARDS: ForwardInfo[] = [];
const NO_SANDBOXES: SandboxInfo[] = [];
const NO_ADVERTISED: AdvertisedPort[] = [];

export function PortsView() {
  const hostId = useStore((s) => s.focusedHostId);
  const hostPorts = useStore((s) => (hostId ? (s.sessions[hostId]?.ports ?? NO_PORTS) : NO_PORTS));
  const allForwards = useStore((s) =>
    hostId ? (s.sessions[hostId]?.forwards ?? NO_FORWARDS) : NO_FORWARDS,
  );
  const sandboxes = useStore((s) =>
    hostId ? (s.sessions[hostId]?.sandboxes ?? NO_SANDBOXES) : NO_SANDBOXES,
  );
  const sbxError = useStore((s) => (hostId ? (s.sessions[hostId]?.sbxError ?? null) : null));
  const target = useStore((s) => (hostId ? (s.sessions[hostId]?.portsTarget ?? null) : null));
  const hostUnsupported = useStore((s) =>
    hostId ? (s.sessions[hostId]?.portsUnsupported ?? false) : false,
  );
  const connState = useStore((s) => (hostId ? s.sessions[hostId]?.conn.state : undefined));
  const portError = useStore((s) => (hostId ? (s.sessions[hostId]?.portError ?? null) : null));
  const setForward = useStore((s) => s.setForward);
  const dismissPortError = useStore((s) => s.dismissPortError);

  if (!hostId) return null;

  // Ports can only be scanned over a live connection. Without one, the scanner
  // never runs — so distinguish "not connected" from "connected, scanning".
  const connected = connState === 'connected';

  // The ports tab shows one target at a time: the host, or one sandbox.
  const sandbox = target != null ? (sandboxes.find((sb) => sb.name === target) ?? null) : null;
  const sandboxStopped = sandbox != null && sandbox.status !== 'running';
  const ports = sandbox ? sandbox.ports : hostPorts;
  const unsupported = sandbox ? false : hostUnsupported;
  const forwards = allForwards.filter((f) => f.sandbox === target);
  const advertised = sandbox?.advertised ?? NO_ADVERTISED;
  const advLabel = new Map<number, string | null>(advertised.map((a) => [a.port, a.label]));
  // Local ports already tunnelling to *another* target of this host — a
  // forward on one of them switches it over instead of binding a new one.
  const localElsewhere = new Map<number, ForwardInfo>(
    allForwards.filter((f) => f.sandbox !== target).map((f) => [f.localPort, f]),
  );
  const describe = (f: ForwardInfo) => (f.sandbox ? `sandbox ${f.sandbox}` : 'the host');

  const fwdByPort = new Map<number, ForwardInfo>(forwards.map((f) => [f.port, f]));
  const scannedPorts = new Set(ports.map((p) => p.port));

  // Union of scanner results and forwards whose remote process is gone — each
  // port exactly once. Active tunnels float to the top: pinned first, then
  // other forwards, then the remaining listening ports — each group by port.
  const rank = (r: { fwd: ForwardInfo | null }) => (r.fwd ? (r.fwd.pinned ? 0 : 1) : 2);
  const rows: {
    port: number;
    info: RemotePort | null;
    fwd: ForwardInfo | null;
    state: RowState;
  }[] = [
    ...ports.map((p) => ({ port: p.port, info: p, fwd: fwdByPort.get(p.port) ?? null })),
    ...forwards
      .filter((f) => !scannedPorts.has(f.port))
      .map((f) => ({ port: f.port, info: null, fwd: f })),
    // Advertised by the sandbox but nothing listens yet (and not forwarded)
    ...advertised
      .filter((a) => !scannedPorts.has(a.port) && !fwdByPort.has(a.port))
      .map((a) => ({ port: a.port, info: null, fwd: null })),
  ]
    .map((r) => ({
      ...r,
      state: (r.fwd
        ? r.fwd.live
          ? 'forwarded'
          : 'waiting'
        : r.info
          ? 'open'
          : 'advertised') as RowState,
    }))
    .sort((a, b) => rank(a) - rank(b) || a.port - b.port);

  return (
    <div className="view">
      <div className="ports-head">
        <div className="flex-1">
          <div className="ports-title">
            {target ? `Port forwarding · ${target}` : 'Port forwarding'}
          </div>
          <div className="ports-desc">
            {target ? (
              <>
                Ports listening inside this Docker sandbox, its docker containers included —
                nothing needs to be published with <code>sbx ports</code>. Forwarding a port
                whose local port another sandbox holds <b>switches</b> it here, so{' '}
                <b>localhost</b> (and the OAuth redirect on it) stays the same.
              </>
            ) : (
              <>
                Ports listening on the remote appear here live. Toggle to tunnel to{' '}
                <b>localhost</b> — click the local target to bind a different port, or pin one
                to keep the tunnel and reclaim it automatically when the dev server restarts.
              </>
            )}
          </div>
        </div>
        <div className="scan-chip">auto-scan · {target ? '5s' : '3s'} ↻</div>
      </div>
      {sandboxes.length > 0 && (
        <SandboxBar
          hostId={hostId}
          target={target}
          sandboxes={sandboxes}
          forwards={allForwards}
        />
      )}
      <div className="ports-body">
        {sbxError && sandboxes.length > 0 && <div className="pane-msg error">{sbxError}</div>}
        {sandbox && !sandboxStopped && (
          <ServicesPanel hostId={hostId} sandbox={sandbox.name} services={sandbox.services} />
        )}
        {portError && (
          <div className="port-err-banner">
            <span className="port-err-msg">{portError.message}</span>
            {portError.hint != null && (
              <button
                className="port-err-action"
                onClick={() =>
                  setForward(
                    hostId,
                    portError.port,
                    true,
                    portError.pinned,
                    portError.hint!,
                    portError.sandbox,
                  )
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
        <div className="pcols">
          <span className="pcol-port">REMOTE</span>
          <span className="pcol-proc">PROCESS</span>
          <span className="pcol-bind">BIND</span>
          <span className="pcol-state">STATE</span>
          <span className="pcol-local">LOCAL TUNNEL</span>
          <span className="pcol-act">ACTIONS</span>
        </div>
        {rows.length === 0 && !unsupported && (
          <div className="pane-msg">
            {sandboxStopped
              ? `${target} is ${sandbox?.status} — nettle doesn't start sandboxes. Run it (sbx run / sbx-forge) and its ports show up here; pinned tunnels resume on their own.`
              : sandbox && connected
                ? 'Nothing is listening in this sandbox yet. Services bound to 127.0.0.1 inside it work too.'
                : connected
              ? 'Scanning remote ports…'
              : connState === 'reconnecting'
                ? 'Reconnecting — ports will refresh once the link is back.'
                : connState === 'connecting' || connState === 'authenticating'
                  ? 'Connecting…'
                  : 'Not connected — this host is unreachable or the session is down.'}
          </div>
        )}
        {rows.map(({ port, info, fwd, state }) => {
          const listening = info != null;
          const dir = shortDir(info?.cwd);
          const holder = fwd ? null : (localElsewhere.get(port) ?? null);
          const adv = advLabel.has(port);
          const label = advLabel.get(port) ?? null;
          return (
            <div key={port} className={`prow${fwd ? ' fwd' : ''}`}>
              <div className="pcol-port" style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                <span className={`pdot${listening ? ' live' : ' waiting'}`} />
                <span className="pport">{port}</span>
              </div>
              <div className="pcol-proc" style={{ minWidth: 0 }}>
                <div className="pproc">
                  {adv && (
                    <span
                      className="padvertised"
                      title="advertised by the sandbox (nettle-port): forwarded as soon as it listens"
                    >
                      ★ {label ?? 'advertised'}
                    </span>
                  )}
                  {info?.process ?? (adv ? '' : '—')}
                  {info?.container && (
                    <span className="pcontainer" title={`docker container: ${info.container}`}>
                      ⬡ {info.container}
                    </span>
                  )}
                </div>
                {dir && (
                  <div className="pcwd" title={info?.cwd ?? undefined}>
                    in {dir}
                  </div>
                )}
              </div>
              <span className="pcol-bind pbind">
                {info ? `${info.bind}:${port}` : '—'}
              </span>
              <span className="pcol-state">
                <span className={`state-chip ${state}`}>
                  {state === 'open' && target ? 'listening' : STATE_LABEL[state]}
                </span>
              </span>
              <div className="pcol-local">
                {holder ? (
                  <span className="plocal off" title={`localhost:${port} tunnels to ${describe(holder)}`}>
                    :{port} → {holder.sandbox ?? 'host'}
                  </span>
                ) : (
                  <LocalTunnel hostId={hostId} port={port} fwd={fwd} sandbox={target} />
                )}
              </div>
              <div
                className="pcol-act"
                style={{ display: 'flex', alignItems: 'center', justifyContent: 'flex-end', gap: 8 }}
              >
                {listening && (
                  <KillButton hostId={hostId} port={port} pid={info?.pid ?? null} sandbox={target} />
                )}
                {(listening || fwd) && (
                  <button
                    className="open-btn"
                    title="open in browser (forwards if needed)"
                    onClick={() =>
                      openPortInBrowser(
                        hostId,
                        port,
                        fwd?.localPort ?? (holder ? port : null),
                        target,
                      )
                    }
                  >
                    ↗ open
                  </button>
                )}
                <button
                  className={`pin-tgl${fwd?.pinned ? ' on' : ''}`}
                  title={
                    fwd?.pinned
                      ? 'unpin — stop keeping this tunnel across restarts'
                      : 'pin — forward now and keep the tunnel across restarts and reconnects'
                  }
                  onClick={() =>
                    setForward(hostId, port, true, !(fwd?.pinned ?? false), fwd?.localPort, target)
                  }
                >
                  <span className="pin-glyph">⚲</span> {fwd?.pinned ? 'pinned' : 'pin'}
                </button>
                <button
                  className={`tgl-btn${fwd ? ' on' : ''}${holder ? ' switch' : ''}`}
                  title={
                    holder
                      ? `localhost:${port} tunnels to ${describe(holder)} — move it here`
                      : undefined
                  }
                  onClick={() =>
                    fwd
                      ? setForward(hostId, port, false, false, undefined, target)
                      : setForward(hostId, port, true, false, undefined, target)
                  }
                >
                  {fwd ? 'forwarded' : holder ? '⇄ switch here' : 'forward'}
                </button>
              </div>
            </div>
          );
        })}
        <AddForward hostId={hostId} sandbox={target} />
      </div>
    </div>
  );
}
