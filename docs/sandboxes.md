# Docker sandboxes (`sbx`)

nettle can reach into [Docker Sandboxes](https://docs.docker.com/ai/sandboxes/) on a host
you connect to: it lists the sandboxes, shows the ports listening inside each one (docker
containers included), forwards them to your machine, follows ports a sandbox *advertises*,
and starts, stops and tails the sandbox's background services.

```
laptop                      SSH host                         sandbox (microVM)
nettle ──── SSH ────► sbx ls / sbx exec <sandbox> … ────► app on 127.0.0.1:4180
localhost:4180  ◄──────────── tunnel (sbx exec -i … socat) ◄──┘
```

Nothing has to be published with `sbx ports`: a tunnel is an `sbx exec` bridge into the
sandbox, so services bound only to the sandbox's loopback work, and the same local port can
point at whichever sandbox you are working in.

- [Host setup](#host-setup)
- [Using it](#using-it)
- [Advertised ports](#advertised-ports)
- [Services](#services)
- [Telling the agent](#telling-the-agent)
- [Setting up sandboxes](#setting-up-sandboxes) (with sbx-forge, or a kit of your own)
- [Troubleshooting](#troubleshooting)

## Host setup

On the SSH host (the machine running the sandboxes):

1. Install Docker Sandboxes and sign in: `sbx login`.
2. Make sure the user nettle logs in as owns the sandboxes: `sbx ls` lists only the
   current user's.
3. Make sure `sbx` is found over a non-interactive SSH session. nettle adds
   `/usr/local/bin`, `/opt/homebrew/bin` and `~/.local/bin` to `PATH`; anything else needs to
   be on the login `PATH`. Check from your laptop:

   ```sh
   ssh you@host 'sbx ls --json'
   ```

That's all nettle needs; there is nothing to install inside the sandboxes for plain port
forwarding. The tunnel uses `socat` in the sandbox, or `nc` when the image has no socat
(Docker's agent templates ship socat).

## Using it

Connect to the host as usual. When nettle finds `sbx` there, the ports tab gets a
**target** bar: *host* plus one chip per sandbox (a green dot = running, `→ :4180` = local
ports tunnelled into it). Pick a sandbox to see its ports.

- **Forwarding** works like for host ports: toggle a port, pin it, change the local port.
- **Switching sandboxes:** if `localhost:4180` already tunnels to sandbox A, sandbox B's 4180
  row offers **⇄ switch here**. That moves the listener in place: connections to A are
  closed, new ones go to B, and a pin moves along, so after a reconnect the tunnel comes back
  pointing at B. Your browser keeps the same origin, so cookies and OAuth redirect URIs on
  `localhost:4180` keep working.
- **Stopped sandboxes** are listed but never started (nettle doesn't call `sbx exec` on
  them, which would boot them). Their pinned tunnels resume once they run.
- **Open** probes `http` vs `https` through the sandbox, **kill** signals the process inside it.

nettle polls `sbx ls --json` every 5 s and scans each running sandbox with one `sbx exec`
(`ss`, else `/proc/net/tcp`; `docker ps` for container names). Each new tunnelled TCP
connection costs about 0.25 s of `sbx exec` start-up; keep-alive connections pay it once.

## Advertised ports

A sandbox can list ports it wants on your machine. nettle shows them in the sandbox's port
list with their label, even before anything listens, and **forwards each one to the same
local port as soon as it listens**.

nettle reads two files inside the sandbox, with its regular scan:

| File | Written by | Purpose |
|---|---|---|
| `/etc/nettle/ports` | the sandbox setup (a kit) | default ports of this kind of sandbox |
| `~/.config/nettle/ports` | the agent or you, at work time | what is running now |

One port per line, an optional label after it, `#` starts a comment. A line in the second
file overrides the same port in the first.

```
# PORT  LABEL
4180    oauth2-proxy
5173    vite dev
8080
```

Writing the file directly works; with sbx-forge's `remote-access` kit there is a helper:

```sh
nettle-port add 5173 vite dev     # advertise
nettle-port rm 5173
nettle-port ls                    # defaults and own ones
```

Auto-forward rules:

- The local port is the advertised port. If that local port already tunnels somewhere else
  (another sandbox, the host), nettle leaves it alone and logs it; use *⇄ switch here*. If
  another app on your machine holds it, the bind error shows in the activity log.
- Each advertised port is forwarded once while it stays advertised. Stop the forward and it
  stays stopped, across reconnects too; it comes back when the port is advertised anew or
  the host session is reopened.
- Auto-forwards aren't pinned; pin one to keep it across restarts of the app.
- Turn the behavior off in **About → Forward ports Docker sandboxes advertise**. Advertised
  ports are still shown.

## Services

nettle shows a sandbox's background services in a **Services** panel on its ports tab:
state, port, whether it autostarts, and **start / stop / restart / logs** (the log viewer
re-reads the last 1000 lines every 2 s). A service's port counts as advertised.

The panel appears when the sandbox has an `sbx-svc` command. sbx-forge's `services` kit
provides one; any other implementation works if it follows this contract (run as the
sandbox's default user, no `sh -c` quoting surprises in names):

| Command | Must do |
|---|---|
| `sbx-svc ls --tsv` | one line per service: `NAME⇥running\|stopped⇥PORT or -⇥yes\|no⇥COMMAND` |
| `sbx-svc start NAME` / `stop NAME` / `restart NAME` | act on one service |
| `sbx-svc logs NAME -n LINES` | print the last LINES lines of its output |

Names are `[A-Za-z0-9._-]`, starting with a letter or digit.

## Telling the agent

An agent in the sandbox only advertises ports and uses `sbx-svc` if it knows about them,
and two things work against that:

- sbx's generated `CLAUDE.md` (next to the workspace) lists kits by their one-line
  description and tells the agent to read a kit's instructions only when relevant.
- The same file always tells the agent that ports must be published with
  `sbx ports --publish` and services must listen on `0.0.0.0`.

So put a short note where the agent always reads it, `~/.claude/CLAUDE.md` (Claude Code) and
`~/.codex/AGENTS.md` (Codex) in the sandbox, and say that it replaces the publishing advice.
sbx-forge's kits do this; the text they use is a good starting point:

```markdown
## Ports: the user opens this sandbox's services through nettle

nettle tunnels every port listening in this sandbox to the same port on the user's
laptop (`localhost:<port>`), over SSH and `sbx exec`. This replaces the "Publishing
ports to the host" advice of the sandbox's CLAUDE.md: don't ask for `sbx ports`, and
binding 127.0.0.1 is fine.

When you start something the user should open in a browser or client:
- Keep the app's normal port. `nettle-port ls` lists the default ports: use those
  ports for those services.
- Advertise it: `nettle-port add <port> <what>`. nettle forwards it once it listens.
- Tell the user the sandbox (`$SANDBOX_NAME`), the port and what runs there.
```

With that in place, Claude Code asked to "serve this folder's index.html so I can open it"
starts the server on 127.0.0.1 (as an `sbx-svc` service when that kit is there), advertises
the port and reports the sandbox and port, without mentioning `sbx ports`.

## Setting up sandboxes

### With sbx-forge

[sbx-forge](https://github.com/gitu/sbx-forge) wraps `sbx env` with ready-made kits; every
env (`github`, `gitlab`, `nimbusplane`) includes:

- **remote-access**: `nettle-port`, `/etc/nettle/ports` from the env's `ports` arg, and the
  agent note above.
- **services**: `sbx-svc` with services from the repo's `.sbx/services` or `sbx-svc add`,
  autostarted on every sandbox start, plus its agent note.

Set the default ports per env in `<env>/<env>.args`, then create the sandbox:

```sh
# nimbusplane/nimbusplane.args
ports=4180=oauth2-proxy 8080=api 3000=ui
```

```sh
cd ~/src/my-repo && sbx-forge nimbusplane     # existing sandbox: sbx-forge nimbusplane rm first
```

### With your own kit

A minimal mixin kit doing the same for any `sbx create` / `sbx env` setup:

```yaml
# my-kits/nettle/spec.yaml
schemaVersion: "2"
kind: mixin
name: nettle
description: >-
  Read before starting a server the user should open: nettle forwards it to
  their localhost; advertise it in ~/.config/nettle/ports (not sbx ports)

setup:
  install:
    - description: Default ports nettle forwards once they listen
      command: |
        mkdir -p /etc/nettle
        printf '4180 oauth2-proxy\n3000 ui\n' > /etc/nettle/ports
    - description: Always-loaded agent note
      user: "1000"
      command: |
        set -e
        for f in ~/.claude/CLAUDE.md ~/.codex/AGENTS.md; do
          mkdir -p "$(dirname "$f")"
          cat >> "$f" <<'EOF'
        ## Ports: the user opens this sandbox's services through nettle
        nettle forwards ports listening here to the same port on the user's localhost,
        which replaces the sandbox CLAUDE.md's "Publishing ports" advice (no `sbx ports`,
        127.0.0.1 is fine). Keep apps on their normal ports; the defaults are in
        /etc/nettle/ports. Advertise a port with
        `echo "<port> <what>" >> ~/.config/nettle/ports`, then tell the user the
        sandbox ($SANDBOX_NAME) and port.
        EOF
        done
```

```sh
sbx create claude . --kit ./my-kits/nettle/
```

Kits apply when a sandbox is created; remove and recreate existing sandboxes to get them.

## Troubleshooting

| Symptom | Check |
|---|---|
| No target bar | `ssh you@host 'sbx ls --json'` must work; the activity log (category *sandbox*) shows what nettle saw. |
| "`sbx ls` failed on the host" | the sandbox daemon is down or you aren't signed in: `sbx ls` on the host. |
| A sandbox has no ports | it is stopped, or nothing listens yet; nettle never starts sandboxes. |
| Tunnel connects, then closes at once | nothing listens on that port in the sandbox, or the image has neither `socat` nor `nc`; the activity log says "the sandbox bridge failed". |
| Advertised port not forwarded | the local port is busy (activity log), auto-forward is off in About, or you stopped it earlier (re-advertise it or reopen the host session). |
| No Services panel | the sandbox has no `sbx-svc` (`sbx exec <sandbox> sbx-svc ls`). |
| First request through a tunnel is slow | each new connection starts an `sbx exec` (~0.25 s); keep-alive helps. |
