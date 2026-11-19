# 02 — Core: system seam, paths, targets, config, roster, checks, status

All signatures below are the public surface. Private helpers are free. Every public item has a
doc comment (`missing_docs` is on) and a test.

## `sys` — the external-command seam

```rust
pub struct Cmd {
    pub program: String,              // looked up on PATH unless it contains '/'
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,   // added to the inherited environment
    pub stdin: Option<Vec<u8>>,       // None = /dev/null
    pub timeout: Option<Duration>,    // None = wait forever
}
pub struct Output { pub code: Option<i32>, pub stdout: String, pub stderr: String, pub timed_out: bool }
pub trait Sys: Send + Sync {
    fn run(&self, cmd: &Cmd) -> std::io::Result<Output>;            // captures stdout/stderr (lossy UTF-8)
    fn run_interactive(&self, cmd: &Cmd) -> std::io::Result<i32>;   // inherits the terminal; used by fix and the editor
    fn spawn_detached(&self, cmd: &Cmd, log: &Path) -> std::io::Result<()>;
    fn which(&self, program: &str) -> Option<PathBuf>;
    fn env(&self, key: &str) -> Option<String>;
    fn now_unix(&self) -> u64;
    fn sleep(&self, d: Duration);
}
pub fn cmd(program: &str, args: &[&str]) -> Cmd;   // convenience constructor
```

`RealSys` (`sys/real.rs`):
- `run`: spawns with piped stdin/stdout/stderr, writes stdin on a thread, reads both pipes on
  threads, and when `timeout` elapses kills the child (`SIGKILL`), reaps it and returns
  `timed_out: true, code: None`. Poll `try_wait` every 20 ms.
- `spawn_detached`: truncates/creates `log`, `stdin` from `/dev/null`, stdout and stderr appended
  to `log`, `pre_exec(|| { libc::setsid(); Ok(()) })` (the crate's only `unsafe`, with a SAFETY
  comment: setsid is async-signal-safe), spawn, then drop the `Child` without waiting.
- `which`: walks `PATH`, returns the first executable regular file.

`FakeSys` (`sys/fake.rs`, `#[doc(hidden)] pub`): scripted responses keyed by program name plus an
args prefix (`fake.on("hyprctl", &["clients", "-j"]).stdout("[]")`), a default response per
program, a recorded call log (`fake.calls()`), a settable clock (`fake.set_now(…)`, `sleep`
advances it), an env map, and a `which` set. Unscripted calls return
`Err(io::ErrorKind::NotFound)`.

## `paths`

```rust
pub const PLUGIN_ID: &str = "jaden-hoenes.gliffherdr";
pub struct Paths { pub config_dir: PathBuf, pub state_dir: PathBuf, pub real_config_home: PathBuf, pub home: PathBuf }
impl Paths {
    pub fn from_sys(sys: &dyn Sys) -> Paths;
    pub fn config_file(&self) -> PathBuf;                 // <config_dir>/config.toml
    pub fn checks_dir(&self) -> PathBuf;                  // <state_dir>/checks
    pub fn check_file(&self, target: &Target) -> PathBuf; // <checks_dir>/<slug>.json
    pub fn recent_file(&self) -> PathBuf;                 // <state_dir>/recent.json
    pub fn window_dir(&self, target: &Target) -> PathBuf; // <state_dir>/windows/<slug>
}
pub fn herdr_component(id: &str) -> String;  // herdr src/plugin_paths.rs plugin_config_path_component @ v0.9.5
pub fn slug(target: &str) -> String;
```

Resolution order (first that is set wins):

