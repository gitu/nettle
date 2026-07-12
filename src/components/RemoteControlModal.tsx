import { useEffect, useState } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { api, type WebConfig } from '../ipc';
import { useStore } from '../store';

export function RemoteControlModal() {
  const open = useStore((s) => s.webOpen);
  const [cfg, setCfg] = useState<WebConfig | null>(null);
  const [link, setLink] = useState('');
  const [portDraft, setPortDraft] = useState('');
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!open) return;
    setErr(null);
    api
      .getWebConfig()
      .then((c) => {
        setCfg(c);
        setPortDraft(String(c.port));
        return refreshLink();
      })
      .catch((e) => setErr(msgOf(e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  if (!open) return null;
  const close = () => useStore.setState({ webOpen: false });

  const refreshLink = () => api.webLink().then(setLink).catch(() => {});

  const save = async (next: WebConfig) => {
    setBusy(true);
    setErr(null);
    try {
      const saved = await api.setWebConfig(next);
      setCfg(saved);
      setPortDraft(String(saved.port));
      await refreshLink();
    } catch (e) {
      setErr(msgOf(e));
      // reload the truth after a failed apply (e.g. port in use)
      api.getWebConfig().then(setCfg).catch(() => {});
    } finally {
      setBusy(false);
    }
  };

  const regenerate = async () => {
    setBusy(true);
    setErr(null);
    try {
      const saved = await api.webRegenerateToken();
      setCfg(saved);
      await refreshLink();
    } catch (e) {
      setErr(msgOf(e));
    } finally {
      setBusy(false);
    }
  };

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(link);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      /* ignore */
    }
  };

  const commitPort = () => {
    if (!cfg) return;
    const p = parseInt(portDraft, 10);
    if (!Number.isInteger(p) || p < 1 || p > 65535 || p === cfg.port) {
      setPortDraft(String(cfg.port));
      return;
    }
    save({ ...cfg, port: p });
  };

  const enabled = cfg?.enabled ?? false;

  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <div className="modal web-modal">
        <div className="modal-title">Remote control</div>
        <div className="web-desc">
          Run a small local web server so you can browse and move files on your
          connected hosts — and connect / disconnect or toggle port forwards —
          from your phone or another browser. Access is granted only through the
          link below, which carries a secret token.
        </div>

        <label className="about-setting" onClick={() => cfg && save({ ...cfg, enabled: !enabled })}>
          <span className={`toggle${enabled ? ' on' : ''}`}>
            <span className="toggle-knob" />
          </span>
          <span className="about-setting-label">
            {enabled ? 'Server running' : 'Enable control server'}
          </span>
        </label>

        {enabled && cfg && (
          <>
            <div className="web-field">
              <span className="web-field-label">Port</span>
              <input
                className="port-input"
                type="number"
                min={1}
                max={65535}
                value={portDraft}
                disabled={busy}
                onChange={(e) => setPortDraft(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && commitPort()}
                onBlur={commitPort}
              />
            </div>

            <label
              className="about-setting"
              onClick={() => !busy && save({ ...cfg, lan: !cfg.lan })}
            >
              <span className={`toggle${cfg.lan ? ' on' : ''}`}>
                <span className="toggle-knob" />
              </span>
              <span className="about-setting-label">
                Reachable from the local network (phone / other devices)
              </span>
            </label>

            <div className="web-link-row">
              <input className="web-link" readOnly value={link} onFocus={(e) => e.target.select()} />
              <button className="toast-secondary" disabled={!link} onClick={copy}>
                {copied ? 'Copied' : 'Copy'}
              </button>
              <button
                className="toast-secondary"
                disabled={!link}
                onClick={() => openUrl(link).catch(() => {})}
              >
                Open
              </button>
            </div>

            <div className="web-hint">
              {cfg.lan
                ? 'Anyone on your network who has this link can reach your hosts. Keep it private; regenerate the token to revoke old links.'
                : 'Bound to this Mac only. Turn on network access above to use the link from another device.'}
            </div>

            <button className="toast-ghost web-regen" disabled={busy} onClick={regenerate}>
              Regenerate token (revokes old links)
            </button>
          </>
        )}

        {err && <div className="web-err">{err}</div>}

        <div className="modal-actions" style={{ justifyContent: 'center' }}>
          <button className="toast-ghost" onClick={close}>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}

function msgOf(e: unknown): string {
  return (e as { message?: string })?.message ?? String(e);
}
