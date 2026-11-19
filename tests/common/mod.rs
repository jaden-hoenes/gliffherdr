//! The CLI integration harness (docs/design/07-testing.md): an isolated world with stub executables on `PATH`.
//!
//! `World::command()` is the only way tests start a child process. It clears the environment,
//! copies the parent's variables except everything that could reach the developer's live herdr,
//! desktop, ssh agent or config (`HERDR_*`, `GLIFFHERDR_*`, `XDG_*`, `WAYLAND_DISPLAY`,
//! `HYPRLAND_*`, `DISPLAY`, `SSH_*`), and then sets the world's own.
#![allow(dead_code)] // each test crate uses a different subset of the helpers

use std::ffi::{OsStr, OsString};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tempfile::TempDir;

/// A `check` report of a machine that is ready: Linux, Hyprland, two screens, gliff-server.
pub const READY: &str = "os=Linux\nhyprland=1\ngliff_server=/usr/bin/gliff-server\noutputs=DP-1 HDMI-A-1\nserver_version=gliff-server 0.4.3\n";

/// What a finished command printed.
#[derive(Debug)]
pub struct Ran {
    /// Exit status (-1 when killed by a signal).
    pub code: i32,
    /// Standard output.
    pub stdout: String,
    /// Standard error.
    pub stderr: String,
}

impl Ran {
    /// Stdout and stderr together, for "contains" assertions.
    pub fn all(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
}

/// Is this parent-environment variable one the tests must never pass on?
fn leaks(key: &OsStr) -> bool {
    let k = key.to_string_lossy();
    k.starts_with("HERDR_")
        || k.starts_with("GLIFFHERDR_")
        || k.starts_with("XDG_")
        || k.starts_with("HYPRLAND_")
        || k.starts_with("SSH_")
        || k == "WAYLAND_DISPLAY"
        || k == "DISPLAY"
}

/// The environment a child gets: the parent's, minus everything that [`leaks`], plus `world`.
pub fn isolated_env(
    parent: impl IntoIterator<Item = (OsString, OsString)>,
    world: &[(String, String)],
) -> Vec<(OsString, OsString)> {
    let mut env: Vec<_> = parent.into_iter().filter(|(k, _)| !leaks(k)).collect();
    for (k, v) in world {
        env.retain(|(ek, _)| ek != OsStr::new(k));
        env.push((k.into(), v.into()));
    }
    env
}

/// A temp dir with `home/`, `state/`, `config/` and `stubs/`, and the environment that points
/// gliffherdr at them.
pub struct World {
    /// The temp dir (removed on drop).
    pub dir: TempDir,
    /// `$HOME`.
    pub home: PathBuf,
    /// `GLIFFHERDR_STATE_DIR`.
    pub state: PathBuf,
    /// `GLIFFHERDR_CONFIG_DIR`.
    pub config: PathBuf,
    /// Stub executables (first on `PATH`).
    pub stubs: PathBuf,
    /// The stubs' fixtures and logs (`$STUB`).
    pub data: PathBuf,
    parent: Vec<(OsString, OsString)>,
    env: Vec<(String, String)>,
}

impl World {
    /// A fresh world with every stub written.
    pub fn new() -> World {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let (home, state, config) = (root.join("home"), root.join("state"), root.join("config"));
        let (stubs, run) = (root.join("stubs"), root.join("run"));
        let data = stubs.join("data");
        for d in [
            &home,
            &state,
            &config,
            &stubs,
            &data,
            &run,
            &home.join(".ssh"),
        ] {
            std::fs::create_dir_all(d).unwrap();
        }
        let parent_path = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into());
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let env = vec![
            ("HOME".into(), s(&home)),
            ("XDG_CONFIG_HOME".into(), s(&home.join(".config"))),
            ("XDG_STATE_HOME".into(), s(&home.join(".local/state"))),
            ("XDG_RUNTIME_DIR".into(), s(&run)),
            ("GLIFFHERDR_CONFIG_DIR".into(), s(&config)),
            ("GLIFFHERDR_STATE_DIR".into(), s(&state)),
            ("HERDR_BIN_PATH".into(), s(&stubs.join("herdr"))),
            ("PATH".into(), format!("{}:{parent_path}", stubs.display())),
            ("WAYLAND_DISPLAY".into(), "wayland-1".into()),
            ("HYPRLAND_INSTANCE_SIGNATURE".into(), "testsig".into()),
            ("GLIFFHERDR_WINDOW_WAIT_MS".into(), "300".into()),
            ("STUB".into(), s(&data)),
        ];
        let world = World {
            dir,
            home,
            state,
            config,
            stubs,
            data,
            parent: std::env::vars_os().collect(),
            env,
        };
        world.write_stubs();
        world
    }

