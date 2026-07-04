import { useStore, type TransferRow } from '../store';
import { api, type DirListing, type FileEntry } from '../ipc';
import { crumbsOf, fileMeta, fmtAgo, fmtBytes } from '../util';

function Pane({
  side,
  listing,
  selected,
  onNavigate,
  onSelect,
  onAction,
}: {
  side: 'remote' | 'local';
  listing: DirListing | null;
  selected: string | null;
  onNavigate: (path: string) => void;
  onSelect: (name: string | null) => void;
  onAction: (name: string) => void;
}) {
  const sep = side === 'local' && listing?.path.includes('\\') ? '\\' : '/';
  const crumbs = listing ? crumbsOf(listing.path, sep) : [];
  const actionGlyph = side === 'remote' ? '⤓' : '⤒';

  return (
    <div className={`pane ${side}`}>
      <div className="pane-head">
        <span className={`pane-label ${side}`}>{side.toUpperCase()}</span>
        <div className="crumbs">
          {crumbs.map((c, i) => (
            <button
              key={c.target}
              className={`crumb${i === crumbs.length - 1 ? ' last' : ''}`}
              onClick={() => onNavigate(c.target)}
            >
              {i === 0 ? c.label : `/ ${c.label}`}
            </button>
          ))}
        </div>
      </div>
      <div className="pane-body">
        {!listing && <div className="pane-msg">Not connected.</div>}
        {listing?.entries.length === 0 && <div className="pane-msg">Empty directory.</div>}
        {listing?.entries.map((f: FileEntry) => {
          const isDir = f.kind === 'dir';
          const meta = fileMeta(f.name);
          return (
            <div
              key={f.name}
              className={`frow${selected === f.name ? ' selected' : ''}`}
              onClick={() => {
                if (isDir) {
                  onNavigate(
                    listing.path.endsWith(sep)
                      ? listing.path + f.name
                      : listing.path + sep + f.name,
                  );
                } else {
                  onSelect(f.name);
                }
              }}
            >
              <span
                className={`ftag${isDir ? ' dir' : ''}`}
                style={isDir ? undefined : { color: meta.color }}
              >
                {isDir ? 'dir' : meta.tag}
              </span>
              <span className={`fname${isDir ? ' dir' : ''}`}>
                {isDir ? `${f.name}/` : f.name}
              </span>
              <span className="fsize">{isDir ? '—' : fmtBytes(f.size)}</span>
              <span className="fmod">{fmtAgo(f.mtime)}</span>
              {isDir ? (
                <span className="fact none" />
              ) : (
                <button
                  className="fact"
                  title={side === 'remote' ? 'download' : 'upload'}
                  onClick={(e) => {
                    e.stopPropagation();
                    onAction(f.name);
                  }}
                >
                  {actionGlyph}
                </button>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

function TransferQueue() {
  const transfers = useStore((s) => s.transfers);
  const rows: TransferRow[] = Object.values(transfers).sort((a, b) =>
    a.id < b.id ? 1 : -1,
  );
  const activeCount = rows.filter((t) => t.status === 'running' || t.status === 'queued').length;

  return (
    <div className="transfers">
      <div className="transfers-head">
        <span className="transfers-label">TRANSFERS</span>
        <span className={`transfers-active${activeCount ? ' busy' : ''}`}>
          {activeCount ? `${activeCount} active` : 'idle'}
        </span>
        <span className="flex-1" />
        <button className="clear-btn" onClick={() => {
          api.transferClearFinished().catch(() => {});
          useStore.setState((s) => ({
            transfers: Object.fromEntries(
              Object.entries(s.transfers).filter(
                ([, t]) => t.status === 'running' || t.status === 'queued',
              ),
            ),
          }));
        }}>
          clear finished
        </button>
      </div>
      <div className="transfers-body">
        {rows.length === 0 && (
          <div className="transfers-empty">
            No transfers yet — hover a file and hit ⤓ / ⤒ to move it.
          </div>
        )}
        {rows.map((t) => {
          const pct =
            t.status === 'done'
              ? 100
              : t.total
                ? Math.min(100, (t.bytes / t.total) * 100)
                : t.status === 'running'
                  ? 30
                  : 0;
          const pctLabel =
            t.status === 'done'
              ? 'done'
              : t.status === 'failed'
                ? 'failed'
                : t.status === 'cancelled'
                  ? 'cancel'
                  : t.status === 'queued'
                    ? 'queued'
                    : `${Math.round(pct)}%`;
          const meta =
            t.status === 'running' && t.rate
              ? `${fmtBytes(t.rate)}/s`
              : `${t.direction === 'down' ? '↓' : '↑'} ${fmtBytes(t.total ?? t.bytes)}`;
          return (
            <div className="trow" key={t.id}>
              <span className={`tdir ${t.direction}`}>{t.direction === 'down' ? '⤓' : '⤒'}</span>
              <span className="tname" title={t.error ?? undefined}>
                {t.name}
              </span>
              <div className="tbar">
                <div
                  className={`tbar-fill${t.status === 'done' ? ' done' : ''}${
                    t.status === 'failed' ? ' failed' : ''
                  }`}
                  style={{ width: `${pct}%` }}
                />
              </div>
              <span
                className={`tpct${t.status === 'done' ? ' done' : ''}${
                  t.status === 'failed' ? ' failed' : ''
                }`}
              >
                {pctLabel}
              </span>
              <span className="tmeta">{meta}</span>
              {(t.status === 'running' || t.status === 'queued') && (
                <button className="tcancel" onClick={() => api.transferCancel(t.id)}>
                  ✕
                </button>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

export function FilesView() {
  const remote = useStore((s) => s.remote);
  const local = useStore((s) => s.local);
  const remoteSel = useStore((s) => s.remoteSel);
  const localSel = useStore((s) => s.localSel);
  const navigateRemote = useStore((s) => s.navigateRemote);
  const navigateLocal = useStore((s) => s.navigateLocal);
  const startTransfer = useStore((s) => s.startTransfer);

  return (
    <div className="view">
      <div className="panes">
        <Pane
          side="remote"
          listing={remote}
          selected={remoteSel}
          onNavigate={(p) => navigateRemote(p).catch(() => {})}
          onSelect={(name) => useStore.setState({ remoteSel: name })}
          onAction={(name) => startTransfer('down', name)}
        />
        <Pane
          side="local"
          listing={local}
          selected={localSel}
          onNavigate={(p) => navigateLocal(p).catch(() => {})}
          onSelect={(name) => useStore.setState({ localSel: name })}
          onAction={(name) => startTransfer('up', name)}
        />
      </div>
      <TransferQueue />
    </div>
  );
}
