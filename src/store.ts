import { create } from 'zustand';
import { listen } from '@tauri-apps/api/event';
import {
  api,
  Channel,
  type AuthRequest,
  type ConnectionSet,
  type ConnState,
  type DirListing,
  type ForwardsChanged,
  type HostConfig,
  type HostKeyPrompt,
  type PortsChanged,
  type Settings,
  type TransferMeta,
  type TransferProgress,
} from './ipc';
import {
  applyConnState,
  applyForwards,
  applyPorts,
  applyTransfer,
  emptySession,
  type SessionState,
  type TransferRow,
} from './sessionReducer';

export type View = 'files' | 'ports' | 'terminal' | 'dashboard';

export type { SessionState, TransferRow };

interface NettleState {
  hosts: HostConfig[];
  sets: ConnectionSet[];
  settings: Settings;

  sessions: Record<string, SessionState>;
  focusedHostId: string | null;
  view: View;

  // shared local file pane
  local: DirListing | null;
  localSel: string | null;

  // modals / prompts (global, one at a time)
  hostKeyPrompt: HostKeyPrompt | null;
  hostKeyMismatch: HostKeyPrompt | null;
  authRequest: AuthRequest | null;
  editHost: HostConfig | 'new' | null;
  editSet: ConnectionSet | 'new' | null;
  aboutOpen: boolean;

  setView: (view: View) => void;
  refreshHosts: () => Promise<void>;
  refreshSets: () => Promise<void>;
  connect: (hostId: string) => Promise<void>;
  focusHost: (hostId: string) => void;
  disconnect: (hostId: string) => Promise<void>;
  navigateRemote: (hostId: string, path: string) => Promise<void>;
  navigateLocal: (path: string) => Promise<void>;
  startTransfer: (hostId: string, direction: 'down' | 'up', name: string) => Promise<void>;
  dismissToast: (hostId: string) => void;
}

export const useStore = create<NettleState>((set, get) => ({
  hosts: [],
  sets: [],
  settings: { keepConnections: true },

  sessions: {},
  focusedHostId: null,
  view: 'ports',

  local: null,
  localSel: null,

  hostKeyPrompt: null,
  hostKeyMismatch: null,
  authRequest: null,
  editHost: null,
  editSet: null,
  aboutOpen: false,

  setView: (view) => set({ view }),

  refreshHosts: async () => set({ hosts: await api.listHosts() }),
  refreshSets: async () => set({ sets: await api.listSets() }),

  connect: async (hostId) => {
    // If keepConnections is off the backend drops the others; mirror that here.
    set((s) => {
      const sessions = s.settings.keepConnections ? { ...s.sessions } : {};
      sessions[hostId] = emptySession(hostId);
      return { sessions, focusedHostId: hostId, view: s.view === 'dashboard' ? 'ports' : s.view };
    });
    try {
      await api.connect(hostId);
    } catch (e: unknown) {
      const msg = (e as { message?: string })?.message ?? String(e);
      patchSession(set, hostId, (sess) => ({ ...sess, connError: msg }));
    }
  },

  focusHost: (hostId) => set({ focusedHostId: hostId, view: get().view === 'dashboard' ? 'ports' : get().view }),

  disconnect: async (hostId) => {
    await api.disconnect(hostId);
    set((s) => {
      const sessions = { ...s.sessions };
      delete sessions[hostId];
      const remaining = Object.keys(sessions);
      const focusedHostId =
        s.focusedHostId === hostId ? (remaining[0] ?? null) : s.focusedHostId;
      return { sessions, focusedHostId };
    });
  },

  navigateRemote: async (hostId, path) => {
    const listing = await api.sftpList(hostId, path);
    patchSession(set, hostId, (sess) => ({ ...sess, remote: listing, remoteSel: null }));
  },

  navigateLocal: async (path) => {
    set({ local: await api.localList(path), localSel: null });
  },

  startTransfer: async (hostId, direction, name) => {
    const sess = get().sessions[hostId];
    const local = get().local;
    if (!sess?.remote || !local) return;
    const remotePath = joinRemote(sess.remote.path, name);
    const localPath = joinLocal(local.path, name);
    const onProgress = new Channel<TransferProgress>();
    onProgress.onmessage = (p) => {
      patchSession(set, hostId, (s) => {
        const row = s.transfers[p.id];
        if (!row) return s;
        return {
          ...s,
          transfers: {
            ...s.transfers,
            [p.id]: { ...row, bytes: p.bytes, total: p.total ?? row.total, rate: p.bytesPerSec },
          },
        };
      });
    };
    await api.transferStart(hostId, direction, remotePath, localPath, onProgress);
    const refreshTarget = direction === 'up' ? sess.remote.path : local.path;
    setTimeout(() => {
      if (direction === 'up') get().navigateRemote(hostId, refreshTarget).catch(() => {});
      else get().navigateLocal(refreshTarget).catch(() => {});
    }, 600);
  },

  dismissToast: (hostId) => patchSession(set, hostId, (s) => ({ ...s, toast: null })),
}));

