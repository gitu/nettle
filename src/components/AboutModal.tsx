import { useEffect, useState } from 'react';
import { getVersion, getTauriVersion } from '@tauri-apps/api/app';
import { openUrl } from '@tauri-apps/plugin-opener';
import { api } from '../ipc';
import { useStore } from '../store';

const REPO_URL = 'https://github.com/gitu/nettle';

export function AboutModal() {
  const open = useStore((s) => s.aboutOpen);
  const settings = useStore((s) => s.settings);
  const [version, setVersion] = useState('');
  const [tauriVersion, setTauriVersion] = useState('');

  useEffect(() => {
    getVersion().then(setVersion).catch(() => {});
    getTauriVersion().then(setTauriVersion).catch(() => {});
  }, []);

  if (!open) return null;
  const close = () => useStore.setState({ aboutOpen: false });

  const toggleKeep = () => {
    const next = { ...settings, keepConnections: !settings.keepConnections };
    useStore.setState({ settings: next });
    api.setSettings(next).catch(() => {});
  };

  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <div className="modal about">
        <div className="about-mark">◆</div>
        <div className="about-name">nettle</div>
        <div className="about-version">
          v{version || '…'}
          {tauriVersion && <span> · tauri {tauriVersion}</span>}
        </div>
        <div className="about-desc">
          A resilient SSH client — live port discovery, pinned tunnels that
          survive reconnects, dual-pane SFTP, and a built-in terminal.
        </div>
        <div className="about-links">
          <button className="toast-secondary" onClick={() => openUrl(REPO_URL).catch(() => {})}>
            GitHub
          </button>
          <button
            className="toast-secondary"
            onClick={() => openUrl(`${REPO_URL}/releases`).catch(() => {})}
          >
            Releases
          </button>
        </div>
        <label className="about-setting" onClick={toggleKeep}>
          <span className={`toggle${settings.keepConnections ? ' on' : ''}`}>
            <span className="toggle-knob" />
          </span>
          <span className="about-setting-label">
            Keep connections open when switching hosts
          </span>
        </label>
        <button
          className="toast-secondary about-remote-btn"
          onClick={() => useStore.setState({ aboutOpen: false, webOpen: true })}
        >
          Remote control…
        </button>
        <div className="about-license">MIT License · © 2026 gitu</div>
        <div className="modal-actions" style={{ justifyContent: 'center' }}>
          <button className="toast-ghost" onClick={close}>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
