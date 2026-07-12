import { useEffect, useRef } from 'react';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { Unicode11Addon } from '@xterm/addon-unicode11';
import '@xterm/xterm/css/xterm.css';
import { api, Channel, type TermData } from '../ipc';
import { useStore } from '../store';

/** Resolve a CSS colour expression (system colours, color-mix, light-dark) to a
 *  concrete rgb string, so the terminal can follow the OS theme + accent. */
function resolveColor(expr: string): string {
  const el = document.createElement('span');
  el.style.color = expr;
  el.style.display = 'none';
  document.body.appendChild(el);
  const rgb = getComputedStyle(el).color;
  el.remove();
  return rgb;
}

/** xterm theme derived entirely from system colours (background/foreground/
 *  cursor/selection) with mode-adaptive ANSI colours via light-dark(). */
function buildTheme() {
  const R = resolveColor;
  return {
    background: R('Canvas'),
    foreground: R('CanvasText'),
    cursor: R('AccentColor'),
    cursorAccent: R('Canvas'),
    selectionBackground: R('color-mix(in srgb, AccentColor 32%, transparent)'),
    black: R('light-dark(#d5d7db, #1a1d21)'),
    red: R('light-dark(#c0392b, #f87171)'),
    green: R('light-dark(#1a8a4f, #4ade80)'),
    yellow: R('light-dark(#b5791a, #fbbf24)'),
    blue: R('light-dark(#2563c9, #60a5fa)'),
    magenta: R('light-dark(#8b3fd6, #c084fc)'),
    cyan: R('light-dark(#1a8a9a, #67e8f9)'),
    white: R('light-dark(#3a3f45, #c8ccd1)'),
    brightBlack: R('light-dark(#9aa0a8, #59616b)'),
    brightRed: R('light-dark(#d0503f, #fca5a5)'),
    brightGreen: R('light-dark(#2ba05f, #86efac)'),
    brightYellow: R('light-dark(#c88a1e, #fde68a)'),
    brightBlue: R('light-dark(#3576e0, #93c5fd)'),
    brightMagenta: R('light-dark(#9d52e8, #d8b4fe)'),
    brightCyan: R('light-dark(#2ba5b8, #a5f3fc)'),
    brightWhite: R('light-dark(#1a1d21, #e7eaee)'),
  };
}

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

// Re-theme every live terminal when the OS appearance flips light/dark.
if (typeof window !== 'undefined' && window.matchMedia) {
  window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => {
    const theme = buildTheme();
    for (const inst of registry.values()) inst.term.options.theme = theme;
  });
}

function ensureTerminal(hostId: string, generation: number): TermInstance {
  const existing = registry.get(hostId);
  if (existing && existing.generation === generation) return existing;
  if (existing) existing.dispose();

  const term = new Terminal({
    fontFamily:
      "'JetBrains Mono', 'IBM Plex Mono', 'MesloLGS NF', 'Symbols Nerd Font Mono', Menlo, monospace",
    fontSize: 13,
    lineHeight: 1.25,
    cursorBlink: true,
    theme: buildTheme(),
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
