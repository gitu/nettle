import { useEffect, useRef, useState } from 'react';
import { api, type SandboxService } from '../ipc';
import { useStore } from '../store';

/** Live tail of one service's log: re-fetched every 2 s while open, pinned to
 *  the bottom unless the user scrolled up. */
function ServiceLogs({
  hostId,
  sandbox,
  service,
  onClose,
}: {
  hostId: string;
  sandbox: string;
  service: string;
  onClose: () => void;
}) {
  const [text, setText] = useState('');
  const [error, setError] = useState<string | null>(null);
  const pre = useRef<HTMLPreElement>(null);
  const stick = useRef(true);

  useEffect(() => {
    let alive = true;
    const load = () =>
      api
        .sandboxServiceLogs(hostId, sandbox, service, 1000)
        .then((t) => {
          if (!alive) return;
          setText(t);
          setError(null);
        })
        .catch((e: unknown) => alive && setError((e as { message?: string })?.message ?? String(e)));
    load();
    const t = setInterval(load, 2000);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, [hostId, sandbox, service]);

  useEffect(() => {
    if (stick.current && pre.current) pre.current.scrollTop = pre.current.scrollHeight;
  }, [text]);

  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal svc-logs">
        <div className="svc-logs-head">
          <span className="svc-logs-title">
            {service} <span className="svc-logs-sub">· {sandbox} · sbx-svc logs</span>
          </span>
          <span className="flex-1" />
          <span className="svc-logs-live">↻ 2s</span>
          <button className="toast-ghost" onClick={onClose}>
            Close
          </button>
        </div>
        {error && <div className="pane-msg error">{error}</div>}
        <pre
          ref={pre}
          className="svc-logs-body"
          onScroll={(e) => {
            const el = e.currentTarget;
            stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
          }}
        >
          {text || (error ? '' : 'loading…')}
        </pre>
      </div>
    </div>
  );
}

/** The sandbox's `sbx-svc` services: state, port, start/stop/restart, logs. */
export function ServicesPanel({
  hostId,
  sandbox,
  services,
}: {
  hostId: string;
  sandbox: string;
  services: SandboxService[];
}) {
  const serviceAction = useStore((s) => s.serviceAction);
  const [busy, setBusy] = useState<string | null>(null);
  const [logsFor, setLogsFor] = useState<string | null>(null);

  if (services.length === 0) return null;
  const act = async (name: string, action: 'start' | 'stop' | 'restart') => {
    setBusy(name);
    await serviceAction(hostId, sandbox, name, action);
    setBusy(null);
  };

  return (
    <div className="svc-panel">
      <div className="svc-panel-title">SERVICES</div>
      {services.map((svc) => (
        <div key={svc.name} className="svc-row">
          <span className={`pdot${svc.running ? ' live' : ''}`} />
          <span className="svc-name">{svc.name}</span>
          <span className={`svc-state${svc.running ? ' running' : ''}`}>
            {busy === svc.name ? '…' : svc.running ? 'running' : 'stopped'}
          </span>
          <span className="svc-port">{svc.port != null ? `:${svc.port}` : ''}</span>
          {svc.autostart && (
            <span className="svc-auto" title="started whenever the sandbox starts">
              auto
            </span>
          )}
          <span className="svc-cmd" title={svc.command}>
            {svc.command}
          </span>
          <span className="svc-actions">
            {svc.running ? (
              <>
                <button className="open-btn" disabled={busy != null} onClick={() => act(svc.name, 'restart')}>
                  ↻ restart
                </button>
                <button className="kill-btn" disabled={busy != null} onClick={() => act(svc.name, 'stop')}>
                  ■ stop
                </button>
              </>
            ) : (
              <button className="tgl-btn" disabled={busy != null} onClick={() => act(svc.name, 'start')}>
                ▶ start
              </button>
            )}
            <button className="open-btn" onClick={() => setLogsFor(svc.name)}>
              ☰ logs
            </button>
          </span>
        </div>
      ))}
      {logsFor && (
        <ServiceLogs
          hostId={hostId}
          sandbox={sandbox}
          service={logsFor}
          onClose={() => setLogsFor(null)}
        />
      )}
    </div>
  );
}
