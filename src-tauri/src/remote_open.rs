//! Links the host asks to open (`xdg-open`), opened on this machine.
//!
//! On a headless host `xdg-open https://…` leads nowhere, so command-line
//! logins that want a browser (`gh auth login`, `aws sso login`, an MCP
//! server's OAuth, …) fail or fall back to copying codes around. With the
//! per-host option on, nettle does the following on the host and in each of
//! its running Docker sandboxes (a *place*):
//!
//! 1. installs a small `xdg-open` shim in `~/.local/bin` (and a `www-browser`
//!    link to it, which is what tools fall back to without a display),
//! 2. keeps a *watcher* running there: a shell loop reading a FIFO in a
//!    private directory under `/tmp`, which the shim writes to. The host's
//!    runs on an SSH exec channel of its own, the sandboxes' (through
//!    `sbx exec`) share a second one,
//! 3. opens each http(s) link in the local browser, after forwarding the
//!    loopback ports it needs: the ones the link names (`http://localhost:8888/…`,
//!    `redirect_uri=http://127.0.0.1:8085/cb`) and the ones the calling program
//!    just started listening on (callback servers whose port is not in the link).
//!
//! Those forwards are temporary: each is removed once its remote port stops
//! listening. While nettle is away the shim hands over to the place's real
//! `xdg-open`, so nothing changes for other sessions.
//!
//! A sign-in redirects to a fixed `localhost:PORT`, so the local port has to
//! be that number. When it already tunnels elsewhere (the same port of another
//! sandbox, say) the forward is left alone, unless the host's "take over"
//! option says a link may have it: then the port is switched to where the link
//! came from, like *⇄ switch here* in the ports tab.
//!
//! The host is not trusted with more than a browser tab: only `http` and
//! `https` links are opened, at a bounded rate.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use russh::ChannelMsg;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

use crate::config::HostConfig;
use crate::error::Result;
use crate::ipc::types::{LogLevel, RemotePort};
use crate::ports::forwards::{free_local_port, ForwardManager, Target};
use crate::ports::parse;
use crate::ports::sandbox::{self, SandboxPorts};
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

/// The `xdg-open` shim, written to `~/.local/bin/xdg-open` at each place. The
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
new="$f.nettle-new.$$"
if mkdir -p "$bin" && cat > "$new" && chmod 755 "$new"; then
  if cmp -s "$new" "$f" 2>/dev/null; then
    rm -f "$new"
    echo "@@SHIM current $f"
  elif mv -f "$new" "$f"; then
    echo "@@SHIM installed $f"
  else
    rm -f "$new"
    echo "@@ERR cannot write $f"
    exit 0
  fi
else
  rm -f "$new"
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
/// place. `$own` is the pid of the caller's own watcher, which is on its way
/// out; [`uninstall_command`] sets it.
const UNINSTALL: &str = r#"
d=/tmp/nettle-$(id -u)
f="$HOME/.local/bin/xdg-open"
if [ -d "$d" ] && [ ! -L "$d" ] && [ -O "$d" ]; then
  for other in "$d"/open.*.fifo; do
    p=${other##*/open.}
    p=${p%.fifo}
    case $p in ""|*[!0-9]*) continue ;; esac
    [ "$p" = "$own" ] && continue
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

/// Where `xdg-open` is intercepted: on the host (`None`), or in one of its
/// running Docker sandboxes.
pub type Place = Option<String>;

fn describe(place: &Place) -> String {
    match place {
        None => "the host".into(),
        Some(name) => format!("sandbox {name}"),
    }
}

/// What a session wants from this module. Follows the host's settings while
/// the session is live.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// Intercept `xdg-open` on the host and in its running sandboxes.
    pub enabled: bool,
    /// A link may take over a local port that already tunnels elsewhere.
    pub take_over: bool,
}

impl From<&HostConfig> for Options {
    fn from(host: &HostConfig) -> Self {
        Self {
            enabled: host.intercept_xdg_open,
            take_over: host.links_take_over_ports,
        }
    }
}

/// A script without its comments and blank lines: it travels inside single
/// quotes.
fn body(script: &str) -> String {
    let lines: Vec<&str> = script
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .collect();
    lines.join("\n")
}

/// `script` as a command line for the host's login shell (whatever that is)
/// that runs it at `place`. With `stdin`, the channel's stdin reaches it.
fn command(place: &Place, script: &str, stdin: bool) -> String {
    let body = body(script);
    match place {
        None => format!("exec sh -c '{body}'"),
        Some(name) if stdin => sandbox::exec_stdin_command(name, &body),
        Some(name) => sandbox::exec_in_command(name, &body),
    }
}

/// Pid of the watcher from its `@@READY <pid>` line.
fn parse_ready(line: &str) -> Option<u32> {
    line.strip_prefix("@@READY ")?.trim().parse().ok()
}

pub fn watch_command(place: &Place) -> String {
    command(place, WATCHER, true)
}

/// The watchers of several sandboxes on one channel. Each line of their
/// output arrives as `@<sandbox> <line>`; `@<sandbox> @@END` when one stops.
pub fn hub_command(sandboxes: &[String]) -> String {
    let names: Vec<&str> = sandboxes.iter().map(String::as_str).collect();
    sandbox::exec_each_command(&names, &body(WATCHER))
}

/// Split `@<sandbox> <line>` as [`hub_command`] prints it.
fn parse_tagged(line: &str) -> Option<(&str, &str)> {
    line.strip_prefix('@')?.split_once(' ')
}

