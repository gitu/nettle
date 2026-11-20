//! Links the host asks to open (`xdg-open`), opened on this machine.
//!
//! On a headless host `xdg-open https://…` leads nowhere, so command-line
//! logins that want a browser (`gh auth login`, `aws sso login`, an MCP
//! server's OAuth, …) fail or fall back to copying codes around. With the
//! per-host option on, nettle
//!
//! 1. installs a small `xdg-open` shim in the host's `~/.local/bin` (and a
//!    `www-browser` link to it, which is what tools fall back to on a host
//!    without a display),
//! 2. keeps a *watcher* running on an SSH exec channel: a shell loop reading a
//!    FIFO in a private directory under `/tmp`, which the shim writes to,
//! 3. opens each http(s) link in the local browser, after forwarding the
//!    loopback ports it needs: the ones the link names (`http://localhost:8888/…`,
//!    `redirect_uri=http://127.0.0.1:8085/cb`) and the ones the calling program
//!    just started listening on (callback servers whose port is not in the link).
//!
//! Those forwards are temporary: each is removed once its remote port stops
//! listening. While nettle is away the shim hands over to the host's real
//! `xdg-open`, so nothing changes for other sessions.
//!
//! The host is not trusted with more than a browser tab: only `http` and
//! `https` links are opened, at a bounded rate, and forwards never replace an
//! existing one.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use russh::ChannelMsg;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use url::Url;
use uuid::Uuid;

use crate::error::Result;
use crate::ipc::types::{LogLevel, RemotePort};
use crate::ports::forwards::{free_local_port, ForwardManager, Target};
use crate::ports::parse;
use crate::ssh::{exec_capture, exec_capture_stdin, ConnectionEpoch, EpochRx};
use crate::state::UiBridge;

/// Longest link taken from the host (sign-in links with embedded request
/// objects run to a few kilobytes).
const MAX_URL_LEN: usize = 16 * 1024;
/// Longest line read from the watcher before the rest of it is skipped.
const MAX_LINE: usize = MAX_URL_LEN + 1024;
/// Links taken per [`RATE_WINDOW`] before further ones are dropped: room for
/// a script that opens a handful at once, not for one stuck in a loop.
const RATE_LIMIT: usize = 12;
const RATE_WINDOW: Duration = Duration::from_secs(30);
/// Ports forwarded for one link at most.
const MAX_PORTS_PER_LINK: usize = 8;
/// How deep loopback URLs are looked for in URLs nested in the query.
const MAX_NESTING: u8 = 3;
/// A port of the calling program counts as opened for this link when the
/// port scanner first saw it at most this long ago (or not yet).
const RECENT_PORT: Duration = Duration::from_secs(120);
/// How long a temporary forward may wait for its port to show up in a scan
/// before it is removed again.
const APPEAR_GRACE: Duration = Duration::from_secs(60);
/// Budget for the short commands run on the host.
const EXEC_TIMEOUT: Duration = Duration::from_secs(10);
/// Pause before trying again after the watcher failed.
const RETRY: Duration = Duration::from_secs(30);

/// The `xdg-open` shim, written to `~/.local/bin/xdg-open` on the host. The
/// `nettle-xdg-open-shim` marker makes the file ours: nettle only ever
/// replaces or removes a file that contains it.
const SHIM: &str = r#"#!/bin/sh
# nettle-xdg-open-shim
#
# Installed by nettle (https://github.com/gitu/nettle), the SSH client, because
# "open links from this host in my browser" is on for this host. While nettle
# is connected, http(s) links given to xdg-open open in the browser of the
# machine nettle runs on, and nettle forwards the localhost ports a sign-in
# needs. Anything else, and everything while nettle is away, goes to the next
# xdg-open on PATH. Turning the option off in nettle removes this file.
#
# www-browser next to it is a link to this file: what Python's webbrowser and
# the xdg-open bundled with Node's "open" look for on a host without a display.

d=/tmp/nettle-$(id -u)
name=${0##*/}

alive() {
  case $1 in ""|*[!0-9]*) return 1 ;; esac
  [ -p "$d/open.$1.fifo" ] && kill -0 "$1" 2>/dev/null
}

# The watcher of the nettle that connected last, else any other live one.
watcher() {
  [ -d "$d" ] && [ ! -L "$d" ] && [ -O "$d" ] || return 1
  w=$(cat "$d/open.pid" 2>/dev/null)
  alive "$w" && return 0
  for f in "$d"/open.*.fifo; do
    w=${f##*/open.}
    w=${w%.fifo}
    alive "$w" && return 0
  done
  return 1
}

# Hand over to the next program of this name on PATH that is not this shim.
# Once per name: the real xdg-open may well come back here (BROWSER, or its
# own search for www-browser).
passthrough() {
  case " $NETTLE_SHIM_SEEN " in
    *" $name "*) ;;
    *)
      NETTLE_SHIM_SEEN="$NETTLE_SHIM_SEEN $name"
      export NETTLE_SHIM_SEEN
      oldifs=$IFS
      IFS=:
      for dir in $PATH; do
        IFS=$oldifs
        next="${dir:-.}/$name"
        [ -f "$next" ] && [ -x "$next" ] || continue
        grep -q nettle-xdg-open-shim "$next" 2>/dev/null && continue
        exec "$next" "$@"
      done
      IFS=$oldifs
      ;;
  esac
  echo "$name: $why, and this host has no other $name. Open it yourself: $*" >&2
  exit 3
}

url=$1
why="nettle only opens http(s) links"
[ $# -eq 1 ] || passthrough "$@"
case $url in
  http://*|https://*) ;;
  *) passthrough "$@" ;;
esac
case $url in
  *"
"*) passthrough "$@" ;;
esac
why="nettle is not connected"
watcher || passthrough "$@"

