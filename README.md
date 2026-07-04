# ◆ nettle

**A resilient SSH client that treats flaky links and restarting dev servers as normal.**

[![CI](https://github.com/gitu/nettle/actions/workflows/ci.yml/badge.svg)](https://github.com/gitu/nettle/actions/workflows/ci.yml)
[![Release](https://github.com/gitu/nettle/actions/workflows/release.yml/badge.svg)](https://github.com/gitu/nettle/releases)

nettle is a cross-platform desktop SSH client (macOS / Linux / Windows) built for
day-to-day work against remote dev boxes:

- **Live port discovery** — nettle watches which TCP ports are listening on the
  remote (process name included) and shows them the moment they appear. A new
  dev server pops a toast: *Forward & pin / Just once / Ignore*.
- **Pinned tunnels that survive everything** — a pinned forward keeps its local
  listener alive across remote process restarts *and* SSH reconnects. Kill your
  remote `vite dev`, start it again, and `localhost:5173` just works — no clicks.
- **Auto-reconnect with fresh DNS** — every reconnect attempt re-resolves the
  hostname, so a remote that comes back on a new IP address is picked up
  automatically. Your terminal reopens itself and tunnels resume.
- **Dual-pane file browser** — remote (SFTP) next to local, with a transfer
  queue, live progress, and cancellation. Transfers run on their own SSH
  channels and never block the terminal.
- **Real terminal** — a full PTY shell rendered with xterm.js.
- **Tray-first** — closing the window hides nettle to the menu bar / system
  tray; the session and every pinned tunnel keep running in the background.

## Install

**macOS (Homebrew):**

```sh
brew install --cask gitu/tap/nettle
```

Or grab the latest build from [Releases](https://github.com/gitu/nettle/releases):
`.dmg` for macOS (Apple Silicon and Intel), `.AppImage` / `.deb` / `.rpm` for
Linux, `.msi` / `.exe` for Windows.

Builds are currently unsigned. On macOS the first launch needs
*right-click → Open* (or `xattr -dr com.apple.quarantine /Applications/nettle.app`);
on Windows, allow the SmartScreen prompt.

## Security model

- **Host keys**: trust-on-first-use with an explicit fingerprint prompt, stored
  in an OpenSSH-format `known_hosts` under nettle's config dir. A changed key is
  a hard failure (possible MITM) — there is deliberately no "accept anyway" button.
- **Auth**: ssh-agent first (`$SSH_AUTH_SOCK`, or the Windows OpenSSH agent
  named pipe), then a configured private key (encrypted keys prompt for the
  passphrase), then password. Passwords/passphrases are held in memory for the
  session only — never written to disk — so auto-reconnect doesn't re-prompt.
- Host definitions (`hosts.json`) contain no secrets.

## Architecture

Tauri v2 · Rust backend (`russh` + `russh-sftp`) · React + TypeScript frontend.

The backend is built around a **connection epoch** model: one actor task owns
the reconnect state machine and publishes `Arc<ConnectionEpoch>` (a cheap-clone
russh handle + a cancellation token) through a `tokio::sync::watch`. Every
subsystem — terminal, SFTP browser, transfer workers, port scanner, forward
manager — races its work against the epoch's cancellation token and re-attaches
to the next epoch after a reconnect. Pinned forwards keep their local
`TcpListener`s *outside* the epoch lifecycle, which is what makes tunnels
survive reconnects for free.

Port discovery execs `ss -tlnp` on the remote every 3 s (falling back to
`netstat`, then `/proc/net/tcp`), diffs the result, and feeds both the UI and
the forward manager. The parsers are pure functions with fixture tests.

The backend never touches the Tauri runtime directly — events go through an
`EventSink` trait — so the entire SSH stack runs headless in integration tests.

```
src-tauri/src/
├── ssh/        session actor (reconnect + fresh DNS), auth, host keys
├── terminal.rs PTY channel with output coalescing
├── sftp/       browse session + concurrent transfer queue
├── ports/      scanner + parsers + forward manager
└── ipc/        Tauri commands & typed event payloads
src/            React UI (zustand store, xterm.js terminal)
```

## Development

Prerequisites: Rust (stable), Node 22+, pnpm. On Linux additionally:
`libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev`.

```sh
pnpm install
pnpm tauri dev      # run the app
pnpm build          # typecheck + bundle the frontend
```

## Testing

Unit tests (parsers, config store) run everywhere:

```sh
cd src-tauri && cargo test --lib
```

The integration suite drives the real backend — connect, TOFU, PTY terminal,
SFTP roundtrip, direct-tcpip tunnel, reconnect with cached auth — against a
disposable sshd:

```sh
docker run -d --name nettle-sshd -p 2222:2222 \
  -e PASSWORD_ACCESS=true -e USER_PASSWORD=nettletest -e USER_NAME=deploy \
  lscr.io/linuxserver/openssh-server:latest
docker exec nettle-sshd sed -i \
  's/^AllowTcpForwarding no/AllowTcpForwarding yes/' /config/sshd/sshd_config
docker exec nettle-sshd sh -c 'kill -HUP $(cat /config/sshd.pid)'

cd src-tauri && NETTLE_E2E=1 cargo test --test e2e
```

CI runs the full suite (fmt, clippy `-D warnings`, unit, E2E against sshd, and
the frontend build) on every push; tagging `v*` builds and publishes installers
for all three platforms.

## License

[MIT](LICENSE)