/// Expects the shim ([`shim`]) on its stdin.
pub fn install_command(place: &Place) -> String {
    command(place, INSTALL, true)
}

/// `own_watcher`: the pid of the watcher that was just told to stop.
pub fn uninstall_command(place: &Place, own_watcher: Option<u32>) -> String {
    let own = own_watcher.map(|pid| pid.to_string()).unwrap_or_default();
    command(place, &format!("own={own}\n{UNINSTALL}"), false)
}

/// The shim file's contents (for the installer's stdin).
pub fn shim() -> &'static str {
    SHIM
}

/// One `xdg-open` call at a place, as the shim reports it.
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

/// Loopback ports of its place a link refers to.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LoopbackPorts {
    /// The link itself points at the place's loopback (`http://localhost:8888/…`).
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

/// How a watcher ended.
enum End {
    /// The connection went away; the next one starts new watchers.
    LinkDown,
    /// Told to stop: the option went off, or the session is going.
    Stopped,
    Failed(String),
}

/// Why the watchers of one connection were wound up.
enum Supervised {
    LinkDown,
    /// The option was switched off.
    Disabled,
    /// The session is gone.
    Gone,
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

/// What to do about one port a link needs on this machine.
#[derive(Debug, PartialEq, Eq)]
enum Plan {
    /// Reachable already, on this local port.
    Use(u16),
    /// Forward it to the local port of the same number.
    Bind,
    /// Forward it to any free local port: the link is rewritten to match.
    BindFree,
    /// Point the local port of its number at it, dropping the forward that
    /// holds that port (or moving its own forward there).
    TakeOver,
    /// It needs a local port that is taken and may not be had.
    Blocked,
}

/// `existing`: the local port the target is forwarded to, if it is. `held`:
/// the local port of its number tunnels to another target. `movable`: the
/// link can be rewritten to another port (it points at the port itself).
fn plan(port: u16, existing: Option<u16>, held: bool, movable: bool, take_over: bool) -> Plan {
    match existing {
        Some(local) if local == port || movable => Plan::Use(local),
        Some(_) if take_over => Plan::TakeOver,
        Some(_) => Plan::Blocked,
        None if !held => Plan::Bind,
        None if take_over => Plan::TakeOver,
        // The same number keeps the page's origin, but any port beats none.
        None if movable => Plan::BindFree,
        None => Plan::Blocked,
    }
}

/// A watcher channel's output as lines; an over-long line is dropped whole.
#[derive(Default)]
struct Lines {
    buf: Vec<u8>,
    /// Inside an over-long line: drop input up to the next newline.
    skipping: bool,
}

impl Lines {
    fn feed(&mut self, data: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        for byte in data.iter().copied() {
            if byte != b'\n' {
                if !self.skipping {
                    self.buf.push(byte);
                    self.skipping = self.buf.len() > MAX_LINE;
                }
                continue;
            }
            let line = String::from_utf8_lossy(&self.buf).into_owned();
            self.buf.clear();
            if !std::mem::take(&mut self.skipping) {
                lines.push(line);
            }
        }
        lines
    }
}

/// The listening ports of one place, as its scanner last saw them.
enum LivePorts {
    Host(watch::Receiver<HashSet<u16>>),
    Sandbox(watch::Receiver<SandboxPorts>, String),
}

impl LivePorts {
    fn now(&mut self) -> HashSet<u16> {
        match self {
            LivePorts::Host(rx) => rx.borrow_and_update().clone(),
            LivePorts::Sandbox(rx, name) => rx
                .borrow_and_update()
                .get(name.as_str())
                .map(|ports| ports.keys().copied().collect())
                .unwrap_or_default(),
        }
    }

    /// Wait for the next scan result; `false` once the scanner is gone.
    async fn changed(&mut self) -> bool {
        match self {
            LivePorts::Host(rx) => rx.changed().await.is_ok(),
            LivePorts::Sandbox(rx, _) => rx.changed().await.is_ok(),
        }
    }

