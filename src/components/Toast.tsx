import { api } from '../ipc';
import { useStore } from '../store';

export function Toast() {
  const toast = useStore((s) => s.toast);
  const dismiss = useStore((s) => s.dismissToast);
  if (!toast) return null;

  const forward = (pinned: boolean) => {
    api.forwardSet(toast.port, true, pinned).catch(() => {});
    useStore.setState({ toast: null, view: 'ports' });
  };

  return (
    <div className="toast">
      <div className="toast-head">
        <span className="toast-dot" />
        <span className="toast-label">NEW PORT DETECTED</span>
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
            api.portIgnore(toast.port).catch(() => {});
            dismiss();
          }}
        >
          Ignore
        </button>
      </div>
    </div>
  );
}
