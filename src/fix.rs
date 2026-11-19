//! Fix-it remedies: what to run, interactively, to make a machine that "needs setup" ready.
//!
//! [`run`] is shared by the `fix` command (the herdr `fix` pane and a shell) and the picker's
//! inline fix: it re-checks the machine, runs the remedy with the terminal inherited, checks
//! again and says what happened. A changed host key is explained, never fixed.

use std::io::Write;

use crate::check::{CheckResult, Status, check_one};
use crate::config::Config;
use crate::paths::Paths;
use crate::roster::Machine;
use crate::status::sentence;
use crate::sys::{Sys, cmd};
use crate::target::Target;

/// The remote command of [`Remedy::InstallServer`].
const INSTALL_SCRIPT: &str = "if command -v omarchy >/dev/null 2>&1; then omarchy pkg add gliff; else echo \"This machine has no omarchy command. Install gliff from https://github.com/omacom/gliff#install\"; exit 1; fi";

/// Something that can be run to fix a machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remedy {
    /// Install gliff on the machine (`omarchy pkg add gliff` over ssh).
    InstallServer,
    /// Copy the user's ssh key to the machine (`ssh-copy-id`).
    CopyKey,
    /// Show the host key and accept it (an interactive `ssh`).
    AcceptHostKey,
}

/// The remedy for a status: `NoServer`, `Auth` and an unknown (not changed) host key have one;
/// everything else has none.
#[must_use]
pub fn remedy_for(status: &Status) -> Option<Remedy> {
    match status {
        Status::NoServer => Some(Remedy::InstallServer),
        Status::Auth => Some(Remedy::CopyKey),
        Status::HostKey { changed: false } => Some(Remedy::AcceptHostKey),
        _ => None,
    }
}

/// The command (program first) that carries out a remedy for `target`.
#[must_use]
pub fn remedy_argv(r: Remedy, target: &Target) -> Vec<String> {
    let t = target.to_string();
    let words: Vec<String> = match r {
        Remedy::InstallServer => vec![
            "ssh".into(),
            "-t".into(),
            "--".into(),
            t,
            INSTALL_SCRIPT.into(),
        ],
        Remedy::CopyKey => vec!["ssh-copy-id".into(), t],
        Remedy::AcceptHostKey => vec![
            "ssh".into(),
            "-o".into(),
            "StrictHostKeyChecking=ask".into(),
            "--".into(),
            t,
            "true".into(),
        ],
    };
    words
}

/// One line saying what a remedy does to `target`.
#[must_use]
pub fn describe(r: Remedy, target: &str) -> String {
    match r {
        Remedy::InstallServer => {
            format!("Install gliff on {target} (runs omarchy pkg add gliff there)")
        }
        Remedy::CopyKey => format!("Copy your ssh key to {target}"),
        Remedy::AcceptHostKey => format!("Check {target}'s host key and accept it"),
    }
}

/// What [`run`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixReport {
    /// The remedy that was run, `None` when the machine had no remedy.
    pub remedy: Option<Remedy>,
    /// The fresh check before the remedy.
    pub before: CheckResult,
    /// The check after the remedy, `None` when no remedy ran.
    pub after: Option<CheckResult>,
}

impl FixReport {
    /// The newest check result (also what the cache now holds).
    #[must_use]
    pub fn result(&self) -> &CheckResult {
        self.after.as_ref().unwrap_or(&self.before)
    }

    /// Whether the machine ended up `Ready`.
    #[must_use]
    pub fn ready(&self) -> bool {
        self.result().status == Status::Ready
    }
}

/// Print the sentence of a status: the message, then the hint indented.
fn say(out: &mut dyn Write, status: &Status, target: &Target) {
    let (message, hint) = sentence(status, target.as_str());
    let _ = writeln!(out, "{message}");
    if let Some(h) = hint {
        let _ = writeln!(out, "  {h}");
    }
}