    /// Wait until the ports satisfy `pred`; `false` once the scanner is gone.
    async fn wait_for(&mut self, pred: impl Fn(&HashSet<u16>) -> bool) -> bool {
        loop {
            if pred(&self.now()) {
                return true;
            }
            if !self.changed().await {
                return false;
            }
        }
    }
}

/// The sandboxes to watch: the running ones, by name. Never a stopped one:
/// `sbx exec` would boot it.
fn running_sandboxes(scan: &SandboxPorts) -> Vec<String> {
    let mut names: Vec<String> = scan
        .keys()
        .filter(|name| sandbox::valid_name(name))
        .cloned()
        .collect();
    names.sort();
    names
}

/// What is known about one running sandbox.
#[derive(Default)]
struct SandboxWatch {
    /// The shim is in place and its watcher is (to be) part of the shared one.
    watched: bool,
    /// Pid of its watcher, once it reported in.
    watcher: Option<u32>,
    /// It was logged that links from here open locally.
    announced: bool,
    ages: PortAges,
    /// Its last failure; each one is logged once.
    failure: Option<String>,
}

/// The sandbox side of one connection.
struct Sandboxes {
    scans: watch::Receiver<SandboxPorts>,
    /// The sandbox scanner is still there.
    scanning: bool,
    known: HashMap<String, SandboxWatch>,
    /// Last failure of the shared watcher; logged once.
    failure: Option<String>,
}

/// How one run of the sandboxes' shared watcher ended.
enum HubEnd {
    LinkDown,
    Stopped,
    /// The set of sandboxes changed, or it is time to try again.
    Again,
}

type Channel = russh::Channel<russh::client::Msg>;

struct Opener {
    ui: Arc<UiBridge>,
    host_id: Uuid,
    forwards: Arc<ForwardManager>,
    ports_live_rx: watch::Receiver<HashSet<u16>>,
    sandbox_live_rx: watch::Receiver<SandboxPorts>,
    options_rx: watch::Receiver<Options>,
    /// One budget for the host and its sandboxes together.
    throttle: StdMutex<Throttle>,
    /// Targets with a forward made here that is still to be removed.
    temporary: StdMutex<HashSet<Target>>,
}

/// Watch the host and its running sandboxes for `xdg-open` calls while
/// `options_rx` says so: install the shim, run the watchers on every
/// connection, and act on what they report.
pub fn spawn(
    ui: Arc<UiBridge>,
    host_id: Uuid,
    mut epoch_rx: EpochRx,
    forwards: Arc<ForwardManager>,
    ports_live_rx: watch::Receiver<HashSet<u16>>,
    sandbox_live_rx: watch::Receiver<SandboxPorts>,
    mut options_rx: watch::Receiver<Options>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let opener = Arc::new(Opener {
            ui,
            host_id,
            forwards,
            ports_live_rx,
            sandbox_live_rx,
            options_rx: options_rx.clone(),
            throttle: StdMutex::new(Throttle::default()),
            temporary: StdMutex::new(HashSet::new()),
        });
        loop {
            // Wait until the option is on and there is a connection.
            let epoch: Arc<ConnectionEpoch> = loop {
                if options_rx.borrow().enabled {
                    if let Some(e) = epoch_rx.borrow().clone() {
                        break e;
                    }
                }
                tokio::select! {
                    r = options_rx.changed() => if r.is_err() { return },
                    r = epoch_rx.changed() => if r.is_err() { return },
                }
            };
            match opener.supervise(&epoch, &mut options_rx).await {
                Supervised::LinkDown => {
                    if epoch_rx.changed().await.is_err() {
                        return;
                    }
                }
                Supervised::Disabled => {}
                Supervised::Gone => return,
            }
        }
    })
}

impl Opener {
    fn log(&self, level: LogLevel, message: impl Into<String>) {
        self.ui.log(level, Some(self.host_id), "open", message);
    }

    /// Log a failure at `place`, unless it is the one logged last.
    fn note_failure(&self, place: &Place, last: &mut Option<String>, why: String) {
        if last.as_deref() != Some(why.as_str()) {
            self.log(
                LogLevel::Warn,
                format!("not opening links from {}: {why}", describe(place)),
            );
            *last = Some(why);
        }
    }

    fn live_ports(&self, place: &Place) -> LivePorts {
        match place {
            None => LivePorts::Host(self.ports_live_rx.clone()),
            Some(name) => LivePorts::Sandbox(self.sandbox_live_rx.clone(), name.clone()),
        }
    }

    /// Run the watchers for as long as this connection lasts and the option
    /// stays on: one channel for the host, one shared by all its sandboxes
    /// (an sshd allows ten sessions per connection by default, and tunnels
    /// into sandboxes need theirs).
    async fn supervise(
        self: &Arc<Self>,
        epoch: &Arc<ConnectionEpoch>,
        options_rx: &mut watch::Receiver<Options>,
    ) -> Supervised {
        let stop = CancellationToken::new();
        // Also when this is dropped half-way (the session is torn down).
        let stopper = stop.clone().drop_guard();
        let uninstall = Arc::new(AtomicBool::new(false));
        let host = tokio::spawn(self.clone().run_host(
            epoch.clone(),
            stop.clone(),
            uninstall.clone(),
        ));
        let sandboxes = tokio::spawn(self.clone().run_sandboxes(
            epoch.clone(),
            stop.clone(),
            uninstall.clone(),
        ));
        let outcome = loop {
            tokio::select! {
                _ = epoch.cancel.cancelled() => break Supervised::LinkDown,
                r = options_rx.changed() => {
                    if r.is_err() {
                        break Supervised::Gone;
                    }
                    if !options_rx.borrow().enabled {
                        break Supervised::Disabled;
                    }
                }
            }
        };
        let off = matches!(outcome, Supervised::Disabled);
        uninstall.store(off, Ordering::SeqCst);
        drop(stopper);
        if off {
            // The shims are gone by the time the option reads as off.
            let _ = tokio::time::timeout(EXEC_TIMEOUT * 3, async {
                let _ = host.await;
                let _ = sandboxes.await;
            })
            .await;
        }
        outcome
    }

    /// Watch the host until told to stop or the connection ends, starting
    /// the watcher again when it fails.
    async fn run_host(
        self: Arc<Self>,
        epoch: Arc<ConnectionEpoch>,
        stop: CancellationToken,
        uninstall: Arc<AtomicBool>,
    ) {
        let place: Place = None;
        let mut live = self.live_ports(&place);
        let mut ages = PortAges::default();
        let mut watcher: Option<u32> = None;
        let mut last_failure: Option<String> = None;
        loop {
            let end = self
                .watch_host(&epoch, &stop, &mut live, &mut ages, &mut watcher)
                .await;
            match end {
                End::LinkDown => return,
                End::Stopped => break,
                End::Failed(why) => {
                    self.note_failure(&place, &mut last_failure, why);
                    tokio::select! {
                        _ = tokio::time::sleep(RETRY) => {}
                        _ = stop.cancelled() => break,
                        _ = epoch.cancel.cancelled() => return,
                    }
                }
            }
        }
        if uninstall.load(Ordering::SeqCst) && !epoch.cancel.is_cancelled() {
            self.uninstall(&place, &epoch, watcher).await;
        }
    }

