import { invoke, Channel } from '@tauri-apps/api/core';

/** Payload of the raw terminal byte stream (Rust sends InvokeResponseBody::Raw). */
export type TermData = ArrayBuffer | Uint8Array | string | number[];

// ---------- types mirroring src-tauri/src/ipc/types.rs ----------

export interface HostConfig {
  id: string;
  name: string;
  hostname: string;
  port: number;
  username: string;
  keyPath?: string | null;
}

export type ConnState =
  | { state: 'disconnected'; hostId: string }
  | { state: 'connecting'; hostId: string }
  | { state: 'authenticating'; hostId: string }
  | { state: 'connected'; hostId: string; ip: string; sinceMs: number; epoch: number }
  | { state: 'reconnecting'; hostId: string; attempt: number; nextRetryMs: number | null }
  | { state: 'failed'; hostId: string; error: string };

export interface SessionInfo {
  hostId: string;
  conn: ConnState;
}

export interface Settings {
  keepConnections: boolean;
}

export interface ConnectionSet {
  id: string;
  name: string;
  hostIds: string[];
}

export interface HostForward {
  hostId: string;
  hostName: string;
  forward: ForwardInfo;
}

export interface FileEntry {
  name: string;
  kind: 'dir' | 'file' | 'link';
  size: number | null;
  mtime: number | null;
}

export interface DirListing {
  path: string;
  entries: FileEntry[];
}

export interface RemotePort {
  port: number;
  bind: string;
  process: string | null;
  pid: number | null;
}

export interface PortsChanged {
  hostId: string;
  all: RemotePort[];
  added: RemotePort[];
  removed: number[];
  isBaseline: boolean;
  unsupported: boolean;
}

export interface ForwardsChanged {
  hostId: string;
  forwards: ForwardInfo[];
}

export interface ForwardInfo {
  port: number;
  pinned: boolean;
  live: boolean;
}

export type TransferDirection = 'down' | 'up';
export type TransferStatus = 'queued' | 'running' | 'done' | 'failed' | 'cancelled';

export interface TransferMeta {
  id: string;
  hostId: string;
  name: string;
  direction: TransferDirection;
  status: TransferStatus;
  total: number | null;
  bytes: number;
  error: string | null;
}

export interface TransferProgress {
  id: string;
  bytes: number;
  total: number | null;
  bytesPerSec: number;
}

export interface HostKeyPrompt {
  host: string;
  port: number;
  keyType: string;
  fingerprint: string;
}

export interface AuthRequest {
  kind: 'password' | 'keyPassphrase';
  username: string;
  host: string;
}

export interface IpcError {
  code: string;
  message: string;
}

// ---------- commands ----------

export const api = {
  listHosts: () => invoke<HostConfig[]>('list_hosts'),
  saveHost: (host: HostConfig) => invoke<HostConfig>('save_host', { host }),
  deleteHost: (hostId: string) => invoke<void>('delete_host', { hostId }),

  connect: (hostId: string) => invoke<void>('connect', { hostId }),
  disconnect: (hostId: string) => invoke<void>('disconnect', { hostId }),
  disconnectAll: () => invoke<void>('disconnect_all'),
  listSessions: () => invoke<SessionInfo[]>('list_sessions'),
  hostKeyDecision: (accept: boolean) => invoke<void>('host_key_decision', { accept }),
  provideSecret: (secret: string | null) => invoke<void>('provide_secret', { secret }),

  getSettings: () => invoke<Settings>('get_settings'),
  setSettings: (settings: Settings) => invoke<void>('set_settings', { settings }),

  listSets: () => invoke<ConnectionSet[]>('list_sets'),
  saveSet: (set: ConnectionSet) => invoke<ConnectionSet>('save_set', { set }),
  deleteSet: (setId: string) => invoke<void>('delete_set', { setId }),
  connectSet: (setId: string) => invoke<void>('connect_set', { setId }),

  termOpen: (hostId: string, cols: number, rows: number, onData: Channel<TermData>) =>
    invoke<void>('term_open', { hostId, cols, rows, onData }),
  termWrite: (hostId: string, data: number[]) => invoke<void>('term_write', { hostId, data }),
  termResize: (hostId: string, cols: number, rows: number) =>
    invoke<void>('term_resize', { hostId, cols, rows }),
  termClose: (hostId: string) => invoke<void>('term_close', { hostId }),

  sftpList: (hostId: string, path: string) => invoke<DirListing>('sftp_list', { hostId, path }),
  sftpHome: (hostId: string) => invoke<string>('sftp_home', { hostId }),
  localList: (path: string) => invoke<DirListing>('local_list', { path }),
  localHomeDir: () => invoke<string>('local_home_dir'),

  transferStart: (
    hostId: string,
    direction: TransferDirection,
    remotePath: string,
    localPath: string,
    onProgress: Channel<TransferProgress>,
  ) => invoke<string>('transfer_start', { hostId, direction, remotePath, localPath, onProgress }),
  transferCancel: (hostId: string, id: string) =>
    invoke<void>('transfer_cancel', { hostId, id }),
  transferList: (hostId: string) => invoke<TransferMeta[]>('transfer_list', { hostId }),
  transferClearFinished: (hostId: string) =>
    invoke<void>('transfer_clear_finished', { hostId }),

  forwardSet: (hostId: string, port: number, enabled: boolean, pinned: boolean) =>
    invoke<void>('forward_set', { hostId, port, enabled, pinned }),
  forwardList: (hostId: string) => invoke<ForwardInfo[]>('forward_list', { hostId }),
  allForwards: () => invoke<HostForward[]>('all_forwards'),
  portIgnore: (hostId: string, port: number) => invoke<void>('port_ignore', { hostId, port }),

  windowControl: (action: 'close' | 'minimize' | 'maximize') =>
    invoke<void>('window_control', { action }),
};

export { Channel };
