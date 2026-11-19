//! `check`, `list --json --check` and `fix` against a real OpenSSH server (docs/design/07-testing.md §Real OpenSSH).
//!
//! A private `sshd` runs as the current user on 127.0.0.1 and plays ten machines: the key a
//! client logs in with decides which fake machine answers (`environment="PATH=…"` in
//! `authorized_keys`), and `ssh_config` aliases decide how the host key looks. Nothing here talks
//! to a real host: the client is the real `ssh` with `-F <tmp>/ssh_config`, the agent is stripped
//! from the environment and every alias is `IdentitiesOnly`.
//!
//! Gated: set `GLIFFHERDR_E2E_SSH=1`. Without it every test prints `skipped` and passes; with
//! `CI=true` a missing `sshd` is a failure.

mod common;

use std::fmt::Write as _;
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use common::{Ran, World, find_tool, ran};
use serde_json::Value;

/// The `check.ssh_timeout_secs` of this suite's config: `silent` then answers after 1 + 10 s.
const SSH_TIMEOUT: u64 = 1;

/// Names of the fake machines that exist as real logins.
const MACHINES: &[&str] = &["ready", "nohypr", "noserver", "darwin"];

fn enabled() -> bool {
    if std::env::var("GLIFFHERDR_E2E_SSH").as_deref() == Ok("1") {
        return true;
    }
    println!("skipped: set GLIFFHERDR_E2E_SSH=1 to run the real-OpenSSH suite");
    false
}

