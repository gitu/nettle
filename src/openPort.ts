import { openUrl } from '@tauri-apps/plugin-opener';
import { api } from './ipc';
import { useStore } from './store';

/** Forward the port if needed, sniff http vs https over the SSH link, then open
 *  the local tunnel in the default browser. Pass the forward's local port, or
 *  null when the port isn't forwarded yet. */
export async function openPortInBrowser(hostId: string, port: number, localPort: number | null) {
  const scheme = await api.probePortScheme(hostId, port).catch(() => 'http' as const);
  let target = localPort ?? port;
  if (localPort == null) {
    await useStore.getState().setForward(hostId, port, true, false);
    target = port;
  }
  await openUrl(`${scheme}://localhost:${target}`).catch(() => {});
}
