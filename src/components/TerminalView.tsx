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

export function TerminalView() {
  const ref = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const connected = useStore((s) => s.conn.state === 'connected' || s.conn.state === 'reconnecting');
  const termGeneration = useStore((s) => s.termGeneration);
  const termClosed = useStore((s) => s.termClosed);

  useEffect(() => {
    if (!ref.current || !connected) return;

    const term = new Terminal({
      // Symbols Nerd Font Mono fills in powerline/devicon glyphs (PUA) that
      // Plex Mono lacks; common user-installed Nerd Fonts come first so a
      // full NF setup wins if present.
      fontFamily:
        "'IBM Plex Mono', 'MesloLGS NF', 'Symbols Nerd Font Mono', Menlo, monospace",
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
    term.open(ref.current);
    fit.fit();
    // Re-measure once web fonts are in, so the cell grid uses Plex Mono
    // metrics rather than the fallback font it may have measured first.
    document.fonts?.ready.then(() => {
      try {
        fit.fit();
      } catch {
        // terminal already disposed
      }
    });
    termRef.current = term;

    const encoder = new TextEncoder();
    const onData = new Channel<TermData>();
    onData.onmessage = (msg) => {
      if (msg instanceof ArrayBuffer) {
        term.write(new Uint8Array(msg));
      } else if (msg instanceof Uint8Array) {
        term.write(msg);
      } else if (typeof msg === 'string') {
        term.write(msg);
      } else if (Array.isArray(msg)) {
        term.write(new Uint8Array(msg as number[]));
      }
    };

    api.termOpen(term.cols, term.rows, onData).catch(() => {});
    useStore.setState({ termClosed: false });

    const dataDisp = term.onData((s) => {
      api.termWrite(Array.from(encoder.encode(s))).catch(() => {});
    });
    const resizeDisp = term.onResize(({ cols, rows }) => {
      api.termResize(cols, rows).catch(() => {});
    });

    const observer = new ResizeObserver(() => {
      try {
        fit.fit();
      } catch {
        // ignore fit races during teardown
      }
    });
    observer.observe(ref.current);
    term.focus();

    return () => {
      observer.disconnect();
      dataDisp.dispose();
      resizeDisp.dispose();
      api.termClose().catch(() => {});
      term.dispose();
      termRef.current = null;
    };
  }, [connected, termGeneration]);

  return (
    <div className="view">
      <div className="term-wrap" onClick={() => termRef.current?.focus()}>
        {!connected && (
          <div className="term-closed">
            <span>Not connected — pick a host to open a shell.</span>
          </div>
        )}
        {connected && termClosed && (
          <div className="term-closed">
            <span>Shell session ended.</span>
            <button
              className="btn-acc"
              onClick={() =>
                useStore.setState((s) => ({
                  termClosed: false,
                  termGeneration: s.termGeneration + 1,
                }))
              }
            >
              new shell
            </button>
          </div>
        )}
        <div ref={ref} style={{ height: '100%' }} />
      </div>
    </div>
  );
}