# Who is asking: this process's ancestors up to their session leader. nettle
# forwards the ports they just started listening on (sign-in callbacks).
pids=
p=$PPID
n=0
while [ "$n" -lt 16 ]; do
  case $p in ""|0|1|*[!0-9]*) break ;; esac
  pids="$pids,$p"
  st=$(cat "/proc/$p/stat" 2>/dev/null) || break
  st=${st##*) }
  set -- $st
  [ "$4" = "$p" ] && break
  p=$2
  n=$((n + 1))
done
pids=${pids#,}

# Read-write, so this never blocks should the watcher be gone by now.
if printf "open %s %s\n" "${pids:--}" "$url" 3<>"$d/open.$w.fifo" >&3; then
  exit 0
fi
passthrough "$url"
"#;

/// The watcher: runs on an exec channel for as long as nettle wants links
/// from this host, and prints what the shim writes to its FIFO. It ends when
/// its stdin closes, which is when nettle disconnects or the link drops.
///
/// Like every script below it is wrapped in `sh -c '…'` (the login shell may
/// be fish or zsh), so it must not contain single quotes.
const WATCHER: &str = r#"
umask 077
d=/tmp/nettle-$(id -u)
mkdir -p "$d" 2>/dev/null
if [ ! -d "$d" ] || [ -L "$d" ] || [ ! -O "$d" ]; then
  echo "@@ERR $d is not a directory of this user"
  exit 3
fi
chmod 700 "$d"
# Leftovers of watchers that died without cleaning up.
for old in "$d"/open.*.fifo; do
  p=${old##*/open.}
  p=${p%.fifo}
  case $p in ""|*[!0-9]*) continue ;; esac
  kill -0 "$p" 2>/dev/null || rm -f "$old"
done
f="$d/open.$$.fifo"
rm -f "$f"
mkfifo "$f" || { echo "@@ERR cannot create $f"; exit 4; }
cleanup() {
  rm -f "$f"
  if [ "$(cat "$d/open.pid" 2>/dev/null)" = "$$" ]; then rm -f "$d/open.pid"; fi
}
trap cleanup EXIT
trap "exit 0" HUP INT TERM PIPE
# Read-write, so the loop below sees no EOF between two writers.
exec 3<>"$f" 4<&0
( cat <&4 >/dev/null 2>&1 3>&-; echo eof >&3 ) >/dev/null 2>&1 &
# The newest watcher is the one the shim prefers.
echo "$$" > "$d/open.pid.$$" && mv -f "$d/open.pid.$$" "$d/open.pid"
echo "@@READY $$"
while IFS= read -r line <&3; do
  case $line in
    eof) break ;;
    *) printf "%s\n" "$line" ;;
  esac
done
"#;

/// Writes the shim (from stdin) unless a file that is not ours is in the way,
/// links `www-browser` to it where that name is free, and reports another
/// `xdg-open` on the non-interactive `PATH`.
const INSTALL: &str = r#"
bin="$HOME/.local/bin"
f="$bin/xdg-open"
if [ -e "$f" ] || [ -L "$f" ]; then
  if ! grep -q nettle-xdg-open-shim "$f" 2>/dev/null; then
    echo "@@FOREIGN $f"
    exit 0
  fi
fi
if mkdir -p "$bin" && cat > "$f.nettle-new" && chmod 755 "$f.nettle-new"; then
  if cmp -s "$f.nettle-new" "$f" 2>/dev/null; then
    rm -f "$f.nettle-new"
    echo "@@SHIM current $f"
  elif mv -f "$f.nettle-new" "$f"; then
    echo "@@SHIM installed $f"
  else
    rm -f "$f.nettle-new"
    echo "@@ERR cannot write $f"
    exit 0
  fi
else
  rm -f "$f.nettle-new"
  echo "@@ERR cannot write $f"
  exit 0
fi
a="$bin/www-browser"
if [ ! -e "$a" ] && [ ! -L "$a" ] || grep -q nettle-xdg-open-shim "$a" 2>/dev/null; then
  ln -sf xdg-open "$a" 2>/dev/null
fi
oldifs=$IFS
IFS=:
for dir in $PATH; do
  IFS=$oldifs
  next="${dir:-.}/xdg-open"
  [ -f "$next" ] && [ -x "$next" ] || continue
  grep -q nettle-xdg-open-shim "$next" 2>/dev/null && continue
  echo "@@REAL $next"
  break
done
"#;

/// Removes the shim again, if it is ours and no other nettle (a second
/// machine, a second host entry for the same account) still watches this
/// host. `$1` is the pid of the caller's own watcher, which is on its way out.
const UNINSTALL: &str = r#"
d=/tmp/nettle-$(id -u)
f="$HOME/.local/bin/xdg-open"
if [ -d "$d" ] && [ ! -L "$d" ] && [ -O "$d" ]; then
  for other in "$d"/open.*.fifo; do
    p=${other##*/open.}
    p=${p%.fifo}
    case $p in ""|*[!0-9]*) continue ;; esac
    [ "$p" = "$1" ] && continue
    if [ -p "$other" ] && kill -0 "$p" 2>/dev/null; then
      echo "@@KEPT $f"
      exit 0
    fi
  done
fi
a="$HOME/.local/bin/www-browser"
if [ -L "$a" ] && grep -q nettle-xdg-open-shim "$a" 2>/dev/null; then
  rm -f "$a"
fi
if grep -q nettle-xdg-open-shim "$f" 2>/dev/null; then
  rm -f "$f" && echo "@@REMOVED $f"
fi
"#;

/// Listening sockets with their pids. `/proc/net/tcp` has none, so unlike the
/// port scanner there is no third fallback.
const PID_SCAN: &str = "ss -tlnp 2>/dev/null || netstat -tlnp 2>/dev/null";

