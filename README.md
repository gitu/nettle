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
- **Many hosts at once** — connect to several servers simultaneously; each
  keeps its own live terminal, tunnels, and transfers. Switching hosts leaves
  the others running (toggle off in About if you prefer one-at-a-time).
- **Tunnels dashboard** — one overview of every forward across every connected
  host, grouped by host, with live/waiting status and quick stop.
- **Connection sets** — name a group of hosts and bring them all up with one
  click (e.g. "production" = api + db + cache).
- **Tray-first** — closing the window hides nettle to the menu bar / system
  tray; sessions and every pinned tunnel keep running in the background.

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
  passphrase), then password. Passwords/passphrases are held in an in-memory
  vault (per host) for the app's runtime — never written to disk — so
  auto-reconnects, disconnect/reconnect cycles, and host switching don't
  re-prompt. Quitting the app forgets everything.
- Host definitions (`hosts.json`) contain no secrets.

## Remote control (optional)

nettle can run a small local HTTP server so you can browse and move files on your
connected hosts — and connect/disconnect or toggle port forwards — from your
phone or another browser. Open **About → Remote control…** to configure it.

- **Off by default.** Enabling it mints a random 128-bit token that is embedded
  in a link the app hands you (the token rides in the URL *fragment*, so it is
  never sent to the server in the clear or written to its logs). Opening the link
  loads a small, self-contained web panel that authenticates with that token.
- **Localhost by default.** The server binds to `127.0.0.1:8760` (the port is
  configurable in the dialog); the link only works on this machine. Flip
  **"reachable from the local network"** to bind `0.0.0.0` and get a link with
  your LAN IP for use from another device — at which point anyone on your network
  who has the link can reach your hosts, so keep it private.
- **The token is the only credential.** Every `/api/*` call requires it (as a
  `Authorization: Bearer` header or `?t=` query param); regenerate it any time to
  instantly revoke every previously-issued link. No remote shell is exposed over
  the web.

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
├── web.rs      optional token-authorized HTTP control server (axum)
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
for all three platforms and bumps the Homebrew cask (needs the
`HOMEBREW_TAP_TOKEN` secret — a PAT with write access to `gitu/homebrew-tap`).

To release, push a `vX.Y.Z` tag on `main` — nothing else. The release workflow
injects the version from the tag into `tauri.conf.json` before building, so the
version fields committed in `tauri.conf.json` / `Cargo.toml` / `package.json`
only affect local dev builds and don't need to be bumped per release (nudge them
occasionally so `pnpm tauri dev` shows something close to reality).

## Signing macOS builds

Release DMGs are ad-hoc signed until Apple credentials are configured. To ship
properly signed + notarized builds you need an
[Apple Developer Program](https://developer.apple.com/programs/) membership,
then:

1. In Xcode or [developer.apple.com](https://developer.apple.com/account/resources/certificates/list),
   create a **Developer ID Application** certificate and export it (with its
   private key) as a `.p12`.
2. Create an **app-specific password** for your Apple ID at
   [appleid.apple.com](https://appleid.apple.com) (used for notarization), and
   note your 10-character **Team ID**.
3. Add these repository secrets (Settings → Secrets → Actions):
   - `APPLE_CERTIFICATE` — the `.p12`, base64-encoded (`base64 -i cert.p12 | pbcopy`)
   - `APPLE_CERTIFICATE_PASSWORD` — the `.p12` export password
   - `APPLE_SIGNING_IDENTITY` — e.g. `Developer ID Application: Your Name (TEAMID)`
   - `APPLE_ID`, `APPLE_PASSWORD` (the app-specific password), `APPLE_TEAM_ID`

The release workflow already forwards these to `tauri-action`; the next tagged
release will be signed, notarized, and stapled automatically — no Gatekeeper
prompt, and the Homebrew cask caveat can be dropped.

## License

[MIT](LICENSE)
