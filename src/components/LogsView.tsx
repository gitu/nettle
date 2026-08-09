import { useEffect, useRef, useState } from 'react';
import { useStore } from '../store';
import type { LogLevel } from '../ipc';

function fmtTime(tsMs: number): string {
  const d = new Date(tsMs);
  const p = (n: number, w = 2) => String(n).padStart(w, '0');
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

/** The in-app activity log: everything nettle has been doing — connections,
 *  tunnels, scans, kills — filterable by host and severity. */
export function LogsView() {
  const activity = useStore((s) => s.activity);
  const hosts = useStore((s) => s.hosts);
  const clearActivity = useStore((s) => s.clearActivity);

  const [levelFilter, setLevelFilter] = useState<LogLevel | 'all'>('all');
  const [hostFilter, setHostFilter] = useState<string>('all');
  const [follow, setFollow] = useState(true);
  const endRef = useRef<HTMLDivElement>(null);

  const hostName = (id: string | null) => {
    if (!id) return 'app';
    return hosts.find((h) => h.id === id)?.name ?? id.slice(0, 8);
  };

  const rows = activity.filter(
    (a) =>
      (levelFilter === 'all' || a.level === levelFilter) &&
      (hostFilter === 'all' || a.hostId === hostFilter),
  );

  useEffect(() => {
    if (follow) endRef.current?.scrollIntoView({ block: 'end' });
  }, [rows.length, follow]);

  const hostIds = [...new Set(activity.map((a) => a.hostId).filter((id): id is string => !!id))];

  return (
    <div className="view">
      <div className="ports-head">
        <div className="flex-1">
          <div className="ports-title">Activity</div>
          <div className="ports-desc">
            A live log of what nettle is doing — connections, tunnels, port scans and remote
            kills. Kept for this app run only (last 1000 entries).
          </div>
        </div>
        <div className="log-controls">
          <select
            className="log-select"
            value={levelFilter}
            onChange={(e) => setLevelFilter(e.target.value as LogLevel | 'all')}
          >
            <option value="all">all levels</option>
            <option value="info">info</option>
            <option value="warn">warn</option>
            <option value="error">error</option>
          </select>
          <select
            className="log-select"
            value={hostFilter}
            onChange={(e) => setHostFilter(e.target.value)}
          >
            <option value="all">all hosts</option>
            {hostIds.map((id) => (
              <option key={id} value={id}>
                {hostName(id)}
              </option>
            ))}
          </select>
          <button
            className={`log-follow${follow ? ' on' : ''}`}
            title="auto-scroll to the newest entry"
            onClick={() => setFollow(!follow)}
          >
            ↓ follow
          </button>
          <button className="log-clear" onClick={() => clearActivity()}>
            clear
          </button>
        </div>
      </div>
      <div className="ports-body log-body">
        {rows.length === 0 && (
          <div className="pane-msg">
            {activity.length === 0
              ? 'Nothing logged yet — connect to a host and activity will show up here.'
              : 'No entries match the current filters.'}
          </div>
        )}
        {rows.map((a) => (
          <div key={a.seq} className={`log-row ${a.level}`}>
            <span className="log-time">{fmtTime(a.tsMs)}</span>
            <span className={`log-level ${a.level}`}>{a.level}</span>
            <span className="log-host" title={a.hostId ?? undefined}>
              {hostName(a.hostId)}
            </span>
            <span className="log-cat">{a.category}</span>
            <span className="log-msg">{a.message}</span>
          </div>
        ))}
        <div ref={endRef} />
      </div>
    </div>
  );
}
