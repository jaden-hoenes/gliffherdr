//! Checking a machine over ssh: is Hyprland running there, is `gliff-server` installed?
//!
//! One ssh call feeds [`REMOTE_SCRIPT`] to `sh -s` on the remote machine; nothing is installed
//! there. Results are cached per machine (see [`cache`]) and classified into a [`Status`].

pub mod cache;
pub mod classify;

use std::collections::VecDeque;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::paths::Paths;
use crate::roster::Machine;
use crate::sys::{Cmd, Sys, cmd};
use crate::target::Target;

pub use cache::load_cached;
pub use classify::classify;

/// The POSIX `sh` script that is run on the remote machine.
pub const REMOTE_SCRIPT: &str = include_str!("check/remote.sh");

/// How many checks run at once.
pub const MAX_PARALLEL: usize = 12;
/// How long `ssh -G` may take.
const SSH_G_TIMEOUT: Duration = Duration::from_secs(3);

/// What a check found out about a machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Status {
    /// Hyprland is running and `gliff-server` is installed.
    Ready,
    /// The machine is a Linux box without a running Hyprland session.
    NoHyprland,
    /// Hyprland runs but `gliff-server` is missing.
    NoServer,
    /// The machine runs something other than Linux.
    Unsupported {
        /// `uname -s` of the machine.
        os: String,
    },
    /// ssh could not reach the machine.
    Offline {
        /// Why, from ssh.
        detail: String,
    },
    /// The machine refused the ssh key.
    Auth,
    /// The host key is unknown or has changed.
    HostKey {
        /// True when a different key is on record.
        changed: bool,
    },
    /// Anything else that went wrong.
    Error {
        /// What ssh said last.
        detail: String,
    },
}

/// A finished check, as cached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    /// The target that was checked.
    pub target: String,
    /// The outcome.
    pub status: Status,
    /// `uname -s` of the machine, when it answered.
    pub os: Option<String>,
    /// Hyprland screens of the machine.
    pub outputs: Vec<String>,
    /// First line of `gliff-server --version`, when it is installed.
    pub server_version: Option<String>,
    /// Unix time of the check.
    pub checked_at: u64,
}

/// What the remote script printed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    /// `uname -s`.
    pub os: String,
    /// Hyprland is running.
    pub hyprland: bool,
    /// Screen names.
    pub outputs: Vec<String>,
    /// Path of `gliff-server`, `custom` for a configured server command, `None` when missing.
    pub gliff_server: Option<String>,
    /// First line of `gliff-server --version`.
    pub server_version: Option<String>,
}

/// Parse the `key=value` lines of the remote script. Unknown keys and lines are ignored.
pub fn parse_report(stdout: &str) -> Report {
    let mut r = Report::default();
    for line in stdout.lines() {
        let line = line.trim_end_matches('\r');
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key {
            "os" => r.os = value.to_string(),
            "hyprland" => r.hyprland = value == "1",
            "outputs" => r.outputs = value.split_whitespace().map(str::to_string).collect(),
            "gliff_server" => r.gliff_server = Some(value.to_string()).filter(|v| !v.is_empty()),
            "server_version" => {
                r.server_version = Some(value.to_string()).filter(|v| !v.is_empty());
            }
            _ => {}
        }
    }
    r
}

/// The status a report means: not Linux is unsupported, no Hyprland, no server, else ready.
/// A report with no `os` line at all is an error (the machine did not run the script).
pub fn status_from_report(r: &Report) -> Status {
    if r.os.is_empty() {
        Status::Error {
            detail: "the machine returned no system report".to_string(),
        }
    } else if r.os != "Linux" {
        Status::Unsupported { os: r.os.clone() }
    } else if !r.hyprland {
        Status::NoHyprland
    } else if r.gliff_server.is_none() {
        Status::NoServer
    } else {
        Status::Ready
    }
}

/// The overall time limit of one check for a given `check.ssh_timeout_secs`: that plus 10 s.
pub fn overall_timeout_secs(ssh_timeout_secs: u64) -> u64 {
    ssh_timeout_secs + 10
}