    /// Put the shim in place at `place`.
    async fn install(
        &self,
        place: &Place,
        epoch: &ConnectionEpoch,
        stop: &CancellationToken,
    ) -> std::result::Result<(), End> {
        let install = install_command(place);
        let installed = tokio::select! {
            _ = epoch.cancel.cancelled() => return Err(End::LinkDown),
            _ = stop.cancelled() => return Err(End::Stopped),
            r = tokio::time::timeout(
                EXEC_TIMEOUT,
                exec_capture_stdin(&epoch.handle, &install, SHIM.as_bytes()),
            ) => r,
        };
        let report = match installed {
            Ok(Ok((out, _))) => parse_install(&out),
            Ok(Err(_)) if epoch.cancel.is_cancelled() => return Err(End::LinkDown),
            Ok(Err(e)) => {
                return Err(End::Failed(format!(
                    "could not install the xdg-open shim ({e})"
                )))
            }
            Err(_) => return Err(End::Failed("installing the xdg-open shim timed out".into())),
        };
        if let Some(path) = report.foreign {
            return Err(End::Failed(format!(
                "{path} there is not nettle's shim and was left alone; \
                 move it away to have xdg-open intercepted"
            )));
        }
        let Some(shim) = report.shim else {
            return Err(End::Failed(report.error.unwrap_or_else(|| {
                "the xdg-open shim could not be installed".into()
            })));
        };
        if report.fresh {
            let hint = report
                .real
                .map(|real| {
                    format!(
                        "; there is also {real}, so the shim only takes over \
                         where ~/.local/bin comes first in PATH"
                    )
                })
                .unwrap_or_default();
            self.log(
                LogLevel::Info,
                format!(
                    "installed the xdg-open shim on {} ({shim}){hint}",
                    describe(place)
                ),
            );
        }
        Ok(())
    }

