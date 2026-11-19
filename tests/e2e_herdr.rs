//! The plugin inside a real, isolated herdr server (docs/design/07-testing.md §Real herdr).
//!
//! Every herdr process here runs with an empty environment plus `HOME`, `XDG_*` pointing into a
//! temp dir and a session named `gliffherdr-e2e-<pid>`: the developer's own herdr (sockets,
//! plugin registry, sessions) is never reachable. The plugin directory is a temp copy of the
//! manifest; its `bin/gliffherdr` is a tiny recording wrapper around the freshly built binary
//! (`CARGO_BIN_EXE_gliffherdr`), so the tests can see exactly what herdr passed to every command,
//! including popups that open and close faster than a test can look.
//!
//! Gated: set `GLIFFHERDR_E2E_HERDR=1` and `GLIFFHERDR_HERDR_BIN=<path to herdr>` (else `herdr`
//! on `PATH`). Without the gate every test prints `skipped` and passes; with `CI=true` a missing
//! herdr is a failure.

mod common;

use std::collections::HashMap;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use common::{World, background_command, isolated_env};
use gliffherdr::herdr::{Herdr as HerdrApi, HerdrError};
use gliffherdr::sys::{Cmd, Output, Sys};
use serde_json::Value;

const PLUGIN: &str = "jaden-hoenes.gliffherdr";
const BIN: &str = env!("CARGO_BIN_EXE_gliffherdr");

fn enabled() -> bool {
    if std::env::var("GLIFFHERDR_E2E_HERDR").as_deref() == Ok("1") {
        return true;
    }
    println!(
        "skipped: set GLIFFHERDR_E2E_HERDR=1 (and GLIFFHERDR_HERDR_BIN) to run the real-herdr suite"
    );
    false
}

/// `GLIFFHERDR_HERDR_BIN`, else `herdr` on `PATH`.
fn herdr_bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("GLIFFHERDR_HERDR_BIN").filter(|p| !p.is_empty()) {
        let p = PathBuf::from(p);
        assert!(
            p.is_file(),
            "GLIFFHERDR_HERDR_BIN={} is not a file",
            p.display()
        );
        return Some(p);
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    let found = std::env::split_paths(&path)
        .map(|d| d.join("herdr"))
        .find(|p| p.is_file());
    if found.is_none() {
        assert!(
            std::env::var("CI").as_deref() != Ok("true"),
            "CI=true but no herdr: set GLIFFHERDR_HERDR_BIN"
        );
        println!("skipped: no herdr binary (set GLIFFHERDR_HERDR_BIN)");
    }
    found
}