    /// Set (or replace) a variable of the children's environment.
    pub fn set_env(&mut self, key: &str, value: &str) {
        self.env.retain(|(k, _)| k != key);
        self.env.push((key.to_string(), value.to_string()));
    }

    /// Remove a variable the world set, so children do not see it.
    pub fn unset_env(&mut self, key: &str) {
        self.env.retain(|(k, _)| k != key);
    }

    /// Pretend the *parent* process has this variable (to prove isolation works).
    pub fn simulate_parent_env(&mut self, key: &str, value: &str) {
        self.parent.push((key.into(), value.into()));
    }

    /// The variables this world sets (for [`background_command`]).
    pub fn env_pairs(&self) -> Vec<(String, String)> {
        self.env.clone()
    }

    /// The environment children get.
    pub fn child_env(&self) -> Vec<(OsString, OsString)> {
        isolated_env(self.parent.clone(), &self.env)
    }

    /// A command for any program, with the isolated environment. The only way tests start
    /// processes.
    pub fn command_for(&self, program: impl AsRef<OsStr>) -> assert_cmd::Command {
        let mut cmd = assert_cmd::Command::new(program);
        cmd.env_clear()
            .envs(self.child_env())
            .current_dir(&self.home);
        cmd
    }

    /// A command for the `gliffherdr` binary.
    pub fn command(&self) -> assert_cmd::Command {
        let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("gliffherdr");
        cmd.env_clear()
            .envs(self.child_env())
            .current_dir(&self.home);
        cmd
    }

    /// Run `gliffherdr args…` and capture the outcome.
    pub fn run(&self, args: &[&str]) -> Ran {
        ran(self.command().args(args))
    }

    /// A `PATH` of the stubs plus symlinks to just `tools` (found on the real `PATH`), so a test
    /// can rely on a program being absent even when the developer's machine has it installed.
    pub fn minimal_path(&self, tools: &[&str]) -> String {
        let bare = self.dir.path().join("bare");
        std::fs::create_dir_all(&bare).unwrap();
        for tool in tools {
            let link = bare.join(tool);
            if !link.exists() {
                std::os::unix::fs::symlink(find_tool(tool), link).unwrap();
            }
        }
        format!("{}:{}", self.stubs.display(), bare.display())
    }

    // ---- files ---------------------------------------------------------------------------

    /// Write `text` to `config.toml`.
    pub fn write_config(&self, text: &str) {
        std::fs::write(self.config.join("config.toml"), text).unwrap();
    }

