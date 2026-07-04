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
  | { state: 'disconnected' }
  | { state: 'connecting'; hostId: string }
  | { state: 'authenticating'; hostId: string }
  | { state: 'connected'; hostId: string; ip: string; sinceMs: number; epoch: number }
  | { state: 'reconnecting'; hostId: string; attempt: number; nextRetryMs: number | null }
  | { state: 'failed'; hostId: string; error: string };

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
  all: RemotePort[];
  added: RemotePort[];
  removed: number[];
  isBaseline: boolean;
  unsupported: boolean;
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
  disconnect: () => invoke<void>('disconnect'),
  getConnectionState: () => invoke<ConnState>('get_connection_state'),
  hostKeyDecision: (accept: boolean) => invoke<void>('host_key_decision', { accept }),
  provideSecret: (secret: string | null) => invoke<void>('provide_secret', { secret }),

  termOpen: (cols: number, rows: number, onData: Channel<TermData>) =>
    invoke<void>('term_open', { cols, rows, onData }),
  termWrite: (data: number[]) => invoke<void>('term_write', { data }),
  termResize: (cols: number, rows: number) => invoke<void>('term_resize', { cols, rows }),
  termClose: () => invoke<void>('term_close'),

  sftpList: (path: string) => invoke<DirListing>('sftp_list', { path }),
  sftpHome: () => invoke<string>('sftp_home'),
  localList: (path: string) => invoke<DirListing>('local_list', { path }),
  localHomeDir: () => invoke<string>('local_home_dir'),

  transferStart: (
    direction: TransferDirection,
    remotePath: string,
    localPath: string,
    onProgress: Channel<TransferProgress>,
  ) => invoke<string>('transfer_start', { direction, remotePath, localPath, onProgress }),
  transferCancel: (id: string) => invoke<void>('transfer_cancel', { id }),
  transferList: () => invoke<TransferMeta[]>('transfer_list'),
  transferClearFinished: () => invoke<void>('transfer_clear_finished'),

  forwardSet: (port: number, enabled: boolean, pinned: boolean) =>
    invoke<void>('forward_set', { port, enabled, pinned }),
  forwardList: () => invoke<ForwardInfo[]>('forward_list'),
  portIgnore: (port: number) => invoke<void>('port_ignore', { port }),

  windowControl: (action: 'close' | 'minimize' | 'maximize') =>
    invoke<void>('window_control', { action }),
};

export { Channel };
