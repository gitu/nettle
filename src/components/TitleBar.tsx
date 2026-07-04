import { api } from '../ipc';
import { useStore } from '../store';

export function TitleBar() {
  const conn = useStore((s) => s.conn);
  const hosts = useStore((s) => s.hosts);

  let suffix = 'not connected';
  if (conn.state !== 'disconnected') {
    const host = hosts.find((h) => 'hostId' in conn && h.id === conn.hostId);
    if (host) suffix = `${host.username}@${host.hostname} — ${host.name}`;
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
