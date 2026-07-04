import { api } from '../ipc';
import { useStore } from '../store';

export function Toast() {
  const hostId = useStore((s) => s.focusedHostId);
  const toast = useStore((s) => (hostId ? (s.sessions[hostId]?.toast ?? null) : null));
  const hostName = useStore((s) => s.hosts.find((h) => h.id === hostId)?.name ?? '');
  const dismiss = useStore((s) => s.dismissToast);
  if (!hostId || !toast) return null;

  const forward = (pinned: boolean) => {
    api.forwardSet(hostId, toast.port, true, pinned).catch(() => {});
    dismiss(hostId);
    useStore.setState({ view: 'ports' });
  };

  return (
    <div className="toast">
      <div className="toast-head">
        <span className="toast-dot" />
        <span className="toast-label">NEW PORT · {hostName.toUpperCase()}</span>
      </div>
      <div className="toast-body">
        Port <b>{toast.port}</b>
        {toast.process ? ` (${toast.process})` : ''} just started listening on the remote.
        Forward it to <span className="acc">localhost:{toast.port}</span>?
      </div>
      <div className="toast-actions">
        <button className="toast-primary" onClick={() => forward(true)}>
          Forward &amp; pin
        </button>
        <button className="toast-secondary" onClick={() => forward(false)}>
          Just once
        </button>
        <button
          className="toast-ghost"
          onClick={() => {
            api.portIgnore(hostId, toast.port).catch(() => {});
            dismiss(hostId);
          }}
        >
          Ignore
        </button>
      </div>
    </div>
  );
}
