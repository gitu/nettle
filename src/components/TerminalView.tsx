import { useEffect, useRef } from 'react';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { Unicode11Addon } from '@xterm/addon-unicode11';
import '@xterm/xterm/css/xterm.css';
import { api, Channel, type TermData } from '../ipc';
import { useStore } from '../store';

const THEME = {
  background: '#08090b',
  foreground: '#c8ccd1',
  cursor: '#c084fc',
  cursorAccent: '#08090b',
  selectionBackground: 'rgba(192,132,252,0.28)',
  black: '#1a1d21',
  red: '#f87171',
  green: '#4ade80',
  yellow: '#fbbf24',
  blue: '#60a5fa',
  magenta: '#c084fc',
  cyan: '#67e8f9',
  white: '#c8ccd1',
  brightBlack: '#59616b',
  brightRed: '#fca5a5',
  brightGreen: '#86efac',
  brightYellow: '#fde68a',
  brightBlue: '#93c5fd',
  brightMagenta: '#d8b4fe',
  brightCyan: '#a5f3fc',
  brightWhite: '#e7eaee',
};

/**
 * One xterm instance per host, kept alive for the lifetime of the session so
 * switching hosts or tabs never loses scrollback or restarts the shell. The
 * instances live outside React; the mounted component just re-parents the
 * terminal element into its container.
 */
interface TermInstance {
  term: Terminal;
  fit: FitAddon;
  generation: number;
  dispose: () => void;
}

const registry = new Map<string, TermInstance>();

function ensureTerminal(hostId: string, generation: number): TermInstance {
  const existing = registry.get(hostId);
  if (existing && existing.generation === generation) return existing;
  if (existing) existing.dispose();

  const term = new Terminal({
    fontFamily: "'IBM Plex Mono', 'MesloLGS NF', 'Symbols Nerd Font Mono', Menlo, monospace",
    fontSize: 13,
    lineHeight: 1.25,
    cursorBlink: true,
    theme: THEME,
    scrollback: 8000,
    allowProposedApi: true,
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.loadAddon(new Unicode11Addon());
  term.unicode.activeVersion = '11';

  const holder = document.createElement('div');
  holder.style.height = '100%';
  term.open(holder);

  const encoder = new TextEncoder();
  const onData = new Channel<TermData>();
  onData.onmessage = (msg) => {
    if (msg instanceof ArrayBuffer) term.write(new Uint8Array(msg));
    else if (msg instanceof Uint8Array) term.write(msg);
    else if (typeof msg === 'string') term.write(msg);
    else if (Array.isArray(msg)) term.write(new Uint8Array(msg as number[]));
  };

  // Wait for the holder to be in the DOM & sized before the first fit.
  const start = () => {
    try {
      fit.fit();
    } catch {
      /* not yet laid out */
    }
    api.termOpen(hostId, term.cols || 80, term.rows || 24, onData).catch(() => {});
  };

  const dataDisp = term.onData((s) => {
    api.termWrite(hostId, Array.from(encoder.encode(s))).catch(() => {});
  });
  const resizeDisp = term.onResize(({ cols, rows }) => {
    api.termResize(hostId, cols, rows).catch(() => {});
  });

  const instance: TermInstance = {
    term,
    fit,
    generation,
    dispose: () => {
      dataDisp.dispose();
      resizeDisp.dispose();
      api.termClose(hostId).catch(() => {});
      term.dispose();
      holder.remove();
      registry.delete(hostId);
    },
  };
  (instance as unknown as { holder: HTMLElement }).holder = holder;
  registry.set(hostId, instance);
  // start after the element gets attached in the effect
  queueMicrotask(start);
  return instance;
}

export function disposeTerminal(hostId: string) {
  registry.get(hostId)?.dispose();
}

export function TerminalView() {
  const hostId = useStore((s) => s.focusedHostId);
  const session = useStore((s) => (hostId ? s.sessions[hostId] : null));
  const connected = session?.conn.state === 'connected' || session?.conn.state === 'reconnecting';
  const generation = session?.termGeneration ?? 0;
  const termClosed = session?.termClosed ?? false;
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!hostId || !connected || !containerRef.current) return;
    const instance = ensureTerminal(hostId, generation);
    const holder = (instance as unknown as { holder: HTMLElement }).holder;
    containerRef.current.appendChild(holder);
    instance.term.focus();

    const observer = new ResizeObserver(() => {
      try {
        instance.fit.fit();
      } catch {
        /* ignore */
      }
    });
    observer.observe(containerRef.current);
    // reflow after fonts load
    document.fonts?.ready.then(() => {
      try {
        instance.fit.fit();
      } catch {
        /* ignore */
      }
    });

    return () => {
      observer.disconnect();
      // detach but keep the instance alive for when we come back
      if (holder.parentElement) holder.parentElement.removeChild(holder);
    };
  }, [hostId, connected, generation]);

  const restart = () => {
    if (!hostId) return;
    disposeTerminal(hostId);
    useStore.setState((s) => {
      const sess = s.sessions[hostId];
      if (!sess) return {};
      return {
        sessions: {
          ...s.sessions,
          [hostId]: { ...sess, termClosed: false, termGeneration: sess.termGeneration + 1 },
        },
      };
    });
  };

  return (
    <div className="view">
      <div
        className="term-wrap"
        onClick={() => hostId && registry.get(hostId)?.term.focus()}
      >
        {!connected && (
          <div className="term-closed">
            <span>Not connected.</span>
          </div>
        )}
        {connected && termClosed && (
          <div className="term-closed">
            <span>Shell session ended.</span>
            <button className="btn-acc" onClick={restart}>
              new shell
            </button>
          </div>
        )}
        <div ref={containerRef} style={{ height: '100%' }} />
      </div>
    </div>
  );
}
