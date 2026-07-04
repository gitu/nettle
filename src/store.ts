import { create } from 'zustand';
import { listen } from '@tauri-apps/api/event';
import {
  api,
  Channel,
  type AuthRequest,
  type ConnState,
  type DirListing,
  type ForwardInfo,
  type HostConfig,
  type HostKeyPrompt,
  type PortsChanged,
  type RemotePort,
  type TransferMeta,
  type TransferProgress,
} from './ipc';

export type View = 'files' | 'ports' | 'terminal';

export interface TransferRow extends TransferMeta {
  rate?: number;
}

interface NettleState {
  hosts: HostConfig[];
  conn: ConnState;
  activeHostId: string | null;
  view: View;

  ports: RemotePort[];
  portsUnsupported: boolean;
  forwards: ForwardInfo[];
  transfers: Record<string, TransferRow>;
  toast: { port: number; process: string | null } | null;

  hostKeyPrompt: HostKeyPrompt | null;
  hostKeyMismatch: HostKeyPrompt | null;
  authRequest: AuthRequest | null;
  editHost: HostConfig | 'new' | null;
  connError: string | null;

  remote: DirListing | null;
  local: DirListing | null;
  remoteSel: string | null;
  localSel: string | null;
  termGeneration: number;
  termClosed: boolean;

  setView: (view: View) => void;
  refreshHosts: () => Promise<void>;
  connect: (hostId: string) => Promise<void>;
  disconnect: () => Promise<void>;
  navigateRemote: (path: string) => Promise<void>;
  navigateLocal: (path: string) => Promise<void>;
  startTransfer: (direction: 'down' | 'up', name: string) => Promise<void>;
  dismissToast: () => void;
}

export const useStore = create<NettleState>((set, get) => ({
  hosts: [],
  conn: { state: 'disconnected' },
  activeHostId: null,
  view: 'ports',

  ports: [],
  portsUnsupported: false,
  forwards: [],
  transfers: {},
  toast: null,

  hostKeyPrompt: null,
  hostKeyMismatch: null,
  authRequest: null,
  editHost: null,
  connError: null,

  remote: null,
  local: null,
  remoteSel: null,
  localSel: null,
  termGeneration: 0,
  termClosed: false,

  setView: (view) => set({ view }),

  refreshHosts: async () => {
    set({ hosts: await api.listHosts() });
  },

  connect: async (hostId) => {
    set({
      activeHostId: hostId,
      connError: null,
      ports: [],
      forwards: [],
      transfers: {},
      remote: null,
      remoteSel: null,
      toast: null,
      termClosed: false,
      termGeneration: get().termGeneration + 1,
    });
    try {
      await api.connect(hostId);
    } catch (e: unknown) {
      const msg = (e as { message?: string })?.message ?? String(e);
      set({ connError: msg });
    }
  },

  disconnect: async () => {
    await api.disconnect();
    set({
      activeHostId: null,
      ports: [],
      forwards: [],
      remote: null,
      toast: null,
    });
  },

  navigateRemote: async (path) => {
    const listing = await api.sftpList(path);
    set({ remote: listing, remoteSel: null });
  },

  navigateLocal: async (path) => {
    const listing = await api.localList(path);
    set({ local: listing, localSel: null });
  },

  startTransfer: async (direction, name) => {
    const { remote, local } = get();
    if (!remote || !local) return;
    const remotePath = joinRemote(remote.path, name);
    const localPath = joinLocal(local.path, name);
    const onProgress = new Channel<TransferProgress>();
    onProgress.onmessage = (p) => {
      set((s) => {
        const row = s.transfers[p.id];
        if (!row) return s;
        return {
          transfers: {
            ...s.transfers,
            [p.id]: { ...row, bytes: p.bytes, total: p.total ?? row.total, rate: p.bytesPerSec },
          },
        };
      });
    };
    await api.transferStart(direction, remotePath, localPath, onProgress);
    if (direction === 'up') {
      // refresh remote listing shortly after upload begins finishing
      setTimeout(() => {
        const r = get().remote;
        if (r) get().navigateRemote(r.path).catch(() => {});
      }, 600);
    } else {
      setTimeout(() => {
        const l = get().local;
        if (l) get().navigateLocal(l.path).catch(() => {});
      }, 600);
    }
  },

  dismissToast: () => set({ toast: null }),
}));

export function joinRemote(dir: string, name: string): string {
  return dir.endsWith('/') ? dir + name : `${dir}/${name}`;
}

export function joinLocal(dir: string, name: string): string {
  const sep = dir.includes('\\') ? '\\' : '/';
  return dir.endsWith(sep) ? dir + name : dir + sep + name;
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
      set({ conn });
      if (conn.state === 'failed') {
        set({ connError: conn.error, activeHostId: null });
      }
      if (conn.state === 'disconnected') {
        set({ activeHostId: null });
      }
      if (conn.state === 'connected') {
        set({ connError: null, activeHostId: conn.hostId });
        // (re)load the remote pane on each new epoch
        get()
          .navigateRemote(get().remote?.path ?? '~')
          .catch(() => {});
      }
    }),

    listen<PortsChanged>('ports-changed', (e) => {
      const p = e.payload;
      set({ ports: p.all, portsUnsupported: p.unsupported });
      if (!p.isBaseline && p.added.length > 0) {
        const candidate = p.added[0];
        set({ toast: { port: candidate.port, process: candidate.process } });
      }
    }),

    listen<ForwardInfo[]>('forwards-changed', (e) => {
      set({ forwards: e.payload });
    }),

    listen<TransferMeta>('transfer-updated', (e) => {
      const meta = e.payload;
      set((s) => ({
        transfers: {
          ...s.transfers,
          [meta.id]: { ...s.transfers[meta.id], ...meta },
        },
      }));
    }),

    listen<HostKeyPrompt>('host-key-prompt', (e) => set({ hostKeyPrompt: e.payload })),
    listen<HostKeyPrompt>('host-key-mismatch', (e) => set({ hostKeyMismatch: e.payload })),
    listen<AuthRequest>('auth-request', (e) => set({ authRequest: e.payload })),
    listen('term-closed', () => set({ termClosed: true })),
  ]);

  // hydrate
  set({ hosts: await api.listHosts(), conn: await api.getConnectionState() });
  try {
    const home = await api.localHomeDir();
    set({ local: await api.localList(home) });
  } catch {
    // ignore
  }
}
