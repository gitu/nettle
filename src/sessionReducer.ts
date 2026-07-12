import type {
  ConnState,
  DirListing,
  ForwardInfo,
  ForwardsChanged,
  PortsChanged,
  RemotePort,
  TransferMeta,
} from './ipc';

export interface TransferRow extends TransferMeta {
  rate?: number;
}

/** All state belonging to one host's live (or reconnecting) session. */
export interface SessionState {
  hostId: string;
  conn: ConnState;
  connError: string | null;
  ports: RemotePort[];
  portsUnsupported: boolean;
  forwards: ForwardInfo[];
  transfers: Record<string, TransferRow>;
  remote: DirListing | null;
  remoteSel: string | null;
  termClosed: boolean;
  termGeneration: number;
  toast: { port: number; process: string | null } | null;
}

/** The slice of store state the reducers fold events into. */
export interface SessionSlice {
  sessions: Record<string, SessionState>;
  focusedHostId: string | null;
}

export function emptySession(hostId: string): SessionState {
  return {
    hostId,
    conn: { state: 'connecting', hostId },
    connError: null,
    ports: [],
    portsUnsupported: false,
    forwards: [],
    transfers: {},
    remote: null,
    remoteSel: null,
    termClosed: false,
    termGeneration: 0,
    toast: null,
  };
}

/**
 * Fold a connection-state event into the session slice.
 *
 * - `disconnected` removes the session and re-focuses a survivor.
 * - any other state upserts the session (creating it if missing) so a
 *   `connecting`/`connected` event can never be dropped just because the
 *   optimistic entry isn't there yet. Dropping it is exactly the
 *   "connecting doesn't work anymore" regression this guards against.
 */
export function applyConnState(slice: SessionSlice, conn: ConnState): SessionSlice {
  const hostId = conn.hostId;

  if (conn.state === 'disconnected') {
    if (!(hostId in slice.sessions)) return slice;
    const sessions = { ...slice.sessions };
    delete sessions[hostId];
    const remaining = Object.keys(sessions);
    const focusedHostId =
      slice.focusedHostId === hostId ? (remaining[0] ?? null) : slice.focusedHostId;
    return { sessions, focusedHostId };
  }

  const prev = slice.sessions[hostId] ?? emptySession(hostId);
  const connError =
    conn.state === 'failed' ? conn.error : conn.state === 'connected' ? null : prev.connError;
  return {
    sessions: { ...slice.sessions, [hostId]: { ...prev, conn, connError } },
    focusedHostId: slice.focusedHostId ?? hostId,
  };
}

/** Fold a ports-changed event (raises a toast for genuinely new ports). */
export function applyPorts(slice: SessionSlice, p: PortsChanged): SessionSlice {
  const sess = slice.sessions[p.hostId];
  if (!sess) return slice;
  let toast = sess.toast;
  if (!p.isBaseline && p.added.length > 0) {
    toast = { port: p.added[0].port, process: p.added[0].process };
  }
  return {
    ...slice,
    sessions: {
      ...slice.sessions,
      [p.hostId]: { ...sess, ports: p.all, portsUnsupported: p.unsupported, toast },
    },
  };
}

/** Fold a forwards-changed event. */
export function applyForwards(slice: SessionSlice, f: ForwardsChanged): SessionSlice {
  const sess = slice.sessions[f.hostId];
  if (!sess) return slice;
  return {
    ...slice,
    sessions: { ...slice.sessions, [f.hostId]: { ...sess, forwards: f.forwards } },
  };
}

/** Fold a transfer-updated (queue-level) event. */
export function applyTransfer(slice: SessionSlice, meta: TransferMeta): SessionSlice {
  const sess = slice.sessions[meta.hostId];
  if (!sess) return slice;
  return {
    ...slice,
    sessions: {
      ...slice.sessions,
      [meta.hostId]: {
        ...sess,
        transfers: { ...sess.transfers, [meta.id]: { ...sess.transfers[meta.id], ...meta } },
      },
    },
  };
}