fn sshd_path() -> Option<PathBuf> {
    ["/usr/sbin/sshd", "/usr/bin/sshd"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
}

/// Skip (or fail under CI) when this machine has no sshd.
fn sshd_or_skip() -> Option<PathBuf> {
    if let Some(p) = sshd_path() {
        return Some(p);
    }
    assert!(
        std::env::var("CI").as_deref() != Ok("true"),
        "CI=true but sshd is not installed (apt install openssh-server)"
    );
    println!("skipped: no sshd at /usr/sbin/sshd or /usr/bin/sshd");
    None
}

fn write_exec(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Accept connections on `listener`, send `greeting` to each and keep it open forever.
fn hold_connections(listener: &TcpListener, greeting: &[u8]) {
    use std::io::Write;
    let mut held = Vec::new();
    while let Ok((mut s, _)) = listener.accept() {
        let _ = s.write_all(greeting);
        held.push(s);
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// The private sshd, the fake machines, the client side and a [`World`] for gliffherdr itself.
struct Ssh {
    w: World,
    /// Keys, configs, machines and logs; kept (and printed) when a test fails.
    root: Option<tempfile::TempDir>,
    sshd: Child,
    port: u16,
    /// A listener that accepts and never speaks (the `silent` alias).
    _silent: TcpListener,
    _stall: TcpListener,
}

impl Ssh {
    #[allow(clippy::too_many_lines)] // one linear fixture: keys, machines, configs, sshd, listeners
    fn new(sshd: &Path) -> Ssh {
        let w = World::new();
        let root = tempfile::Builder::new()
            .prefix("gliffherdr-e2e-ssh-")
            .tempdir()
            .unwrap();
        let r = root.path().to_path_buf();
        let keys = r.join("keys");
        std::fs::create_dir_all(&keys).unwrap();

        let keygen = |name: &str| {
            let out = ran(w
                .command_for(find_tool("ssh-keygen"))
                .args(["-q", "-t", "ed25519", "-N", "", "-C", name, "-f"])
                .arg(keys.join(name)));
            assert_eq!(out.code, 0, "ssh-keygen {name}: {}", out.all());
        };
        for name in ["hostkey", "otherhost", "nokey"].iter().chain(MACHINES) {
            keygen(name);
        }
        let pubkey = |name: &str| {
            std::fs::read_to_string(keys.join(format!("{name}.pub")))
                .unwrap()
                .trim()
                .to_string()
        };

        // Fake machines: a bin/ per machine that is the whole PATH of a login there.
        for m in MACHINES {
            let bin = r.join("machines").join(m).join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            for tool in ["sh", "id", "sed", "tr", "head", "cat"] {
                std::os::unix::fs::symlink(find_tool(tool), bin.join(tool)).unwrap();
            }
            let uname = if *m == "darwin" { "Darwin" } else { "Linux" };
            write_exec(&bin.join("uname"), &format!("#!/bin/sh\necho {uname}\n"));
            write_exec(&bin.join("pgrep"), "#!/bin/sh\nexit 1\n");
            if matches!(*m, "ready" | "noserver") {
                write_exec(
                    &bin.join("hyprctl"),
                    "#!/bin/sh\n[ \"$1\" = -i ] && shift 2\ncase $1 in\n  version) echo Hyprland 0.56.2 ;;\n  monitors) printf 'Monitor DP-1 (ID 0):\\n\\t2560x1440\\nMonitor HDMI-A-1 (ID 1):\\n\\t1920x1080\\n' ;;\nesac\nexit 0\n",
                );
            }
            if matches!(*m, "ready" | "nohypr" | "darwin") {
                write_exec(
                    &bin.join("gliff-server"),
                    "#!/bin/sh\n[ \"$1\" = --version ] && echo 'gliff-server 0.4.3'\nexit 0\n",
                );
            }
        }

        let authorized = r.join("authorized_keys");
        let mut lines = String::new();
        for m in MACHINES {
            let bin = r.join("machines").join(m).join("bin");
            let _ = writeln!(
                lines,
                "environment=\"PATH={}\" {}",
                bin.display(),
                pubkey(m)
            );
        }
        std::fs::write(&authorized, lines).unwrap();

        let host_pub = pubkey("hostkey");
        let other_pub = pubkey("otherhost");
        let alias_hosts = ["ready", "nohypr", "noserver", "darwin", "nokey"];
        let mut known = String::new();
        for a in alias_hosts {
            let _ = writeln!(known, "{a} {host_pub}");
        }
        // `changed` has a key on file that is not the server's.
        let _ = writeln!(known, "changed {other_pub}");
        std::fs::write(r.join("known_hosts"), known).unwrap();

        let port = free_port();
        let sshd_config = format!(
            "Port {port}\n{listen}HostKey {hk}\nPidFile none\nUsePAM no\n\
             StrictModes no\nPermitUserEnvironment yes\nAuthorizedKeysFile {ak}\n\
             PasswordAuthentication no\nKbdInteractiveAuthentication no\nPubkeyAuthentication yes\n\
             LogLevel ERROR\n",
            listen = (1..=ALIASES.len()).fold(String::new(), |mut acc, i| {
                let _ = writeln!(acc, "ListenAddress 127.0.0.{i}");
                acc
            }),
            hk = keys.join("hostkey").display(),
            ak = authorized.display(),
        );
        std::fs::write(r.join("sshd_config"), sshd_config).unwrap();
        let log = std::fs::File::create(r.join("sshd.log")).unwrap();
        let mut sshd_cmd = common::background_command(sshd, &w.env_pairs());
        sshd_cmd
            .args(["-D", "-e", "-f"])
            .arg(r.join("sshd_config"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log);
        let mut child = sshd_cmd.spawn().expect("start sshd");
        let end = Instant::now() + Duration::from_secs(10);
        while TcpStream::connect(("127.0.0.1", port)).is_err() {
            if let Some(status) = child.try_wait().unwrap() {
                panic!(
                    "sshd exited early ({status}): {}",
                    std::fs::read_to_string(r.join("sshd.log")).unwrap_or_default()
                );
            }
            assert!(Instant::now() < end, "sshd never listened on {port}");
            std::thread::sleep(Duration::from_millis(20));
        }

        // The silent machine: accepts, never speaks.
        let silent_addr = ALIASES.iter().position(|a| *a == "silent").unwrap() + 1;
        let silent = TcpListener::bind(format!("127.0.0.{silent_addr}:0")).unwrap();
        let silent_port = silent.local_addr().unwrap().port();
        let acceptor = silent.try_clone().unwrap();
        std::thread::spawn(move || hold_connections(&acceptor, b""));
        // The stalling machine: greets like sshd, then never continues the key exchange, so
        // ssh's own ConnectTimeout (which only covers the banner) never fires.
        let stall = TcpListener::bind(format!("127.0.0.{}:0", ALIASES.len() + 1)).unwrap();
        let stall_port = stall.local_addr().unwrap().port();
        let stall_acceptor = stall.try_clone().unwrap();
        std::thread::spawn(move || {
            hold_connections(&stall_acceptor, b"SSH-2.0-OpenSSH_9.9 stall\r\n");
        });
        let closed_port = free_port();

        let user = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_else(|_| "nobody".into());
        // Every alias is a different loopback address: gliffherdr lists a machine once per
        // `user@host:port`, so ten aliases of one address would collapse into one row.
        let host = |alias: &str, key: &str, port: u16, extra: &str| {
            let n = ALIASES
                .iter()
                .chain(&["stall"])
                .position(|a| *a == alias)
                .unwrap()
                + 1;
            format!(
                "Host {alias}\n  HostName 127.0.0.{n}\n  Port {port}\n  User {user}\n  IdentityFile {}\n  \
                 HostKeyAlias {alias}\n{extra}",
                keys.join(key).display()
            )
        };
        let mut cfg = String::new();
        for m in MACHINES {
            cfg.push_str(&host(m, m, port, ""));
        }
        cfg.push_str(&host("nokey", "nokey", port, ""));
        cfg.push_str(&host("changed", "ready", port, ""));
        cfg.push_str(&host("newhost", "ready", port, ""));
        cfg.push_str(&host(
            "strict",
            "ready",
            port,
            "  StrictHostKeyChecking yes\n",
        ));
        cfg.push_str(&host("closed", "ready", closed_port, ""));
        cfg.push_str(&host("silent", "ready", silent_port, ""));
        cfg.push_str(&host("stall", "ready", stall_port, ""));
        let _ = write!(
            cfg,
            "Host *\n  IdentitiesOnly yes\n  IdentityAgent none\n  ForwardAgent no\n  \
             PreferredAuthentications publickey\n  UserKnownHostsFile {}\n  GlobalKnownHostsFile /dev/null\n",
            r.join("known_hosts").display()
        );
        std::fs::write(r.join("ssh_config"), cfg).unwrap();

        // The client side: a `ssh` wrapper on PATH that uses our config.
        let clientbin = r.join("clientbin");
        write_exec(
            &clientbin.join("ssh"),
            &format!(
                "#!/bin/sh\nexec {} -F {} \"$@\"\n",
                find_tool("ssh").display(),
                r.join("ssh_config").display()
            ),
        );

        // The machines are listed in the config (aliases are not in any ssh config the roster
        // reads), with the short timeout `silent` needs.
        let mut config = format!(
            "[check]\nssh_timeout_secs = {SSH_TIMEOUT}\n\n[sources]\nherdr = false\nrecent = false\ngliff = false\nssh_config = false\ntailscale = false\n"
        );
        for a in ALIASES {
            let _ = write!(config, "\n[[machine]]\ntarget = \"{a}\"\n");
        }
        w.write_config(&config);

        Ssh {
            w,
            root: Some(root),
            sshd: child,
            port,
            _silent: silent,
            _stall: stall,
        }
    }

    fn root(&self) -> &Path {
        self.root.as_ref().unwrap().path()
    }

    fn path_env(&self) -> String {
        let real = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into());
        format!("{}:{real}", self.root().join("clientbin").display())
    }

    /// Run `gliffherdr args…` with the ssh wrapper first on `PATH`.
    fn gliffherdr(&self, args: &[&str]) -> Ran {
        ran(self
            .w
            .command()
            .env("PATH", self.path_env())
            .args(args)
            .timeout(Duration::from_secs(60)))
    }

    /// The cached check result of `alias` as JSON.
    fn cached(&self, alias: &str) -> Value {
        let t = gliffherdr::target::Target::parse(alias).unwrap();
        let file = self
            .w
            .state
            .join("checks")
            .join(format!("{}.json", gliffherdr::paths::slug(t.as_str())));
        let text = std::fs::read_to_string(&file)
            .unwrap_or_else(|e| panic!("no cache for {alias} at {}: {e}", file.display()));
        serde_json::from_str(&text).unwrap()
    }

    /// `check <alias>`: its stdout line and the cached `status` object.
    fn check(&self, alias: &str) -> (Ran, Value) {
        let r = self.gliffherdr(&["check", alias]);
        assert_eq!(r.code, 0, "check {alias}: {}", r.all());
        let cached = self.cached(alias);
        (r, cached)
    }

    fn known_hosts(&self) -> String {
        std::fs::read_to_string(self.root().join("known_hosts")).unwrap()
    }
}

impl Drop for Ssh {
    fn drop(&mut self) {
        let _ = self.sshd.kill();
        let _ = self.sshd.wait();
        if std::thread::panicking()
            && let Some(root) = self.root.take()
        {
            let path = root.keep();
            let log = std::fs::read_to_string(path.join("sshd.log")).unwrap_or_default();
            eprintln!(
                "e2e_ssh failed: files kept in {} (sshd.log below, port {})\n{log}",
                path.display(),
                self.port
            );
        }
    }
}

/// Every alias of `ssh_config`.
const ALIASES: &[&str] = &[
    "ready", "nohypr", "noserver", "darwin", "nokey", "changed", "newhost", "strict", "closed",
    "silent",
];

/// Run `body` against a fresh private sshd, or skip.
fn with_ssh(body: impl FnOnce(&Ssh)) {
    if !enabled() {
        return;
    }
    let Some(sshd) = sshd_or_skip() else { return };
    let ssh = Ssh::new(&sshd);
    body(&ssh);
}

fn kind(status: &Value) -> &str {
    status["status"]["kind"].as_str().unwrap()
}

#[test]
fn ready_machine_reports_outputs_and_server_version() {
    with_ssh(|s| {
        let (r, c) = s.check("ready");
        assert_eq!(kind(&c), "ready", "{}", r.all());
        assert_eq!(c["os"], "Linux");
        assert_eq!(c["outputs"], serde_json::json!(["DP-1", "HDMI-A-1"]));
        assert_eq!(c["server_version"], "gliff-server 0.4.3");
        assert!(r.stdout.contains("ready"), "{}", r.stdout);
    });
}

#[test]
fn machine_without_hyprland_is_no_hyprland() {
    with_ssh(|s| assert_eq!(kind(&s.check("nohypr").1), "no_hyprland"));
}

#[test]
fn machine_without_gliff_server_is_no_server() {
    with_ssh(|s| assert_eq!(kind(&s.check("noserver").1), "no_server"));
}

#[test]
fn darwin_machine_is_unsupported() {
    with_ssh(|s| {
        let c = s.check("darwin").1;
        assert_eq!(kind(&c), "unsupported");
        assert_eq!(c["status"]["os"], "Darwin");
    });
}

#[test]
fn unauthorised_key_is_auth() {
    with_ssh(|s| assert_eq!(kind(&s.check("nokey").1), "auth"));
}

#[test]
fn a_changed_host_key_is_refused_and_marked_changed() {
    with_ssh(|s| {
        let c = s.check("changed").1;
        assert_eq!(kind(&c), "host_key");
        assert_eq!(c["status"]["changed"], true);
        let kh = s.known_hosts();
        assert_eq!(
            kh.lines().filter(|l| l.starts_with("changed ")).count(),
            1,
            "{kh}"
        );
    });
}

#[test]
fn an_unknown_host_key_is_accepted_like_gliff_and_written_to_known_hosts() {
    with_ssh(|s| {
        assert!(!s.known_hosts().contains("newhost "), "precondition");
        let (r, c) = s.check("newhost");
        assert_eq!(kind(&c), "ready", "{}", r.all());
        let kh = s.known_hosts();
        assert!(
            kh.lines().any(|l| l.starts_with("newhost ssh-ed25519 ")),
            "known_hosts must hold the new key:\n{kh}"
        );
        // And the second check no longer needs to learn it.
        let again = s.check("newhost").1;
        assert_eq!(kind(&again), "ready");
    });
}

#[test]
fn strict_host_key_checking_refuses_an_unknown_key() {
    with_ssh(|s| {
        let c = s.check("strict").1;
        assert_eq!(kind(&c), "host_key");
        assert_eq!(c["status"]["changed"], false);
        assert!(
            !s.known_hosts().contains("strict "),
            "nothing may be learned"
        );
    });
}

#[test]
fn a_closed_port_is_offline() {
    with_ssh(|s| {
        let c = s.check("closed").1;
        assert_eq!(kind(&c), "offline");
        let detail = c["status"]["detail"].as_str().unwrap();
        assert!(detail.contains("Connection refused"), "{detail}");
    });
}

#[test]
fn a_server_that_never_speaks_is_offline() {
    with_ssh(|s| {
        let started = Instant::now();
        let c = s.check("silent").1;
        assert_eq!(kind(&c), "offline");
        let detail = c["status"]["detail"].as_str().unwrap();
        eprintln!("silent: {detail} after {:?}", started.elapsed());
        assert!(
            detail.contains("timed out") || detail.contains("no answer within"),
            "{detail}"
        );
    });
}

#[test]
fn a_server_that_stalls_after_its_banner_hits_the_overall_timeout() {
    with_ssh(|s| {
        let started = Instant::now();
        let (r, c) = s.check("stall");
        assert_eq!(kind(&c), "offline", "{}", r.all());
        // The overall timeout is the check timeout plus 10 s.
        assert_eq!(
            c["status"]["detail"],
            format!("no answer within {}s", SSH_TIMEOUT + 10)
        );
        assert!(started.elapsed() >= Duration::from_secs(SSH_TIMEOUT + 10));
    });
}

#[test]
fn list_json_check_classifies_every_alias() {
    with_ssh(|s| {
        let r = s.gliffherdr(&["list", "--json", "--check"]);
        assert_eq!(r.code, 0, "{}", r.all());
        let rows: Vec<Value> = serde_json::from_str(&r.stdout).unwrap();
        let row = |t: &str| {
            rows.iter()
                .find(|r| r["target"] == t)
                .unwrap_or_else(|| panic!("no {t}: {rows:?}"))
        };
        let want = [
            ("ready", "ready", "ready"),
            ("nohypr", "needs_setup", "no_hyprland"),
            ("noserver", "needs_setup", "no_server"),
            ("darwin", "unsupported", "unsupported"),
            ("nokey", "needs_setup", "auth"),
            ("changed", "needs_setup", "host_key"),
            ("newhost", "ready", "ready"),
            ("strict", "needs_setup", "host_key"),
            ("closed", "unreachable", "offline"),
            ("silent", "unreachable", "offline"),
        ];
        for (t, state, k) in want {
            let r = row(t);
            assert_eq!(r["state"], state, "{t}: {r}");
            assert_eq!(r["status"]["kind"], k, "{t}: {r}");
        }
        assert_eq!(
            row("ready")["outputs"],
            serde_json::json!(["DP-1", "HDMI-A-1"])
        );
        assert_eq!(row("ready")["server_version"], "gliff-server 0.4.3");
        assert_eq!(row("ready")["os"], "Linux");
        assert_eq!(row("changed")["status"]["changed"], true);
        assert_eq!(row("strict")["status"]["changed"], false);
    });
}

#[test]
fn fix_nokey_copies_the_key_and_ends_ready() {
    with_ssh(|s| {
        assert_eq!(kind(&s.check("nokey").1), "auth");
        // A stub ssh-copy-id that does what the real one does for this server: authorise the key.
        let r = s.root();
        write_exec(
            &r.join("clientbin/ssh-copy-id"),
            &format!(
                "#!/bin/sh\necho \"$*\" >>{log}\n\
                 printf 'environment=\"PATH={bin}\" %s\\n' \"$(cat {key})\" >>{ak}\n",
                log = r.join("ssh-copy-id.log").display(),
                bin = r.join("machines/ready/bin").display(),
                key = r.join("keys/nokey.pub").display(),
                ak = r.join("authorized_keys").display(),
            ),
        );
        let out = s.gliffherdr(&["fix", "nokey"]);
        assert_eq!(out.code, 0, "{}", out.all());
        assert_eq!(
            std::fs::read_to_string(r.join("ssh-copy-id.log")).unwrap(),
            "nokey\n"
        );
        assert!(
            out.stdout.contains("Copy your ssh key to nokey"),
            "{}",
            out.stdout
        );
        assert!(
            out.stdout.trim_end().ends_with("nokey is ready."),
            "{}",
            out.stdout
        );
        assert_eq!(kind(&s.cached("nokey")), "ready");
    });
}

#[test]
fn fix_noserver_runs_the_install_remedy_over_real_ssh_and_reports_that_it_cannot() {
    with_ssh(|s| {
        // The fake machine has no `omarchy`, so the remedy prints its explanation and fails: the
        // machine stays "needs setup" and fix exits 1 (nothing is installed anywhere).
        let out = s.gliffherdr(&["fix", "noserver"]);
        assert_eq!(out.code, 1, "{}", out.all());
        assert!(
            out.stdout.contains("Install gliff on noserver"),
            "{}",
            out.stdout
        );
        assert!(
            out.all().contains("This machine has no omarchy command"),
            "{}",
            out.all()
        );
        assert_eq!(kind(&s.cached("noserver")), "no_server");
    });
}

#[test]
fn every_alias_of_the_suite_is_covered() {
    // The ten aliases of docs/design/07-testing.md, so a rename cannot silently drop one.
    assert_eq!(ALIASES.len(), 10);
    let _ = (MACHINES, SSH_TIMEOUT);
}
