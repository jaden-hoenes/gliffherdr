//! `gliffherdr connect`: open a machine in gliff, or focus its window. `--from-pane` connects to
//! the machine the focused herdr pane is ssh'd into.

use std::io::IsTerminal;
use std::sync::Arc;

use super::open_picker::{self, NOTIFY_TITLE};
use super::{CliError, ConnectArgs, Io, load_config};
use crate::connect::{ConnectError, ConnectRequest, Connected, Ctx, Progress, connect};
use crate::herdr::pane_from_env;
use crate::launch::{ScreenChoice, valid_output_name};
use crate::paths::Paths;
use crate::roster::ssh_argv::ssh_destination;
use crate::sys::Sys;
use crate::target::Target;

/// Run `connect`; progress lines go to stderr when it is a terminal.
pub fn run(args: &ConnectArgs, sys: &Arc<dyn Sys>, io: &mut Io<'_>) -> Result<(), CliError> {
    run_with(args, sys, io, std::io::stderr().is_terminal())
}

/// [`run`] with the progress lines switched explicitly (tests).
pub fn run_with(
    args: &ConnectArgs,
    sys: &Arc<dyn Sys>,
    io: &mut Io<'_>,
    progress_lines: bool,
) -> Result<(), CliError> {
    if let Some(pane) = &args.from_pane {
        return from_pane(pane.as_deref(), sys, io);
    }
    let text = args.target.as_deref().unwrap_or_default();
    let target = Target::parse(text)
        .map_err(|e| CliError::usage(format!("not an ssh destination: {text}: {e}")))?;
    let choice = if args.mirror {
        Some(ScreenChoice::Mirror)
    } else if args.headless {
        Some(ScreenChoice::Headless)
    } else if let Some(name) = &args.output {
        if !valid_output_name(name) {
            return Err(CliError::usage(format!("not a screen name: {name}")));
        }
        Some(ScreenChoice::Output(name.clone()))
    } else {
        None
    };
    let ctx = context(sys)?;
    let t = target.to_string();
    let choice = if args.choose {
        choose(&ctx, &target)?
    } else {
        choice
    };
    let req = ConnectRequest { target, choice };
    let mut say = |p: Progress| {
        if progress_lines {
            let line = match p {
                Progress::Checking => format!("Checking {t}…"),
                Progress::Launching => format!("Opening {t}…"),
                Progress::Waiting => "Waiting for the window…".to_string(),
            };
            let _ = writeln!(io.err, "{line}");
        }
    };
    let done = connect(&ctx, &req, &mut say).map_err(|e| cli_error(&e, &t))?;
    let _ = match done {
        Connected::Focused => writeln!(io.out, "Focused the open window for {t}."),
        Connected::Launched => writeln!(io.out, "Opened {t}."),
    };
    Ok(())
}

/// `--choose`: the picker's screen chooser, full-screen. Cancelling is a failure (exit 1).
fn choose(ctx: &Ctx, target: &Target) -> Result<Option<ScreenChoice>, CliError> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(CliError::usage("--choose needs a terminal"));
    }
    match crate::picker::choose_screen(ctx, target) {
        Ok(Some(choice)) => Ok(Some(choice)),
        Ok(None) => Err(CliError::failed("cancelled")),
        Err(e) => Err(CliError::failed(format!("{e:#}"))),
    }
}

fn context(sys: &Arc<dyn Sys>) -> Result<Ctx, CliError> {
    let paths = Paths::from_sys(sys.as_ref());
    let config = load_config(&paths)?;
    Ok(Ctx::new(Arc::clone(sys), paths, config))
}

/// The hint for the command line: the picker's `ctrl-f` becomes `gliffherdr fix <target>`.
#[must_use]
pub fn cli_hint(hint: &str, target: &str) -> String {
    let fix = format!("gliffherdr fix {target}");
    hint.replace("Press ctrl-f to", &format!("Run `{fix}` to"))
        .replace("ctrl-f", &format!("`{fix}`"))
}

