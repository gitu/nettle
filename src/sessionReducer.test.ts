import { describe, expect, it } from 'vitest';
import {
  applyConnState,
  applyForwards,
  applyPorts,
  applyTransfer,
  emptySession,
  type SessionSlice,
} from './sessionReducer';
import type { ConnState, PortsChanged, RemotePort, TransferMeta } from './ipc';

const A = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa';
const B = 'bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb';

function empty(): SessionSlice {
  return { sessions: {}, focusedHostId: null };
}

function withSession(hostId: string, conn: ConnState): SessionSlice {
  return { sessions: { [hostId]: { ...emptySession(hostId), conn } }, focusedHostId: hostId };
}

describe('applyConnState — the connect lifecycle', () => {
  it('upserts a session for a connecting host that was never optimistically created', () => {
    // This is the regression: a `connecting` event arriving with no prior
    // session entry must CREATE the session, not be dropped.
    const next = applyConnState(empty(), { state: 'connecting', hostId: A });
    expect(next.sessions[A]).toBeDefined();
    expect(next.sessions[A].conn.state).toBe('connecting');
    expect(next.focusedHostId).toBe(A);
  });

  it('drives a full connecting -> authenticating -> connected sequence without dropping the session', () => {
    let s = empty();
    for (const conn of [
      { state: 'connecting', hostId: A } as ConnState,
      { state: 'authenticating', hostId: A } as ConnState,
      { state: 'connected', hostId: A, ip: '10.0.0.5', sinceMs: 1, epoch: 1 } as ConnState,
    ]) {
      s = applyConnState(s, conn);
      expect(s.sessions[A]).toBeDefined();
    }
    expect(s.sessions[A].conn.state).toBe('connected');
    expect(s.sessions[A].connError).toBeNull();
  });

  it('survives a stale disconnect racing a fresh connect (the exact reconnect bug)', () => {
    // Optimistic connecting entry exists...
    let s = withSession(A, { state: 'connecting', hostId: A });
    // ...a stale `disconnected` from a torn-down actor arrives and removes it...
    s = applyConnState(s, { state: 'disconnected', hostId: A });
    expect(s.sessions[A]).toBeUndefined();
    // ...but the new session's `connected` event re-creates it rather than vanishing.
    s = applyConnState(s, { state: 'connected', hostId: A, ip: '10.0.0.5', sinceMs: 2, epoch: 2 });
    expect(s.sessions[A]).toBeDefined();
    expect(s.sessions[A].conn.state).toBe('connected');
  });

  it('records the error on failed and clears it again on a later connected', () => {
    let s = applyConnState(empty(), { state: 'failed', hostId: A, error: 'auth denied' });
    expect(s.sessions[A].connError).toBe('auth denied');
    s = applyConnState(s, { state: 'connected', hostId: A, ip: '1.2.3.4', sinceMs: 1, epoch: 1 });
    expect(s.sessions[A].connError).toBeNull();
  });

  it('preserves connError across a reconnecting event', () => {
    let s = applyConnState(empty(), { state: 'failed', hostId: A, error: 'boom' });
    s = applyConnState(s, { state: 'reconnecting', hostId: A, attempt: 1, nextRetryMs: 1000 });
    expect(s.sessions[A].connError).toBe('boom');
  });

  it('removes the session on disconnect and re-focuses a survivor', () => {
    let s = withSession(A, { state: 'connected', hostId: A, ip: '1.1.1.1', sinceMs: 1, epoch: 1 });
    s = applyConnState(s, { state: 'connecting', hostId: B });
    expect(Object.keys(s.sessions)).toHaveLength(2);
    expect(s.focusedHostId).toBe(A);

    s = applyConnState(s, { state: 'disconnected', hostId: A });
    expect(s.sessions[A]).toBeUndefined();
    expect(s.sessions[B]).toBeDefined();
    expect(s.focusedHostId).toBe(B); // re-focused the survivor
  });

  it('clears focus when the last session disconnects', () => {
    let s = withSession(A, { state: 'connected', hostId: A, ip: '1.1.1.1', sinceMs: 1, epoch: 1 });
    s = applyConnState(s, { state: 'disconnected', hostId: A });
    expect(s.focusedHostId).toBeNull();
    expect(Object.keys(s.sessions)).toHaveLength(0);
  });

  it('leaves focus untouched when a non-focused host disconnects', () => {
    let s = withSession(A, { state: 'connected', hostId: A, ip: '1.1.1.1', sinceMs: 1, epoch: 1 });
    s = applyConnState(s, { state: 'connected', hostId: B, ip: '2.2.2.2', sinceMs: 1, epoch: 1 });
    // A is focused; disconnect B
    s = applyConnState(s, { state: 'disconnected', hostId: B });
    expect(s.focusedHostId).toBe(A);
  });

  it('is a no-op when disconnecting a host with no session', () => {
    const s = empty();
    expect(applyConnState(s, { state: 'disconnected', hostId: A })).toBe(s);
  });

  it('keeps two sessions fully independent', () => {
    let s = applyConnState(empty(), { state: 'connecting', hostId: A });
    s = applyConnState(s, { state: 'connected', hostId: A, ip: '1.1.1.1', sinceMs: 1, epoch: 1 });
    s = applyConnState(s, { state: 'connecting', hostId: B });
    s = applyConnState(s, { state: 'failed', hostId: B, error: 'nope' });

    expect(s.sessions[A].conn.state).toBe('connected');
    expect(s.sessions[A].connError).toBeNull();
    expect(s.sessions[B].conn.state).toBe('failed');
    expect(s.sessions[B].connError).toBe('nope');
  });
});

