//! `gliffherdr check`: check machines over ssh and cache the results.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::mpsc;

use super::open_picker::NOTIFY_TITLE;
use super::{CheckArgs, CliError, Io, load_config};
use crate::check::{CheckEvent, CheckResult, Status, check_many, is_fresh, load_cached};
use crate::config::Config;
use crate::herdr::Herdr;
use crate::paths::Paths;
use crate::roster::{Machine, Source, bare_machine, collect};
use crate::status::{status_detail, status_name};
use crate::sys::Sys;
use crate::target::Target;
use crate::text::{pad, width};

/// Run `check`.
pub fn run(args: &CheckArgs, sys: &Arc<dyn Sys>, io: &mut Io<'_>) -> Result<(), CliError> {
    let paths = Paths::from_sys(sys.as_ref());
    let config = load_config(&paths)?;
    let machines = select(args, sys.as_ref(), &paths, &config, io)?;
    if machines.is_empty() {
        if !args.quiet && !args.startup {
            let _ = writeln!(io.err, "nothing to check");
        }
        return notify_summary(args, sys, &summary(&[]));
    }
    let results = run_checks(sys, &paths, &config, &machines);
    if !args.quiet && !args.startup {
        let _ = write!(io.out, "{}", format_results(&results));
    }
    notify_summary(args, sys, &summary(&results))
}

/// With `--notify`, show `text` as a herdr notification; a refusal fails the command.
fn notify_summary(args: &CheckArgs, sys: &Arc<dyn Sys>, text: &str) -> Result<(), CliError> {
    if !args.notify {
        return Ok(());
    }
    Herdr::from_sys(sys.as_ref())
        .notify(sys.as_ref(), NOTIFY_TITLE, text)
        .map_err(|e| CliError::failed(format!("could not show the notification: {e}")))
}

/// The notification text: `<a> ready · <b> need setup · <c> unreachable · <d> unsupported`
/// (zero parts omitted), or `nothing to check`.
#[must_use]
pub fn summary(results: &[CheckResult]) -> String {
    let count = |f: fn(&Status) -> bool| results.iter().filter(|r| f(&r.status)).count();
    let parts = [
        (count(|s| matches!(s, Status::Ready)), "ready"),
        (
            count(|s| {
                matches!(
                    s,
                    Status::NoHyprland | Status::NoServer | Status::Auth | Status::HostKey { .. }
                )
            }),
            "need setup",
        ),
        (
            count(|s| matches!(s, Status::Offline { .. } | Status::Error { .. })),
            "unreachable",
        ),
        (
            count(|s| matches!(s, Status::Unsupported { .. })),
            "unsupported",
        ),
    ];
    let text: Vec<String> = parts
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, what)| format!("{n} {what}"))
        .collect();
    if text.is_empty() {
        "nothing to check".to_string()
    } else {
        text.join(" · ")
    }
}

/// The machines `args` asks for: explicit targets (even outside the roster), then `--all`
/// (every checkable machine), `--stale` (checkable ones with a missing or stale result) and
/// `--startup` (stale non-Tailscale machines, when `check.on_startup` is set).
fn select(
    args: &CheckArgs,
    sys: &dyn Sys,
    paths: &Paths,
    config: &Config,
    io: &mut Io<'_>,
) -> Result<Vec<Machine>, CliError> {
    let mut machines: Vec<Machine> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for t in &args.targets {
        let target = Target::parse(t)
            .map_err(|e| CliError::usage(format!("not an ssh destination: {t}: {e}")))?;
        if seen.insert(target.to_string()) {
            machines.push(bare_machine(sys, config, target));
        }
    }
    let startup = args.startup && config.check.on_startup;
    if args.all || args.stale || startup {
        let roster = collect(sys, paths, config);
        for w in &roster.warnings {
            let _ = writeln!(io.err, "gliffherdr: {w}");
        }
        let now = sys.now_unix();
        let fresh_for = config.check.fresh_for_secs;
        for m in roster.machines {
            if !m.checkable() || seen.contains(m.target.as_str()) {
                continue;
            }
            let stale =
                || load_cached(paths, &m.target).is_none_or(|r| !is_fresh(&r, now, fresh_for));
            let wanted = args.all
                || (args.stale && stale())
                || (startup && m.sources != [Source::Tailscale] && stale());
            if wanted {
                seen.insert(m.target.to_string());
                machines.push(m);
            }
        }
    }
    Ok(machines)
}

/// Check `machines` (at most 12 at a time) and return their results in the same order.
pub fn run_checks(
    sys: &Arc<dyn Sys>,
    paths: &Paths,
    config: &Config,
    machines: &[Machine],
) -> Vec<CheckResult> {
    let (tx, rx) = mpsc::channel();
    let handle = check_many(
        Arc::clone(sys),
        paths.clone(),
        config.clone(),
        machines.to_vec(),
        tx,
    );
    let mut done: Vec<CheckResult> = Vec::new();
    for event in rx {
        if let CheckEvent::Finished(r) = event {
            done.push(r);
        }
    }
    let _ = handle.join();
    machines
        .iter()
        .filter_map(|m| done.iter().find(|r| r.target == m.target.as_str()).cloned())
        .collect()
}

