import { useStore, type View } from '../store';

const TABS: View[] = ['files', 'ports', 'terminal'];

export function Topbar() {
  const conn = useStore((s) => s.conn);
  const hosts = useStore((s) => s.hosts);
  const activeHostId = useStore((s) => s.activeHostId);
  const view = useStore((s) => s.view);
  const setView = useStore((s) => s.setView);
  const forwards = useStore((s) => s.forwards);
  const disconnect = useStore((s) => s.disconnect);

  const host = hosts.find((h) => h.id === activeHostId);
  const reconnecting = conn.state === 'reconnecting';

  const dotClass =
    conn.state === 'connected'
      ? 'online'
      : reconnecting || conn.state === 'connecting' || conn.state === 'authenticating'
        ? 'reconnecting'
        : 'off';

  return (
    <div className="topbar">
      <div className="topbar-session">
        <span className={`conn-dot ${dotClass}`} />
        <span className="session-name">{host?.name ?? '—'}</span>
        <span className="session-addr">
          {host ? `${host.username}@${host.hostname}` : ''}
        </span>
        {reconnecting && <span className="reconn-chip">reconnecting · resolving DNS…</span>}
      </div>
      {TABS.map((t) => (
        <button
          key={t}
          className={`tab${view === t ? ' active' : ''}`}
          onClick={() => setView(t)}
        >
          {t}
          {t === 'ports' && <span className="tab-badge">{forwards.length}</span>}
        </button>
      ))}
      <div className="flex-1" />
      {activeHostId && (
        <button className="disconnect-btn" onClick={() => disconnect()}>
          <span className="dot" />
          disconnect
        </button>
      )}
    </div>
  );
}
