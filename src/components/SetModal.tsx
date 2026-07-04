import { useEffect, useState } from 'react';
import { api, type ConnectionSet } from '../ipc';
import { useStore } from '../store';

const EMPTY: ConnectionSet = {
  id: '00000000-0000-0000-0000-000000000000',
  name: '',
  hostIds: [],
};

export function SetModal() {
  const editSet = useStore((s) => s.editSet);
  const hosts = useStore((s) => s.hosts);
  const refreshSets = useStore((s) => s.refreshSets);
  const [draft, setDraft] = useState<ConnectionSet>(EMPTY);

  useEffect(() => {
    if (editSet === 'new') setDraft(EMPTY);
    else if (editSet) setDraft({ ...editSet, hostIds: [...editSet.hostIds] });
  }, [editSet]);

  if (!editSet) return null;
  const isNew = editSet === 'new';
  const close = () => useStore.setState({ editSet: null });

  const toggle = (id: string) =>
    setDraft((d) => ({
      ...d,
      hostIds: d.hostIds.includes(id) ? d.hostIds.filter((x) => x !== id) : [...d.hostIds, id],
    }));

  const save = async () => {
    if (!draft.name.trim() || draft.hostIds.length === 0) return;
    await api.saveSet(draft);
    await refreshSets();
    close();
  };

  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <div className="modal">
        <div className="modal-title">{isNew ? 'NEW CONNECTION SET' : 'EDIT CONNECTION SET'}</div>
        <div className="modal-fields">
          <div className="field">
            <label>NAME</label>
            <input
              autoFocus
              placeholder="production, homelab, …"
              value={draft.name}
              onChange={(e) => setDraft({ ...draft, name: e.target.value })}
              onKeyDown={(e) => e.key === 'Escape' && close()}
            />
          </div>
          <div className="field">
            <label>HOSTS ({draft.hostIds.length})</label>
            <div className="set-hosts">
              {hosts.length === 0 && <div className="pane-msg">No hosts to add yet.</div>}
              {hosts.map((h) => (
                <label key={h.id} className="set-host">
                  <input
                    type="checkbox"
                    checked={draft.hostIds.includes(h.id)}
                    onChange={() => toggle(h.id)}
                  />
                  <span className="set-host-name">{h.name}</span>
                  <span className="set-host-addr">
                    {h.username}@{h.hostname}
                  </span>
                </label>
              ))}
            </div>
          </div>
        </div>
        <div className="modal-actions">
          {!isNew && (
            <button
              className="danger-btn"
              onClick={async () => {
                await api.deleteSet(draft.id);
                await refreshSets();
                close();
              }}
            >
              delete
            </button>
          )}
          <button className="toast-ghost" onClick={close}>
            Cancel
          </button>
          <button className="toast-primary" style={{ flex: 'none', padding: '8px 16px' }} onClick={save}>
            Save
          </button>
        </div>
      </div>
    </div>
  );
}