    /// Install the shim on the host, then run its watcher until the
    /// connection ends, `stop` is cancelled, or the watcher stops.
    async fn watch_host(
        self: &Arc<Self>,
        epoch: &Arc<ConnectionEpoch>,
        stop: &CancellationToken,
        live: &mut LivePorts,
        ages: &mut PortAges,
        watcher: &mut Option<u32>,
    ) -> End {
        let place: Place = None;
        if let Err(end) = self.install(&place, epoch, stop).await {
            return end;
        }
        let opened = tokio::select! {
            _ = epoch.cancel.cancelled() => return End::LinkDown,
            _ = stop.cancelled() => return End::Stopped,
            r = open_channel(epoch, watch_command(&place)) => r,
        };
        let mut channel = match opened {
            Ok(c) => c,
            Err(_) if epoch.cancel.is_cancelled() => return End::LinkDown,
            Err(e) => return End::Failed(format!("could not start the watcher ({e})")),
        };

        // What is listening by now has been for an unknown time. With nothing
        // listed yet the first scan is still out, and primes when it arrives.
        if !ages.primed {
            let now = live.now();
            if !now.is_empty() {
                ages.observe(&now, Instant::now());
            }
        }

        let mut lines = Lines::default();
        let mut scanning = true;
        let mut error: Option<String> = None;
        let mut exit: Option<u32> = None;
        loop {
            tokio::select! {
                _ = epoch.cancel.cancelled() => return End::LinkDown,
                _ = stop.cancelled() => {
                    // Closing the channel closes the watcher's stdin: it
                    // cleans up after itself and exits.
                    let _ = channel.close().await;
                    return End::Stopped;
                }
                more = live.changed(), if scanning => {
                    scanning = more;
                    if more {
                        ages.observe(&live.now(), Instant::now());
                    }
                }
                msg = channel.wait() => match msg {
                    Some(ChannelMsg::Data { data }) => {
                        for line in lines.feed(&data) {
                            if let Some(msg) = line.strip_prefix("@@ERR ") {
                                error = Some(msg.to_string());
                            } else if let Some(pid) = parse_ready(&line) {
                                *watcher = Some(pid);
                                self.log(
                                    LogLevel::Info,
                                    "links opened with xdg-open on the host now open in the local browser",
                                );
                            } else if let Some(request) = parse_request(&line) {
                                self.handle(&place, epoch, request, ages).await;
                            }
                        }
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => exit = Some(exit_status),
                    Some(ChannelMsg::Close) | None => {
                        if epoch.cancel.is_cancelled() {
                            return End::LinkDown;
                        }
                        return End::Failed(error.unwrap_or_else(|| format!(
                            "the watcher stopped (exit {})",
                            exit.map(|c| c.to_string()).unwrap_or_else(|| "?".into())
                        )));
                    }
                    Some(_) => {}
                },
            }
        }
    }

    /// Watch the host's running sandboxes until told to stop or the
    /// connection ends: the shim goes into each one, and one channel carries
    /// all their watchers. It is started anew when the set of sandboxes changes.
    async fn run_sandboxes(
        self: Arc<Self>,
        epoch: Arc<ConnectionEpoch>,
        stop: CancellationToken,
        uninstall: Arc<AtomicBool>,
    ) {
        let mut sandboxes = Sandboxes {
            scans: self.sandbox_live_rx.clone(),
            scanning: true,
            known: HashMap::new(),
            failure: None,
        };
        loop {
            let running = running_sandboxes(&sandboxes.scans.borrow_and_update());
            sandboxes.known.retain(|name, _| running.contains(name));
            for name in &running {
                sandboxes.known.entry(name.clone()).or_default();
            }
            match self
                .install_in_sandboxes(&mut sandboxes, &epoch, &stop)
                .await
            {
                Ok(_) => {}
                Err(End::LinkDown) => return,
                Err(_) => break,
            }
            match self.hub(&mut sandboxes, &epoch, &stop).await {
                HubEnd::LinkDown => return,
                HubEnd::Stopped => break,
                HubEnd::Again => {}
            }
        }
        if !uninstall.load(Ordering::SeqCst) {
            return;
        }
        for (name, state) in &sandboxes.known {
            // Not in one that stopped meanwhile: `sbx exec` would boot it.
            let running = self.sandbox_live_rx.borrow().contains_key(name);
            if running && !epoch.cancel.is_cancelled() {
                self.uninstall(&Some(name.clone()), &epoch, state.watcher)
                    .await;
            }
        }
    }

    /// Install the shim in every known sandbox that is not watched yet.
    /// Returns whether one was added.
    async fn install_in_sandboxes(
        &self,
        sandboxes: &mut Sandboxes,
        epoch: &ConnectionEpoch,
        stop: &CancellationToken,
    ) -> std::result::Result<bool, End> {
        let mut added = false;
        for (name, state) in sandboxes.known.iter_mut().filter(|(_, s)| !s.watched) {
            let place = Some(name.clone());
            match self.install(&place, epoch, stop).await {
                Ok(()) => {
                    state.watched = true;
                    added = true;
                    // A sandbox is only listed once it was scanned: what
                    // listens in it now has been there for an unknown time.
                    state.ages = PortAges::default();
                    state
                        .ages
                        .observe(&self.live_ports(&place).now(), Instant::now());
                }
                Err(End::Failed(why)) => self.note_failure(&place, &mut state.failure, why),
                Err(end) => return Err(end),
            }
        }
        Ok(added)
    }

    /// Run the shared watcher of the watched sandboxes until the set of
    /// running sandboxes changes, it has to be tried again, or it is stopped.
    async fn hub(
        self: &Arc<Self>,
        sandboxes: &mut Sandboxes,
        epoch: &Arc<ConnectionEpoch>,
        stop: &CancellationToken,
    ) -> HubEnd {
        let mut watched: Vec<String> = sandboxes
            .known
            .iter()
            .filter(|(_, state)| state.watched)
            .map(|(name, _)| name.clone())
            .collect();
        watched.sort();
        let mut channel: Option<Channel> = None;
        if !watched.is_empty() {
            let opened = tokio::select! {
                _ = epoch.cancel.cancelled() => return HubEnd::LinkDown,
                _ = stop.cancelled() => return HubEnd::Stopped,
                r = open_channel(epoch, hub_command(&watched)) => r,
            };
            match opened {
                Ok(c) => channel = Some(c),
                Err(_) if epoch.cancel.is_cancelled() => return HubEnd::LinkDown,
                Err(e) => {
                    let why = format!("could not start the watcher in the sandboxes ({e})");
                    self.log_hub_failure(sandboxes, why);
                }
            }
        }

        let mut lines = Lines::default();
        let retry = tokio::time::sleep(RETRY);
        tokio::pin!(retry);
        loop {
            // Something to try again: a sandbox without its shim or watcher,
            // or the shared watcher itself.
            let pending = sandboxes.known.values().any(|state| !state.watched)
                || (channel.is_none() && !watched.is_empty());
            tokio::select! {
                _ = epoch.cancel.cancelled() => return HubEnd::LinkDown,
                _ = stop.cancelled() => {
                    close(&mut channel).await;
                    return HubEnd::Stopped;
                }
                _ = &mut retry, if pending => {
                    // Start anew only if that gets a sandbox (back) in.
                    let added = match self.install_in_sandboxes(sandboxes, epoch, stop).await {
                        Ok(added) => added,
                        Err(End::LinkDown) => return HubEnd::LinkDown,
                        Err(_) => {
                            close(&mut channel).await;
                            return HubEnd::Stopped;
                        }
                    };
                    if added || (channel.is_none() && !watched.is_empty()) {
                        close(&mut channel).await;
                        return HubEnd::Again;
                    }
                    retry.as_mut().reset(tokio::time::Instant::now() + RETRY);
                }
                more = sandboxes.scans.changed(), if sandboxes.scanning => {
                    sandboxes.scanning = more.is_ok();
                    if more.is_err() {
                        continue;
                    }
                    let scan = sandboxes.scans.borrow_and_update().clone();
                    let running = running_sandboxes(&scan);
                    let same = running.len() == sandboxes.known.len()
                        && running.iter().all(|name| sandboxes.known.contains_key(name));
                    if !same {
                        close(&mut channel).await;
                        return HubEnd::Again;
                    }
                    let now = Instant::now();
                    for (name, state) in sandboxes.known.iter_mut() {
                        if let Some(ports) = scan.get(name) {
                            state.ages.observe(&ports.keys().copied().collect(), now);
                        }
                    }
                }
                msg = next_message(&mut channel) => match msg {
                    Some(ChannelMsg::Data { data }) => {
                        for line in lines.feed(&data) {
                            self.hub_line(&line, sandboxes, epoch).await;
                        }
                    }
                    Some(ChannelMsg::Close) | None => {
                        if epoch.cancel.is_cancelled() {
                            return HubEnd::LinkDown;
                        }
                        channel = None;
                        // Each sandbox reports when its watcher ends (it
                        // stopped, say); without that, something broke.
                        if sandboxes.known.values().any(|state| state.watched) {
                            self.log_hub_failure(
                                sandboxes,
                                "the watcher in the sandboxes stopped".into(),
                            );
                        }
                        retry.as_mut().reset(tokio::time::Instant::now() + RETRY);
                    }
                    Some(_) => {}
                },
            }
        }
    }

    fn log_hub_failure(&self, sandboxes: &mut Sandboxes, why: String) {
        if sandboxes.failure.as_deref() != Some(why.as_str()) {
            self.log(
                LogLevel::Warn,
                format!("not opening links from the sandboxes: {why}"),
            );
            sandboxes.failure = Some(why);
        }
    }

    /// One line of the shared watcher: `@<sandbox> <what its watcher printed>`.
    async fn hub_line(
        self: &Arc<Self>,
        line: &str,
        sandboxes: &mut Sandboxes,
        epoch: &ConnectionEpoch,
    ) {
        let Some((name, rest)) = parse_tagged(line) else {
            return;
        };
        let Some(state) = sandboxes.known.get_mut(name) else {
            return;
        };
        let place = Some(name.to_string());
        if rest == "@@END" {
            // Its watcher is gone: the sandbox is stopping, or it failed.
            // Tried again later if the sandbox stays.
            state.watched = false;
            let why = "its watcher stopped".to_string();
            if state.failure.is_none() {
                state.failure = Some(why);
            }
        } else if let Some(msg) = rest.strip_prefix("@@ERR ") {
            self.note_failure(&place, &mut state.failure, msg.to_string());
        } else if let Some(pid) = parse_ready(rest) {
            state.watcher = Some(pid);
            state.failure = None;
            sandboxes.failure = None;
            if !std::mem::replace(&mut state.announced, true) {
                self.log(
                    LogLevel::Info,
                    format!(
                        "links opened with xdg-open in sandbox {name} now open in the local browser"
                    ),
                );
            }
        } else if let Some(request) = parse_request(rest) {
            self.handle(&place, epoch, request, &state.ages).await;
        }
    }

    async fn uninstall(&self, place: &Place, epoch: &ConnectionEpoch, watcher: Option<u32>) {
        let at = describe(place);
        let removed = tokio::time::timeout(
            EXEC_TIMEOUT,
            exec_capture(&epoch.handle, &uninstall_command(place, watcher)),
        )
        .await;
        let Ok(Ok((out, _))) = removed else {
            return;
        };
        if let Some(path) = out.lines().find_map(|l| l.strip_prefix("@@REMOVED ")) {
            self.log(
                LogLevel::Info,
                format!("removed the xdg-open shim from {at} ({path})"),
            );
        } else if out.lines().any(|l| l.starts_with("@@KEPT ")) {
            self.log(
                LogLevel::Info,
                format!("left the xdg-open shim on {at}: another nettle still uses it"),
            );
        }
    }

    /// Forward what the link needs, then open it.
    async fn handle(
        self: &Arc<Self>,
        place: &Place,
        epoch: &ConnectionEpoch,
        request: OpenRequest,
        ages: &PortAges,
    ) {
        let at = describe(place);
        let admitted = self.throttle.lock().unwrap().admit(Instant::now());
        if let Admit::No { first } = admitted {
            if first {
                self.log(
                    LogLevel::Warn,
                    format!(
                        "more than {RATE_LIMIT} links to open in {} s (the latest from {at}); \
                         ignoring the rest of the burst",
                        RATE_WINDOW.as_secs()
                    ),
                );
            }
            return;
        }
        let mut url = match check_url(&request.url) {
            Ok(url) => url,
            Err(why) => {
                self.log(
                    LogLevel::Warn,
                    format!("ignored a link {at} asked to open: {why}"),
                );
                return;
            }
        };

        let refs = loopback_ports(&url);
        let mut wanted = refs.redirects.clone();
        // Ports the calling program opened for this: a callback server whose
        // port the link carries in a form only the sign-in page understands.
        if !request.pids.is_empty() {
            let pid_scan = command(place, PID_SCAN, false);
            let scan = tokio::select! {
                _ = epoch.cancel.cancelled() => return,
                r = tokio::time::timeout(EXEC_TIMEOUT, exec_capture(&epoch.handle, &pid_scan)) => r,
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
            // The link points at the place's own loopback.
            match self.forward(place, port, true).await {
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
                            "could not forward port {port} of {at}; not opening {}",
                            display_url(&url)
                        ),
                    );
                    return;
                }
            }
        }
        for port in wanted {
            forwarded.extend(self.forward(place, port, false).await);
        }