/// One line per result: `<target>  <status>  <detail>`, columns aligned.
pub fn format_results(results: &[CheckResult]) -> String {
    let target_w = results.iter().map(|r| width(&r.target)).max().unwrap_or(0);
    let mut out = String::new();
    for r in results {
        let line = format!(
            "{}  {}  {}",
            pad(&r.target, target_w),
            pad(status_name(&r.status), 12),
            status_detail(&r.status)
        );
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::Status;
    use crate::sys::fake::FakeSys;

    fn result(target: &str, status: Status) -> CheckResult {
        CheckResult {
            target: target.into(),
            status,
            os: None,
            outputs: vec![],
            server_version: None,
            checked_at: 1,
        }
    }

    #[test]
    fn lines_are_aligned_and_trimmed() {
        let text = format_results(&[
            result("j@10.0.0.2", Status::Ready),
            result(
                "mac",
                Status::Unsupported {
                    os: "Darwin".into(),
                },
            ),
            result(
                "x",
                Status::Error {
                    detail: "boom".into(),
                },
            ),
        ]);
        assert_eq!(
            text,
            concat!(
                "j@10.0.0.2  ready\n",
                "mac         unsupported   Darwin\n",
                "x           error         boom\n",
            )
        );
    }

    #[test]
    fn nothing_to_format() {
        assert_eq!(format_results(&[]), "");
    }

    fn notify_args() -> CheckArgs {
        CheckArgs {
            targets: vec!["j@h".into()],
            all: false,
            stale: false,
            startup: false,
            quiet: false,
            notify: true,
        }
    }

    fn world() -> (tempfile::TempDir, Arc<FakeSys>) {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(FakeSys::new());
        fake.set_env(
            "GLIFFHERDR_CONFIG_DIR",
            dir.path().join("c").to_str().unwrap(),
        );
        fake.set_env(
            "GLIFFHERDR_STATE_DIR",
            dir.path().join("s").to_str().unwrap(),
        );
        fake.set_env("HERDR_BIN_PATH", "/h");
        (dir, fake)
    }

    fn run_notify(args: &CheckArgs, fake: &Arc<FakeSys>) -> (Result<(), CliError>, String) {
        let sys: Arc<dyn Sys> = fake.clone();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let r = run(
            args,
            &sys,
            &mut Io {
                out: &mut out,
                err: &mut err,
                color: false,
            },
        );
        (r, String::from_utf8(err).unwrap())
    }

    fn body(fake: &FakeSys) -> Option<String> {
        fake.calls_to("/h")
            .iter()
            .find(|c| c.args[0] == "notification")
            .map(|c| c.args[4].clone())
    }

    #[test]
    fn notify_shows_the_summary_as_a_herdr_notification() {
        let (_d, fake) = world();
        fake.on_arg("ssh", "sh")
            .stdout("os=Linux\nhyprland=1\ngliff_server=/x\n");
        fake.on("/h", &["notification"]).stdout("");
        let (r, _) = run_notify(&notify_args(), &fake);
        assert!(r.is_ok());
        assert_eq!(body(&fake).as_deref(), Some("1 ready"));
        let note = fake.calls_to("/h").into_iter().next().unwrap();
        assert_eq!(
            &note.args[..4],
            ["notification", "show", "Remote desktop", "--body"]
        );
    }

    #[test]
    fn notify_with_nothing_to_check_says_so() {
        let (_d, fake) = world();
        fake.on("/h", &[]).stdout("[]");
        let mut a = notify_args();
        a.targets = vec![];
        a.all = true;
        let (r, err) = run_notify(&a, &fake);
        assert!(r.is_ok());
        assert_eq!(err, "nothing to check\n");
        assert_eq!(body(&fake).as_deref(), Some("nothing to check"));
    }

    #[test]
    fn a_failed_notification_fails_the_command() {
        let (_d, fake) = world();
        fake.on_arg("ssh", "sh")
            .stdout("os=Linux\nhyprland=1\ngliff_server=/x\n");
        fake.on("/h", &["notification"]).code(1).stderr("no server");
        let (r, _) = run_notify(&notify_args(), &fake);
        assert_eq!(r.unwrap_err().code, 1);
    }

    #[test]
    fn without_notify_herdr_is_never_called_for_explicit_targets() {
        let (_d, fake) = world();
        fake.on_arg("ssh", "sh")
            .stdout("os=Linux\nhyprland=1\ngliff_server=/x\n");
        let mut a = notify_args();
        a.notify = false;
        assert!(run_notify(&a, &fake).0.is_ok());
        assert_eq!(fake.calls_to("/h").len(), 0);
    }

    fn res(status: Status) -> CheckResult {
        result("x", status)
    }

    #[test]
    fn summary_text_omits_zero_parts() {
        assert_eq!(summary(&[]), "nothing to check");
        let all = [
            res(Status::Ready),
            res(Status::Ready),
            res(Status::NoServer),
            res(Status::Auth),
            res(Status::HostKey { changed: true }),
            res(Status::NoHyprland),
            res(Status::Offline { detail: "x".into() }),
            res(Status::Error { detail: "y".into() }),
            res(Status::Unsupported {
                os: "Darwin".into(),
            }),
        ];
        assert_eq!(
            summary(&all),
            "2 ready · 4 need setup · 2 unreachable · 1 unsupported"
        );
        assert_eq!(summary(&all[..2]), "2 ready");
        assert_eq!(summary(&all[6..7]), "1 unreachable");
    }
}
