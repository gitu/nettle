import { describe, expect, it } from 'vitest';
import { applyFileView, type FileViewOpts } from './fileView';
import type { FileEntry } from './ipc';

const NOW = 1_000_000; // seconds

function f(name: string, kind: FileEntry['kind'], mtime: number | null): FileEntry {
  return { name, kind, size: kind === 'dir' ? null : 10, mtime };
}

// A mixed listing: two dirs, three files with distinct mtimes.
const entries: FileEntry[] = [
  f('zeta', 'file', NOW - 30), // 30s ago (newest file)
  f('alpha', 'dir', NOW - 5),
  f('beta.log', 'file', NOW - 3600 * 2), // 2h ago
  f('Mods', 'dir', NOW - 999),
  f('gamma.txt', 'file', NOW - 3600 * 48), // 2 days ago
];

const base: FileViewOpts = {
  query: '',
  sortBy: 'name',
  groupDirs: true,
  sinceHours: null,
  nowSec: NOW,
};

const names = (es: FileEntry[]) => es.map((e) => e.name);

describe('applyFileView', () => {
  it('groups directories first, then sorts by name case-insensitively', () => {
    expect(names(applyFileView(entries, base))).toEqual([
      'alpha',
      'Mods',
      'beta.log',
      'gamma.txt',
      'zeta',
    ]);
  });

  it('can mix folders in when grouping is disabled', () => {
    const out = names(applyFileView(entries, { ...base, groupDirs: false }));
    // pure alphabetical, dirs interleaved
    expect(out).toEqual(['alpha', 'beta.log', 'gamma.txt', 'Mods', 'zeta']);
  });

  it('sorts by modified time newest-first, dirs still grouped', () => {
    const out = names(applyFileView(entries, { ...base, sortBy: 'modified' }));
    // dirs first by their own mtime (alpha newer than Mods), then files newest→oldest
    expect(out).toEqual(['alpha', 'Mods', 'zeta', 'beta.log', 'gamma.txt']);
  });

  it('sorts by modified across dirs and files when not grouping', () => {
    const out = names(applyFileView(entries, { ...base, sortBy: 'modified', groupDirs: false }));
    expect(out).toEqual(['alpha', 'zeta', 'Mods', 'beta.log', 'gamma.txt']);
  });

  it('filters by case-insensitive name substring (dirs included)', () => {
    expect(names(applyFileView(entries, { ...base, query: 'a' }))).toEqual([
      'alpha',
      'beta.log',
      'gamma.txt',
      'zeta',
    ]);
    expect(names(applyFileView(entries, { ...base, query: 'MOD' }))).toEqual(['Mods']);
    expect(names(applyFileView(entries, { ...base, query: 'nope' }))).toEqual([]);
  });

  it('time filter keeps recent files and always keeps directories', () => {
    // last 1 hour: only zeta (30s) qualifies among files; both dirs stay.
    expect(names(applyFileView(entries, { ...base, sinceHours: 1 }))).toEqual([
      'alpha',
      'Mods',
      'zeta',
    ]);
    // last 24 hours: zeta + beta.log (2h), not gamma (2 days).
    expect(names(applyFileView(entries, { ...base, sinceHours: 24 }))).toEqual([
      'alpha',
      'Mods',
      'beta.log',
      'zeta',
    ]);
  });

  it('drops files with unknown mtime when a time filter is active', () => {
    const withUnknown = [...entries, f('mystery.bin', 'file', null)];
    const out = names(applyFileView(withUnknown, { ...base, sinceHours: 24 }));
    expect(out).not.toContain('mystery.bin');
  });

  it('combines search + time filter + modified sort', () => {
    const out = names(
      applyFileView(entries, {
        ...base,
        query: '.',
        sinceHours: 24,
        sortBy: 'modified',
        groupDirs: false,
      }),
    );
    // names containing '.', modified within 24h: beta.log (2h). zeta has no dot.
    expect(out).toEqual(['beta.log']);
  });

  it('does not mutate the input array', () => {
    const input = [...entries];
    applyFileView(input, { ...base, sortBy: 'modified' });
    expect(names(input)).toEqual(names(entries));
  });
});
