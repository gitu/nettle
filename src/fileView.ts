import type { FileEntry } from './ipc';

export type SortBy = 'name' | 'modified';

export interface FileViewOpts {
  /** Case-insensitive substring match on the file name. */
  query: string;
  sortBy: SortBy;
  /** Keep directories sorted ahead of files. */
  groupDirs: boolean;
  /** Only show files modified within this many hours (null = no time filter). */
  sinceHours: number | null;
  /** Current time in seconds since the epoch (injected so this stays pure). */
  nowSec: number;
}

export const TIME_FILTERS: { label: string; hours: number | null }[] = [
  { label: 'All', hours: null },
  { label: '1h', hours: 1 },
  { label: '24h', hours: 24 },
  { label: '7d', hours: 24 * 7 },
];

function nameLt(a: string, b: string): number {
  return a.toLowerCase().localeCompare(b.toLowerCase());
}

/**
 * Filter and sort a directory listing for display.
 *
 * - `query` matches file names (case-insensitive substring).
 * - `sinceHours` hides files older than the cutoff, but always keeps
 *   directories so you can still navigate into them.
 * - `sortBy` orders by name or by modified-time (newest first); when
 *   `groupDirs` is on, directories are kept ahead of files regardless.
 */
export function applyFileView(entries: FileEntry[], o: FileViewOpts): FileEntry[] {
  const q = o.query.trim().toLowerCase();
  const cutoff = o.sinceHours != null ? o.nowSec - o.sinceHours * 3600 : null;

  const filtered = entries.filter((e) => {
    if (q && !e.name.toLowerCase().includes(q)) return false;
    if (cutoff != null && e.kind !== 'dir') {
      if (e.mtime == null || e.mtime < cutoff) return false;
    }
    return true;
  });

  const sorted = [...filtered].sort((a, b) => {
    if (o.groupDirs) {
      const ad = a.kind === 'dir';
      const bd = b.kind === 'dir';
      if (ad !== bd) return ad ? -1 : 1;
    }
    if (o.sortBy === 'modified') {
      const am = a.mtime ?? 0;
      const bm = b.mtime ?? 0;
      if (am !== bm) return bm - am; // newest first
      return nameLt(a.name, b.name);
    }
    return nameLt(a.name, b.name);
  });

  return sorted;
}