/// `script` as one command for the user's login shell, whatever that is.
fn sh_command(script: &str) -> String {
    let body: Vec<&str> = script
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .collect();
    format!("exec sh -c '{}'", body.join("\n"))
}

/// Pid of the watcher from its `@@READY <pid>` line.
fn parse_ready(line: &str) -> Option<u32> {
    line.strip_prefix("@@READY ")?.trim().parse().ok()
}

pub fn watch_command() -> String {
    sh_command(WATCHER)
}

pub fn install_command() -> String {
    sh_command(INSTALL)
}

/// `own_watcher`: the pid of the watcher that was just told to stop.
pub fn uninstall_command(own_watcher: Option<u32>) -> String {
    let own = own_watcher.map(|pid| pid.to_string()).unwrap_or_default();
    // Arguments after the script: `$0`, then `$1`.
    format!("{} nettle {own}", sh_command(UNINSTALL))
}

/// The shim file's contents (for the installer's stdin).
pub fn shim() -> &'static str {
    SHIM
}

/// One `xdg-open` call on the host, as the shim reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRequest {
    pub url: String,
    /// The calling process and its ancestors up to their session leader,
    /// nearest first. Empty when the host has no `/proc`.
    pub pids: Vec<u32>,
}

/// Parse a watcher line: `open <pid,pid,…|-> <url>`.
pub fn parse_request(line: &str) -> Option<OpenRequest> {
    let rest = line.strip_prefix("open ")?;
    let (pids, url) = rest.split_once(' ')?;
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    Some(OpenRequest {
        url: url.to_string(),
        pids: pids.split(',').filter_map(|p| p.parse().ok()).collect(),
    })
}

/// What the installer reported.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct InstallReport {
    /// Path of the shim, once it is in place.
    pub shim: Option<String>,
    /// The shim was written just now (not already there).
    pub fresh: bool,
    /// A file that is not nettle's shim sits where the shim would go.
    pub foreign: Option<String>,
    pub error: Option<String>,
    /// Another `xdg-open` on the host's `PATH`.
    pub real: Option<String>,
}

pub fn parse_install(output: &str) -> InstallReport {
    let mut report = InstallReport::default();
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("@@SHIM ") {
            if let Some((state, path)) = rest.split_once(' ') {
                report.fresh = state == "installed";
                report.shim = Some(path.to_string());
            }
        } else if let Some(path) = line.strip_prefix("@@FOREIGN ") {
            report.foreign = Some(path.to_string());
        } else if let Some(msg) = line.strip_prefix("@@ERR ") {
            report.error = Some(msg.to_string());
        } else if let Some(path) = line.strip_prefix("@@REAL ") {
            report.real = Some(path.to_string());
        }
    }
    report
}

/// Only web links are opened. What the string is, is up to the host, and
/// other schemes reach local files and applications.
pub fn check_url(raw: &str) -> std::result::Result<Url, &'static str> {
    if raw.len() > MAX_URL_LEN {
        return Err("the link is too long");
    }
    if raw.chars().any(char::is_control) {
        return Err("the link contains control characters");
    }
    let url = Url::parse(raw).map_err(|_| "it is not a valid URL")?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("only http and https links are opened");
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err("the link has no host");
    }
    Ok(url)
}

/// The link for the activity log: without query and fragment, which carry
/// sign-in state and tokens.
pub fn display_url(url: &Url) -> String {
    let mut out = format!("{}://{}", url.scheme(), url.host_str().unwrap_or_default());
    if let Some(port) = url.port() {
        out.push_str(&format!(":{port}"));
    }
    out.extend(url.path().chars().take(80));
    if url.path().chars().count() > 80 {
        out.push('…');
    }
    if url.query().is_some() || url.fragment().is_some() {
        out.push_str("?…");
    }
    out
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(name)) => name == "localhost" || name.ends_with(".localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback() || ip.is_unspecified(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback() || ip.is_unspecified(),
        None => false,
    }
}

/// Loopback ports of the host a link refers to.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LoopbackPorts {
    /// The link itself points at the host's loopback (`http://localhost:8888/…`).
    pub own: Option<u16>,
    /// Loopback URLs in its query: where a sign-in sends the browser back to.
    pub redirects: Vec<u16>,
}

pub fn loopback_ports(url: &Url) -> LoopbackPorts {
    let mut ports = LoopbackPorts::default();
    if is_loopback(url) {
        ports.own = url.port_or_known_default();
    }
    collect_redirects(url, 0, &mut ports.redirects);
    ports.redirects.sort_unstable();
    ports.redirects.dedup();
    ports.redirects.retain(|p| Some(*p) != ports.own);
    ports
}

fn collect_redirects(url: &Url, depth: u8, out: &mut Vec<u16>) {
    if depth >= MAX_NESTING {
        return;
    }
    for (_, value) in url.query_pairs() {
        let Ok(inner) = Url::parse(value.trim()) else {
            continue;
        };
        if !matches!(inner.scheme(), "http" | "https") {
            continue;
        }
        if is_loopback(&inner) {
            out.extend(inner.port_or_known_default());
        }
        collect_redirects(&inner, depth + 1, out);
    }
}

/// Parse [`PID_SCAN`] output, whichever tool produced it.
pub fn parse_listening(output: &str) -> Vec<RemotePort> {
    let rows = parse::parse_ss(output);
    if rows.is_empty() {
        parse::parse_netstat(output)
    } else {
        rows
    }
}