        let shown = display_url(&url);
        match self.ui.open_url(url.as_str()) {
            Ok(()) => {
                let via = if forwarded.is_empty() {
                    String::new()
                } else {
                    let list: Vec<String> =
                        forwarded.iter().map(|p| format!("localhost:{p}")).collect();
                    format!(", with {} forwarded there", list.join(", "))
                };
                self.log(
                    LogLevel::Info,
                    format!("opened {shown} from {at} in the local browser{via}"),
                );
            }
            Err(e) => self.log(LogLevel::Error, format!("could not open {shown}: {e}")),
        }
    }

    /// Make `port` of `place` reachable on this machine and return the local
    /// port. Unless `movable`, that has to be the same number: a page that
    /// redirects to `localhost:port` knows no other.
    async fn forward(self: &Arc<Self>, place: &Place, port: u16, movable: bool) -> Option<u16> {
        let target = Target {
            sandbox: place.clone(),
            port,
        };
        let existing = self
            .forwards
            .list()
            .into_iter()
            .find(|f| f.sandbox == *place && f.port == port);
        let holder = self
            .forwards
            .local_target(port)
            .filter(|holder| *holder != target);
        let take_over = self.options_rx.borrow().take_over;
        let decision = plan(
            port,
            existing.as_ref().map(|f| f.local_port),
            holder.is_some(),
            movable,
            take_over,
        );
        match decision {
            Plan::Use(local) => Some(local),
            Plan::Bind => {
                let bound = self.forwards.set_target(target.clone(), port, true, false);
                if bound.await.is_ok() {
                    self.remove_when_closed(target, port);
                    return Some(port);
                }
                // The forward manager logged why the port could not be bound:
                // another program has it, or it is privileged.
                if movable {
                    return self.forward_to_free_port(target).await;
                }
                self.log(
                    LogLevel::Warn,
                    format!(
                        "localhost:{port} is not available on this machine, so a redirect \
                         to it will not reach {target}"
                    ),
                );
                None
            }
            Plan::BindFree => self.forward_to_free_port(target).await,
            Plan::TakeOver => {
                // A forward of its own is moved, and keeps its pin. Moving
                // unbinds first, so make sure the new place can be had.
                let pinned = existing.as_ref().is_some_and(|f| f.pinned);
                if existing.is_some() && holder.is_none() && !can_bind(port).await {
                    self.log(
                        LogLevel::Warn,
                        format!(
                            "a link expects {target} on localhost:{port}, which another \
                             program on this machine holds"
                        ),
                    );
                    return None;
                }
                // Otherwise the listener is switched in place, like "switch
                // here" in the ports tab (the manager logs it).
                let taken = self.forwards.set_target(target.clone(), port, true, pinned);
                taken.await.is_ok().then_some(port)
            }
            Plan::Blocked => {
                let held_by = match (&existing, &holder) {
                    (Some(own), _) => {
                        format!("but {target} is forwarded to localhost:{}", own.local_port)
                    }
                    (None, Some(holder)) => format!("which tunnels to {holder}"),
                    (None, None) => "which is taken".to_string(),
                };
                self.log(
                    LogLevel::Warn,
                    format!(
                        "a link expects {target} on localhost:{port}, {held_by}; \
                         \"Let links take over local ports\" in the host's settings allows it"
                    ),
                );
                None
            }
        }
    }

    async fn forward_to_free_port(self: &Arc<Self>, target: Target) -> Option<u16> {
        let local = free_local_port().await?;
        let bound = self.forwards.set_target(target.clone(), local, true, false);
        bound.await.ok()?;
        self.remove_when_closed(target, local);
        Some(local)
    }

    /// Remove the temporary forward of `target` once its port stops listening
    /// (or never shows up), unless it was pinned or changed meanwhile.
    fn remove_when_closed(self: &Arc<Self>, target: Target, local: u16) {
        if !self.temporary.lock().unwrap().insert(target.clone()) {
            return;
        }
        let this = self.clone();
        let mut live = self.live_ports(&target.sandbox);
        let port = target.port;
        tokio::spawn(async move {
            let listening =
                tokio::time::timeout(APPEAR_GRACE, live.wait_for(|set| set.contains(&port))).await;
            let closed = match listening {
                Ok(true) => live.wait_for(|set| !set.contains(&port)).await,
                // The session is gone, and its forwards with it.
                Ok(false) => false,
                // Never seen listening: a sign-in that was over within a
                // scan interval, or a port nothing ever opened.
                Err(_) => true,
            };
            this.temporary.lock().unwrap().remove(&target);
            if !closed {
                return;
            }
            let ours = this.forwards.list().into_iter().any(|f| {
                f.sandbox == target.sandbox && f.port == port && f.local_port == local && !f.pinned
            });
            if ours {
                let _ = this.forwards.set_target(target, 0, false, false).await;
            }
        });
    }
}

