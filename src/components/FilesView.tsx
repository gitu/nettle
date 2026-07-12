import { useState } from 'react';
import { useStore, type TransferRow } from '../store';
import { api, type DirListing, type FileEntry } from '../ipc';
import { applyFileView, TIME_FILTERS, type FileViewOpts } from '../fileView';
import { crumbsOf, fileMeta, fmtAgo, fmtBytes } from '../util';

function Pane({
  side,
  listing,
  selected,
  error,
  onNavigate,
  onSelect,
  onAction,
}: {
  side: 'remote' | 'local';
  listing: DirListing | null;
  selected: string | null;
  error?: string | null;
  onNavigate: (path: string) => void;
  onSelect: (name: string | null) => void;
  onAction: (name: string) => void;
}) {
  // Search / sort / filter state is per-pane, so local and remote are
  // independent.
  const [view, setView] = useState<Omit<FileViewOpts, 'nowSec'>>({
    query: '',
    sortBy: 'name',
    groupDirs: true,
    sinceHours: null,
  });

  const sep = side === 'local' && listing?.path.includes('\\') ? '\\' : '/';
  const crumbs = listing ? crumbsOf(listing.path, sep) : [];
  const actionGlyph = side === 'remote' ? '⤓' : '⤒';
  const entries = listing
    ? applyFileView(listing.entries, { ...view, nowSec: Date.now() / 1000 })
    : [];
  // The directory has content, but the current search/filter hid all of it.
  const filteredEmpty = !error && !!listing && listing.entries.length > 0 && entries.length === 0;
  const patch = (p: Partial<Omit<FileViewOpts, 'nowSec'>>) => setView((v) => ({ ...v, ...p }));

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
      <FileControls opts={view} onChange={patch} />
      <div className="pane-body">
        {error && <div className="pane-msg error">{error}</div>}
        {!error && !listing && (
          <div className="pane-msg">{side === 'remote' ? 'Not connected.' : 'No folder open.'}</div>
        )}
        {!error && listing?.entries.length === 0 && (
          <div className="pane-msg">Empty directory.</div>
        )}
        {filteredEmpty && <div className="pane-msg">No files match the current filter.</div>}
        {listing &&
          entries.map((f: FileEntry) => {
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

function TransferQueue({ hostId }: { hostId: string }) {
  const transfers = useStore((s) => s.sessions[hostId]?.transfers ?? {});
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
          api.transferClearFinished(hostId).catch(() => {});
          useStore.setState((s) => {
            const sess = s.sessions[hostId];
            if (!sess) return {};
            const kept = Object.fromEntries(
              Object.entries(sess.transfers).filter(
                ([, t]) => t.status === 'running' || t.status === 'queued',
              ),
            );
            return { sessions: { ...s.sessions, [hostId]: { ...sess, transfers: kept } } };
          });
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
                <button className="tcancel" onClick={() => api.transferCancel(hostId, t.id)}>
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

function FileControls({
  opts,
  onChange,
}: {
  opts: Omit<FileViewOpts, 'nowSec'>;
  onChange: (patch: Partial<Omit<FileViewOpts, 'nowSec'>>) => void;
}) {
  return (
    <div className="file-controls">
      <input
        className="file-search"
        placeholder="search files…"
        value={opts.query}
        onChange={(e) => onChange({ query: e.target.value })}
      />
      {opts.query && (
        <button className="file-clear" title="clear" onClick={() => onChange({ query: '' })}>
          ✕
        </button>
      )}
      <span className="fc-sep" />
      <span className="fc-label">sort</span>
      <div className="fc-group">
        <button
          className={`fc-chip${opts.sortBy === 'name' ? ' on' : ''}`}
          onClick={() => onChange({ sortBy: 'name' })}
        >
          name
        </button>
        <button
          className={`fc-chip${opts.sortBy === 'modified' ? ' on' : ''}`}
          onClick={() => onChange({ sortBy: 'modified' })}
        >
          modified
        </button>
      </div>
      <span className="fc-sep" />
      <span className="fc-label">modified</span>
      <div className="fc-group">
        {TIME_FILTERS.map((t) => (
          <button
            key={t.label}
            className={`fc-chip${opts.sinceHours === t.hours ? ' on' : ''}`}
            onClick={() => onChange({ sinceHours: t.hours })}
          >
            {t.label}
          </button>
        ))}
      </div>
      <span className="fc-sep" />
      <button
        className={`fc-chip${opts.groupDirs ? ' on' : ''}`}
        title="keep folders sorted before files"
        onClick={() => onChange({ groupDirs: !opts.groupDirs })}
      >
        group folders
      </button>
    </div>
  );
}

export function FilesView() {
  const hostId = useStore((s) => s.focusedHostId);
  const remote = useStore((s) => (hostId ? (s.sessions[hostId]?.remote ?? null) : null));
  const remoteSel = useStore((s) => (hostId ? (s.sessions[hostId]?.remoteSel ?? null) : null));
  const local = useStore((s) => s.local);
  const localSel = useStore((s) => s.localSel);
  const localError = useStore((s) => s.localError);
  const navigateRemote = useStore((s) => s.navigateRemote);
  const navigateLocal = useStore((s) => s.navigateLocal);
  const startTransfer = useStore((s) => s.startTransfer);

  if (!hostId) return null;

  return (
    <div className="view">
      <div className="panes">
        <Pane
          side="local"
          listing={local}
          selected={localSel}
          error={localError}
          onNavigate={(p) => navigateLocal(p).catch(() => {})}
          onSelect={(name) => useStore.setState({ localSel: name })}
          onAction={(name) => startTransfer(hostId, 'up', name)}
        />
        <Pane
          side="remote"
          listing={remote}
          selected={remoteSel}
          onNavigate={(p) => navigateRemote(hostId, p).catch(() => {})}
          onSelect={(name) =>
            useStore.setState((s) => {
              const sess = s.sessions[hostId];
              if (!sess) return {};
              return { sessions: { ...s.sessions, [hostId]: { ...sess, remoteSel: name } } };
            })
          }
          onAction={(name) => startTransfer(hostId, 'down', name)}
        />
      </div>
      <TransferQueue hostId={hostId} />
    </div>
  );
}