/// Ports one of `pids` listens on.
pub fn ports_of(rows: &[RemotePort], pids: &[u32]) -> Vec<u16> {
    let mut ports: Vec<u16> = rows
        .iter()
        .filter(|r| r.pid.is_some_and(|pid| pids.contains(&pid)))
        .map(|r| r.port)
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// When the port scanner first saw each listening port. Tells a callback
/// server started for a sign-in from a service that has been up for hours.
#[derive(Default)]
struct PortAges {
    /// Ports of the first scan observed: up for an unknown time, so not new.
    old: HashSet<u16>,
    first_seen: HashMap<u16, Instant>,
    primed: bool,
}

impl PortAges {
    fn observe(&mut self, live: &HashSet<u16>, now: Instant) {
        self.old.retain(|port| live.contains(port));
        self.first_seen.retain(|port, _| live.contains(port));
        if !self.primed {
            self.old = live.clone();
            self.primed = true;
            return;
        }
        for port in live {
            if !self.old.contains(port) {
                self.first_seen.entry(*port).or_insert(now);
            }
        }
    }

    /// The port was opened a moment ago: no scan has seen it yet, or the
    /// first one that did was recent.
    fn is_recent(&self, port: u16, now: Instant) -> bool {
        if self.old.contains(&port) {
            return false;
        }
        match self.first_seen.get(&port) {
            Some(seen) => now.duration_since(*seen) <= RECENT_PORT,
            None => true,
        }
    }
}

/// How the watcher on one connection ended.
enum End {
    /// The connection went away; the next one starts a new watcher.
    LinkDown,
    /// The option was switched off; the watcher that ran until then.
    Disabled(Option<u32>),
    /// The session is gone.
    Gone,
    Failed(String),
}

/// Drops links when the host asks for too many at once.
#[derive(Default)]
struct Throttle {
    opened: VecDeque<Instant>,
    /// A drop was logged for the current burst.
    told: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Admit {
    Yes,
    /// Dropped; `first` for the first one of a burst.
    No {
        first: bool,
    },
}

impl Throttle {
    fn admit(&mut self, now: Instant) -> Admit {
        while self
            .opened
            .front()
            .is_some_and(|t| now.duration_since(*t) >= RATE_WINDOW)
        {
            self.opened.pop_front();
        }
        if self.opened.len() >= RATE_LIMIT {
            let first = !self.told;
            self.told = true;
            return Admit::No { first };
        }
        self.told = false;
        self.opened.push_back(now);
        Admit::Yes
    }
}

struct Opener {
    ui: Arc<UiBridge>,
    host_id: Uuid,
    forwards: Arc<ForwardManager>,
    ports_live_rx: watch::Receiver<HashSet<u16>>,
    /// Remote ports with a forward made here that is still to be removed.
    temporary: StdMutex<HashSet<u16>>,
}

/// Watch the host for `xdg-open` calls while `enabled_rx` says so: install
/// the shim, run the watcher on every connection, and act on what it reports.
pub fn spawn(
    ui: Arc<UiBridge>,
    host_id: Uuid,
    mut epoch_rx: EpochRx,
    forwards: Arc<ForwardManager>,
    ports_live_rx: watch::Receiver<HashSet<u16>>,
    mut enabled_rx: watch::Receiver<bool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut live_rx = ports_live_rx.clone();
        let opener = Arc::new(Opener {
            ui,
            host_id,
            forwards,
            ports_live_rx,
            temporary: StdMutex::new(HashSet::new()),
        });
        let mut ages = PortAges::default();
        let mut throttle = Throttle::default();
        let mut last_failure: Option<String> = None;

        loop {
            // Wait until the option is on and there is a connection.
            let epoch: Arc<ConnectionEpoch> = loop {
                if *enabled_rx.borrow() {
                    if let Some(e) = epoch_rx.borrow().clone() {
                        break e;
                    }
                }
                tokio::select! {
                    r = enabled_rx.changed() => if r.is_err() { return },
                    r = epoch_rx.changed() => if r.is_err() { return },
                }
            };

            let end = opener
                .watch(
                    &epoch,
                    &mut enabled_rx,
                    &mut live_rx,
                    &mut ages,
                    &mut throttle,
                )
                .await;
            match end {
                End::LinkDown => {
                    if epoch_rx.changed().await.is_err() {
                        return;
                    }
                }
                End::Disabled(watcher) => {
                    last_failure = None;
                    // What listens when the option comes back on is not new.
                    ages = PortAges::default();
                    if !epoch.cancel.is_cancelled() {
                        opener.uninstall(&epoch, watcher).await;
                    }
                }
                End::Gone => return,
                End::Failed(why) => {
                    // The same failure after every retry is logged once.
                    if last_failure.as_deref() != Some(why.as_str()) {
                        opener.log(
                            LogLevel::Warn,
                            format!("not opening links from this host: {why}"),
                        );
                        last_failure = Some(why);
                    }
                    tokio::select! {
                        _ = tokio::time::sleep(RETRY) => {}
                        r = enabled_rx.changed() => if r.is_err() { return },
                        r = epoch_rx.changed() => if r.is_err() { return },
                    }
                }
            }
        }
    })
}

impl Opener {
    fn log(&self, level: LogLevel, message: impl Into<String>) {
        self.ui.log(level, Some(self.host_id), "open", message);
    }

