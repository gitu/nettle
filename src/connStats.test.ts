import { describe, expect, it } from 'vitest';
import { liveUptimeMs, statsChips } from './components/ConnStatsStrip';
import type { ConnStats } from './ipc';
import { fmtDuration, fmtDurationShort } from './util';

const HOST = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa';

function stats(over: Partial<ConnStats> = {}): ConnStats {
  return {
    hostId: HOST,
    sinceMs: 0,
    connected: false,
    connects: 0,
    reconnects: 0,
    linkDrops: 0,
    connectFailures: 0,
    connectedSinceMs: null,
    priorUptimeMs: 0,
    lastIp: null,
    lastDropAtMs: null,
    lastDropReason: null,
    lastError: null,
    tunnelConnsTotal: 0,
    tunnelConnsActive: 0,
    tunnelRefused: 0,
    tunnelWaitTimeouts: 0,
    tunnelBytesUp: 0,
    tunnelBytesDown: 0,
    scans: 0,
    scanFailures: 0,
    lastScanMs: 0,
    ...over,
  };
}

const chip = (chips: ReturnType<typeof statsChips>, label: string) =>
  chips.find((c) => c.label === label)!;

describe('fmtDuration / fmtDurationShort', () => {
  it('formats hh:mm:ss and clamps negatives to zero', () => {
    expect(fmtDuration(0)).toBe('00:00:00');
    expect(fmtDuration(-5000)).toBe('00:00:00');
    expect(fmtDuration(3_723_000)).toBe('01:02:03');
    // hours keep growing rather than wrapping at 24
    expect(fmtDuration(100 * 3600_000)).toBe('100:00:00');
  });

  it('picks the coarsest useful unit for prose', () => {
    expect(fmtDurationShort(12_000)).toBe('12s');
    expect(fmtDurationShort(4 * 60_000)).toBe('4m');
    expect(fmtDurationShort(2 * 3600_000 + 5 * 60_000)).toBe('2h 05m');
    expect(fmtDurationShort(3 * 86400_000 + 4 * 3600_000)).toBe('3d 04h');
  });
});

describe('liveUptimeMs', () => {
  it('is zero while disconnected and counts from connectedSinceMs otherwise', () => {
    expect(liveUptimeMs(stats(), 10_000)).toBe(0);
    expect(liveUptimeMs(stats({ connected: true, connectedSinceMs: 4_000 }), 10_000)).toBe(6_000);
    // a clock skew can't produce a negative uptime
    expect(liveUptimeMs(stats({ connected: true, connectedSinceMs: 20_000 }), 10_000)).toBe(0);
  });
});

describe('statsChips', () => {
  it('shows a live link and adds it to the prior uptime', () => {
    const chips = statsChips(
      stats({ connected: true, connectedSinceMs: 0, priorUptimeMs: 60_000, connects: 1 }),
      30_000,
    );
    expect(chip(chips, 'link').value).toBe('00:00:30');
    expect(chip(chips, 'total').value).toBe('1m');
    expect(chip(chips, 'drops').warn).toBe(false);
  });

  it('marks the link as down and keeps the folded uptime when disconnected', () => {
    const chips = statsChips(stats({ priorUptimeMs: 90_000 }), 999_999);
    expect(chip(chips, 'link').value).toBe('down');
    expect(chip(chips, 'total').value).toBe('1m');
  });

  it('flags drops, refused tunnels and scan failures', () => {
    const chips = statsChips(
      stats({
        linkDrops: 2,
        reconnects: 2,
        tunnelRefused: 1,
        tunnelConnsActive: 3,
        tunnelConnsTotal: 17,
        tunnelBytesUp: 1536,
        tunnelBytesDown: 5 * 1024 * 1024,
        scans: 40,
        scanFailures: 1,
        lastScanMs: 120,
      }),
      0,
    );
    expect(chip(chips, 'drops')).toMatchObject({ value: '2', warn: true });
    expect(chip(chips, 'conns')).toMatchObject({ value: '3 / 17', warn: true });
    expect(chip(chips, '↑').value).toBe('1.5 KB');
    expect(chip(chips, '↓').value).toBe('5.0 MB');
    expect(chip(chips, 'scan')).toMatchObject({ value: '120 ms', warn: true });
  });

  it('shows a placeholder before the first scan', () => {
    expect(chip(statsChips(stats(), 0), 'scan').value).toBe('—');
  });
});
