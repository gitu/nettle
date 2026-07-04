import { useEffect, useState } from 'react';
import { getVersion, getTauriVersion } from '@tauri-apps/api/app';
import { openUrl } from '@tauri-apps/plugin-opener';
import { useStore } from '../store';

const REPO_URL = 'https://github.com/gitu/nettle';

export function AboutModal() {
  const open = useStore((s) => s.aboutOpen);
  const [version, setVersion] = useState('');
  const [tauriVersion, setTauriVersion] = useState('');

  useEffect(() => {
    getVersion().then(setVersion).catch(() => {});
    getTauriVersion().then(setTauriVersion).catch(() => {});
  }, []);

  if (!open) return null;
  const close = () => useStore.setState({ aboutOpen: false });

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