    /// Install the shim, then run the watcher until the connection ends, the
    /// option is switched off, or the watcher stops.
    async fn watch(
        self: &Arc<Self>,
        epoch: &Arc<ConnectionEpoch>,
        enabled_rx: &mut watch::Receiver<bool>,
        live_rx: &mut watch::Receiver<HashSet<u16>>,
        ages: &mut PortAges,
        throttle: &mut Throttle,
    ) -> End {
        let install = install_command();
        let installed = tokio::select! {
            _ = epoch.cancel.cancelled() => return End::LinkDown,
            r = tokio::time::timeout(
                EXEC_TIMEOUT,
                exec_capture_stdin(&epoch.handle, &install, SHIM.as_bytes()),
            ) => r,
        };
        let report = match installed {
            Ok(Ok((out, _))) => parse_install(&out),
            Ok(Err(_)) if epoch.cancel.is_cancelled() => return End::LinkDown,
            Ok(Err(e)) => return End::Failed(format!("could not install the xdg-open shim ({e})")),
            Err(_) => return End::Failed("installing the xdg-open shim timed out".into()),
        };
        if let Some(path) = report.foreign {
            return End::Failed(format!(
                "{path} on the host is not nettle's shim and was left alone; \
                 move it away to have xdg-open intercepted"
            ));
        }
        let Some(shim) = report.shim else {
            return End::Failed(
                report
                    .error
                    .unwrap_or_else(|| "the xdg-open shim could not be installed".into()),
            );
        };
        if report.fresh {
            let hint = report
                .real
                .map(|real| {
                    format!(
                        "; the host also has {real}, so the shim only takes over \
                         where ~/.local/bin comes first in PATH"
                    )
                })
                .unwrap_or_default();
            self.log(
                LogLevel::Info,
                format!("installed the xdg-open shim on the host ({shim}){hint}"),
            );
        }

        let opened = tokio::select! {
            _ = epoch.cancel.cancelled() => return End::LinkDown,
            r = open_watcher(epoch) => r,
        };
        let mut channel = match opened {
            Ok(c) => c,
            Err(_) if epoch.cancel.is_cancelled() => return End::LinkDown,
            Err(e) => return End::Failed(format!("could not start the watcher on the host ({e})")),
        };

        // What is listening by now has been for an unknown time. (Later scans
        // arrive through `changed()` below.)
        if !ages.primed {
            let live = live_rx.borrow_and_update().clone();
            // Still empty: no scan yet, the first one primes.
            if !live.is_empty() {
                ages.observe(&live, Instant::now());
            }
        }

        let mut buf: Vec<u8> = Vec::new();
        // Inside an over-long line: drop input up to the next newline.
        let mut skipping = false;
        let mut watcher: Option<u32> = None;
        let mut error: Option<String> = None;
        let mut exit: Option<u32> = None;
        loop {
            tokio::select! {
                _ = epoch.cancel.cancelled() => return End::LinkDown,
                r = enabled_rx.changed() => {
                    if r.is_err() {
                        return End::Gone;
                    }
                    if !*enabled_rx.borrow() {
                        // Closing the channel closes the watcher's stdin: it
                        // cleans up after itself and exits.
                        let _ = channel.close().await;
                        return End::Disabled(watcher);
                    }
                }
                r = live_rx.changed() => {
                    if r.is_ok() {
                        let live = live_rx.borrow_and_update().clone();
                        ages.observe(&live, Instant::now());
                    }
                }
                msg = channel.wait() => match msg {
                    Some(ChannelMsg::Data { data }) => {
                        for byte in data.iter().copied() {
                            if byte != b'\n' {
                                if !skipping {
                                    buf.push(byte);
                                    skipping = buf.len() > MAX_LINE;
                                }
                                continue;
                            }
                            let line = String::from_utf8_lossy(&buf).into_owned();
                            buf.clear();
                            if std::mem::take(&mut skipping) {
                                continue;
                            }
                            if let Some(msg) = line.strip_prefix("@@ERR ") {
                                error = Some(msg.to_string());
                            } else if let Some(pid) = parse_ready(&line) {
                                watcher = Some(pid);
                                self.log(
                                    LogLevel::Info,
                                    "links the host opens with xdg-open now open in the local browser",
                                );
                            } else if let Some(request) = parse_request(&line) {
                                self.handle(epoch, request, ages, throttle).await;
                            }
                        }
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => exit = Some(exit_status),
                    Some(ChannelMsg::Close) | None => {
                        if epoch.cancel.is_cancelled() {
                            return End::LinkDown;
                        }
                        return End::Failed(error.unwrap_or_else(|| format!(
                            "the watcher on the host stopped (exit {})",
                            exit.map(|c| c.to_string()).unwrap_or_else(|| "?".into())
                        )));
                    }
                    Some(_) => {}
                },
            }
        }
    }

    async fn uninstall(&self, epoch: &ConnectionEpoch, watcher: Option<u32>) {
        let removed = tokio::time::timeout(
            EXEC_TIMEOUT,
            exec_capture(&epoch.handle, &uninstall_command(watcher)),
        )
        .await;
        let Ok(Ok((out, _))) = removed else {
            return;
        };
        if let Some(path) = out.lines().find_map(|l| l.strip_prefix("@@REMOVED ")) {
            self.log(
                LogLevel::Info,
                format!("removed the xdg-open shim from the host ({path})"),
            );
        } else if out.lines().any(|l| l.starts_with("@@KEPT ")) {
            self.log(
                LogLevel::Info,
                "left the xdg-open shim on the host: another nettle still uses it",
            );
        }
    }

    /// Forward what the link needs, then open it.
    async fn handle(
        self: &Arc<Self>,
        epoch: &ConnectionEpoch,
        request: OpenRequest,
        ages: &PortAges,
        throttle: &mut Throttle,
    ) {
        match throttle.admit(Instant::now()) {
            Admit::Yes => {}
            Admit::No { first } => {
                if first {
                    self.log(
                        LogLevel::Warn,
                        format!(
                            "the host asks to open more than {RATE_LIMIT} links in {} s; \
                             ignoring the rest of the burst",
                            RATE_WINDOW.as_secs()
                        ),
                    );
                }
                return;
            }
        }
        let mut url = match check_url(&request.url) {
            Ok(url) => url,
            Err(why) => {
                self.log(
                    LogLevel::Warn,
                    format!("ignored a link the host asked to open: {why}"),
                );
                return;
            }
        };

        let refs = loopback_ports(&url);
        let mut wanted = refs.redirects.clone();
        // Ports the calling program opened for this: a callback server whose
        // port the link carries in a form only the sign-in page understands.
        if !request.pids.is_empty() {
            let scan = tokio::select! {
                _ = epoch.cancel.cancelled() => return,
                r = tokio::time::timeout(EXEC_TIMEOUT, exec_capture(&epoch.handle, PID_SCAN)) => r,
            };
            if let Ok(Ok((out, _))) = scan {
                let now = Instant::now();
                for port in ports_of(&parse_listening(&out), &request.pids) {
                    if ages.is_recent(port, now) && !wanted.contains(&port) {
                        wanted.push(port);
                    }
                }
            }
        }
        wanted.retain(|p| *p != 0 && Some(*p) != refs.own);
        wanted.truncate(MAX_PORTS_PER_LINK);

        let mut forwarded: Vec<u16> = Vec::new();
        if let Some(port) = refs.own.filter(|p| *p != 0) {
            // The link is the host's own loopback: any free local port does.
            match self.forward(port, true).await {
                Some(local) => {
                    if local != port {
                        let _ = url.set_port(Some(local));
                    }
                    forwarded.push(local);
                }
                None => {
                    self.log(
                        LogLevel::Warn,
                        format!(
                            "could not forward the host's port {port}; not opening {}",
                            display_url(&url)
                        ),
                    );
                    return;
                }
            }
        }
        for port in wanted {
            forwarded.extend(self.forward(port, false).await);
        }

        let shown = display_url(&url);
        match self.ui.open_url(url.as_str()) {
            Ok(()) => {
                let via = if forwarded.is_empty() {
                    String::new()
                } else {
                    let list: Vec<String> =
                        forwarded.iter().map(|p| format!("localhost:{p}")).collect();
                    format!(" (with {} forwarded to the host)", list.join(", "))
                };
                self.log(
                    LogLevel::Info,
                    format!("opened {shown} in the local browser{via}"),
                );
            }
            Err(e) => self.log(LogLevel::Error, format!("could not open {shown}: {e}")),
        }
    }

    /// Make the host's `port` reachable on this machine and return the local
    /// port. Unless `movable`, that has to be the same number: a page that
    /// redirects to `localhost:port` knows no other.
    async fn forward(self: &Arc<Self>, port: u16, movable: bool) -> Option<u16> {
        let target = Target::host(port);
        // Forwarded already: never touch what is there.
        let existing = self
            .forwards
            .list()
            .into_iter()
            .find(|f| f.sandbox.is_none() && f.port == port);
        if let Some(existing) = existing {
            if existing.local_port != port && !movable {
                self.log(
                    LogLevel::Warn,
                    format!(
                        "the link expects the host's port {port} on localhost:{port}, \
                         but it is forwarded to localhost:{}",
                        existing.local_port
                    ),
                );
                return None;
            }
            return Some(existing.local_port);
        }

        // The same local port may tunnel elsewhere (a sandbox): asking the
        // manager for it would switch that tunnel over.
        let mut local = port;
        if let Some(holder) = self.forwards.local_target(port) {
            if !movable {
                self.log(
                    LogLevel::Warn,
                    format!(
                        "the link expects the host's port {port} on localhost:{port}, \
                         which tunnels to {holder}"
                    ),
                );
                return None;
            }
            local = free_local_port().await?;
        }

        let mut bound = self
            .forwards
            .set_target(target.clone(), local, true, false)
            .await;
        if bound.is_err() && movable && local == port {
            // Taken by another program, or privileged: any other port will do.
            local = free_local_port().await?;
            bound = self
                .forwards
                .set_target(target.clone(), local, true, false)
                .await;
        }
        if bound.is_err() {
            // The forward manager logged why the port could not be bound.
            if !movable {
                self.log(
                    LogLevel::Warn,
                    format!(
                        "localhost:{port} is not available on this machine, so a redirect \
                         to it will not reach the host"
                    ),
                );
            }
            return None;
        }
        self.remove_when_closed(port, local);
        Some(local)
    }

    /// Remove the temporary forward of `port` once the port stops listening
    /// (or never shows up), unless it was pinned or changed meanwhile.
    fn remove_when_closed(self: &Arc<Self>, port: u16, local: u16) {
        if !self.temporary.lock().unwrap().insert(port) {
            return;
        }
        let this = self.clone();
        let mut live = self.ports_live_rx.clone();
        tokio::spawn(async move {
            let listening =
                tokio::time::timeout(APPEAR_GRACE, live.wait_for(|set| set.contains(&port)))
                    .await
                    .map(|r| r.is_ok());
            let closed = match listening {
                Ok(true) => live.wait_for(|set| !set.contains(&port)).await.is_ok(),
                // The session is gone, and its forwards with it.
                Ok(false) => false,
                // Never seen listening: a sign-in that was over within a
                // scan interval, or a port nothing ever opened.
                Err(_) => true,
            };
            this.temporary.lock().unwrap().remove(&port);
            if !closed {
                return;
            }
            let ours = this.forwards.list().into_iter().any(|f| {
                f.sandbox.is_none() && f.port == port && f.local_port == local && !f.pinned
            });
            if ours {
                let _ = this
                    .forwards
                    .set_target(Target::host(port), 0, false, false)
                    .await;
            }
        });
    }
}

async fn open_watcher(epoch: &ConnectionEpoch) -> Result<russh::Channel<russh::client::Msg>> {
    let channel = epoch.handle.channel_open_session().await?;
    channel.exec(true, watch_command()).await?;
    Ok(channel)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        check_url(s).unwrap()
    }