/// The ssh command that runs the remote script on `target`. `accept_new` adds
/// `StrictHostKeyChecking=accept-new`; `server_bin` is the configured server command.
pub fn ssh_command(
    target: &Target,
    ssh_timeout_secs: u64,
    accept_new: bool,
    server_bin: Option<&str>,
) -> Cmd {
    let mut args = vec![
        "-T".to_string(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        format!("ConnectTimeout={ssh_timeout_secs}"),
        "-o".into(),
        "LogLevel=ERROR".into(),
    ];
    if accept_new {
        args.extend(["-o".into(), "StrictHostKeyChecking=accept-new".into()]);
    }
    args.extend([
        "--".into(),
        target.as_str().to_string(),
        "sh".into(),
        "-s".into(),
    ]);
    let mut stdin = String::new();
    if let Some(bin) = server_bin.filter(|b| !b.trim().is_empty()) {
        stdin = format!("server_bin='{}'\n", bin.replace('\'', "'\\''"));
    }
    stdin.push_str(REMOTE_SCRIPT);
    Cmd {
        program: "ssh".to_string(),
        args,
        stdin: Some(stdin.into_bytes()),
        timeout: Some(Duration::from_secs(overall_timeout_secs(ssh_timeout_secs))),
        ..Cmd::default()
    }
}

/// Whether ssh would stop and ask about an unknown host key, as gliff decides
/// (`ssh -G` prints `stricthostkeychecking ask`); then the check accepts new keys like gliff.
fn host_key_default_is_ask(sys: &dyn Sys, target: &Target) -> bool {
    let c = cmd("ssh", &["-G", "--", target.as_str()]).with_timeout(SSH_G_TIMEOUT);
    sys.run(&c).is_ok_and(|o| {
        o.success()
            && o.stdout
                .lines()
                .any(|l| l.trim() == "stricthostkeychecking ask")
    })
}

/// Check one machine over ssh and save the result in the cache.
pub fn check_one(sys: &dyn Sys, paths: &Paths, config: &Config, machine: &Machine) -> CheckResult {
    let t = config.check.ssh_timeout_secs;
    let server_bin = machine
        .settings
        .server_bin
        .as_ref()
        .or(config.connect.server_bin.as_ref());
    let accept_new = host_key_default_is_ask(sys, &machine.target);
    let c = ssh_command(
        &machine.target,
        t,
        accept_new,
        server_bin.map(String::as_str),
    );

    let mut result = CheckResult {
        target: machine.target.to_string(),
        status: Status::Ready,
        os: None,
        outputs: Vec::new(),
        server_version: None,
        checked_at: 0,
    };
    match sys.run(&c) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            result.status = Status::Error {
                detail: "ssh is not installed".to_string(),
            };
        }
        Err(e) => {
            result.status = Status::Error {
                detail: format!("could not run ssh: {e}"),
            }
        }
        Ok(out) if out.success() => {
            let report = parse_report(&out.stdout);
            result.status = status_from_report(&report);
            if !report.os.is_empty() {
                result.os = Some(report.os);
            }
            result.outputs = report.outputs;
            result.server_version = report.server_version;
        }
        Ok(out) => result.status = classify(&out, t),
    }
    result.checked_at = sys.now_unix();
    // A cache that cannot be written only costs a repeated check.
    let _ = cache::save(paths, &machine.target, &result);
    result
}

/// Progress of [`check_many`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckEvent {
    /// A check of this target began.
    Started(String),
    /// A check finished (its result is already cached).
    Finished(CheckResult),
}