/// Whether a listener on the local `port` could be bound right now.
async fn can_bind(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).await.is_ok()
}

async fn open_channel(epoch: &ConnectionEpoch, command: String) -> Result<Channel> {
    let channel = epoch.handle.channel_open_session().await?;
    channel.exec(true, command).await?;
    Ok(channel)
}

/// The next message of a channel that may not be there (then: never).
async fn next_message(channel: &mut Option<Channel>) -> Option<ChannelMsg> {
    match channel {
        Some(channel) => channel.wait().await,
        None => std::future::pending().await,
    }
}

async fn close(channel: &mut Option<Channel>) {
    if let Some(channel) = channel.take() {
        let _ = channel.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        check_url(s).unwrap()
    }

    #[test]
    fn scripts_survive_the_sh_c_wrapper() {
        let host: Place = None;
        let uninstall = uninstall_command(&host, Some(4711));
        assert!(uninstall.starts_with("exec sh -c 'own=4711\n"));
        assert!(uninstall_command(&host, None).starts_with("exec sh -c 'own=\n"));
        for cmd in [watch_command(&host), install_command(&host), uninstall] {
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
    fn sandbox_scripts_go_through_sbx_exec() {
        let sandbox: Place = Some("sb-a".into());
        // The installer needs the channel's stdin inside the sandbox.
        let install = install_command(&sandbox);
        let (_, script) = install
            .split_once("; exec sbx exec -i sb-a sh -c '")
            .expect("runs through sbx exec -i");
        let script = script.strip_suffix('\'').expect("one quoted script");
        assert!(!script.contains('\''), "single quote in: {script}");

        // All sandboxes' watchers share one channel, their lines tagged.
        let hub = hub_command(&["sb-a".to_string(), "sb-b".to_string()]);
        assert!(hub.contains("exec 3<&0; ( sbx exec -i sb-a sh -c '"));
        assert!(hub.contains(" & ( sbx exec -i sb-b sh -c '"));
        assert!(hub.contains("printf \"@%s @@END\\n\" sb-b ) & wait"));
        assert_eq!(
            hub.matches(" <&3 2>/dev/null | while IFS= read -r l; do ")
                .count(),
            2
        );
        assert_eq!(
            parse_tagged("@sb-a open 12,7 https://example.com/a b"),
            Some(("sb-a", "open 12,7 https://example.com/a b"))
        );
        assert_eq!(parse_tagged("@sb-a @@END"), Some(("sb-a", "@@END")));
        // A sandbox cannot speak for another one: its lines carry its tag.
        assert_eq!(
            parse_tagged("@sb-a @sb-b open - https://example.com/"),
            Some(("sb-a", "@sb-b open - https://example.com/"))
        );
        assert_eq!(parse_tagged("via-sbx"), None);
        assert_eq!(parse_tagged("@sb-a"), None);
        let uninstall = uninstall_command(&sandbox, Some(7));
        assert!(uninstall.contains("; sbx exec sb-a sh -c 'own=7\n"));
        assert!(uninstall.ends_with("' </dev/null"));
        let scan = command(&sandbox, PID_SCAN, false);
        assert!(scan.contains("sbx exec sb-a sh -c 'ss -tlnp 2>/dev/null || netstat"));
        assert_eq!(describe(&sandbox), "sandbox sb-a");
        assert_eq!(describe(&None), "the host");
    }

    #[test]
    fn a_needed_port_is_bound_reused_taken_over_or_left() {
        // (existing forward of the target, local port held by another target,
        //  link can be rewritten, take-over option) → plan, for port 4180
        let p = |existing, held, movable, take_over| plan(4180, existing, held, movable, take_over);
        // Nothing in the way: its own number, whatever the option says.
        assert_eq!(p(None, false, false, false), Plan::Bind);
        assert_eq!(p(None, false, true, true), Plan::Bind);
        // Forwarded there already.
        assert_eq!(p(Some(4180), false, false, false), Plan::Use(4180));
        assert_eq!(p(Some(4180), false, true, true), Plan::Use(4180));
        // The local port tunnels elsewhere (the same port of another sandbox).
        assert_eq!(p(None, true, false, false), Plan::Blocked);
        assert_eq!(p(None, true, false, true), Plan::TakeOver);
        // A link to the port itself can do with another one, but keeps its
        // origin when it may take the port.
        assert_eq!(p(None, true, true, false), Plan::BindFree);
        assert_eq!(p(None, true, true, true), Plan::TakeOver);
        // The target is forwarded to another local port.
        assert_eq!(p(Some(9000), false, true, false), Plan::Use(9000));
        assert_eq!(p(Some(9000), false, true, true), Plan::Use(9000));
        assert_eq!(p(Some(9000), false, false, false), Plan::Blocked);
        assert_eq!(p(Some(9000), false, false, true), Plan::TakeOver);
        assert_eq!(p(Some(9000), true, false, true), Plan::TakeOver);
    }

    #[test]
    fn over_long_lines_are_dropped_whole() {
        let mut lines = Lines::default();
        assert_eq!(lines.feed(b"@@READY 12\nopen - http"), ["@@READY 12"]);
        assert_eq!(
            lines.feed(b"s://example.com/\n"),
            ["open - https://example.com/"]
        );
        let long = vec![b'a'; MAX_LINE + 10];
        assert!(lines.feed(&long).is_empty());
        assert!(lines.feed(b"still the long one\n").is_empty());
        assert_eq!(lines.feed(b"next\n\n"), ["next", ""]);
    }

    #[test]
    fn only_running_sandboxes_with_safe_names_are_watched() {
        let scan: SandboxPorts = [
            ("sb-b".to_string(), HashMap::new()),
            (
                "sb-a".to_string(),
                HashMap::from([(4180, "127.0.0.1".to_string())]),
            ),
            ("bad name".to_string(), HashMap::new()),
        ]
        .into();
        assert_eq!(running_sandboxes(&scan), ["sb-a", "sb-b"]);
    }

    #[test]
    fn options_follow_the_host_settings() {
        let mut host: HostConfig = serde_json::from_str(
            r#"{ "id": "00000000-0000-0000-0000-000000000001", "name": "a",
                "hostname": "a.example", "port": 22, "username": "u" }"#,
        )
        .unwrap();
        assert_eq!(Options::from(&host), Options::default());
        host.intercept_xdg_open = true;
        host.links_take_over_ports = true;
        assert_eq!(
            Options::from(&host),
            Options {
                enabled: true,
                take_over: true,
            }
        );
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