    #[test]
    fn scripts_survive_the_sh_c_wrapper() {
        let uninstall = uninstall_command(Some(4711));
        let uninstall = uninstall
            .strip_suffix(" nettle 4711")
            .expect("the watcher pid is the script's $1");
        assert!(uninstall_command(None).ends_with("' nettle "));
        for cmd in [watch_command(), install_command(), uninstall.to_string()] {
            let body = cmd
                .strip_prefix("exec sh -c '")
                .and_then(|c| c.strip_suffix('\''))
                .expect("wrapped in sh -c");
            assert!(!body.contains('\''), "single quote in: {body}");
            // fish treats `\\` and `\'` in single quotes as escapes.
            assert!(!body.contains("\\\\"), "double backslash in: {body}");
            assert!(
                !body.lines().any(|l| l.trim_start().starts_with('#')),
                "comments are stripped"
            );
        }
    }

    #[test]
    fn only_files_with_the_marker_are_ours() {
        assert!(SHIM.starts_with("#!/bin/sh\n# nettle-xdg-open-shim\n"));
        // The installer, the uninstaller and the shim's own PATH walk all
        // recognise the shim by the same marker.
        for script in [SHIM, INSTALL, UNINSTALL] {
            assert!(script.contains("grep -q nettle-xdg-open-shim"));
        }
        // Shim, watcher and uninstaller agree on where the FIFOs live.
        for script in [SHIM, WATCHER, UNINSTALL] {
            assert!(script.contains("d=/tmp/nettle-$(id -u)"));
        }
    }