/// Check `machines` on at most [`MAX_PARALLEL`] threads, reporting through `tx`. The returned
/// handle finishes when every check is done. Stops early when the receiver is dropped.
pub fn check_many(
    sys: Arc<dyn Sys>,
    paths: Paths,
    config: Config,
    machines: Vec<Machine>,
    tx: Sender<CheckEvent>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let workers = machines.len().min(MAX_PARALLEL);
        let queue = Arc::new(Mutex::new(VecDeque::from(machines)));
        let shared = Arc::new((paths, config));
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                let (queue, shared, sys, tx) = (
                    Arc::clone(&queue),
                    Arc::clone(&shared),
                    Arc::clone(&sys),
                    tx.clone(),
                );
                thread::spawn(move || {
                    loop {
                        let next = queue.lock().ok().and_then(|mut q| q.pop_front());
                        let Some(machine) = next else { break };
                        if tx
                            .send(CheckEvent::Started(machine.target.to_string()))
                            .is_err()
                        {
                            break;
                        }
                        let result = check_one(sys.as_ref(), &shared.0, &shared.1, &machine);
                        if tx.send(CheckEvent::Finished(result)).is_err() {
                            break;
                        }
                    }
                })
            })
            .collect();
        for h in handles {
            let _ = h.join();
        }
    })
}

/// Whether a result is still fresh at `now`.
pub fn is_fresh(r: &CheckResult, now: u64, fresh_for_secs: u64) -> bool {
    now.saturating_sub(r.checked_at) <= fresh_for_secs
}