type SetFn = (partial: Partial<NettleState> | ((s: NettleState) => Partial<NettleState>)) => void;

function patchSession(
  set: SetFn,
  hostId: string,
  fn: (s: SessionState) => SessionState,
) {
  set((state) => {
    const existing = state.sessions[hostId];
    if (!existing) return {};
    return { sessions: { ...state.sessions, [hostId]: fn(existing) } };
  });
}

export function joinRemote(dir: string, name: string): string {
  return dir.endsWith('/') ? dir + name : `${dir}/${name}`;
}

export function joinLocal(dir: string, name: string): string {
  const sep = dir.includes('\\') ? '\\' : '/';
  return dir.endsWith(sep) ? dir + name : dir + sep + name;
}

export function useFocusedSession(): SessionState | null {
  return useStore((s) => (s.focusedHostId ? (s.sessions[s.focusedHostId] ?? null) : null));
}

// ---------- global event wiring (call once at startup) ----------

let wired = false;

export async function initStore() {
  if (wired) return;
  wired = true;
  const set = useStore.setState;
  const get = useStore.getState;

  await Promise.all([
    listen<ConnState>('connection-state', (e) => {
      const conn = e.payload;
      set((s) => applyConnState(s, conn));
      if (conn.state === 'connected') {
        const cur = get().sessions[conn.hostId];
        get()
          .navigateRemote(conn.hostId, cur?.remote?.path ?? '~')
          .catch(() => {});
      }
    }),

    listen<PortsChanged>('ports-changed', (e) => set((s) => applyPorts(s, e.payload))),

    listen<ForwardsChanged>('forwards-changed', (e) => set((s) => applyForwards(s, e.payload))),

    listen<TransferMeta>('transfer-updated', (e) => set((s) => applyTransfer(s, e.payload))),

    listen<HostKeyPrompt>('host-key-prompt', (e) => set({ hostKeyPrompt: e.payload })),
    listen<HostKeyPrompt>('host-key-mismatch', (e) => set({ hostKeyMismatch: e.payload })),
    listen<AuthRequest>('auth-request', (e) => set({ authRequest: e.payload })),
    listen<{ hostId: string }>('term-closed', (e) =>
      patchSession(set, e.payload.hostId, (s) => ({ ...s, termClosed: true })),
    ),
    listen('open-about', () => set({ aboutOpen: true })),
  ]);

  // hydrate
  const [hosts, sets, settings, sessions] = await Promise.all([
    api.listHosts(),
    api.listSets(),
    api.getSettings(),
    api.listSessions(),
  ]);
  const sessionMap: Record<string, SessionState> = {};
  for (const info of sessions) {
    sessionMap[info.hostId] = { ...emptySession(info.hostId), conn: info.conn };
  }
  set({
    hosts,
    sets,
    settings,
    sessions: sessionMap,
    focusedHostId: Object.keys(sessionMap)[0] ?? null,
  });
  for (const hostId of Object.keys(sessionMap)) {
    get().navigateRemote(hostId, '~').catch(() => {});
  }
  try {
    set({ local: await api.localList(await api.localHomeDir()) });
  } catch {
    // ignore
  }
}