    #[test]
    fn parses_requests() {
        assert_eq!(
            parse_request("open 812,640,601 https://example.com/a?b=c d"),
            Some(OpenRequest {
                url: "https://example.com/a?b=c d".into(),
                pids: vec![812, 640, 601],
            })
        );
        assert_eq!(
            parse_request("open - http://localhost:3000/").unwrap().pids,
            Vec::<u32>::new()
        );
        assert_eq!(parse_request("open 1,x,3 http://a/").unwrap().pids, [1, 3]);
        assert_eq!(parse_request("open - "), None);
        assert_eq!(parse_request("open"), None);
        assert_eq!(parse_request("eof"), None);
        assert_eq!(parse_request("@@READY 268"), None);
        assert_eq!(parse_ready("@@READY 268"), Some(268));
        assert_eq!(parse_ready("@@READY"), None);
        assert_eq!(parse_ready("open - http://a/"), None);
    }

    #[test]
    fn parses_install_reports() {
        let fresh = parse_install(
            "@@SHIM installed /home/u/.local/bin/xdg-open\n@@REAL /usr/bin/xdg-open\n",
        );
        assert!(fresh.fresh);
        assert_eq!(fresh.shim.as_deref(), Some("/home/u/.local/bin/xdg-open"));
        assert_eq!(fresh.real.as_deref(), Some("/usr/bin/xdg-open"));

        let current = parse_install("@@SHIM current /home/u/.local/bin/xdg-open\n");
        assert!(!current.fresh);
        assert!(current.shim.is_some() && current.real.is_none());

        let foreign = parse_install("@@FOREIGN /home/u/.local/bin/xdg-open\n");
        assert_eq!(
            foreign.foreign.as_deref(),
            Some("/home/u/.local/bin/xdg-open")
        );
        assert!(foreign.shim.is_none());

        let failed = parse_install("@@ERR cannot write /home/u/.local/bin/xdg-open\n");
        assert!(failed.shim.is_none());
        assert!(failed.error.unwrap().starts_with("cannot write"));
        assert_eq!(parse_install("motd noise\n"), InstallReport::default());
    }