| Dir | 1 | 2 | 3 (herdr's own rule) |
|---|---|---|---|
| config | `GLIFFHERDR_CONFIG_DIR` | `HERDR_PLUGIN_CONFIG_DIR` | `${XDG_CONFIG_HOME:-$HOME/.config}/herdr/plugins/config/<herdr_component(id)>` |
| state | `GLIFFHERDR_STATE_DIR` | `HERDR_PLUGIN_STATE_DIR` | `${XDG_STATE_HOME:-$HOME/.local/state}/herdr/plugins/<herdr_component(id)>` |

`id` is `HERDR_PLUGIN_ID` if set, else `PLUGIN_ID`. `real_config_home` is
`${XDG_CONFIG_HOME:-$HOME/.config}`. The herdr rules are copied from
`.vendor/herdr/src/plugin_paths.rs` (`plugin_config_dir`, `plugin_state_dir`,
`plugin_config_path_component`) and `.vendor/herdr/src/config/io.rs` (`state_dir`); port
`plugin_config_path_component` byte for byte (lowercase ASCII letters, digits, `.`, `_`, `-`
kept; every other byte `%XX` uppercase hex; a trailing `.` becomes `%2E`) and copy its unit-test
vectors.

`slug(t)`: lowercase `t`; replace every char outside `[a-z0-9._@-]` with `_`; keep the first 60
chars; append `-` and the first 8 lowercase hex digits of the 64-bit FNV-1a hash of the original
`t` bytes (offset basis `0xcbf29ce484222325`, prime `0x100000001b3`). Test vectors:
`slug("j@192.0.2.10")` and `slug("ssh://User@Host:2222")` are fixed in a unit test.

Writes are atomic: write `<file>.tmp.<pid>` then rename. Directories are created on first write.

## `target`

```rust
pub struct Target(String);              // validated, immutable
impl Target { pub fn parse(s: &str) -> Result<Target, TargetError>; pub fn as_str(&self) -> &str }
pub struct Resolved { pub user: String, pub host: String, pub port: u16 }   // host lowercased
pub fn resolve(sys: &dyn Sys, t: &Target) -> Resolved;
```

`parse` trims surrounding whitespace, then requires: 1–255 bytes; first char in
`[A-Za-z0-9_]`; every other char in `[A-Za-z0-9._@%:/+-]`. Errors: `Empty`, `TooLong`,
`InvalidChar(char)`. A leading `-` is therefore impossible (it would be an ssh option).

`resolve` runs `ssh -G -- <target>` (timeout 3 s) and reads the `user`, `hostname`, `port`
lines. If ssh fails: strip a leading `ssh://`, take what follows the last `@` as host (and a
`:port` suffix if numeric), user `-`, port 22.

## `config`

File: `Paths::config_file()`. Missing file = all defaults. Parse with `toml` into:

```rust
#[serde(deny_unknown_fields, default)] pub struct Config {
    pub connect: ConnectSettings, pub sources: SourceSettings, pub check: CheckSettings,
    #[serde(rename = "machine")] pub machines: Vec<MachineConfig>,
}
pub struct ConnectSettings { pub mode: Mode, pub gliff_args: Vec<String>, pub server_bin: Option<String> }
pub struct SourceSettings { pub herdr: bool, pub recent: bool, pub gliff: bool, pub ssh_config: bool, pub tailscale: bool, pub hide: Vec<String> }
pub struct CheckSettings { pub on_startup: bool, pub fresh_for_secs: u64, pub ssh_timeout_secs: u64 }
pub struct MachineConfig { pub target: String, pub label: Option<String>, pub mode: Option<Mode>,
    pub output: Option<String>, pub server_bin: Option<String>, pub gliff_args: Option<Vec<String>> }
#[serde(rename_all = "lowercase")] pub enum Mode { Mirror, Headless }
pub fn load(paths: &Paths) -> Result<Config, ConfigError>;
pub fn write_template_if_missing(paths: &Paths) -> std::io::Result<bool>;
pub const TEMPLATE: &str = include_str!("config/template.toml");
```

Defaults: `mode = mirror`, `gliff_args = []`, `server_bin = None` (an empty string also means
None), all sources `true`, `hide = []`, `on_startup = true`, `fresh_for_secs = 600`,
`ssh_timeout_secs = 5`. `ConfigError` carries the path and toml's message, which includes line
and column. Validation after parsing: every `machine.target` passes `Target::parse`; `output`
matches `[A-Za-z0-9._-]+`; `fresh_for_secs ≥ 1`; `1 ≤ ssh_timeout_secs ≤ 60`.

`template.toml` documents every key, with every line commented out except the four section
headers, so loading the template yields the defaults (a test asserts this). It is the only
place config keys are explained to users besides `docs/configuration.md`.

## `roster`

```rust
pub enum Source { Config, Herdr, Recent, Gliff, SshConfig, Tailscale }   // Display: config herdr recent gliff ssh tailscale
pub enum OsHint { Linux, MacOs, Windows, Other(String) }
pub struct MachineSettings { pub mode: Option<Mode>, pub output: Option<String>, pub server_bin: Option<String>, pub gliff_args: Option<Vec<String>> }
pub struct Machine { pub target: Target, pub label: Option<String>, pub sources: Vec<Source>,
    pub os_hint: Option<OsHint>, pub settings: MachineSettings, pub resolved: Resolved }
impl Machine { pub fn display_label(&self) -> &str; pub fn checkable(&self) -> bool }  // checkable: os_hint None or Linux
pub struct Roster { pub machines: Vec<Machine>, pub warnings: Vec<String> }
pub fn collect(sys: &dyn Sys, paths: &Paths, config: &Config) -> Roster;
pub fn settings_for(config: &Config, target: &Target, resolved: &Resolved) -> MachineSettings;
```

Sources, collected in this order; each is skipped when its `sources.*` flag is false (Config is
always on). A source that fails adds a one-line warning and contributes nothing.

| # | Source | How | Label |
|---|---|---|---|
| 1 | Config | `config.machines` | `label` |
| 2 | Herdr | `$HERDR_BIN_PATH` (else `herdr` on PATH) `machine list --json`; array of `{target, label}` | `label` |
| 3 | Recent | `<state>/recent.json`, a JSON array of targets, most recent first | none |
| 4 | Gliff | `<real_config_home>/gliff/config.toml`, its `machines` array (parse with `toml`, ignore other keys) | none |
| 5 | SshConfig | `Host` lines in `~/.ssh/config` plus files named by its `Include` lines (one level; relative paths are under `~/.ssh`; globs expanded by a small `*`/`?` matcher on the file names of that directory). Patterns containing `*`, `?` or `!` are skipped. | none |
| 6 | Tailscale | `tailscale status --json` (timeout 3 s); `.Peer[]` with `Online == true`; target = `DNSName` without the trailing `.`, else the first `TailscaleIPs` entry | `HostName` |

Filters: SshConfig entries whose resolved user is `git` or whose resolved host matches
`^(github\.com|.*\.github\.com|gitlab\.com|bitbucket\.org|codeberg\.org|git\.sr\.ht|aur\.archlinux\.org|ssh\.dev\.azure\.com|vs-ssh\.visualstudio\.com)$`
are dropped. Tailscale peers with OS `iOS` or `android` (case-insensitive) are dropped; `OS`
maps to `OsHint` (`linux`, `macOS`, `windows`, other). Entries whose target or label equals an
item of `sources.hide` (case-insensitive) are dropped. Invalid targets are dropped with a warning.

Merging: keep a map from the full key `user@host:port` and a set of host keys. A non-Tailscale
candidate whose full key is known is merged into that machine (append the source, fill a missing
label). A Tailscale candidate is merged into the first machine whose host keys contain any of its
DNS name, lowercase HostName or IPs; otherwise it becomes a new machine. A new machine registers
its full key and host keys (resolved host, plus a Tailscale peer's DNS name, HostName and IPs).
Label precedence: Config > Herdr > Tailscale. `settings` come from the Config entry matching by
target string or by full key.

## `check`

```rust
pub enum Status { Ready, NoHyprland, NoServer, Unsupported { os: String }, Offline { detail: String },
    Auth, HostKey { changed: bool }, Error { detail: String } }
pub struct CheckResult { pub target: String, pub status: Status, pub os: Option<String>, pub outputs: Vec<String>,
    pub server_version: Option<String>, pub checked_at: u64 }
pub struct Report { pub os: String, pub hyprland: bool, pub outputs: Vec<String>, pub gliff_server: Option<String>, pub server_version: Option<String> }
pub const REMOTE_SCRIPT: &str = include_str!("check/remote.sh");
pub fn parse_report(stdout: &str) -> Report;
pub fn classify(out: &Output, timeout_secs: u64) -> Status;   // pure
pub fn check_one(sys: &dyn Sys, paths: &Paths, config: &Config, machine: &Machine) -> CheckResult;  // runs ssh, saves the cache file
pub enum CheckEvent { Started(String), Finished(CheckResult) }
pub fn check_many(sys: Arc<dyn Sys>, paths: Paths, config: Config, machines: Vec<Machine>, tx: Sender<CheckEvent>) -> JoinHandle<()>;
pub fn load_cached(paths: &Paths, target: &Target) -> Option<CheckResult>;
pub fn is_fresh(r: &CheckResult, now: u64, fresh_for_secs: u64) -> bool;
pub fn needs_check(r: Option<&CheckResult>, now: u64, fresh_for_secs: u64) -> bool;  // not fresh, or a failure other than Unsupported (used before connecting)
```

The ssh call (`check_one`):
`ssh -T -o BatchMode=yes -o ConnectTimeout=<t> -o LogLevel=ERROR [-o StrictHostKeyChecking=accept-new] -- <target> sh -s`
with timeout `t + 10` s, where `t = check.ssh_timeout_secs`. The accept-new option is added
exactly when `ssh -G -- <target>` contains the line `stricthostkeychecking ask` (00 ⚑1). Stdin
is, when a server command is configured (machine `server_bin` else global), the line
`server_bin='<value with ' escaped as '\''>'` followed by `REMOTE_SCRIPT`.

`remote.sh` (POSIX sh, shellcheck-clean) prints `key=value` lines: `os` (`uname -s`),
`hyprland` (1 when `hyprctl [-i 0] version` succeeds, `-i 0` only when
`HYPRLAND_INSTANCE_SIGNATURE` is unset, else when `pgrep -x Hyprland` finds one), `outputs`
(space-separated names from `hyprctl [-i 0] monitors`, lines `Monitor <name> (ID n):`),
`gliff_server` (`custom` when `server_bin` is set, else `command -v gliff-server`),
`server_version` (first line of `gliff-server --version` when found, e.g. `gliff-server 0.4.2`).
It defaults `XDG_RUNTIME_DIR` to `/run/user/$(id -u)`. The bash prototype's probe script was the starting
point.

`parse_report` → `Status`: `os != "Linux"` → `Unsupported{os}`; `!hyprland` → `NoHyprland`;
`gliff_server` empty → `NoServer`; else `Ready`.

`classify` (on failure; `last` = last non-blank stderr line, CR stripped):

| Condition (checked in order) | Status |
|---|---|
| `timed_out` | `Offline { detail: "no answer within <t+10>s" }` |
| stderr contains `REMOTE HOST IDENTIFICATION HAS CHANGED` | `HostKey { changed: true }` |
| stderr contains `Host key verification failed` | `HostKey { changed: false }` |
| stderr contains `Permission denied` | `Auth` |
| stderr contains `Could not resolve hostname` | `Offline { detail: "cannot resolve the host name" }` |
| stderr matches `timed out`, `No route to host`, `Network is unreachable`, `Connection refused`, `Connection closed`, `Connection reset` | `Offline { detail: last }` |
| otherwise | `Error { detail: last, or "ssh exited with status <code>" }` |

Cache file `<checks_dir>/<slug>.json`: `{"version":1,"target":…,"status":{"kind":"ready"}|{"kind":"offline","detail":…}|…,"os":…,"outputs":[…],"server_version":…,"checked_at":…}`.
Unknown `version` or unparsable file = no cache. `check_many` runs at most 12 checks at once,
sends `Started` before and `Finished` after each, and returns when all are done.

## `status` — what a row shows (shared by `list` and the picker)

```rust
pub enum RowState { Open, Ready, Checking, NotChecked, NeedsSetup(Status), Unreachable(Status), Unsupported(String) }
pub enum Tone { Good, Accent, Busy, Muted, Warn, Bad }
pub fn row_state(m: &Machine, cached: Option<&CheckResult>, checking: bool, open: bool) -> RowState;
pub fn rank(s: &RowState) -> u8;
pub fn glyph(s: &RowState, spinner_frame: usize) -> &'static str;
pub fn tone(s: &RowState) -> Tone;
pub fn short_text(s: &RowState) -> String;
pub fn sentence(status: &Status, target: &str) -> (String, Option<String>);   // message, hint
```

Precedence in `row_state`: `open` → `Open`; `checking` → `Checking`; not checkable (os_hint) →
`Unsupported(os name)`; no cache → `NotChecked`; else by status: Ready → `Ready`;
NoHyprland/NoServer/Auth/HostKey → `NeedsSetup`; Offline/Error → `Unreachable`;
Unsupported → `Unsupported`.

| RowState | rank | glyph | tone | short_text |
|---|---|---|---|---|
| Open | 0 | `▣` | Accent | `window open` |
| Ready | 1 | `●` | Good | `ready` |
| Checking | 2 | braille spinner `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏` | Busy | `checking…` |
| NotChecked | 3 | `○` | Muted | `not checked` |
| NeedsSetup | 4 | `▲` | Warn | `Hyprland not running` / `gliff-server missing` / `ssh key refused` / `unknown host key` / `host key changed` |
| Unreachable | 5 | `✕` | Bad | `offline` / the error detail (truncated by the caller) |
| Unsupported | 6 | `–` | Muted | `macOS: needs Hyprland` (`<OS name>: needs Hyprland`; `Darwin`→`macOS`) |

`sentence` (message, hint):

| Status | Message | Hint |
|---|---|---|
| NoHyprland | `<t>: Hyprland is not running there.` | `gliff shows a running Hyprland session; log in on that machine first.` |
| NoServer | `<t>: gliff-server is not installed there.` | `Press ctrl-f to install it, or set server_bin in config.toml.` |
| Unsupported | `<t> runs <OS>; gliff connects to Hyprland only.` | none |
| Offline | `<t> is not reachable: <detail>.` | none |
| Auth | `<t> refused the ssh key.` | `gliff needs a key login without a prompt. Press ctrl-f to run ssh-copy-id.` |
| HostKey{false} | `<t> has a host key ssh does not know.` | `Press ctrl-f to check and accept it.` |
| HostKey{true} | `<t>'s host key has changed.` | `This can mean an attack. If the machine was reinstalled, remove the old key: ssh-keygen -R <host>` |
| Error | `<t>: <detail>` | none |
| Ready | `<t> is ready.` | none |

The CLI prints the same sentences with `ctrl-f` replaced by `gliffherdr fix <t>`.
