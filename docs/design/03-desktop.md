# 03 — Desktop side: Hyprland, launching gliff, connect, fix

## The gliff contract (gliff 0.4.3 @ `9d93732`)

Everything gliffherdr relies on, each pinned by `tests/gliff_contract.rs`, which reads
`.vendor/gliff` and asserts the quoted source still holds:

| # | Fact | Source |
|---|---|---|
| G1 | Binary `gliff`; flags `--headless` (conflicts with `--output`), `--output <NAME>`, `--server-bin <CMD>` (default `gliff-server`), `--no-clipboard`, `--release-hotkey`, `--video`; one optional positional `host` | `crates/gliff/src/main.rs` `struct Cli` |
| G2 | Window class/app id is `gliff` (`glib::set_prgname(Some(APP_NAME))`, `APP_NAME = "gliff"`) | `main.rs` |
| G3 | Window title is `format!("{} — Gliff", machine_name(endpoint))`; `machine_name` is the host string as given | `main.rs` `window_title` |
| G4 | No application id is set (`adw::Application::builder().build()`), so every launch is a new process and window | `main.rs` `fn main` |
| G5 | On start, every machine in config `open` is reconnected, then CLI hosts not already open | `main.rs` around `config.open` |
| G6 | Config path `glib::user_config_dir()/gliff/config.toml` (honours `XDG_CONFIG_HOME`), keys `machines`, `open`, `shown` | `crates/gliff/src/recent.rs` |
| G7 | Theme comes from `$HOME/.local/state/omarchy/current`, not from `XDG_CONFIG_HOME` | `crates/gliff/src/theme.rs` |
| G8 | ssh is run as `ssh -T … [-o StrictHostKeyChecking=accept-new] -- <host> <server_bin> --stdio <server args>`; accept-new only when `ssh -G` reports `stricthostkeychecking ask` | `crates/gliff-transport/src/ssh.rs` |
| G9 | `gliff-server --version` prints `gliff-server <version>` | `crates/gliff-server/src/main.rs` `#[command(version)]` |

If gliff upstream changes one of these, the contract test fails and points at the row.

## `hypr`

```rust
pub struct Graphical { pub env: Vec<(String, String)>, pub instance: Option<String> }
pub fn graphical_env(sys: &dyn Sys) -> Option<Graphical>;
pub struct Client { pub address: String, pub class: String, pub title: String, pub pid: i64 }
pub struct Hypr<'a> { sys: &'a dyn Sys, g: &'a Graphical }
impl<'a> Hypr<'a> {
    pub fn new(sys: &'a dyn Sys, g: &'a Graphical) -> Self;
    pub fn clients(&self) -> Vec<Client>;                 // hyprctl clients -j; [] on any failure
    pub fn find_window(&self, target: &str) -> Option<Client>;   // class == "gliff" && title starts with "<target> — "
    pub fn open_targets(&self) -> HashSet<String>;        // for every gliff client, the title before " — "
    pub fn focus(&self, address: &str) -> Result<(), HyprError>;
}
```