    #[test]
    fn only_web_links_pass() {
        assert!(check_url("https://github.com/login/device").is_ok());
        assert!(check_url("http://localhost:8888/?token=abc").is_ok());
        for bad in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ftp://example.com/x",
            "ssh://example.com",
            "vscode://file/etc/passwd",
            "mailto:a@example.com",
            "/etc/passwd",
            "--version",
            "example.com",
            "http://",
            "https://exa\u{7}mple.com/",
            "https://example.com/\u{1b}[31m",
        ] {
            assert!(check_url(bad).is_err(), "{bad:?} must be refused");
        }
        let long = format!("https://example.com/?q={}", "a".repeat(MAX_URL_LEN));
        assert!(check_url(&long).is_err());
    }

    #[test]
    fn finds_the_login_redirect_port() {
        // gcloud-style: the callback is a query parameter.
        let google = url(
            "https://accounts.google.com/o/oauth2/auth?response_type=code\
             &client_id=x.apps.googleusercontent.com\
             &redirect_uri=http%3A%2F%2Flocalhost%3A8085%2F&scope=openid&state=abc",
        );
        assert_eq!(
            loopback_ports(&google),
            LoopbackPorts {
                own: None,
                redirects: vec![8085],
            }
        );
        // aws sso / az: 127.0.0.1 and an ephemeral port.
        let aws = url(
            "https://oidc.eu-west-1.amazonaws.com/authorize?response_type=code\
             &redirect_uri=http%3A%2F%2F127.0.0.1%3A41763%2Foauth%2Fcallback&code_challenge=y",
        );
        assert_eq!(loopback_ports(&aws).redirects, [41763]);
        let v6 = url("https://id.example/auth?redirect_uri=http%3A%2F%2F%5B%3A%3A1%5D%3A9005%2Fcb");
        assert_eq!(loopback_ports(&v6).redirects, [9005]);
        // A sign-in page that wraps the real request in a `continue` link.
        let nested = url(
            "https://sso.example/login?continue=https%3A%2F%2Fidp.example%2Fauth\
             %3Fredirect_uri%3Dhttp%253A%252F%252Flocalhost%253A18000%252Fcb",
        );
        assert_eq!(loopback_ports(&nested).redirects, [18000]);
        // No port given: the scheme's default.
        let bare = url("https://id.example/auth?redirect_uri=http%3A%2F%2Flocalhost%2Fcb");
        assert_eq!(loopback_ports(&bare).redirects, [80]);
    }

    #[test]
    fn a_link_to_the_hosts_loopback_is_its_own_port() {
        let jupyter = url("http://localhost:8888/tree?token=abc");
        assert_eq!(
            loopback_ports(&jupyter),
            LoopbackPorts {
                own: Some(8888),
                redirects: vec![],
            }
        );
        assert_eq!(
            loopback_ports(&url("http://127.0.0.1:5173/")).own,
            Some(5173)
        );
        assert_eq!(loopback_ports(&url("http://[::1]:3000/")).own, Some(3000));
        assert_eq!(loopback_ports(&url("http://0.0.0.0:8000/")).own, Some(8000));
        assert_eq!(
            loopback_ports(&url("http://app.localhost:4000/")).own,
            Some(4000)
        );
        assert_eq!(loopback_ports(&url("https://localhost/")).own, Some(443));
        // Its own port is not listed again as a redirect.
        let both = url(
            "http://localhost:4180/start?rd=http%3A%2F%2Flocalhost%3A4180%2Fdone\
             &next=http%3A%2F%2Flocalhost%3A3000%2F",
        );
        assert_eq!(
            loopback_ports(&both),
            LoopbackPorts {
                own: Some(4180),
                redirects: vec![3000],
            }
        );
    }

    #[test]
    fn other_hosts_and_schemes_are_not_loopback() {
        for link in [
            "https://example.com/?redirect_uri=https%3A%2F%2Fapp.example.com%2Fcb",
            "https://localhost.example.com:8443/",
            "https://example.com/?next=localhost%3A8080",
            "https://example.com/?port=8085",
            "https://example.com/?u=ftp%3A%2F%2Flocalhost%3A2121%2F",
            "https://example.com/#redirect_uri=http%3A%2F%2Flocalhost%3A8085",
        ] {
            assert_eq!(
                loopback_ports(&url(link)),
                LoopbackPorts::default(),
                "{link}"
            );
        }
    }

    #[test]
    fn the_log_shows_links_without_their_secrets() {
        assert_eq!(
            display_url(&url(
                "https://user:pw@id.example:8443/auth?state=s3cret#frag"
            )),
            "https://id.example:8443/auth?…"
        );
        assert_eq!(
            display_url(&url("https://example.com/docs")),
            "https://example.com/docs"
        );
        let long = url(&format!("https://example.com/{}", "a".repeat(300)));
        assert!(display_url(&long).chars().count() < 110);
    }

    #[test]
    fn picks_the_ports_of_the_calling_processes() {
        let ss = "State  Recv-Q Send-Q Local Address:Port Peer Address:Port Process\n\
LISTEN 0      128        127.0.0.1:8085       0.0.0.0:*     users:((\"gcloud\",pid=812,fd=5))\n\
LISTEN 0      511                *:5173             *:*     users:((\"node\",pid=640,fd=20))\n\
LISTEN 0      128          0.0.0.0:22         0.0.0.0:*\n\
LISTEN 0      128             [::1]:8085          [::]:*     users:((\"gcloud\",pid=812,fd=6))\n";
        let rows = parse_listening(ss);
        assert_eq!(ports_of(&rows, &[900, 812]), [8085]);
        assert_eq!(ports_of(&rows, &[640, 812]), [5173, 8085]);
        assert!(ports_of(&rows, &[1]).is_empty());
        assert!(ports_of(&rows, &[]).is_empty());

        // busybox / net-tools netstat when the host has no ss.
        let netstat = "Active Internet connections (only servers)\n\
Proto Recv-Q Send-Q Local Address           Foreign Address         State       PID/Program name\n\
tcp        0      0 0.0.0.0:2222            0.0.0.0:*               LISTEN      -\n\
tcp        0      0 0.0.0.0:43299           0.0.0.0:*               LISTEN      297/nc\n";
        assert_eq!(ports_of(&parse_listening(netstat), &[297]), [43299]);
        assert!(parse_listening("").is_empty());
    }

    #[test]
    fn only_freshly_opened_ports_count_as_recent() {
        let t0 = Instant::now();
        let mut ages = PortAges::default();
        // Before any scan nothing is known: give the port the benefit.
        assert!(ages.is_recent(8085, t0));

        // What the first scan finds has been up for who knows how long.
        ages.observe(&HashSet::from([22, 5173]), t0);
        assert!(!ages.is_recent(22, t0));
        assert!(!ages.is_recent(5173, t0));
        // Opened since, not scanned yet.
        assert!(ages.is_recent(8085, t0));

        let t1 = t0 + Duration::from_secs(3);
        ages.observe(&HashSet::from([22, 5173, 8085]), t1);
        assert!(ages.is_recent(8085, t1 + RECENT_PORT));
        assert!(!ages.is_recent(8085, t1 + RECENT_PORT + Duration::from_secs(1)));
        assert!(!ages.is_recent(5173, t1));

        // A service that went away and came back is new again.
        let t2 = t1 + Duration::from_secs(600);
        ages.observe(&HashSet::from([22, 8085]), t2);
        ages.observe(
            &HashSet::from([22, 5173, 8085]),
            t2 + Duration::from_secs(3),
        );
        assert!(ages.is_recent(5173, t2 + Duration::from_secs(4)));
        assert!(!ages.is_recent(8085, t2 + Duration::from_secs(4)));
    }

    #[test]
    fn a_burst_of_links_is_cut_off() {
        let t0 = Instant::now();
        let mut throttle = Throttle::default();
        for _ in 0..RATE_LIMIT {
            assert_eq!(throttle.admit(t0), Admit::Yes);
        }
        assert_eq!(throttle.admit(t0), Admit::No { first: true });
        assert_eq!(throttle.admit(t0), Admit::No { first: false });
        // The window slides: once the burst is old, links open again.
        let later = t0 + RATE_WINDOW;
        assert_eq!(throttle.admit(later), Admit::Yes);
        for _ in 1..RATE_LIMIT {
            assert_eq!(throttle.admit(later), Admit::Yes);
        }
        assert_eq!(throttle.admit(later), Admit::No { first: true });
    }
}