fn write_exec(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Poll `f` until it returns `Some`, or panic with `what` after `secs`.
fn wait_for<T>(what: &str, secs: u64, mut f: impl FnMut() -> Option<T>) -> T {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(
            Instant::now() < end,
            "timed out after {secs}s waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// What a finished herdr CLI call printed.
struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Out {
    fn json(&self) -> Value {
        serde_json::from_str(self.stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}\n{}", self.stdout, self.stderr))
    }
}

/// An isolated herdr: its own HOME and XDG dirs, a plugin dir and (once started) a server.
struct Herdr {
    /// Everything: homes, plugin dir, logs. Kept (and printed) when a test fails.
    root: Option<tempfile::TempDir>,
    /// The stub world whose `PATH` entries shadow gliff, hyprctl, uwsm, tailscale and ssh stubs;
    /// held so its directories live as long as the server.
    #[allow(dead_code)] // only kept alive, no longer read since the picker is built
    w: World,
    bin: PathBuf,
    session: String,
    env: Vec<(String, String)>,
    server: Option<Child>,
    /// Ports nothing listens on, for the two configured machines.
    closed: [u16; 2],
    /// A short symlink to `root/config`, used as `XDG_CONFIG_HOME` when the real path would make
    /// herdr's socket path longer than `sun_path` allows (long `TMPDIR`, as on CI runners).
    config_link: Option<PathBuf>,
}

impl Herdr {
    fn new(bin: &Path) -> Herdr {
        let root = tempfile::Builder::new()
            .prefix("gliffherdr-e2e-herdr-")
            .tempdir()
            .unwrap();
        let r = root.path().to_path_buf();
        let s = |p: PathBuf| p.to_string_lossy().into_owned();
        let session = format!("gliffherdr-e2e-{}", std::process::id());
        // herdr puts its sockets under $XDG_CONFIG_HOME/herdr/sessions/<session>/; a unix socket
        // path is limited to 107 bytes.
        let longest = r
            .join("config/herdr/sessions")
            .join(&session)
            .join("herdr-client.sock");
        let (config_home, config_link) = if longest.as_os_str().len() > 100 {
            let name = r.file_name().unwrap().to_string_lossy().into_owned();
            let link = PathBuf::from("/tmp").join(format!("ge-{}", &name[name.len() - 6..]));
            std::os::unix::fs::symlink(r.join("config"), &link).unwrap();
            (link.clone(), Some(link))
        } else {
            (r.join("config"), None)
        };
        for d in [
            "home",
            "config",
            "state",
            "data",
            "cache",
            "run",
            "plugin/bin",
            "inv",
        ] {
            std::fs::create_dir_all(r.join(d)).unwrap();
        }
        std::fs::set_permissions(r.join("run"), std::fs::Permissions::from_mode(0o700)).unwrap();

        // The stub world: stub gliff/hyprctl/uwsm/tailscale first on PATH so the real ones can
        // never be reached from a plugin command. A stub `ssh` would hide the real one: drop it.
        let w = World::new();
        std::fs::remove_file(w.stubs.join("ssh")).unwrap();
        let real_path = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into());
        let env = vec![
            ("HOME".to_string(), s(r.join("home"))),
            ("XDG_CONFIG_HOME".into(), s(config_home.clone())),
            ("XDG_STATE_HOME".into(), s(r.join("state"))),
            ("XDG_DATA_HOME".into(), s(r.join("data"))),
            ("XDG_CACHE_HOME".into(), s(r.join("cache"))),
            ("XDG_RUNTIME_DIR".into(), s(r.join("run"))),
            ("PATH".into(), format!("{}:{real_path}", w.stubs.display())),
            ("STUB".into(), s(w.data.clone())),
            // Panes start $SHELL; a user's zsh would run its new-user wizard in the empty HOME.
            ("SHELL".into(), "/bin/sh".into()),
            ("WAYLAND_DISPLAY".into(), "wayland-1".into()),
            ("HYPRLAND_INSTANCE_SIGNATURE".into(), "testsig".into()),
            ("GLIFFHERDR_WINDOW_WAIT_MS".into(), "300".into()),
        ];

        // A herdr that is a launcher script (it resolves its real binary under $HOME) still has to
        // find it from the isolated HOME: link the real data dir in.
        let mut head = [0u8; 4];
        if let Ok(mut f) = std::fs::File::open(bin) {
            use std::io::Read;
            let _ = f.read(&mut head);
        }
        if head != *b"\x7fELF"
            && let Some(real_home) = std::env::var_os("HOME")
        {
            let share = PathBuf::from(real_home).join(".local/share/herdr");
            if share.is_dir() {
                std::fs::create_dir_all(r.join("home/.local/share")).unwrap();
                std::os::unix::fs::symlink(share, r.join("home/.local/share/herdr")).unwrap();
            }
        }

        // The plugin dir: the manifest and a recording wrapper as bin/gliffherdr.
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("herdr-plugin.toml"),
            r.join("plugin/herdr-plugin.toml"),
        )
        .unwrap();
        std::os::unix::fs::symlink(BIN, r.join("plugin/bin/gliffherdr.real")).unwrap();
        write_exec(&r.join("plugin/bin/gliffherdr"), &wrapper(&r.join("inv")));

        let h = Herdr {
            root: Some(root),
            w,
            bin: bin.to_path_buf(),
            session,
            env,
            server: None,
            closed: [free_port(), free_port()],
            config_link,
        };
        // The user's real herdr must be unreachable from this environment.
        let sessions = h.cli(&["session", "list"]);
        assert!(
            sessions
                .stdout
                .contains(&h.config_home().display().to_string()),
            "isolation check failed, session list shows foreign dirs:\n{}",
            sessions.stdout
        );
        let plugins = h.cli(&["plugin", "list"]);
        assert!(
            plugins.stdout.contains("No plugins installed"),
            "isolation check failed, a fresh herdr has plugins:\n{}",
            plugins.stdout
        );
        h
    }

    fn root(&self) -> &Path {
        self.root.as_ref().unwrap().path()
    }

    /// The `XDG_CONFIG_HOME` herdr runs with (`root/config`, or a short symlink to it).
    fn config_home(&self) -> PathBuf {
        PathBuf::from(
            self.env
                .iter()
                .find(|(k, _)| k == "XDG_CONFIG_HOME")
                .map(|(_, v)| v.clone())
                .unwrap(),
        )
    }

    fn plugin_dir(&self) -> PathBuf {
        self.root().join("plugin")
    }

    /// `herdr <args>` (no session flag), isolated, with a timeout.
    fn cli(&self, args: &[&str]) -> Out {
        let mut cmd = assert_cmd::Command::new(&self.bin);
        cmd.env_clear()
            .envs(isolated_env(std::env::vars_os(), &self.env))
            .current_dir(self.root())
            .timeout(Duration::from_secs(30))
            .args(args);
        let o = cmd.output().unwrap();
        Out {
            code: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    /// `herdr --session <ours> <args>`.
    fn session_cli(&self, args: &[&str]) -> Out {
        let mut all = vec!["--session", self.session.as_str()];
        all.extend_from_slice(args);
        self.cli(&all)
    }

    fn link(&self) -> Out {
        let dir = self.plugin_dir();
        self.cli(&["plugin", "link", dir.to_str().unwrap()])
    }

    /// The plugin's config dir as herdr computes it.
    fn config_dir(&self) -> PathBuf {
        let o = self.cli(&["plugin", "config-dir", PLUGIN]);
        assert_eq!(o.code, 0, "{}{}", o.stdout, o.stderr);
        PathBuf::from(o.stdout.trim())
    }

    /// A `config.toml` with two closed-port machines and every other source off.
    fn write_config(&self) {
        let [a, b] = self.closed;
        let text = format!(
            "[sources]\nherdr = false\nrecent = false\ngliff = false\nssh_config = false\n\
             tailscale = false\n\n[check]\nssh_timeout_secs = 1\n\n\
             [[machine]]\ntarget = \"ssh://e2e@127.0.0.1:{a}\"\nlabel = \"closed-a\"\n\n\
             [[machine]]\ntarget = \"ssh://e2e@127.0.0.1:{b}\"\nlabel = \"closed-b\"\n"
        );
        let dir = self.config_dir();
        std::fs::write(dir.join("config.toml"), text).unwrap();
    }

    /// Where the plugin's check cache lives under herdr's state dir.
    fn checks_dir(&self) -> PathBuf {
        self.root()
            .join("state/herdr/plugins")
            .join(PLUGIN)
            .join("checks")
    }

    /// Start `herdr --session <ours> server` in the background and wait until it reports running.
    fn start_server(&mut self) {
        let log = std::fs::File::create(self.root().join("server.log")).unwrap();
        let log2 = log.try_clone().unwrap();
        let mut cmd = background_command(&self.bin, &self.env);
        cmd.args(["--session", &self.session, "server"])
            .current_dir(self.root())
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(log2);
        self.server = Some(cmd.spawn().expect("start herdr server"));
        wait_for("the herdr server to report running", 30, || {
            if let Some(child) = self.server.as_mut()
                && let Some(st) = child.try_wait().unwrap()
            {
                panic!(
                    "herdr server exited early ({st}): {}",
                    std::fs::read_to_string(self.root().join("server.log")).unwrap_or_default()
                );
            }
            let o = self.session_cli(&["status"]);
            o.stdout.contains("status: running").then_some(())
        });
    }

    /// The plugin's command log entries, oldest first.
    fn logs(&self) -> Vec<Value> {
        let o = self.session_cli(&[
            "plugin", "log", "list", "--plugin", PLUGIN, "--limit", "100",
        ]);
        if o.code != 0 {
            return Vec::new();
        }
        o.json()["result"]["logs"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    /// Invoke an action and wait for its log entry to leave `running`; returns that entry.
    fn invoke(&self, action: &str) -> Value {
        let o = self.session_cli(&["plugin", "action", "invoke", action, "--plugin", PLUGIN]);
        assert_eq!(o.code, 0, "invoke {action}: {}{}", o.stdout, o.stderr);
        let id = o.json()["result"]["log"]["log_id"]
            .as_str()
            .expect("log id")
            .to_string();
        wait_for(&format!("the {action} command to finish"), 30, || {
            self.logs()
                .into_iter()
                .find(|l| l["log_id"] == id.as_str() && l["status"] != "running")
        })
    }

    /// The recording wrapper's files, one per invocation: (argv line, whole text).
    fn invocations(&self) -> Vec<String> {
        let mut files: Vec<_> = std::fs::read_dir(self.root().join("inv"))
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect();
        files.sort();
        files
            .iter()
            .filter_map(|p| std::fs::read_to_string(p).ok())
            .collect()
    }

    fn create_workspace(&self) -> Value {
        let cwd = self.root().to_string_lossy().into_owned();
        let o = self.session_cli(&[
            "workspace",
            "create",
            "--cwd",
            &cwd,
            "--label",
            "e2e",
            "--focus",
        ]);
        assert_eq!(o.code, 0, "{}{}", o.stdout, o.stderr);
        o.json()["result"].clone()
    }

    /// One raw socket request (`{"id","method","params":{}}`) to this session's API socket.
    fn api(&self, method: &str) -> Value {
        use std::io::{BufRead, BufReader, Write};
        let sock = self
            .config_home()
            .join("herdr/sessions")
            .join(&self.session)
            .join("herdr.sock");
        let mut stream = std::os::unix::net::UnixStream::connect(&sock)
            .unwrap_or_else(|e| panic!("connect {}: {e}", sock.display()));
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        writeln!(
            stream,
            r#"{{"id":"e2e","method":"{method}","params":{{}}}}"#
        )
        .unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("{method} reply {line:?}: {e}"))
    }

    /// Stop the server: `server stop`, then wait; kill it by PID if it will not go.
    fn stop_server(&mut self) {
        let Some(mut child) = self.server.take() else {
            return;
        };
        let _ = self.session_cli(&["server", "stop"]);
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            if child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

impl Drop for Herdr {
    fn drop(&mut self) {
        let failed = std::thread::panicking();
        if failed {
            eprintln!("--- plugin log ---");
            for l in self.logs() {
                eprintln!("{l}");
            }
            eprintln!("--- wrapper invocations ---");
            for i in self.invocations() {
                eprintln!("{i}");
            }
            let server_log =
                std::fs::read_to_string(self.root().join("server.log")).unwrap_or_default();
            eprintln!("--- server stdout/stderr ---\n{server_log}");
        }
        self.stop_server();
        if let Some(link) = &self.config_link {
            let _ = std::fs::remove_file(link);
        }
        if failed && let Some(root) = self.root.take() {
            eprintln!(
                "e2e_herdr failed: temp dir kept at {}",
                root.keep().display()
            );
        }
    }
}

/// The recording wrapper installed as `bin/gliffherdr`: one file per invocation with argv, the
/// variables herdr is documented to set and the context JSON, then the real binary runs and its
/// exit code is recorded.
fn wrapper(inv: &Path) -> String {
    format!(
        r#"#!/bin/sh
out={inv}/$$.txt
{{
  printf 'argv:'; for a in "$@"; do printf ' %s' "$a"; done; printf '\n'
  printf 'pwd: %s\n' "$PWD"
  printf 'entrypoint: %s\n' "${{HERDR_PLUGIN_ENTRYPOINT_ID:-}}"
  printf 'action: %s\n' "${{HERDR_PLUGIN_ACTION_ID:-}}"
  printf 'event: %s\n' "${{HERDR_PLUGIN_EVENT:-}}"
  printf 'plugin_id: %s\n' "${{HERDR_PLUGIN_ID:-}}"
  printf 'plugin_root: %s\n' "${{HERDR_PLUGIN_ROOT:-}}"
  printf 'config_dir: %s\n' "${{HERDR_PLUGIN_CONFIG_DIR:-}}"
  printf 'state_dir: %s\n' "${{HERDR_PLUGIN_STATE_DIR:-}}"
  printf 'bin_path: %s\n' "${{HERDR_BIN_PATH:-}}"
  printf 'pane_id: %s\n' "${{HERDR_PANE_ID:-}}"
  printf 'env: %s\n' "${{HERDR_ENV:-}}"
  printf 'fix_target: %s\n' "${{GLIFFHERDR_FIX_TARGET:-}}"
  printf 'context: %s\n' "${{HERDR_PLUGIN_CONTEXT_JSON:-}}"
}} >"$out"
"$(dirname "$0")/gliffherdr.real" "$@"
rc=$?
echo "exit: $rc" >>"$out"
exit $rc
"#,
        inv = inv.display()
    )
}

/// A field of one wrapper record (`key: value`).
fn field<'a>(record: &'a str, key: &str) -> Option<&'a str> {
    record
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{key}: ")))
}

/// The wrapper records whose argv is `args`.
fn records_for<'a>(records: &'a [String], args: &str) -> Vec<&'a String> {
    records
        .iter()
        .filter(|r| field(r, "argv") == Some(args))
        .collect()
}

/// Processes of this test's herdr (their environment mentions `root`) whose last argument is
/// `last` and that run one of our binaries. Looking at the environment keeps a developer's own
/// running picker out of the result.
fn processes_of(root: &Path, last: &str) -> Vec<u32> {
    let root = root.to_string_lossy().into_owned();
    let mut found = Vec::new();
    for entry in std::fs::read_dir("/proc").unwrap().filter_map(Result::ok) {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(cmdline) = std::fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let args: Vec<String> = cmdline
            .split(|b| *b == 0)
            .filter(|a| !a.is_empty())
            .map(|a| String::from_utf8_lossy(a).into_owned())
            .collect();
        let ours = args
            .iter()
            .any(|a| a.ends_with("bin/gliffherdr") || a.ends_with("bin/gliffherdr.real"));
        if args.last().map(String::as_str) != Some(last) || !ours {
            continue;
        }
        let Ok(environ) = std::fs::read(entry.path().join("environ")) else {
            continue;
        };
        if String::from_utf8_lossy(&environ).contains(&root) {
            found.push(pid);
        }
    }
    found
}

/// Run `body` against an isolated herdr, or skip.
fn with_herdr(body: impl FnOnce(&mut Herdr)) {
    if !enabled() {
        return;
    }
    let Some(bin) = herdr_bin() else { return };
    let mut h = Herdr::new(&bin);
    body(&mut h);
}

#[test]
fn linking_registers_three_actions_and_two_panes_with_the_manifest_titles() {
    with_herdr(|h| {
        let o = h.link();
        assert_eq!(o.code, 0, "{}{}", o.stdout, o.stderr);
        let plugin = &o.json()["result"]["plugin"];
        assert_eq!(plugin["plugin_id"], PLUGIN);
        assert_eq!(plugin["enabled"], true);
        let titles = |kind: &str| -> Vec<(String, String)> {
            let mut v: Vec<_> = plugin[kind]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| {
                    (
                        e["id"].as_str().unwrap().into(),
                        e["title"].as_str().unwrap().into(),
                    )
                })
                .collect();
            v.sort();
            v
        };
        assert_eq!(
            titles("actions"),
            [
                ("connect", "Remote desktop: connect to a machine"),
                ("connect-pane", "Remote desktop: this pane's machine"),
                ("recheck", "Remote desktop: re-check machines"),
            ]
            .map(|(a, b)| (a.to_string(), b.to_string()))
        );
        assert_eq!(
            titles("panes"),
            [("fix", "Remote desktop: fix"), ("picker", "Remote desktop")]
                .map(|(a, b)| (a.to_string(), b.to_string()))
        );
        // herdr's own config dir for the plugin is where our path rules look when run from a shell.
        let cfg = h.config_dir();
        assert_eq!(
            cfg,
            h.config_home().join("herdr/plugins/config").join(PLUGIN)
        );
        let list = h.cli(&["plugin", "list"]);
        assert!(
            list.stdout.contains("jaden-hoenes.gliffherdr"),
            "{}",
            list.stdout
        );
    });
}

#[test]
fn the_startup_hook_checks_both_machines_into_herdrs_state_dir() {
    with_herdr(|h| {
        assert_eq!(h.link().code, 0);
        h.write_config();
        h.start_server();
        let dir = h.checks_dir();
        let files = wait_for("two offline check files from the startup hook", 20, || {
            let mut found: Vec<Value> = std::fs::read_dir(&dir)
                .ok()?
                .filter_map(Result::ok)
                .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                .filter_map(|t| serde_json::from_str(&t).ok())
                .collect();
            found.sort_by_key(|v| v["target"].as_str().unwrap_or("").to_string());
            (found.len() == 2).then_some(found)
        });
        for (f, port) in files.iter().zip(sorted(h.closed)) {
            assert_eq!(f["status"]["kind"], "offline", "{f}");
            assert_eq!(f["target"], format!("ssh://e2e@127.0.0.1:{port}"));
        }
        // The hook itself: run by herdr as the startup event with herdr's own directories.
        let recs = h.invocations();
        let startup = records_for(&recs, "check --startup --quiet");
        assert_eq!(startup.len(), 1, "{recs:?}");
        let rec = startup[0];
        assert_eq!(field(rec, "event"), Some("startup"));
        assert_eq!(field(rec, "plugin_id"), Some(PLUGIN));
        assert_eq!(field(rec, "env"), Some("1"));
        assert_eq!(
            field(rec, "pwd").map(Path::new),
            Some(h.plugin_dir().as_path())
        );
        assert_eq!(
            field(rec, "state_dir").map(Path::new),
            h.checks_dir().parent()
        );
        assert!(rec.ends_with("exit: 0\n"), "{rec}");
        let log = h
            .logs()
            .into_iter()
            .find(|l| l["event"] == "startup")
            .expect("startup log entry");
        assert_eq!(log["exit_code"], 0, "{log}");
    });
}

fn sorted(ports: [u16; 2]) -> [String; 2] {
    let mut v = ports.map(|p| format!("ssh://e2e@127.0.0.1:{p}"));
    v.sort();
    v.map(|t| t.rsplit(':').next().unwrap().to_string())
}

#[test]
fn the_recheck_action_runs_and_reports_both_machines_offline() {
    with_herdr(|h| {
        assert_eq!(h.link().code, 0);
        h.write_config();
        h.start_server();
        let log = h.invoke("recheck");
        assert_eq!(log["status"], "succeeded", "{log}");
        assert_eq!(log["exit_code"], 0, "{log}");
        let stdout = log["stdout"].as_str().unwrap();
        assert_eq!(stdout.matches("offline").count(), 2, "{stdout}");
        for p in h.closed {
            assert!(stdout.contains(&format!("127.0.0.1:{p}")), "{stdout}");
        }
        let recs = h.invocations();
        let rec = records_for(&recs, "check --all --notify");
        assert_eq!(rec.len(), 1, "{recs:?}");
        assert_eq!(field(rec[0], "action"), Some("recheck"));
        let bin_path = field(rec[0], "bin_path").unwrap_or_default();
        assert!(Path::new(bin_path).is_file(), "HERDR_BIN_PATH={bin_path:?}");
    });
}

#[test]
fn the_connect_action_without_a_workspace_fails_with_herdrs_message() {
    with_herdr(|h| {
        assert_eq!(h.link().code, 0);
        h.write_config();
        h.start_server();
        // A headless server has no active workspace, so herdr cannot place a popup.
        let log = h.invoke("connect");
        assert_eq!(log["status"], "failed", "{log}");
        assert_eq!(log["exit_code"], 1, "{log}");
        assert_eq!(
            log["stderr"].as_str().unwrap().trim(),
            "gliffherdr: no active workspace"
        );
    });
}

#[test]
fn the_connect_action_opens_the_picker_popup() {
    with_herdr(|h| {
        assert_eq!(h.link().code, 0);
        h.write_config();
        h.start_server();
        let ws = h.create_workspace();
        let pane = ws["root_pane"]["pane_id"].as_str().unwrap().to_string();

        let log = h.invoke("connect");
        assert_eq!(log["status"], "succeeded", "{log}");
        assert_eq!(log["exit_code"], 0, "{log}");
        assert_eq!(log["stderr"], "", "{log}");

        // herdr started the manifest's picker command as a popup, with the plugin env and no pane.
        let rec = wait_for("herdr to run `bin/gliffherdr pick`", 15, || {
            h.invocations()
                .into_iter()
                .find(|r| field(r, "argv") == Some("pick"))
        });
        assert_eq!(field(&rec, "entrypoint"), Some("picker"), "{rec}");
        assert_eq!(field(&rec, "plugin_id"), Some(PLUGIN));
        assert_eq!(
            field(&rec, "pane_id"),
            Some(""),
            "a popup gets no HERDR_PANE_ID: {rec}"
        );
        assert_eq!(field(&rec, "action"), Some(""));
        assert_eq!(
            field(&rec, "pwd").map(Path::new),
            Some(h.plugin_dir().as_path())
        );
        let context: Value = serde_json::from_str(field(&rec, "context").unwrap()).unwrap();
        assert_eq!(
            context["focused_pane_id"], pane,
            "the underlying pane is in the context JSON: {rec}"
        );
        // Our own parser reads that very JSON (popups have no HERDR_PANE_ID).
        let ours = gliffherdr::herdr::pane_from_context(field(&rec, "context").unwrap());
        assert_eq!(ours.as_deref(), Some(pane.as_str()));

        // The popup runs the real picker: a live `bin/gliffherdr pick` process, even with no
        // Hyprland session in this environment.
        let pids = wait_for("a running `gliffherdr pick` process", 10, || {
            let p = processes_of(h.root(), "pick");
            (!p.is_empty()).then_some(p)
        });
        // Close it the way herdr's API does (popup.close is socket-only), then it must be gone.
        let reply = h.api("popup.close");
        assert_eq!(reply["result"]["type"], "ok", "{reply}");
        wait_for("the popup process to end", 10, || {
            pids.iter()
                .all(|p| !Path::new(&format!("/proc/{p}")).exists())
                .then_some(())
        });
        assert_eq!(h.api("popup.close")["error"]["code"], "popup_not_open");
    });
}

/// Accept connections on `listener` and never answer: a real `ssh` pointed at it waits in the
/// banner exchange, so it stays the pane's foreground process.
fn hold(listener: TcpListener) {
    std::thread::spawn(move || {
        let mut held = Vec::new();
        while let Ok((s, _)) = listener.accept() {
            held.push(s);
        }
    });
}

#[test]
fn the_connect_pane_action_finds_the_ssh_destination_of_the_focused_pane() {
    with_herdr(|h| {
        assert_eq!(h.link().code, 0);
        h.write_config();
        h.start_server();
        let ws = h.create_workspace();
        let pane = ws["root_pane"]["pane_id"].as_str().unwrap().to_string();

        // A real ssh in the pane, stuck waiting for a server that never speaks.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        hold(listener);
        let cmd = format!("ssh -p {port} -l e2e 127.0.0.1");
        let run = h.session_cli(&["pane", "run", &pane, &cmd]);
        assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
        let argv = wait_for("ssh to be the pane's foreground process", 20, || {
            let info = h.session_cli(&["pane", "process-info", "--pane", &pane]);
            let procs = info.json()["result"]["process_info"]["foreground_processes"].clone();
            procs.as_array()?.iter().find_map(|p| {
                let argv: Vec<String> = p["argv"]
                    .as_array()?
                    .iter()
                    .filter_map(|a| a.as_str().map(String::from))
                    .collect();
                (argv.first().is_some_and(|a| a.ends_with("ssh"))).then_some(argv)
            })
        });
        assert_eq!(
            argv.last().map(String::as_str),
            Some("127.0.0.1"),
            "{argv:?}"
        );

        // The action runs with the focused pane in its environment and context; the machine it
        // finds is checked (config: one second to answer) and found unreachable, which is what a
        // failed connect does without a terminal: say so on stderr and notify.
        let log = h.invoke("connect-pane");
        let target = format!("ssh://e2e@127.0.0.1:{port}");
        assert_eq!(log["status"], "failed", "{log}");
        assert_eq!(log["exit_code"], 1, "{log}");
        let stderr = log["stderr"].as_str().unwrap();
        assert!(
            stderr.contains(&format!("{target} is not reachable")),
            "stderr: {stderr}"
        );
        let recs = h.invocations();
        let rec = records_for(&recs, "connect --from-pane");
        assert_eq!(rec.len(), 1, "{recs:?}");
        assert_eq!(field(rec[0], "action"), Some("connect-pane"));
        assert_eq!(field(rec[0], "pane_id"), Some(pane.as_str()), "{}", rec[0]);
        // ... and the check result it cached is for exactly that destination.
        let cached = std::fs::read_dir(h.checks_dir())
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .any(|t| t.contains(&target) && t.contains("offline"));
        assert!(cached, "no cached check for {target}");
    });
}

/// A [`Sys`] that runs commands in an isolated environment and answers `env` from a fixed map:
/// what a plugin command sees inside herdr (`HERDR_BIN_PATH`, `HERDR_SOCKET_PATH`,
/// `HERDR_PLUGIN_ID`), so the library's own `Herdr` wrapper can be driven against a real server.
struct PluginSys {
    env: HashMap<String, String>,
    base: Vec<(String, String)>,
}

impl Sys for PluginSys {
    fn run(&self, cmd: &Cmd) -> std::io::Result<Output> {
        let mut vars = self.base.clone();
        vars.extend(cmd.env.iter().cloned());
        let mut child = background_command(&cmd.program, &vars)
            .args(&cmd.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let end = Instant::now() + cmd.timeout.unwrap_or(Duration::from_secs(30));
        let mut timed_out = false;
        while child.try_wait()?.is_none() {
            if Instant::now() > end {
                timed_out = true;
                let _ = child.kill();
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let out = child.wait_with_output()?;
        Ok(Output {
            code: if timed_out { None } else { out.status.code() },
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            timed_out,
        })
    }

    fn run_interactive(&self, _: &Cmd) -> std::io::Result<i32> {
        Err(std::io::Error::other("not used by the herdr wrapper"))
    }

    fn spawn_detached(&self, _: &Cmd, _: &Path) -> std::io::Result<()> {
        Err(std::io::Error::other("not used by the herdr wrapper"))
    }

    fn which(&self, _: &str) -> Option<PathBuf> {
        None
    }

    fn env(&self, key: &str) -> Option<String> {
        self.env.get(key).cloned()
    }

    fn now_unix(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    }

    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

#[test]
fn the_herdr_wrapper_speaks_to_a_real_server() {
    with_herdr(|h| {
        assert_eq!(h.link().code, 0);
        h.write_config();
        h.start_server();
        let ws = h.create_workspace();
        let pane = ws["root_pane"]["pane_id"].as_str().unwrap().to_string();

        // What herdr puts in a plugin command's environment; a socket path selects the session.
        // HERDR_BIN_PATH is the one herdr itself reports to the startup hook (the real binary,
        // not a launcher script that would reset the session).
        let hook = wait_for("the startup hook's record", 15, || {
            h.invocations()
                .into_iter()
                .find(|r| field(r, "argv") == Some("check --startup --quiet"))
        });
        let herdr_bin = field(&hook, "bin_path").unwrap().to_string();
        let socket = h
            .config_home()
            .join("herdr/sessions")
            .join(&h.session)
            .join("herdr.sock");
        let env: HashMap<String, String> = [
            ("HERDR_BIN_PATH", herdr_bin),
            ("HERDR_SOCKET_PATH", socket.to_string_lossy().into_owned()),
            ("HERDR_PLUGIN_ID", PLUGIN.to_string()),
            ("HERDR_ENV", "1".to_string()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        let mut base = h.env.clone();
        base.extend(env.iter().map(|(k, v)| (k.clone(), v.clone())));
        let sys = PluginSys { env, base };
        let api = HerdrApi::from_sys(&sys);

        let version = api.version(&sys).expect("--version");
        assert!(version.contains("0.9"), "{version}");
        assert_eq!(api.machines(&sys).expect("machine list --json"), vec![]);
        api.notify(&sys, "Remote desktop", "e2e notification")
            .expect("notification show");
        let procs = api.pane_processes(&sys, &pane).expect("pane process-info");
        assert!(!procs.is_empty() && !procs[0].is_empty(), "{procs:?}");

        // The fix pane exactly as the picker opens it: split, focused, with the machine in its env.
        let target = format!("ssh://e2e@127.0.0.1:{}", h.closed[0]);
        api.open_pane(
            &sys,
            "fix",
            Some("split"),
            &[("GLIFFHERDR_FIX_TARGET", &target)],
            true,
        )
        .expect("plugin pane open fix");
        let rec = wait_for("the fix pane's command to start", 15, || {
            h.invocations()
                .into_iter()
                .find(|r| field(r, "argv") == Some("fix"))
        });
        assert_eq!(field(&rec, "entrypoint"), Some("fix"), "{rec}");
        assert_eq!(field(&rec, "fix_target"), Some(target.as_str()), "{rec}");
        let fix_pane = field(&rec, "pane_id").unwrap().to_string();
        assert!(
            !fix_pane.is_empty() && fix_pane != pane,
            "a split pane: {rec}"
        );
        // It checked the machine, said it is not reachable and now waits for Enter.
        wait_for("the fix pane to explain and wait", 20, || {
            let o = h.session_cli(&["pane", "read", &fix_pane, "--source", "recent"]);
            let text = o.stdout;
            (text.contains("is not reachable") && text.contains("Press Enter to close"))
                .then_some(())
        });

        // herdr's own error text comes through the wrapper.
        match api.open_pane(&sys, "no-such-pane", None, &[], false) {
            Err(HerdrError::Failed(msg)) => {
                assert!(!msg.is_empty() && msg != "no details", "{msg}");
            }
            other => panic!("expected a herdr error, got {other:?}"),
        }
    });
}