/// A failed connect as a command line error (exit 1): the message, the hint, and the log path.
fn cli_error(e: &ConnectError, target: &str) -> CliError {
    let mut lines: Vec<String> = Vec::new();
    if let Some(h) = &e.hint {
        lines.push(cli_hint(h, target));
    }
    if let Some(log) = &e.log {
        lines.push(format!("log: {}", log.display()));
    }
    let mut err = CliError::failed(e.message.clone());
    if !lines.is_empty() {
        err = err.with_hint(lines.join("\n  "));
    }
    err
}

/// Connect to the machine of the pane's `ssh` process; nothing to connect to means the picker.
///
/// There is no terminal (herdr runs actions without one), so every failure is also shown as a
/// herdr notification.
fn from_pane(pane_arg: Option<&str>, sys: &Arc<dyn Sys>, io: &mut Io<'_>) -> Result<(), CliError> {
    let ctx = context(sys)?;
    let notify = |text: &str| {
        let _ = ctx.herdr.notify(sys.as_ref(), NOTIFY_TITLE, text);
    };
    let pane = pane_arg
        .map(str::to_string)
        .filter(|p| !p.trim().is_empty())
        .or_else(|| pane_from_env(sys.as_ref()));
    let Some(pane) = pane else {
        return open_picker::run(sys.as_ref(), io);
    };
    let processes = ctx.herdr.pane_processes(sys.as_ref(), &pane).map_err(|e| {
        notify(&e.to_string());
        CliError::failed(e.to_string())
    })?;
    let Some(dest) = processes.iter().find_map(|argv| ssh_destination(argv)) else {
        return open_picker::run(sys.as_ref(), io);
    };
    let target = Target::parse(&dest)
        .map_err(|e| CliError::failed(format!("not an ssh destination: {dest}: {e}")))?;
    let t = target.to_string();
    let req = ConnectRequest {
        target,
        choice: None,
    };
    let done = connect(&ctx, &req, &mut |_| {}).map_err(|e| {
        let err = cli_error(&e, &t);
        notify(&match &e.hint {
            Some(h) => format!("{} {}", e.message, cli_hint(h, &t)),
            None => e.message.clone(),
        });
        err
    })?;
    let _ = match done {
        Connected::Focused => writeln!(io.out, "Focused the open window for {t}."),
        Connected::Launched => writeln!(io.out, "Opened {t}."),
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::fake::FakeSys;

    fn args(target: Option<&str>) -> ConnectArgs {
        ConnectArgs {
            target: target.map(str::to_string),
            mirror: false,
            headless: false,
            output: None,
            choose: false,
            from_pane: None,
        }
    }

    struct Ran {
        result: Result<(), CliError>,
        out: String,
        err: String,
    }

    fn run_args(a: &ConnectArgs, fake: &Arc<FakeSys>, lines: bool) -> Ran {
        let sys: Arc<dyn Sys> = fake.clone();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let result = run_with(
            a,
            &sys,
            &mut Io {
                out: &mut out,
                err: &mut err,
                color: false,
            },
            lines,
        );
        Ran {
            result,
            out: String::from_utf8(out).unwrap(),
            err: String::from_utf8(err).unwrap(),
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
        fake.set_env("HOME", dir.path().join("h").to_str().unwrap());
        fake.set_env("HERDR_BIN_PATH", "/h");
        fake.set_env("WAYLAND_DISPLAY", "wayland-1");
        fake.set_env("HYPRLAND_INSTANCE_SIGNATURE", "sig");
        fake.set_env("XDG_RUNTIME_DIR", "/run/user/1");
        fake.add_which("gliff");
        (dir, fake)
    }

    #[test]
    fn rejects_bad_targets_before_touching_anything() {
        let (_d, fake) = world();
        for bad in ["-oProxyCommand=touch /tmp/x", "a;b", ""] {
            let r = run_args(&args(Some(bad)), &fake, false);
            let e = r.result.unwrap_err();
            assert_eq!(e.code, 2, "{bad:?}");
            assert!(
                e.message.starts_with("not an ssh destination"),
                "{}",
                e.message
            );
        }
        assert_eq!(fake.calls().len(), 0);
    }

    #[test]
    fn bad_screen_names_are_usage_errors() {
        let (_d, fake) = world();
        let mut a = args(Some("j@h"));
        a.output = Some("a b".into());
        let e = run_args(&a, &fake, false).result.unwrap_err();
        assert_eq!(e.code, 2);
        assert_eq!(fake.calls().len(), 0);
    }

    #[test]
    fn focus_prints_the_sentence_of_the_spec() {
        let (_d, fake) = world();
        fake.on("hyprctl", &["clients", "-j"])
            .stdout(r#"[{"address":"0xabc","class":"gliff","title":"j@h — Gliff"}]"#);
        fake.on("hyprctl", &["dispatch"]).stdout("ok");
        let r = run_args(&args(Some("j@h")), &fake, true);
        assert!(r.result.is_ok());
        assert_eq!(r.out, "Focused the open window for j@h.\n");
        assert_eq!(r.err, "", "no progress when nothing is slow");
    }

    #[test]
    fn launch_prints_progress_only_when_asked() {
        for lines in [true, false] {
            let (_d, fake) = world();
            fake.on("hyprctl", &["clients", "-j"])
                .stdout("[]")
                .then()
                .stdout(r#"[{"address":"0xabc","class":"gliff","title":"j@h — Gliff"}]"#);
            fake.on_arg("ssh", "sh")
                .stdout("os=Linux\nhyprland=1\ngliff_server=/x\n");
            fake.on("env", &[]);
            let r = run_args(&args(Some("j@h")), &fake, lines);
            assert!(r.result.is_ok(), "{:?}", r.result);
            assert_eq!(r.out, "Opened j@h.\n");
            let want = if lines {
                "Checking j@h…\nOpening j@h…\nWaiting for the window…\n"
            } else {
                ""
            };
            assert_eq!(r.err, want);
        }
    }

    #[test]
    fn failures_carry_the_cli_flavoured_hint_and_the_log() {
        let (_d, fake) = world();
        fake.on("hyprctl", &["clients", "-j"]).stdout("[]");
        fake.on_arg("ssh", "sh")
            .stdout("os=Linux\nhyprland=1\ngliff_server=\n");
        let e = run_args(&args(Some("j@h")), &fake, false)
            .result
            .unwrap_err();
        assert_eq!(e.code, 1);
        assert_eq!(e.message, "j@h: gliff-server is not installed there.");
        let hint = e.hint.unwrap();
        assert!(
            hint.contains("Run `gliffherdr fix j@h` to install it"),
            "{hint}"
        );
        assert!(!hint.contains("ctrl-f"));
    }

    #[test]
    fn the_hint_rewrite_covers_every_picker_phrase() {
        assert_eq!(
            cli_hint("Press ctrl-f to check and accept it.", "h"),
            "Run `gliffherdr fix h` to check and accept it."
        );
        assert_eq!(cli_hint("use ctrl-f", "h"), "use `gliffherdr fix h`");
        assert_eq!(
            cli_hint("omarchy pkg add gliff", "h"),
            "omarchy pkg add gliff"
        );
    }

    #[test]
    fn log_path_is_printed_with_the_hint() {
        let e = ConnectError {
            message: "gliff did not open a window for h.".into(),
            hint: Some("tail".into()),
            remedy: None,
            log: Some("/s/gliff.log".into()),
        };
        let c = cli_error(&e, "h");
        assert_eq!(c.hint.as_deref(), Some("tail\n  log: /s/gliff.log"));
    }

    // ---- --from-pane ------------------------------------------------------------------

    fn from_pane_args(pane: Option<&str>) -> ConnectArgs {
        let mut a = args(None);
        a.from_pane = Some(pane.map(str::to_string));
        a
    }

    const PROCESS_INFO: &str = r#"{"result":{"process_info":{"foreground_processes":[{"argv":["fish"]},{"argv":["ssh","-p","2222","j@h"]}]}}}"#;

    #[test]
    fn from_pane_connects_to_the_ssh_destination_of_the_pane() {
        let (_d, fake) = world();
        fake.set_env("HERDR_PANE_ID", "w1:p1");
        fake.on("/h", &["pane", "process-info", "--pane", "w1:p1"])
            .stdout(PROCESS_INFO);
        fake.on("hyprctl", &["clients", "-j"])
            .stdout(r#"[{"address":"0xabc","class":"gliff","title":"ssh://j@h:2222 — Gliff"}]"#);
        fake.on("hyprctl", &["dispatch"]).stdout("ok");
        let r = run_args(&from_pane_args(None), &fake, false);
        assert!(r.result.is_ok(), "{:?}", r.result);
        assert_eq!(r.out, "Focused the open window for ssh://j@h:2222.\n");
    }

    #[test]
    fn from_pane_uses_an_explicit_pane_then_the_context_json() {
        let (_d, fake) = world();
        fake.set_env("HERDR_PLUGIN_CONTEXT_JSON", r#"{"focused_pane_id":"ctx"}"#);
        fake.on("/h", &["pane", "process-info", "--pane", "explicit"])
            .stdout(PROCESS_INFO);
        fake.on("/h", &["pane", "process-info", "--pane", "ctx"])
            .stdout(PROCESS_INFO);
        fake.on("hyprctl", &[]).stdout("[]").code(1);
        let _ = run_args(&from_pane_args(Some("explicit")), &fake, false);
        let _ = run_args(&from_pane_args(None), &fake, false);
        let panes: Vec<String> = fake
            .calls_to("/h")
            .iter()
            .filter(|c| c.args[0] == "pane")
            .map(|c| c.args[3].clone())
            .collect();
        assert_eq!(panes, ["explicit", "ctx"]);
    }

    #[test]
    fn from_pane_failure_notifies_with_message_and_hint() {
        let (_d, fake) = world();
        fake.set_env("HERDR_PANE_ID", "p");
        fake.on("/h", &["pane"]).stdout(PROCESS_INFO);
        fake.on("/h", &["notification"]).stdout("");
        fake.on("hyprctl", &["clients", "-j"]).stdout("[]");
        fake.on_arg("ssh", "sh")
            .stdout("os=Linux\nhyprland=1\ngliff_server=\n");
        let r = run_args(&from_pane_args(None), &fake, false);
        let e = r.result.unwrap_err();
        assert_eq!(e.code, 1);
        let note = fake
            .calls_to("/h")
            .into_iter()
            .find(|c| c.args[0] == "notification")
            .unwrap();
        assert_eq!(note.args[2], "Remote desktop");
        assert_eq!(note.args[3], "--body");
        assert!(
            note.args[4].starts_with("ssh://j@h:2222: gliff-server is not installed there. Run `gliffherdr fix ssh://j@h:2222` to install it"),
            "{}",
            note.args[4]
        );
    }

    #[test]
    fn from_pane_without_ssh_opens_the_picker() {
        let (_d, fake) = world();
        fake.set_env("HERDR_PANE_ID", "p");
        fake.on("/h", &["pane"])
            .stdout(r#"{"result":{"process_info":{"foreground_processes":[{"argv":["fish"]},{"argv":["ssh","-V"]}]}}}"#);
        fake.on("/h", &["plugin"]).stdout("{}");
        let r = run_args(&from_pane_args(None), &fake, false);
        assert!(r.result.is_ok());
        assert!(
            fake.call_lines()
                .last()
                .unwrap()
                .contains("plugin pane open --plugin")
        );
        assert!(
            fake.call_lines()
                .last()
                .unwrap()
                .ends_with("--entrypoint picker")
        );
        assert_eq!(
            fake.calls_to("hyprctl").len(),
            0,
            "never touched the desktop"
        );
    }

    #[test]
    fn from_pane_without_a_pane_opens_the_picker() {
        let (_d, fake) = world();
        fake.on("/h", &["plugin"]).stdout("{}");
        let r = run_args(&from_pane_args(None), &fake, false);
        assert!(r.result.is_ok());
        assert_eq!(fake.call_lines().len(), 1);
    }

    #[test]
    fn from_pane_herdr_failure_is_notified() {
        let (_d, fake) = world();
        fake.set_env("HERDR_PANE_ID", "p");
        fake.on("/h", &["pane"])
            .code(1)
            .stderr(r#"{"error":{"message":"pane not found"}}"#);
        fake.on("/h", &["notification"]).stdout("");
        let e = run_args(&from_pane_args(None), &fake, false)
            .result
            .unwrap_err();
        assert_eq!(e.message, "pane not found");
        assert_eq!(fake.calls_to("/h")[1].args[4], "pane not found");
    }
}
