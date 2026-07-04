import { api } from '../ipc';
import { useStore } from '../store';

export function TitleBar() {
  const sessions = useStore((s) => s.sessions);
  const hosts = useStore((s) => s.hosts);
  const focusedHostId = useStore((s) => s.focusedHostId);

  const count = Object.keys(sessions).length;
  const focusedHost = hosts.find((h) => h.id === focusedHostId);

  let suffix = 'not connected';
  if (focusedHost) {
    suffix = `${focusedHost.username}@${focusedHost.hostname}`;
    if (count > 1) suffix += ` · ${count} sessions`;
  } else if (count > 0) {
    suffix = `${count} session${count === 1 ? '' : 's'}`;
  }

  return (
    <div className="titlebar" data-tauri-drag-region>
      <div className="traffic">
        <button className="close" onClick={() => api.windowControl('close')} aria-label="close" />
        <button className="min" onClick={() => api.windowControl('minimize')} aria-label="minimize" />
        <button className="max" onClick={() => api.windowControl('maximize')} aria-label="maximize" />
      </div>
      <div className="titlebar-title" data-tauri-drag-region>
        ◆ nettle — {suffix}
      </div>
      <div className="titlebar-spacer" />
    </div>
  );
}