describe('applyPorts', () => {
  const port = (n: number): RemotePort => ({ port: n, bind: '0.0.0.0', process: 'node', pid: 1 });
  const change = (over: Partial<PortsChanged>): PortsChanged => ({
    hostId: A,
    all: [],
    added: [],
    removed: [],
    isBaseline: false,
    unsupported: false,
    ...over,
  });

  it('ignores events for an unknown host', () => {
    const s = empty();
    expect(applyPorts(s, change({ all: [port(8080)] }))).toBe(s);
  });

  it('stores the port list without a toast on the baseline scan', () => {
    const s = withSession(A, { state: 'connected', hostId: A, ip: '1.1.1.1', sinceMs: 1, epoch: 1 });
    const next = applyPorts(s, change({ all: [port(22)], added: [port(22)], isBaseline: true }));
    expect(next.sessions[A].ports).toHaveLength(1);
    expect(next.sessions[A].toast).toBeNull();
  });

  it('raises a toast for a genuinely new port after the baseline', () => {
    const s = withSession(A, { state: 'connected', hostId: A, ip: '1.1.1.1', sinceMs: 1, epoch: 1 });
    const next = applyPorts(s, change({ all: [port(8000)], added: [port(8000)] }));
    expect(next.sessions[A].toast).toEqual({ port: 8000, process: 'node' });
  });

  it('propagates the unsupported flag', () => {
    const s = withSession(A, { state: 'connected', hostId: A, ip: '1.1.1.1', sinceMs: 1, epoch: 1 });
    const next = applyPorts(s, change({ unsupported: true }));
    expect(next.sessions[A].portsUnsupported).toBe(true);
  });
});

describe('applyForwards', () => {
  it('replaces the forwards list for the host', () => {
    const s = withSession(A, { state: 'connected', hostId: A, ip: '1.1.1.1', sinceMs: 1, epoch: 1 });
    const next = applyForwards(s, {
      hostId: A,
      forwards: [{ port: 8080, pinned: true, live: true }],
    });
    expect(next.sessions[A].forwards).toHaveLength(1);
    expect(next.sessions[A].forwards[0].pinned).toBe(true);
  });

  it('ignores forwards for an unknown host', () => {
    const s = empty();
    expect(applyForwards(s, { hostId: A, forwards: [] })).toBe(s);
  });
});

describe('applyTransfer', () => {
  const meta = (over: Partial<TransferMeta>): TransferMeta => ({
    id: 't1',
    hostId: A,
    name: 'file.bin',
    direction: 'down',
    status: 'running',
    total: 100,
    bytes: 0,
    error: null,
    ...over,
  });

  it('merges transfer metadata by id', () => {
    let s = withSession(A, { state: 'connected', hostId: A, ip: '1.1.1.1', sinceMs: 1, epoch: 1 });
    s = applyTransfer(s, meta({ bytes: 10 }));
    s = applyTransfer(s, meta({ bytes: 100, status: 'done' }));
    expect(s.sessions[A].transfers.t1.bytes).toBe(100);
    expect(s.sessions[A].transfers.t1.status).toBe('done');
  });

  it('ignores transfers for an unknown host', () => {
    const s = empty();
    expect(applyTransfer(s, meta({}))).toBe(s);
  });
});