/// Whether to check again before connecting: no result, a stale one, or a failure that may have
/// been fixed since (everything except `Ready` and `Unsupported`; a machine's OS does not change).
pub fn needs_check(r: Option<&CheckResult>, now: u64, fresh_for_secs: u64) -> bool {
    match r {
        None => true,
        Some(r) => {
            !is_fresh(r, now, fresh_for_secs)
                || !matches!(r.status, Status::Ready | Status::Unsupported { .. })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roster::bare_machine;
    use crate::sys::fake::FakeSys;
    use std::sync::mpsc;

    const READY: &str = "os=Linux\nhyprland=1\noutputs=DP-1 HDMI-A-1 \ngliff_server=/usr/bin/gliff-server\nserver_version=gliff-server 0.4.3\n";

    fn machine(fake: &FakeSys, config: &Config, t: &str) -> Machine {
        bare_machine(fake, config, Target::parse(t).unwrap())
    }

    struct Env {
        _dir: tempfile::TempDir,
        paths: Paths,
        fake: Arc<FakeSys>,
    }

    fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        let fake = Arc::new(FakeSys::new());
        fake.set_now(1000);
        Env {
            _dir: dir,
            paths,
            fake,
        }
    }

    #[test]
    fn parse_report_reads_every_key() {
        let r = parse_report(READY);
        assert_eq!(
            r,
            Report {
                os: "Linux".into(),
                hyprland: true,
                outputs: vec!["DP-1".into(), "HDMI-A-1".into()],
                gliff_server: Some("/usr/bin/gliff-server".into()),
                server_version: Some("gliff-server 0.4.3".into()),
            }
        );
    }

    #[test]
    fn parse_report_tolerates_noise() {
        let r = parse_report(
            "Welcome!\r\nos=Darwin\r\nhyprland=0\r\noutputs=\r\ngliff_server=\r\nfoo=bar\nno equals\n",
        );
        assert_eq!(
            r,
            Report {
                os: "Darwin".into(),
                ..Report::default()
            }
        );
        assert_eq!(parse_report(""), Report::default());
        assert_eq!(
            parse_report("gliff_server=custom\n")
                .gliff_server
                .as_deref(),
            Some("custom")
        );
    }

    #[test]
    fn status_from_report_table() {
        let linux = |hypr, server: Option<&str>| Report {
            os: "Linux".into(),
            hyprland: hypr,
            gliff_server: server.map(str::to_string),
            ..Report::default()
        };
        let cases = [
            (linux(true, Some("/x")), Status::Ready),
            (linux(true, Some("custom")), Status::Ready),
            (linux(true, None), Status::NoServer),
            (linux(false, Some("/x")), Status::NoHyprland),
            (linux(false, None), Status::NoHyprland),
            (
                Report {
                    os: "Darwin".into(),
                    hyprland: true,
                    ..Report::default()
                },
                Status::Unsupported {
                    os: "Darwin".into(),
                },
            ),
            (
                Report::default(),
                Status::Error {
                    detail: "the machine returned no system report".into(),
                },
            ),
        ];
        for (report, want) in cases {
            assert_eq!(status_from_report(&report), want, "{report:?}");
        }
    }

    #[test]
    fn ssh_command_shape() {
        let t = Target::parse("j@h").unwrap();
        let c = ssh_command(&t, 5, false, None);
        assert_eq!(
            c.args,
            [
                "-T",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=5",
                "-o",
                "LogLevel=ERROR",
                "--",
                "j@h",
                "sh",
                "-s"
            ]
        );
        assert_eq!(c.timeout, Some(Duration::from_secs(15)));
        assert_eq!(c.stdin.as_deref(), Some(REMOTE_SCRIPT.as_bytes()));

        let c = ssh_command(&t, 1, true, Some("gliff-server --low-bandwidth"));
        assert!(
            c.args
                .windows(2)
                .any(|w| w == ["-o", "StrictHostKeyChecking=accept-new"])
        );
        let sep = c.args.iter().position(|a| a == "--").unwrap();
        assert_eq!(c.args[sep + 1], "j@h", "the target always follows --");
        assert_eq!(c.timeout, Some(Duration::from_secs(11)));
        let stdin = String::from_utf8(c.stdin.unwrap()).unwrap();
        assert!(
            stdin.starts_with("server_bin='gliff-server --low-bandwidth'\n#!/bin/sh"),
            "{stdin}"
        );
    }

    #[test]
    fn server_bin_quotes_are_escaped_and_blank_is_ignored() {
        let t = Target::parse("h").unwrap();
        let stdin = |b| String::from_utf8(ssh_command(&t, 5, false, b).stdin.unwrap()).unwrap();
        assert!(stdin(Some("a'b; rm -rf ~")).starts_with("server_bin='a'\\''b; rm -rf ~'\n"));
        assert!(stdin(Some("  ")).starts_with("#!/bin/sh"));
    }

    #[test]
    fn check_one_ready_saves_the_cache() {
        let e = env();
        e.fake
            .on("ssh", &["-G"])
            .stdout("user j\nhostname h\nport 22\n");
        e.fake.on_arg("ssh", "sh").stdout(READY);
        let m = machine(&e.fake, &Config::default(), "j@h");
        let r = check_one(e.fake.as_ref(), &e.paths, &Config::default(), &m);
        assert_eq!(r.status, Status::Ready);
        assert_eq!(r.os.as_deref(), Some("Linux"));
        assert_eq!(r.outputs, ["DP-1", "HDMI-A-1"]);
        assert_eq!(r.server_version.as_deref(), Some("gliff-server 0.4.3"));
        assert_eq!(r.checked_at, 1000);
        assert_eq!(load_cached(&e.paths, &m.target), Some(r));
    }

    #[test]
    fn check_one_accepts_new_host_keys_only_when_ssh_would_ask() {
        for (g, expect) in [
            ("stricthostkeychecking ask\n", true),
            ("stricthostkeychecking true\n", false),
            ("stricthostkeychecking accept-new\n", false),
            ("stricthostkeychecking yes\nuser x\n", false),
            ("", false),
        ] {
            let e = env();
            e.fake.on("ssh", &["-G"]).stdout(g);
            e.fake.on_arg("ssh", "sh").stdout(READY);
            let m = machine(&e.fake, &Config::default(), "h");
            check_one(e.fake.as_ref(), &e.paths, &Config::default(), &m);
            let check = e
                .fake
                .calls_to("ssh")
                .into_iter()
                .find(|c| c.args.contains(&"sh".to_string()))
                .unwrap();
            assert_eq!(
                check
                    .args
                    .iter()
                    .any(|a| a == "StrictHostKeyChecking=accept-new"),
                expect,
                "{g:?}"
            );
        }
        // ssh -G failing means no accept-new either.
        let e = env();
        e.fake.on("ssh", &["-G"]).code(255);
        e.fake.on_arg("ssh", "sh").stdout(READY);
        let m = machine(&e.fake, &Config::default(), "h");
        check_one(e.fake.as_ref(), &e.paths, &Config::default(), &m);
        assert!(
            !e.fake.calls_to("ssh")[2]
                .args
                .iter()
                .any(|a| a.contains("accept-new"))
        );
    }

    #[test]
    fn check_one_uses_machine_then_global_server_bin() {
        let cfg: Config = toml::from_str(
            "[connect]\nserver_bin = \"global\"\n[[machine]]\ntarget = \"a\"\nserver_bin = \"own\"\n",
        )
        .unwrap();
        for (t, want) in [("a", "own"), ("b", "global")] {
            let e = env();
            e.fake.on("ssh", &[]).stdout(READY);
            let m = machine(&e.fake, &cfg, t);
            check_one(e.fake.as_ref(), &e.paths, &cfg, &m);
            let call = e.fake.calls_to("ssh").pop().unwrap();
            let stdin = String::from_utf8(call.stdin.unwrap()).unwrap();
            assert!(
                stdin.starts_with(&format!("server_bin='{want}'\n")),
                "{t}: {stdin}"
            );
        }
    }

    #[test]
    fn check_one_classifies_every_failure_and_still_caches() {
        let cases: Vec<(&str, i32, &str, Status)> = vec![
            (
                "auth",
                255,
                "j@box: Permission denied (publickey).",
                Status::Auth,
            ),
            (
                "off",
                255,
                "ssh: connect to host off port 22: No route to host",
                Status::Offline {
                    detail: "ssh: connect to host off port 22: No route to host".into(),
                },
            ),
            (
                "hk",
                255,
                "Host key verification failed.",
                Status::HostKey { changed: false },
            ),
            (
                "weird",
                1,
                "something else broke",
                Status::Error {
                    detail: "something else broke".into(),
                },
            ),
        ];
        for (t, code, err, want) in cases {
            let e = env();
            e.fake.on("ssh", &[]).code(code).stderr(err);
            let m = machine(&e.fake, &Config::default(), t);
            let r = check_one(e.fake.as_ref(), &e.paths, &Config::default(), &m);
            assert_eq!(r.status, want, "{t}");
            assert_eq!((r.os, r.outputs.len()), (None, 0));
            assert_eq!(load_cached(&e.paths, &m.target).unwrap().status, want);
        }
    }

    #[test]
    fn check_one_timeout_and_spawn_errors() {
        let e = env();
        e.fake.on("ssh", &["-G"]).stdout("");
        e.fake.on_arg("ssh", "sh").timed_out();
        let cfg: Config = toml::from_str("[check]\nssh_timeout_secs = 1\n").unwrap();
        let m = machine(&e.fake, &cfg, "h");
        let r = check_one(e.fake.as_ref(), &e.paths, &cfg, &m);
        assert_eq!(
            r.status,
            Status::Offline {
                detail: "no answer within 11s".into()
            }
        );

        let none = env(); // no ssh at all
        let m = machine(&none.fake, &Config::default(), "h");
        let r = check_one(none.fake.as_ref(), &none.paths, &Config::default(), &m);
        assert_eq!(
            r.status,
            Status::Error {
                detail: "ssh is not installed".into()
            }
        );
    }

    #[test]
    fn check_one_reports_non_linux_and_missing_pieces() {
        let table = [
            (
                "os=Darwin\nhyprland=0\ngliff_server=\n",
                Status::Unsupported {
                    os: "Darwin".into(),
                },
            ),
            (
                "os=Linux\nhyprland=0\ngliff_server=/usr/bin/gliff-server\n",
                Status::NoHyprland,
            ),
            ("os=Linux\nhyprland=1\ngliff_server=\n", Status::NoServer),
        ];
        for (out, want) in table {
            let e = env();
            e.fake.on("ssh", &[]).stdout(out);
            let m = machine(&e.fake, &Config::default(), "h");
            let r = check_one(e.fake.as_ref(), &e.paths, &Config::default(), &m);
            assert_eq!(r.status, want);
        }
    }

    #[test]
    fn check_many_reports_started_and_finished_for_each() {
        let e = env();
        e.fake.on("ssh", &[]).stdout(READY);
        let machines: Vec<_> = (0..30)
            .map(|i| machine(&e.fake, &Config::default(), &format!("u@h{i}")))
            .collect();
        let (tx, rx) = mpsc::channel();
        let sys: Arc<dyn Sys> = e.fake.clone();
        let handle = check_many(sys, e.paths.clone(), Config::default(), machines, tx);
        let events: Vec<CheckEvent> = rx.iter().collect();
        handle.join().unwrap();
        assert_eq!(events.len(), 60);
        for i in 0..30 {
            let t = format!("u@h{i}");
            let started = events
                .iter()
                .position(|ev| *ev == CheckEvent::Started(t.clone()))
                .unwrap();
            let finished = events
                .iter()
                .position(|ev| matches!(ev, CheckEvent::Finished(r) if r.target == t))
                .unwrap();
            assert!(started < finished, "{t}");
            assert!(load_cached(&e.paths, &Target::parse(&t).unwrap()).is_some());
        }
    }

    #[test]
    fn check_many_never_runs_more_than_twelve_at_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let e = env();
        let (now, peak) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let (n2, p2) = (Arc::clone(&now), Arc::clone(&peak));
        e.fake.on_arg("ssh", "sh").handler(move |_| {
            let cur = n2.fetch_add(1, Ordering::SeqCst) + 1;
            p2.fetch_max(cur, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(30));
            n2.fetch_sub(1, Ordering::SeqCst);
            Ok(crate::sys::Output {
                code: Some(0),
                stdout: READY.into(),
                ..Default::default()
            })
        });
        let machines: Vec<_> = (0..40)
            .map(|i| machine(&e.fake, &Config::default(), &format!("h{i}")))
            .collect();
        let (tx, rx) = mpsc::channel();
        let sys: Arc<dyn Sys> = e.fake.clone();
        let h = check_many(sys, e.paths.clone(), Config::default(), machines, tx);
        assert_eq!(rx.iter().count(), 80);
        h.join().unwrap();
        let peak = peak.load(Ordering::SeqCst);
        assert!((2..=MAX_PARALLEL).contains(&peak), "peak {peak}");
    }

    #[test]
    fn check_many_with_nothing_or_a_dropped_receiver() {
        let e = env();
        let (tx, rx) = mpsc::channel();
        let sys: Arc<dyn Sys> = e.fake.clone();
        check_many(sys.clone(), e.paths.clone(), Config::default(), vec![], tx)
            .join()
            .unwrap();
        assert_eq!(rx.iter().count(), 0);

        e.fake.on("ssh", &[]).stdout(READY);
        let machines = vec![machine(&e.fake, &Config::default(), "x")];
        let (tx, rx) = mpsc::channel();
        drop(rx);
        check_many(sys, e.paths.clone(), Config::default(), machines, tx)
            .join()
            .unwrap();
    }

    fn res(status: Status, at: u64) -> CheckResult {
        CheckResult {
            target: "t".into(),
            status,
            os: None,
            outputs: vec![],
            server_version: None,
            checked_at: at,
        }
    }

    #[test]
    fn freshness_boundaries() {
        let r = res(Status::Ready, 1000);
        assert!(is_fresh(&r, 1000, 600));
        assert!(is_fresh(&r, 1600, 600));
        assert!(!is_fresh(&r, 1601, 600));
        assert!(
            is_fresh(&r, 900, 600),
            "a clock that went backwards is still fresh"
        );
    }

    #[test]
    fn needs_check_table() {
        let fresh = |s| res(s, 1000);
        let off = Status::Offline { detail: "x".into() };
        let cases = [
            (None, true),
            (Some(fresh(Status::Ready)), false),
            (
                Some(fresh(Status::Unsupported {
                    os: "Darwin".into(),
                })),
                false,
            ),
            (Some(fresh(Status::NoServer)), true),
            (Some(fresh(Status::NoHyprland)), true),
            (Some(fresh(Status::Auth)), true),
            (Some(fresh(Status::HostKey { changed: false })), true),
            (Some(fresh(off)), true),
            (Some(fresh(Status::Error { detail: "e".into() })), true),
            (Some(res(Status::Ready, 100)), true),
            (
                Some(res(
                    Status::Unsupported {
                        os: "Darwin".into(),
                    },
                    100,
                )),
                true,
            ),
        ];
        for (r, want) in cases {
            assert_eq!(needs_check(r.as_ref(), 1100, 600), want, "{r:?}");
        }
    }
}
