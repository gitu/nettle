import { useStore, type View } from '../store';

const SESSION_TABS: View[] = ['files', 'ports', 'terminal'];

export function Topbar() {
  const hosts = useStore((s) => s.hosts);
  const focusedHostId = useStore((s) => s.focusedHostId);
  const session = useStore((s) => (focusedHostId ? s.sessions[focusedHostId] : null));
  const view = useStore((s) => s.view);
  const setView = useStore((s) => s.setView);
  const disconnect = useStore((s) => s.disconnect);

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

  const dashboardTab = (
    <button
      className={`tab dash${view === 'dashboard' ? ' active' : ''}`}
      title="This host's tunnels"
      onClick={() => setView('dashboard')}
    >
      ⚲ dashboard
      {forwards.length > 0 && <span className="tab-badge">{forwards.length}</span>}
    </button>
  );

  return (
    <div className="topbar">
      {session && host ? (
        <>
          <div className="topbar-session">
            <span className={`conn-dot ${dotClass}`} />
            <span className="session-name">{host.name}</span>
            <span className="session-addr">
              {host.username}@{host.hostname}
            </span>
            {reconnecting && <span className="reconn-chip">reconnecting · resolving DNS…</span>}
          </div>
          <span className="topbar-divider" />
          {dashboardTab}
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
      ) : (
        <>
          <div className="topbar-session">
            <span className="session-name">⚲ Dashboard</span>
            <span className="session-addr">all hosts</span>
          </div>
          <div className="flex-1" />
        </>
      )}
    </div>
  );
}
