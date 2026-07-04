import { useStore, type View } from '../store';

const SESSION_TABS: View[] = ['files', 'ports', 'terminal'];

export function Topbar() {
  const hosts = useStore((s) => s.hosts);
  const focusedHostId = useStore((s) => s.focusedHostId);
  const session = useStore((s) => (focusedHostId ? s.sessions[focusedHostId] : null));
  const view = useStore((s) => s.view);
  const setView = useStore((s) => s.setView);
  const disconnect = useStore((s) => s.disconnect);
  const activeSessions = useStore((s) => Object.keys(s.sessions).length);

  const host = hosts.find((h) => h.id === focusedHostId);
  const conn = session?.conn;
  const reconnecting = conn?.state === 'reconnecting';
  const forwards = session?.forwards ?? [];

  const dotClass =
    conn?.state === 'connected'
      ? 'online'
      : reconnecting || conn?.state === 'connecting' || conn?.state === 'authenticating'
        ? 'reconnecting'
        : 'off';

  return (
    <div className="topbar">
      <button
        className={`tab dash${view === 'dashboard' ? ' active' : ''}`}
        title="Tunnels dashboard"
        onClick={() => setView('dashboard')}
      >
        ⚲ dashboard
        {activeSessions > 0 && <span className="tab-badge">{activeSessions}</span>}
      </button>
      {session && host && (
        <>
          <span className="topbar-divider" />
          <div className="topbar-session">
            <span className={`conn-dot ${dotClass}`} />
            <span className="session-name">{host.name}</span>
            <span className="session-addr">
              {host.username}@{host.hostname}
            </span>
            {reconnecting && <span className="reconn-chip">reconnecting · resolving DNS…</span>}
          </div>
          {SESSION_TABS.map((t) => (
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
          <button className="disconnect-btn" onClick={() => disconnect(host.id)}>
            <span className="dot" />
            disconnect
          </button>
        </>
      )}
      {!session && <div className="flex-1" />}
    </div>
  );
}