/// Fix `machine`: check it afresh; with no remedy say why (already ready, or what is wrong);
/// otherwise print the remedy, run it with the terminal inherited, check again and print the
/// outcome. With `wait`, finish by printing `Press Enter to close.` and waiting for a line, so a
/// pane that closes with the process keeps its text on screen.
pub fn run(
    sys: &dyn Sys,
    paths: &Paths,
    config: &Config,
    machine: &Machine,
    out: &mut dyn Write,
    wait: bool,
) -> FixReport {
    let target = &machine.target;
    let before = check_one(sys, paths, config, machine);
    let mut report = FixReport {
        remedy: remedy_for(&before.status),
        before,
        after: None,
    };
    match report.remedy {
        None => say(out, &report.before.status, target),
        Some(remedy) => {
            let argv = remedy_argv(remedy, target);
            let _ = writeln!(out, "{}\n", describe(remedy, target.as_str()));
            let _ = out.flush();
            let rest: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
            match sys.run_interactive(&cmd(&argv[0], &rest)) {
                Ok(0) => {}
                Ok(code) => {
                    let _ = writeln!(out, "\n{} exited with status {code}.", argv[0]);
                }
                Err(e) => {
                    let _ = writeln!(out, "\ncould not run {}: {e}", argv[0]);
                }
            }
            let after = check_one(sys, paths, config, machine);
            let _ = writeln!(out);
            say(out, &after.status, target);
            report.after = Some(after);
        }
    }
    if wait {
        let _ = writeln!(out, "\nPress Enter to close.");
        let _ = out.flush();
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roster::bare_machine;
    use crate::sys::fake::{CallKind, FakeSys};

    fn t(s: &str) -> Target {
        Target::parse(s).unwrap()
    }

    #[test]
    fn remedies_by_status() {
        let table: [(Status, Option<Remedy>); 9] = [
            (Status::Ready, None),
            (Status::NoHyprland, None),
            (Status::NoServer, Some(Remedy::InstallServer)),
            (Status::Auth, Some(Remedy::CopyKey)),
            (
                Status::HostKey { changed: false },
                Some(Remedy::AcceptHostKey),
            ),
            (Status::HostKey { changed: true }, None),
            (
                Status::Unsupported {
                    os: "Darwin".into(),
                },
                None,
            ),
            (Status::Offline { detail: "x".into() }, None),
            (Status::Error { detail: "x".into() }, None),
        ];
        for (status, want) in table {
            assert_eq!(remedy_for(&status), want, "{status:?}");
        }
    }

    #[test]
    fn argv_and_descriptions() {
        let j = t("j@10.0.0.2");
        assert_eq!(
            remedy_argv(Remedy::InstallServer, &j),
            ["ssh", "-t", "--", "j@10.0.0.2", INSTALL_SCRIPT]
        );
        assert!(INSTALL_SCRIPT.contains("omarchy pkg add gliff"));
        assert_eq!(
            remedy_argv(Remedy::CopyKey, &j),
            ["ssh-copy-id", "j@10.0.0.2"]
        );
        assert_eq!(
            remedy_argv(Remedy::AcceptHostKey, &j),
            [
                "ssh",
                "-o",
                "StrictHostKeyChecking=ask",
                "--",
                "j@10.0.0.2",
                "true"
            ]
        );
        assert_eq!(
            describe(Remedy::InstallServer, "j@h"),
            "Install gliff on j@h (runs omarchy pkg add gliff there)"
        );
        assert_eq!(describe(Remedy::CopyKey, "j@h"), "Copy your ssh key to j@h");
        assert_eq!(
            describe(Remedy::AcceptHostKey, "j@h"),
            "Check j@h's host key and accept it"
        );
    }

    struct Env {
        _dir: tempfile::TempDir,
        paths: Paths,
        config: Config,
        fake: FakeSys,
        machine: Machine,
    }

    fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let fake = FakeSys::new();
        let config = Config::default();
        let machine = bare_machine(&fake, &config, t("j@h"));
        Env {
            paths: Paths::in_dir(dir.path()),
            _dir: dir,
            config,
            fake,
            machine,
        }
    }

    const READY: &str = "os=Linux\nhyprland=1\ngliff_server=/x\n";

    fn run_fix(e: &Env) -> (FixReport, String) {
        let mut out = Vec::new();
        let r = run(&e.fake, &e.paths, &e.config, &e.machine, &mut out, false);
        (r, String::from_utf8(out).unwrap())
    }

    #[test]
    fn ready_machine_needs_nothing() {
        let e = env();
        e.fake.on_arg("ssh", "sh").stdout(READY);
        let (r, out) = run_fix(&e);
        assert!(r.ready() && r.remedy.is_none() && r.after.is_none());
        assert_eq!(out, "j@h is ready.\n");
        assert!(
            e.fake
                .calls()
                .iter()
                .all(|c| c.kind != CallKind::Interactive)
        );
    }

    #[test]
    fn a_machine_without_a_remedy_is_explained_and_not_ready() {
        let e = env();
        e.fake.on_arg("ssh", "sh").stdout("os=Linux\nhyprland=0\n");
        let (r, out) = run_fix(&e);
        assert!(!r.ready() && r.remedy.is_none());
        assert!(out.contains("Hyprland is not running there."), "{out}");
        let changed = env();
        changed.fake.on_arg("ssh", "sh").code(255).stderr(
            "WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!\nHost key verification failed.\n",
        );
        let (r, out) = run_fix(&changed);
        assert_eq!(r.before.status, Status::HostKey { changed: true });
        assert!(out.contains("ssh-keygen -R h"), "{out}");
    }

    #[test]
    fn install_remedy_runs_interactively_then_rechecks() {
        let e = env();
        e.fake
            .on_arg("ssh", "sh")
            .stdout("os=Linux\nhyprland=1\ngliff_server=\n")
            .then()
            .stdout(READY);
        e.fake.on("ssh", &["-t"]).code(0);
        let (r, out) = run_fix(&e);
        assert_eq!(r.remedy, Some(Remedy::InstallServer));
        assert_eq!(r.before.status, Status::NoServer);
        assert!(r.ready());
        assert!(out.starts_with("Install gliff on j@h (runs omarchy pkg add gliff there)\n"));
        assert!(out.ends_with("j@h is ready.\n"), "{out}");
        let interactive: Vec<_> = e
            .fake
            .calls()
            .into_iter()
            .filter(|c| c.kind == CallKind::Interactive)
            .collect();
        assert_eq!(interactive.len(), 1);
        assert_eq!(interactive[0].cmd.args[..3], ["-t", "--", "j@h"]);
        // The cache holds the newest result.
        assert_eq!(
            crate::check::load_cached(&e.paths, &e.machine.target)
                .unwrap()
                .status,
            Status::Ready
        );
    }

    #[test]
    fn a_failing_remedy_leaves_the_machine_not_ready() {
        let e = env();
        e.fake
            .on_arg("ssh", "sh")
            .code(255)
            .stderr("Permission denied (publickey).");
        e.fake.on("ssh-copy-id", &[]).code(1);
        let (r, out) = run_fix(&e);
        assert_eq!(r.remedy, Some(Remedy::CopyKey));
        assert!(!r.ready());
        assert_eq!(r.result().status, Status::Auth);
        assert!(out.contains("ssh-copy-id exited with status 1."), "{out}");
        assert!(out.contains("refused the ssh key"), "{out}");
    }

    #[test]
    fn a_missing_remedy_program_is_reported() {
        let e = env();
        e.fake
            .on_arg("ssh", "sh")
            .code(255)
            .stderr("Permission denied (publickey).");
        let (_, out) = run_fix(&e);
        assert!(out.contains("could not run ssh-copy-id"), "{out}");
    }

    #[test]
    fn accepting_a_host_key_uses_the_ask_form() {
        let e = env();
        e.fake
            .on_arg("ssh", "sh")
            .code(255)
            .stderr("No ECDSA host key is known for h and you have requested strict checking.\nHost key verification failed.\n")
            .then()
            .code(0)
            .stderr("")
            .stdout(READY);
        e.fake
            .on("ssh", &["-o", "StrictHostKeyChecking=ask"])
            .code(0);
        let (r, _) = run_fix(&e);
        assert_eq!(r.remedy, Some(Remedy::AcceptHostKey));
        assert!(r.ready());
    }
}
