export function fmtBytes(n: number | null | undefined): string {
  if (n == null) return '—';
  if (n < 1024) return `${n} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v >= 100 ? Math.round(v) : v.toFixed(1)} ${units[i]}`;
}

export function fmtAgo(unixSecs: number | null | undefined): string {
  if (!unixSecs) return '—';
  const d = Math.max(0, Date.now() / 1000 - unixSecs);
  if (d < 60) return 'now';
  if (d < 3600) return `${Math.floor(d / 60)}m ago`;
  if (d < 86400) return `${Math.floor(d / 3600)}h ago`;
  if (d < 604800) return `${Math.floor(d / 86400)}d ago`;
  return `${Math.floor(d / 604800)}w ago`;
}

export function fmtUptime(sinceMs: number, nowMs: number): string {
  const s = Math.max(0, Math.floor((nowMs - sinceMs) / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const pad = (x: number) => String(x).padStart(2, '0');
  return `${pad(h)}:${pad(m)}:${pad(sec)}`;
}

const EXT_META: Record<string, { tag: string; color: string }> = {
  js: { tag: 'js', color: 'var(--amber)' },
  jsx: { tag: 'js', color: 'var(--amber)' },
  ts: { tag: 'js', color: 'var(--amber)' },
  tsx: { tag: 'js', color: 'var(--amber)' },
  mjs: { tag: 'js', color: 'var(--amber)' },
  json: { tag: 'jsn', color: 'var(--green)' },
  css: { tag: 'css', color: 'var(--blue)' },
  html: { tag: 'htm', color: 'var(--blue)' },
  htm: { tag: 'htm', color: 'var(--blue)' },
  md: { tag: 'md', color: 'var(--dim)' },
  env: { tag: 'env', color: 'var(--faint)' },
  svg: { tag: 'svg', color: 'var(--violet)' },
  py: { tag: 'py', color: 'var(--blue)' },
  rs: { tag: 'rs', color: 'var(--amber)' },
  go: { tag: 'go', color: 'var(--blue)' },
  sh: { tag: 'sh', color: 'var(--green)' },
  yml: { tag: 'yml', color: 'var(--green)' },
  yaml: { tag: 'yml', color: 'var(--green)' },
  toml: { tag: 'tml', color: 'var(--green)' },
  lock: { tag: 'lck', color: 'var(--faint)' },
  ico: { tag: 'bin', color: 'var(--faint)' },
  png: { tag: 'img', color: 'var(--violet)' },
  jpg: { tag: 'img', color: 'var(--violet)' },
  gif: { tag: 'img', color: 'var(--violet)' },
  log: { tag: 'log', color: 'var(--faint)' },
};

export function fileMeta(name: string): { tag: string; color: string } {
  if (/^dockerfile$/i.test(name)) return { tag: 'dkr', color: 'var(--faint)' };
  if (name.startsWith('.env')) return { tag: 'env', color: 'var(--faint)' };
  const ext = name.includes('.') ? name.split('.').pop()!.toLowerCase() : '';
  return EXT_META[ext] ?? { tag: 'txt', color: 'var(--dim)' };
}

export function parentDir(path: string, sep: string): string | null {
  const trimmed = path.endsWith(sep) && path.length > 1 ? path.slice(0, -1) : path;
  const idx = trimmed.lastIndexOf(sep);
  if (idx <= 0) return trimmed.length > 1 ? sep : null;
  return trimmed.slice(0, idx);
}

export function crumbsOf(path: string, sep: string): { label: string; target: string }[] {
  if (!path) return [];
  const parts = path.split(sep).filter(Boolean);
  const crumbs: { label: string; target: string }[] = [];
  if (path.startsWith(sep)) {
    crumbs.push({ label: sep, target: sep });
  }
  let acc = path.startsWith(sep) ? '' : '';
  for (const part of parts) {
    acc = acc + sep + part;
    const target = path.startsWith(sep) ? acc : acc.slice(1);
    crumbs.push({ label: part, target });
  }
  return crumbs;
}
