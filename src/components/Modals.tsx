import { useEffect, useState } from 'react';
import { api, type HostConfig } from '../ipc';
import { useStore } from '../store';

export function HostKeyModal() {
  const prompt = useStore((s) => s.hostKeyPrompt);
  if (!prompt) return null;

  const answer = (accept: boolean) => {
    api.hostKeyDecision(accept).catch(() => {});
    useStore.setState({ hostKeyPrompt: null });
  };

  return (
    <div className="overlay">
      <div className="modal">
        <div className="modal-title">UNKNOWN HOST KEY</div>
        <div className="modal-body">
          First connection to{' '}
          <span className="mono">
            {prompt.host}:{prompt.port}
          </span>
          . Verify the {prompt.keyType} fingerprint:
          <br />
          <br />
          <span className="mono">{prompt.fingerprint}</span>
        </div>
        <div className="modal-actions">
          <button className="toast-ghost" onClick={() => answer(false)}>
            Reject
          </button>
          <button className="toast-primary" style={{ flex: 'none', padding: '8px 16px' }} onClick={() => answer(true)}>
            Trust &amp; continue
          </button>
        </div>
      </div>
    </div>
  );
}

export function HostKeyMismatchModal() {
  const mismatch = useStore((s) => s.hostKeyMismatch);
  if (!mismatch) return null;

  return (
    <div className="overlay">
      <div className="modal danger">
        <div className="modal-title">HOST KEY CHANGED</div>
        <div className="modal-body">
          The key for{' '}
          <span className="mono">
            {mismatch.host}:{mismatch.port}
          </span>{' '}
          does not match the recorded one — possible man-in-the-middle. Connection refused.
          <br />
          <br />
          Offered {mismatch.keyType}: <span className="mono">{mismatch.fingerprint}</span>
          <br />
          <br />
          If the server was legitimately reinstalled, remove its entry from Nettle's
          known_hosts file and reconnect.
        </div>
        <div className="modal-actions">
          <button
            className="toast-secondary"
            onClick={() => useStore.setState({ hostKeyMismatch: null })}
          >
            Close
          </button>
        </div>
      </div>
    </div>
  );
}

export function AuthModal() {
  const req = useStore((s) => s.authRequest);
  const [secret, setSecret] = useState('');

  useEffect(() => setSecret(''), [req]);
  if (!req) return null;

  const submit = (value: string | null) => {
    api.provideSecret(value).catch(() => {});
    useStore.setState({ authRequest: null });
  };

  return (
    <div className="overlay">
      <div className="modal">
        <div className="modal-title">
          {req.kind === 'password' ? 'PASSWORD REQUIRED' : 'KEY PASSPHRASE REQUIRED'}
        </div>
        <div className="modal-body">
          {req.kind === 'password' ? (
            <>
              Password for{' '}
              <span className="mono">
                {req.username}@{req.host}
              </span>
            </>
          ) : (
            <>
              Passphrase for the private key used with{' '}
              <span className="mono">
                {req.username}@{req.host}
              </span>
            </>
          )}
        </div>
        <div className="modal-fields">
          <div className="field">
            <input
              type="password"
              autoFocus
              value={secret}
              onChange={(e) => setSecret(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') submit(secret);
                if (e.key === 'Escape') submit(null);
              }}
            />
          </div>
        </div>
        <div className="modal-actions">
          <button className="toast-ghost" onClick={() => submit(null)}>
            Cancel
          </button>
          <button
            className="toast-primary"
            style={{ flex: 'none', padding: '8px 16px' }}
            onClick={() => submit(secret)}
          >
            Unlock
          </button>
        </div>
      </div>
    </div>
  );
}

const EMPTY_HOST: HostConfig = {
  id: '00000000-0000-0000-0000-000000000000',
  name: '',
  hostname: '',
  port: 22,
  username: '',
  keyPath: '',
};

export function HostModal() {
  const editHost = useStore((s) => s.editHost);
  const refreshHosts = useStore((s) => s.refreshHosts);
  const [draft, setDraft] = useState<HostConfig>(EMPTY_HOST);

  useEffect(() => {
    if (editHost === 'new') setDraft(EMPTY_HOST);
    else if (editHost) setDraft({ ...editHost, keyPath: editHost.keyPath ?? '' });
  }, [editHost]);

  if (!editHost) return null;
  const isNew = editHost === 'new';
  const close = () => useStore.setState({ editHost: null });

  const save = async () => {
    if (!draft.hostname || !draft.username) return;
    await api.saveHost({
      ...draft,
      name: draft.name || draft.hostname,
      keyPath: draft.keyPath || null,
    });
    await refreshHosts();
    close();
  };

  const field = (
    label: string,
    key: keyof HostConfig,
    placeholder: string,
    type: 'text' | 'number' = 'text',
  ) => (
    <div className="field">
      <label>{label}</label>
      <input
        type={type}
        placeholder={placeholder}
        value={String(draft[key] ?? '')}
        onChange={(e) =>
          setDraft({
            ...draft,
            [key]: type === 'number' ? Number(e.target.value) || 22 : e.target.value,
          })
        }
        onKeyDown={(e) => {
          if (e.key === 'Enter') save();
          if (e.key === 'Escape') close();
        }}
      />
    </div>
  );

  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <div className="modal">
        <div className="modal-title">{isNew ? 'ADD HOST' : 'EDIT HOST'}</div>
        <div className="modal-fields">
          {field('NAME', 'name', 'prod-eu-1')}
          <div className="field-row">
            {field('HOSTNAME', 'hostname', 'server.example.com')}
            {field('PORT', 'port', '22', 'number')}
          </div>
          {field('USERNAME', 'username', 'deploy')}
          {field('PRIVATE KEY PATH (OPTIONAL)', 'keyPath', '~/.ssh/id_ed25519 — agent is tried first')}
        </div>
        <div className="modal-actions">
          {!isNew && (
            <button
              className="danger-btn"
              onClick={async () => {
                await api.deleteHost(draft.id);
                await refreshHosts();
                close();
              }}
            >
              delete
            </button>
          )}
          <button className="toast-ghost" onClick={close}>
            Cancel
          </button>
          <button
            className="toast-primary"
            style={{ flex: 'none', padding: '8px 16px' }}
            onClick={save}
          >
            Save
          </button>
        </div>
      </div>
    </div>
  );
}
