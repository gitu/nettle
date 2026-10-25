import { openUrl } from '@tauri-apps/plugin-opener';
import { api } from './ipc';
import { useStore } from './store';

/** Forward the port if needed, sniff http vs https over the SSH link, then open
 *  the local tunnel in the default browser. Pass the forward's local port, or
 *  null when the port isn't forwarded yet; `sandbox` names the Docker sandbox
 *  the port lives in (null = the host). */
export async function openPortInBrowser(
  hostId: string,
  port: number,
  localPort: number | null,
  sandbox: string | null = null,
) {
  const scheme = await api.probePortScheme(hostId, port, sandbox).catch(() => 'http' as const);
  let target = localPort ?? port;
  if (localPort == null) {
    await useStore.getState().setForward(hostId, port, true, false, undefined, sandbox);
    // Bind failed (e.g. local port in use) — the banner explains it; opening
    // localhost would just land on whatever squats on that port.
    const err = useStore.getState().sessions[hostId]?.portError;
    if (err?.port === port && err.sandbox === sandbox) return;
    target = port;
  }
  await openUrl(`${scheme}://localhost:${target}`).catch(() => {});
}