`graphical_env`: start from the process env. `XDG_RUNTIME_DIR` defaults to `/run/user/<uid>`,
with the uid read safely from `std::fs::metadata("/proc/self")` (`MetadataExt::uid`), never
from `libc::getuid` (keeps the crate's single `unsafe` block in `sys/real.rs`). If
`WAYLAND_DISPLAY` is unset, read `systemctl --user show-environment` and take
`WAYLAND_DISPLAY`, `HYPRLAND_INSTANCE_SIGNATURE`, `DISPLAY`, `XDG_CURRENT_DESKTOP`,
`XDG_SESSION_TYPE` (skip values that start with `$'`). If still unset, parse
`hyprctl instances`: the first `instance <sig>:` line and its `wl socket: <name>` line. Return
`None` when `WAYLAND_DISPLAY` is still unknown. (A herdr server started over ssh has no Wayland
env; this was observed in practice.)

Every `hyprctl` call passes `Graphical.env`; when `HYPRLAND_INSTANCE_SIGNATURE` is unknown the
call is `hyprctl -i 0 …`.

### Focus

Hyprland 0.56 replaced dispatcher syntax with Lua (`hyprctl dispatch exec x` → error, exit 7;
`hyprctl dispatch 'hl.dsp.focus({ window = "address:0x…" })'` → `ok`; verified on 0.56.2).
`focus` runs the Lua form first and succeeds if stdout trimmed is `ok`; otherwise it runs
`hyprctl dispatch focuswindow address:<addr>` and succeeds on `ok`; otherwise
`HyprError::Focus(<both outputs>)`. Addresses are validated as `0x[0-9a-f]+` before use.

## `launch`

```rust
pub enum ScreenChoice { Mirror, Headless, Output(String) }
pub struct LaunchPlan { pub argv: Vec<String>, pub env: Vec<(String, String)>, pub config_home: PathBuf, pub log: PathBuf }
pub fn plan(paths: &Paths, config: &Config, settings: &MachineSettings, target: &Target,
            choice: &ScreenChoice, uwsm: bool, g: &Graphical) -> LaunchPlan;       // pure
pub fn prepare_config_home(real_config_home: &Path, dir: &Path) -> std::io::Result<()>;
pub fn uwsm_active(sys: &dyn Sys) -> bool;     // `uwsm-app` on PATH && `uwsm check is-active` exits 0
pub fn launch(sys: &dyn Sys, plan: &LaunchPlan) -> std::io::Result<()>;
pub fn wait_for_window(sys: &dyn Sys, hypr: &Hypr, target: &str, within: Duration) -> Option<Client>;
```

`plan` builds, in order:

```
[uwsm-app -a gliff -d "Gliff: <target>" --]      only when uwsm
env XDG_CONFIG_HOME=<window_dir>/config
gliff <gliff_args…> [--server-bin <cmd>] [--headless | --output <name>] -- <target>
```

`gliff_args` = machine `gliff_args` if set, else global. `server_bin` = machine, else global,
else omitted. Mirror adds no flag. `env` = `Graphical.env`. `log` = `<window_dir>/gliff.log`.

`prepare_config_home`: create `<dir>`; for each of `fontconfig`, `gtk-4.0`, `gtk-3.0`, `dconf`
that exists in `real_config_home` and not in `<dir>`, symlink it (G6/G7 explain why only these).

`launch`: `prepare_config_home`, write the log's first line `<RFC 3339 time> <argv joined with
spaces>`, then `sys.spawn_detached`. `wait_for_window` polls `find_window` every 250 ms.
The wait is 8 s, overridable by `GLIFFHERDR_WINDOW_WAIT_MS` (tests use 300).

## `connect`

```rust
pub struct Ctx { pub sys: Arc<dyn Sys>, pub paths: Paths, pub config: Config, pub herdr: Herdr }
pub struct ConnectRequest { pub target: Target, pub choice: Option<ScreenChoice> }
pub enum Progress { Checking, Launching, Waiting }
pub enum Connected { Focused, Launched }
pub struct ConnectError { pub message: String, pub hint: Option<String>, pub remedy: Option<Remedy>, pub log: Option<PathBuf> }
pub fn connect(ctx: &Ctx, req: &ConnectRequest, progress: &mut dyn FnMut(Progress)) -> Result<Connected, ConnectError>;
pub fn remember_recent(paths: &Paths, target: &Target) -> std::io::Result<()>;   // dedupe, most recent first, keep 30
```

Steps:

1. `which("gliff")` or fail: `gliff is not installed on this machine.` / hint `omarchy pkg add gliff`.
2. `graphical_env` or fail: `No Hyprland session found to open a window in.` / hint
   `herdr runs plugin commands on its own machine; gliff needs a desktop there.`
3. If `find_window(target)` → `focus` → `remember_recent` → `Focused`.
4. Build the `Machine` for the target (from the roster if listed, else a bare one with
   `settings_for`). If `needs_check(cached)` → `progress(Checking)` → `check_one`.
5. Not `Ready` → fail with `sentence(status)` and `remedy_for(status)`.
6. Choice = request choice, else machine `mode`/`output` (`output` set ⇒ `Output`), else global `mode`.
7. `progress(Launching)` → `launch(plan)` → `remember_recent` → `progress(Waiting)` →
   `wait_for_window`; none → fail `gliff did not open a window for <t>.` with hint = last 3 log
   lines and `log` set.
8. `Launched`.

## `fix`

```rust
pub enum Remedy { InstallServer, CopyKey, AcceptHostKey }
pub fn remedy_for(status: &Status) -> Option<Remedy>;   // NoServer, Auth, HostKey{changed:false}; nothing else
pub fn remedy_argv(r: Remedy, target: &Target) -> Vec<String>;
pub fn describe(r: Remedy) -> &'static str;
```

| Remedy | argv | describe |
|---|---|---|
| InstallServer | `ssh -t -- <t> 'if command -v omarchy >/dev/null 2>&1; then omarchy pkg add gliff; else echo "This machine has no omarchy command. Install gliff from https://github.com/omacom/gliff#install"; exit 1; fi'` | `Install gliff on <t> (runs omarchy pkg add gliff there)` |
| CopyKey | `ssh-copy-id <t>` | `Copy your ssh key to <t>` |
| AcceptHostKey | `ssh -o StrictHostKeyChecking=ask -- <t> true` | `Check <t>'s host key and accept it` |

`gliffherdr fix [TARGET]` (target from the argument or `GLIFFHERDR_FIX_TARGET`): run a fresh
`check_one`; if it has no remedy, print the sentence and exit 0 (already ready) or 1. Otherwise
print `describe`, run `remedy_argv` with `run_interactive`, run `check_one` again, print the new
sentence, then, when stdin is a terminal, print `Press Enter to close.` and wait for a line. Exit
0 when the machine ends `Ready`, else 1.

## `herdr`

```rust
pub struct Herdr { bin: String }
impl Herdr {
    pub fn from_sys(sys: &dyn Sys) -> Herdr;   // HERDR_BIN_PATH, else "herdr"
    pub fn machines(&self, sys: &dyn Sys) -> Result<Vec<(String, Option<String>)>, HerdrError>;
    pub fn open_pane(&self, sys: &dyn Sys, entrypoint: &str, placement: Option<&str>, env: &[(&str, &str)], focus: bool) -> Result<(), HerdrError>;
    pub fn notify(&self, sys: &dyn Sys, title: &str, body: &str) -> Result<(), HerdrError>;
    pub fn pane_processes(&self, sys: &dyn Sys, pane_id: &str) -> Result<Vec<Vec<String>>, HerdrError>;  // argv of each foreground process
    pub fn in_herdr(sys: &dyn Sys) -> bool;     // HERDR_ENV == "1"
}
```

Command shapes (herdr v0.9.5 CLI, see `.vendor/herdr/docs/next/website/src/content/docs/cli-reference.mdx`):
`machine list --json`; `plugin pane open --plugin <id> --entrypoint <e> [--placement <p>] [--env K=V]… [--focus]`;
`notification show <title> --body <text>`; `pane process-info --pane <id>` →
`.result.process_info.foreground_processes[].argv`. Every call has a 5 s timeout. Non-zero exit
→ `HerdrError` with herdr's stderr (herdr prints JSON errors on stderr with exit 1).

## `roster::ssh_argv` — "this pane's machine"

```rust
pub fn ssh_destination(argv: &[String]) -> Option<String>;
```

Accepts argv whose program basename is `ssh`. Options that take a value: `-B -b -c -D -E -e -F
-I -i -J -L -l -m -O -o -P -p -Q -R -S -W -w` (attached `-p2222` or separate). `--` ends
options. The destination is the first non-option argument. `-l <user>` and `-p <port>` combine:
`-l u -p 2222 h` → `ssh://u@h:2222`; `-p 2222 h` → `ssh://h:2222`; `-l u h` → `u@h`; a
destination that already has `user@` keeps it. Returns `None` for `ssh -V`, `ssh` alone, and
anything that does not parse to a valid `Target`. Unit tests cover each line above plus `-J`,
`-o Foo=bar`, `-t`, `-4`, `-vvv`.