    /// Write a fixture or stub data file (under `$STUB`).
    pub fn write_data(&self, name: &str, text: &str) {
        let p = self.data.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    /// Read a stub data file; empty when it does not exist.
    pub fn read_data(&self, name: &str) -> String {
        std::fs::read_to_string(self.data.join(name)).unwrap_or_default()
    }

    /// Wait up to 2 s for a stub data file to exist (detached launches are asynchronous).
    pub fn wait_for_data(&self, name: &str) -> String {
        let end = Instant::now() + Duration::from_secs(2);
        loop {
            if let Ok(text) = std::fs::read_to_string(self.data.join(name)) {
                return text;
            }
            assert!(
                Instant::now() < end,
                "{name} never appeared in {}",
                self.data.display()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Script `ssh` for a check of `dest`: exit status, stdout (the report), stderr.
    pub fn host_answer(&self, dest: &str, rc: i32, out: &str, err: &str) {
        let n = safe(dest);
        self.write_data(&format!("hosts/{n}.out"), out);
        self.write_data(&format!("hosts/{n}.err"), err);
        self.write_data(&format!("hosts/{n}.rc"), &format!("{rc}\n"));
    }

    /// Script what `ssh -G -- dest` prints.
    pub fn ssh_g(&self, dest: &str, user: &str, host: &str) {
        self.write_data(
            &format!("ssh-config/{}", safe(dest)),
            &format!("user {user}\nhostname {host}\nport 22\n"),
        );
    }

    /// Script what `ssh -G -- dest` prints, verbatim.
    pub fn ssh_g_raw(&self, dest: &str, text: &str) {
        self.write_data(&format!("ssh-config/{}", safe(dest)), text);
    }

    /// The targets of every check ssh made, one entry per call, in call order.
    pub fn ssh_checked(&self) -> Vec<String> {
        self.read_data("ssh.log")
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Every ssh invocation's argv, one line per call.
    pub fn ssh_calls(&self) -> Vec<String> {
        self.read_data("ssh.argv")
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// What the check script was fed on stdin for `dest`.
    pub fn ssh_stdin(&self, dest: &str) -> String {
        self.read_data(&format!("stdin/{}", safe(dest)))
    }

    /// Write `~/.ssh/config`.
    pub fn ssh_config(&self, text: &str) {
        std::fs::write(self.home.join(".ssh/config"), text).unwrap();
    }

    /// Write gliff's `config.toml` under the world's `XDG_CONFIG_HOME`.
    pub fn gliff_config(&self, text: &str) {
        let dir = self.home.join(".config/gliff");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), text).unwrap();
    }

    /// The `roster_fixture`: herdr machines, Tailscale peers, ssh config hosts and gliff's
    /// own list.
    pub fn roster_fixture(&self) {
        self.write_data(
            "machines.json",
            r#"[{"id":"a","label":"j","target":"j@10.0.0.2","session":"default","enabled":true},
 {"id":"b","label":"Q","target":"mac@10.0.0.3","session":"default","enabled":true}]"#,
        );
        self.write_data(
            "tailscale.json",
            r#"{"Self":{"HostName":"me","Online":true},
 "Peer":{
  "k1":{"HostName":"j","DNSName":"j.tail.ts.net.","OS":"linux","Online":true,"TailscaleIPs":["10.0.0.2"]},
  "k2":{"HostName":"box","DNSName":"box.tail.ts.net.","OS":"linux","Online":true,"TailscaleIPs":["100.64.0.9"]},
  "k3":{"HostName":"laptop","DNSName":"laptop.tail.ts.net.","OS":"macOS","Online":true,"TailscaleIPs":["100.64.0.10"]},
  "k4":{"HostName":"phone","DNSName":"phone.tail.ts.net.","OS":"iOS","Online":true,"TailscaleIPs":["100.64.0.11"]},
  "k5":{"HostName":"asleep","DNSName":"asleep.tail.ts.net.","OS":"linux","Online":false,"TailscaleIPs":["100.64.0.12"]}}}"#,
        );
        self.ssh_config("Host github.com\n  User git\nHost gitbox\n  User git\nHost devbox\n  HostName 10.0.0.2\n  User j\nHost *\n  ServerAliveInterval 30\n");
        self.ssh_g("github.com", "git", "github.com");
        self.ssh_g("gitbox", "git", "gitbox.lan");
        self.ssh_g("devbox", "j", "10.0.0.2");
        self.config_file_machines("[[machine]]\ntarget = \"me@desk.local\"\nlabel = \"desk\"\n");
        self.gliff_config("machines = [\"k@10.0.0.7\", \"j@10.0.0.2\"]\nopen = []\n");
    }

    /// Append `[[machine]]` entries (or any config) to `config.toml`.
    pub fn config_file_machines(&self, text: &str) {
        let p = self.config.join("config.toml");
        let mut cur = std::fs::read_to_string(&p).unwrap_or_default();
        cur.push_str(text);
        std::fs::write(p, cur).unwrap();
    }

    // ---- stubs ---------------------------------------------------------------------------

    /// Write one executable stub: a POSIX `sh` script.
    pub fn write_stub(&self, name: &str, body: &str) {
        let p = self.stubs.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn write_stubs(&self) {
        self.write_stub("ssh", SSH);
        self.write_stub("herdr", HERDR);
        self.write_stub("tailscale", TAILSCALE);
        self.write_stub("hyprctl", HYPRCTL);
        self.write_stub("uwsm", UWSM);
        self.write_stub("uwsm-app", UWSM_APP);
        self.write_stub("gliff", GLIFF);
        self.write_stub("systemctl", SYSTEMCTL);
    }
}

impl Default for World {
    fn default() -> Self {
        World::new()
    }
}

/// A `std` command for a long-running child (sshd, a herdr server) that the test spawns and
/// kills itself. Like [`World::command`] it starts from an empty environment: the parent's minus
/// everything that [`leaks`], plus `world_env`. Tests build processes only here.
#[allow(clippy::disallowed_methods)] // the harness is the one place tests may build a Command
pub fn background_command(
    program: impl AsRef<OsStr>,
    world_env: &[(String, String)],
) -> std::process::Command {
    let mut cmd = std::process::Command::new(program);
    cmd.env_clear()
        .envs(isolated_env(std::env::vars_os(), world_env));
    cmd
}

/// Run a prepared command and capture the outcome.
pub fn ran(cmd: &mut assert_cmd::Command) -> Ran {
    let out = cmd.output().unwrap();
    Ran {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// The absolute path of a program on the real `PATH`.
pub fn find_tool(name: &str) -> PathBuf {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
        .unwrap_or_else(|| panic!("{name} is not on PATH"))
}

/// File-name form of a destination, as the stubs compute it.
fn safe(dest: &str) -> String {
    dest.replace('/', "_")
}

/// `ssh -G -- DEST` answers from `$STUB/ssh-config/<dest>` or generically; a check
/// (`ssh [opts] -- DEST sh -s`) records its stdin and answers from `$STUB/hosts/<dest>.{out,err,rc}`
/// (status 255 when nothing is scripted).
const SSH: &str = r#"echo "$*" >>"$STUB/ssh.argv"
if [ "$1" = -G ]; then
  for dest; do :; done
  safe=$(printf '%s' "$dest" | tr '/' '_')
  if [ -f "$STUB/ssh-config/$safe" ]; then cat "$STUB/ssh-config/$safe"; exit 0; fi
  user=testuser
  host=$dest
  case $dest in *@*) user=${dest%@*}; host=${dest#*@} ;; esac
  printf 'user %s\nhostname %s\nport 22\n' "$user" "$host"
  exit 0
fi
while [ $# -gt 0 ] && [ "$1" != -- ]; do shift; done
dest=$2
safe=$(printf '%s' "$dest" | tr '/' '_')
mkdir -p "$STUB/stdin"
cat >"$STUB/stdin/$safe"
cp "$STUB/stdin/$safe" "$STUB/last-stdin"
echo "$dest" >>"$STUB/ssh.log"
[ -f "$STUB/hosts/$safe.out" ] && cat "$STUB/hosts/$safe.out"
[ -f "$STUB/hosts/$safe.err" ] && cat "$STUB/hosts/$safe.err" >&2
exit "$(cat "$STUB/hosts/$safe.rc" 2>/dev/null || echo 255)"
"#;

/// `herdr machine list --json` prints `$STUB/machines.json`; `pane process-info` prints
/// `$STUB/process-info.json`; `--version` prints a version; every call is logged.
const HERDR: &str = r#"echo "$*" >>"$STUB/herdr.log"
if [ "$1" = machine ] && [ "$2" = list ]; then cat "$STUB/machines.json" 2>/dev/null || echo '[]'; fi
if [ "$1" = pane ] && [ "$2" = process-info ]; then cat "$STUB/process-info.json" 2>/dev/null; fi
if [ "$1" = --version ]; then echo "herdr 0.0.0-stub"; fi
exit 0
"#;

/// `tailscale status --json` prints `$STUB/tailscale.json` (no peers when absent).
const TAILSCALE: &str = r#"if [ "$1" = status ]; then cat "$STUB/tailscale.json" 2>/dev/null || echo '{"Peer":{}}'; fi
exit 0
"#;

/// `hyprctl`: logs `HIS=<signature>|<args>`; `clients -j` prints `$STUB/clients.json`;
/// `dispatch` prints `$STUB/dispatch.reply` or `ok`; `instances` prints `$STUB/instances.txt`.
const HYPRCTL: &str = r#"printf 'HIS=%s|%s\n' "${HYPRLAND_INSTANCE_SIGNATURE:-}" "$*" >>"$STUB/hyprctl.log"
[ "$1" = -i ] && shift 2
case $1 in
  clients) cat "$STUB/clients.json" 2>/dev/null || echo '[]' ;;
  dispatch) cat "$STUB/dispatch.reply" 2>/dev/null || echo ok ;;
  instances) cat "$STUB/instances.txt" 2>/dev/null ;;
  version) if [ "$2" = -j ]; then echo '{"branch":"stub","version":"0.0.0-stub"}'; else echo "Hyprland stub"; fi ;;
  monitors) printf 'Monitor DP-1 (ID 0):\n\t2560x1440\nMonitor HDMI-A-1 (ID 1):\n\t1920x1080\n' ;;
esac
exit 0
"#;

/// `uwsm check is-active` succeeds only when `$STUB/uwsm-active` exists.
const UWSM: &str = r#"[ -f "$STUB/uwsm-active" ]
"#;

/// `uwsm-app [opts] -- CMD…` logs its argv one per line and execs `CMD…`.
const UWSM_APP: &str = r#"printf '%s\n' "$@" >"$STUB/uwsm-app.argv"
while [ $# -gt 0 ] && [ "$1" != -- ]; do shift; done
shift
exec "$@"
"#;

/// `gliff`: writes its environment, adds its window to `$STUB/clients.json` (unless
/// `$STUB/no-window` exists) and finally its argv (one per line) to `$STUB/gliff.argv`, which is
/// the file tests wait for.
const GLIFF: &str = r#"if [ "$1" = --version ]; then echo "gliff 0.4.3"; exit 0; fi
printf 'XDG_CONFIG_HOME=%s\nWAYLAND_DISPLAY=%s\n' "${XDG_CONFIG_HOME:-}" "${WAYLAND_DISPLAY:-}" >"$STUB/gliff.env"
if [ ! -f "$STUB/no-window" ]; then
  for target; do :; done
  printf '[{"class":"gliff","title":"%s — Gliff","address":"0xnew"}]\n' "$target" >"$STUB/clients.json"
fi
printf '%s\n' "$@" >"$STUB/gliff.argv"
exit 0
"#;

/// `systemctl --user show-environment` prints `$STUB/systemd-env.txt`.
const SYSTEMCTL: &str = r#"cat "$STUB/systemd-env.txt" 2>/dev/null
exit 0
"#;
