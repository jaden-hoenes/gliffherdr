//! The connect flow: focus the machine's open window, or check the machine and open one.
//!
//! [`connect`] is blocking and does no terminal I/O; it reports progress through a callback so
//! the command line can print lines and the picker can run it on a thread and show a spinner.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::check::{CheckResult, Status, check_one, load_cached, needs_check};
use crate::config::Config;
use crate::fix::{Remedy, remedy_for};
use crate::herdr::Herdr;
use crate::hypr::{Hypr, graphical_env};
use crate::launch::{ScreenChoice, launch, plan, uwsm_active, valid_output_name, wait_for_window};
use crate::paths::{Paths, write_atomic};
use crate::roster::{Machine, bare_machine, collect};
use crate::status::sentence;
use crate::sys::Sys;
use crate::target::Target;

/// How many targets `recent.json` keeps.
const RECENT_KEEP: usize = 30;
/// How long to wait for the window, in milliseconds, unless `GLIFFHERDR_WINDOW_WAIT_MS` says.
const WINDOW_WAIT_MS: u64 = 8000;

/// Everything the flows need: the system, the directories, the configuration and herdr.
#[derive(Clone)]
pub struct Ctx {
    /// Every external command goes through this.
    pub sys: Arc<dyn Sys>,
    /// Config and state directories.
    pub paths: Paths,
    /// The loaded configuration.
    pub config: Config,
    /// The herdr command line.
    pub herdr: Herdr,
}

impl Ctx {
    /// A context for `sys`, taking the herdr command from its environment.
    pub fn new(sys: Arc<dyn Sys>, paths: Paths, config: Config) -> Ctx {
        let herdr = Herdr::from_sys(sys.as_ref());
        Ctx {
            sys,
            paths,
            config,
            herdr,
        }
    }
}

/// A machine to open and, optionally, what to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectRequest {
    /// The machine.
    pub target: Target,
    /// The screen to show; `None` means the machine's own setting, else `[connect] mode`.
    pub choice: Option<ScreenChoice>,
}

/// The slow steps of [`connect`], reported as they begin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// The machine is being checked over ssh.
    Checking,
    /// gliff is being started.
    Launching,
    /// Waiting for gliff's window to appear.
    Waiting,
}

/// How a successful connect ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connected {
    /// The machine's window was already open and has been focused.
    Focused,
    /// A new window was opened.
    Launched,
}

/// Why connecting failed, with what to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectError {
    /// What went wrong, one sentence.
    pub message: String,
    /// What to do about it. Statuses with a remedy say `ctrl-f` (the picker's key); the command
    /// line rewrites that to `gliffherdr fix <target>` (see `cli::connect::cli_hint`).
    pub hint: Option<String>,
    /// The fix that can be run for this machine.
    pub remedy: Option<Remedy>,
    /// gliff's log, when the failure is that no window opened.
    pub log: Option<PathBuf>,
}

impl ConnectError {
    fn new(message: impl Into<String>, hint: Option<&str>) -> ConnectError {
        ConnectError {
            message: message.into(),
            hint: hint.map(str::to_string),
            remedy: None,
            log: None,
        }
    }
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)?;
        if let Some(h) = &self.hint {
            write!(f, " {h}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ConnectError {}

/// Remember `target` as the most recently used machine: `recent.json` is a JSON array of
/// targets, newest first, without duplicates, at most 30.
pub fn remember_recent(paths: &Paths, target: &Target) -> std::io::Result<()> {
    let file = paths.recent_file();
    let mut list: Vec<String> = std::fs::read_to_string(&file)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    list.retain(|t| t != target.as_str());
    list.insert(0, target.to_string());
    list.truncate(RECENT_KEEP);
    let text = serde_json::to_string(&list).map_err(std::io::Error::other)?;
    write_atomic(&file, format!("{text}\n").as_bytes())
}

/// The last three non-empty lines of gliff's log, on one line.
fn log_tail(log: &std::path::Path) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    lines[lines.len().saturating_sub(3)..].join(" ")
}

/// The machine for `target`: its roster entry when it is listed, else a bare one.
fn machine_for(ctx: &Ctx, target: &Target) -> Machine {
    let roster = collect(ctx.sys.as_ref(), &ctx.paths, &ctx.config);
    roster
        .find(target)
        .cloned()
        .unwrap_or_else(|| bare_machine(ctx.sys.as_ref(), &ctx.config, target.clone()))
}

/// The screen to show: the request's choice, else the machine's `output` or `mode`, else
/// `[connect] mode`.
fn choose(ctx: &Ctx, machine: &Machine, requested: Option<&ScreenChoice>) -> ScreenChoice {
    if let Some(c) = requested {
        return c.clone();
    }
    if let Some(o) = &machine.settings.output {
        return ScreenChoice::Output(o.clone());
    }
    ScreenChoice::from_mode(machine.settings.mode.unwrap_or(ctx.config.connect.mode))
}

/// Open `req.target` in gliff, or focus its window when one is open.
///
/// - gliff must be installed and there must be a Hyprland session.
/// - An open window is focused.
/// - A missing, stale or failed check is repeated (`Progress::Checking`); a machine that is not
///   `Ready` fails with its sentence and remedy.
/// - The screen is chosen, gliff is launched detached (`Launching`) and its window awaited
///   (`Waiting`, up to 8 s or `GLIFFHERDR_WINDOW_WAIT_MS`).
pub fn connect(
    ctx: &Ctx,
    req: &ConnectRequest,
    progress: &mut dyn FnMut(Progress),
) -> Result<Connected, ConnectError> {
    let sys = ctx.sys.as_ref();
    let target = &req.target;
    if let Some(ScreenChoice::Output(name)) = &req.choice
        && !valid_output_name(name)
    {
        return Err(ConnectError::new(
            format!("Not a screen name: {name}"),
            None,
        ));
    }
    if sys.which("gliff").is_none() {
        return Err(ConnectError::new(
            "gliff is not installed on this machine.",
            Some("omarchy pkg add gliff"),
        ));
    }
    let Some(g) = graphical_env(sys) else {
        return Err(ConnectError::new(
            "No Hyprland session found to open a window in.",
            Some("herdr runs plugin commands on its own machine; gliff needs a desktop there."),
        ));
    };
    let hypr = Hypr::new(sys, &g);

    if let Some(window) = hypr.find_window(target.as_str()) {
        hypr.focus(&window.address).map_err(|e| {
            ConnectError::new(
                format!("Could not focus the open gliff window for {target}."),
                Some(&e.to_string()),
            )
        })?;
        let _ = remember_recent(&ctx.paths, target);
        return Ok(Connected::Focused);
    }

    let machine = machine_for(ctx, target);
    let cached = load_cached(&ctx.paths, target);
    let result: CheckResult = if needs_check(
        cached.as_ref(),
        sys.now_unix(),
        ctx.config.check.fresh_for_secs,
    ) {
        progress(Progress::Checking);
        check_one(sys, &ctx.paths, &ctx.config, &machine)
    } else {
        cached.unwrap_or_else(|| check_one(sys, &ctx.paths, &ctx.config, &machine))
    };
    if result.status != Status::Ready {
        let (message, hint) = sentence(&result.status, target.as_str());
        return Err(ConnectError {
            message,
            hint,
            remedy: remedy_for(&result.status),
            log: None,
        });
    }

    let choice = choose(ctx, &machine, req.choice.as_ref());
    let launch_plan = plan(
        &ctx.paths,
        &ctx.config,
        &machine.settings,
        target,
        &choice,
        uwsm_active(sys),
        &g,
    );
    progress(Progress::Launching);
    launch(sys, &launch_plan).map_err(|e| {
        let mut err = ConnectError::new(format!("Could not start gliff: {e}."), None);
        err.log = Some(launch_plan.log.clone());
        err
    })?;
    let _ = remember_recent(&ctx.paths, target);
    progress(Progress::Waiting);
    let wait = sys
        .env("GLIFFHERDR_WINDOW_WAIT_MS")
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(WINDOW_WAIT_MS);
    if wait_for_window(sys, &hypr, target.as_str(), Duration::from_millis(wait)).is_none() {
        return Err(ConnectError {
            message: format!("gliff did not open a window for {target}."),
            hint: Some(log_tail(&launch_plan.log)).filter(|t| !t.is_empty()),
            remedy: None,
            log: Some(launch_plan.log),
        });
    }
    Ok(Connected::Launched)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::fake::{CallKind, FakeSys};

    const READY: &str = "os=Linux\nhyprland=1\ngliff_server=/x\noutputs=DP-1 HDMI-A-1\n";
    const WINDOW: &str = r#"[{"address":"0xabc","class":"gliff","title":"j@h — Gliff"}]"#;

    struct Env {
        _dir: tempfile::TempDir,
        fake: Arc<FakeSys>,
        ctx: Ctx,
    }

    fn t(s: &str) -> Target {
        Target::parse(s).unwrap()
    }

    /// gliff installed, a Hyprland session, a ready machine `j@h`, no window yet; the window
    /// shows up on the second `clients` poll.
    fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(FakeSys::new());
        fake.add_which("gliff");
        fake.set_now(1_000_000);
        fake.set_env("WAYLAND_DISPLAY", "wayland-1");
        fake.set_env("HYPRLAND_INSTANCE_SIGNATURE", "sig");
        fake.set_env("XDG_RUNTIME_DIR", "/run/user/1");
        fake.set_env("HERDR_BIN_PATH", "herdr");
        fake.on("hyprctl", &["clients", "-j"])
            .stdout("[]")
            .then()
            .stdout(WINDOW);
        fake.on_arg("ssh", "sh").stdout(READY);
        fake.on("env", &[]);
        fake.on("uwsm", &[]).code(1);
        let sys: Arc<dyn Sys> = fake.clone();
        let ctx = Ctx::new(sys, Paths::in_dir(dir.path()), Config::default());
        Env {
            _dir: dir,
            fake,
            ctx,
        }
    }

    fn go(
        e: &Env,
        target: &str,
        choice: Option<ScreenChoice>,
    ) -> (Result<Connected, ConnectError>, Vec<Progress>) {
        let mut seen = Vec::new();
        let r = connect(
            &e.ctx,
            &ConnectRequest {
                target: t(target),
                choice,
            },
            &mut |p| seen.push(p),
        );
        (r, seen)
    }

    fn detached(e: &Env) -> Vec<crate::sys::Cmd> {
        e.fake
            .calls()
            .into_iter()
            .filter(|c| c.kind == CallKind::Detached)
            .map(|c| c.cmd)
            .collect()
    }

    #[test]
    fn launches_a_ready_machine_and_remembers_it() {
        let e = env();
        let (r, seen) = go(&e, "j@h", None);
        assert_eq!(r, Ok(Connected::Launched));
        assert_eq!(
            seen,
            [Progress::Checking, Progress::Launching, Progress::Waiting]
        );
        let d = detached(&e);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].program, "env");
        assert_eq!(d[0].args.last().map(String::as_str), Some("j@h"));
        assert!(d[0].args[0].starts_with("XDG_CONFIG_HOME="));
        assert_eq!(
            std::fs::read_to_string(e.ctx.paths.recent_file()).unwrap(),
            "[\"j@h\"]\n"
        );
        let log =
            std::fs::read_to_string(e.ctx.paths.window_dir(&t("j@h")).join("gliff.log")).unwrap();
        assert!(log.contains("env XDG_CONFIG_HOME="), "{log}");
    }

    #[test]
    fn focuses_an_open_window_without_checking() {
        let e = env();
        e.fake.on("hyprctl", &["clients", "-j"]).stdout(WINDOW);
        e.fake.on("hyprctl", &["dispatch"]).stdout("ok");
        let (r, seen) = go(&e, "j@h", None);
        assert_eq!(r, Ok(Connected::Focused));
        assert_eq!(seen, []);
        assert_eq!(e.fake.calls_to("ssh").len(), 0, "no ssh at all");
        assert_eq!(detached(&e).len(), 0);
        assert!(
            e.fake
                .call_lines()
                .iter()
                .any(|l| l.contains("address:0xabc")),
            "{:?}",
            e.fake.call_lines()
        );
        assert_eq!(
            std::fs::read_to_string(e.ctx.paths.recent_file()).unwrap(),
            "[\"j@h\"]\n"
        );
    }

    #[test]
    fn a_window_that_cannot_be_focused_is_an_error() {
        let e = env();
        e.fake.on("hyprctl", &["clients", "-j"]).stdout(WINDOW);
        e.fake.on("hyprctl", &["dispatch"]).stdout("Error").code(7);
        let (r, _) = go(&e, "j@h", None);
        let err = r.unwrap_err();
        assert_eq!(
            err.message,
            "Could not focus the open gliff window for j@h."
        );
        assert!(err.hint.unwrap().contains("Hyprland would not focus"));
    }

    #[test]
    fn no_gliff_is_the_first_failure() {
        let e = env();
        let fake = Arc::new(FakeSys::new());
        let sys: Arc<dyn Sys> = fake.clone();
        let ctx = Ctx::new(sys, e.ctx.paths.clone(), Config::default());
        let r = connect(
            &ctx,
            &ConnectRequest {
                target: t("j@h"),
                choice: None,
            },
            &mut |_| {},
        );
        let err = r.unwrap_err();
        assert_eq!(err.message, "gliff is not installed on this machine.");
        assert_eq!(err.hint.as_deref(), Some("omarchy pkg add gliff"));
        assert_eq!(fake.calls().len(), 0);
    }

    #[test]
    fn no_graphical_session_is_a_failure() {
        let e = env();
        e.fake.unset_env("WAYLAND_DISPLAY");
        e.fake.on("systemctl", &[]).stdout("HOME=/x\n");
        e.fake.on("hyprctl", &["instances"]).stdout("");
        let (r, _) = go(&e, "j@h", None);
        let err = r.unwrap_err();
        assert_eq!(
            err.message,
            "No Hyprland session found to open a window in."
        );
        assert_eq!(
            err.hint.as_deref(),
            Some("herdr runs plugin commands on its own machine; gliff needs a desktop there.")
        );
    }

    #[test]
    fn not_ready_fails_with_the_sentence_and_remedy() {
        let table: [(&str, i32, &str, &str, Option<Remedy>); 3] = [
            (
                "os=Linux\nhyprland=1\ngliff_server=\n",
                0,
                "",
                "gliff-server is not installed there.",
                Some(Remedy::InstallServer),
            ),
            (
                "",
                255,
                "Permission denied (publickey).",
                "refused the ssh key",
                Some(Remedy::CopyKey),
            ),
            (
                "os=Linux\nhyprland=0\n",
                0,
                "",
                "Hyprland is not running there.",
                None,
            ),
        ];
        for (out, code, err_text, expect, remedy) in table {
            let e = env();
            e.fake
                .on_arg("ssh", "sh")
                .stdout(out)
                .code(code)
                .stderr(err_text);
            let (r, seen) = go(&e, "j@h", None);
            let err = r.unwrap_err();
            assert!(err.message.contains(expect), "{err:?}");
            assert_eq!(err.remedy, remedy);
            assert!(err.log.is_none());
            assert_eq!(seen, [Progress::Checking]);
            assert_eq!(detached(&e).len(), 0, "never launched");
        }
    }

    #[test]
    fn a_fresh_ready_result_is_not_rechecked() {
        let e = env();
        let (r, _) = go(&e, "j@h", None);
        assert!(r.is_ok());
        let checks = e
            .fake
            .calls_to("ssh")
            .iter()
            .filter(|c| c.args.contains(&"sh".to_string()))
            .count();
        assert_eq!(checks, 1);
        e.fake
            .on("hyprctl", &["clients", "-j"])
            .stdout("[]")
            .then()
            .stdout(WINDOW);
        e.fake.set_now(1_000_100);
        let (r, seen) = go(&e, "j@h", None);
        assert!(r.is_ok());
        assert_eq!(
            seen,
            [Progress::Launching, Progress::Waiting],
            "no Checking"
        );
        let checks = e
            .fake
            .calls_to("ssh")
            .iter()
            .filter(|c| c.args.contains(&"sh".to_string()))
            .count();
        assert_eq!(checks, 1);
    }

    #[test]
    fn a_stale_result_is_rechecked() {
        let e = env();
        assert!(go(&e, "j@h", None).0.is_ok());
        e.fake
            .on("hyprctl", &["clients", "-j"])
            .stdout("[]")
            .then()
            .stdout(WINDOW);
        e.fake.set_now(1_000_000 + 601);
        let (_, seen) = go(&e, "j@h", None);
        assert_eq!(seen[0], Progress::Checking);
    }

    #[test]
    fn a_cached_failure_is_rechecked_after_the_fix() {
        let e = env();
        e.fake
            .on_arg("ssh", "sh")
            .stdout("os=Linux\nhyprland=1\ngliff_server=\n");
        assert_eq!(
            go(&e, "j@h", None).0.unwrap_err().remedy,
            Some(Remedy::InstallServer)
        );
        e.fake.on_arg("ssh", "sh").stdout(READY);
        e.fake
            .on("hyprctl", &["clients", "-j"])
            .stdout("[]")
            .then()
            .stdout(WINDOW);
        let (r, seen) = go(&e, "j@h", None);
        assert_eq!(r, Ok(Connected::Launched));
        assert_eq!(seen[0], Progress::Checking);
    }

    #[test]
    fn screen_choice_precedence() {
        let flags = |e: &Env| detached(e)[0].args.clone();
        // request > machine output > machine mode > global mode
        let e = env();
        go(&e, "j@h", Some(ScreenChoice::Headless)).0.unwrap();
        assert!(flags(&e).contains(&"--headless".to_string()));

        let mut e = env();
        e.ctx.config.connect.mode = crate::config::Mode::Headless;
        go(&e, "j@h", None).0.unwrap();
        assert!(flags(&e).contains(&"--headless".to_string()), "global mode");

        let mut e = env();
        e.ctx.config = crate::config::parse(
            "[connect]\nmode = \"headless\"\n[[machine]]\ntarget = \"j@h\"\nmode = \"mirror\"\n",
            std::path::Path::new("c"),
        )
        .unwrap();
        go(&e, "j@h", None).0.unwrap();
        let a = flags(&e);
        assert!(
            !a.contains(&"--headless".to_string()) && !a.contains(&"--output".to_string()),
            "machine mode wins: {a:?}"
        );

        let mut e = env();
        e.ctx.config = crate::config::parse(
            "[[machine]]\ntarget = \"j@h\"\nmode = \"headless\"\noutput = \"DP-1\"\n",
            std::path::Path::new("c"),
        )
        .unwrap();
        go(&e, "j@h", None).0.unwrap();
        let a = flags(&e);
        assert!(a.windows(2).any(|w| w == ["--output", "DP-1"]), "{a:?}");
        assert!(!a.contains(&"--headless".to_string()));

        let e = env();
        go(&e, "j@h", Some(ScreenChoice::Output("HDMI-A-1".into())))
            .0
            .unwrap();
        assert!(flags(&e).windows(2).any(|w| w == ["--output", "HDMI-A-1"]));
    }

    #[test]
    fn a_bad_screen_name_is_refused_before_anything_runs() {
        let e = env();
        let (r, _) = go(&e, "j@h", Some(ScreenChoice::Output("a b;c".into())));
        assert_eq!(r.unwrap_err().message, "Not a screen name: a b;c");
        assert_eq!(e.fake.calls().len(), 0);
    }

    #[test]
    fn launch_uses_uwsm_when_active() {
        let e = env();
        e.fake.add_which("uwsm-app");
        e.fake.on("uwsm", &["check", "is-active"]).code(0);
        e.fake.on("uwsm-app", &[]);
        go(&e, "j@h", None).0.unwrap();
        let d = detached(&e);
        assert_eq!(d[0].program, "uwsm-app");
        assert_eq!(d[0].args[..5], ["-a", "gliff", "-d", "Gliff: j@h", "--"]);
    }

    #[test]
    fn no_window_after_launch_fails_with_the_log_tail() {
        let e = env();
        e.fake.on("hyprctl", &["clients", "-j"]).stdout("[]");
        e.fake.set_env("GLIFFHERDR_WINDOW_WAIT_MS", "300");
        let (r, seen) = go(&e, "j@h", None);
        let err = r.unwrap_err();
        assert_eq!(err.message, "gliff did not open a window for j@h.");
        let log = err.log.clone().unwrap();
        assert!(log.ends_with("gliff.log"));
        assert!(err.hint.unwrap().contains("env XDG_CONFIG_HOME="));
        assert_eq!(seen.last(), Some(&Progress::Waiting));
        assert_eq!(
            e.fake.sleeps().iter().sum::<Duration>(),
            Duration::from_millis(300)
        );
        // The machine was still remembered: gliff did start.
        assert!(e.ctx.paths.recent_file().exists());
    }

    #[test]
    fn a_failed_spawn_is_reported() {
        let e = env();
        e.fake.on("env", &[]).io_error(std::io::ErrorKind::NotFound);
        let (r, seen) = go(&e, "j@h", None);
        let err = r.unwrap_err();
        assert!(
            err.message.starts_with("Could not start gliff: "),
            "{err:?}"
        );
        assert_eq!(seen.last(), Some(&Progress::Launching));
    }

    #[test]
    fn roster_settings_apply_to_a_listed_machine() {
        let e = env();
        let mut e = e;
        e.ctx.config = crate::config::parse(
            "[[machine]]\ntarget = \"j@h\"\nserver_bin = \"gs --x\"\ngliff_args = [\"--no-clipboard\"]\n",
            std::path::Path::new("c"),
        )
        .unwrap();
        go(&e, "j@h", None).0.unwrap();
        let a = detached(&e)[0].args.clone();
        assert!(
            a.windows(2).any(|w| w == ["--server-bin", "gs --x"]),
            "{a:?}"
        );
        assert!(a.contains(&"--no-clipboard".to_string()));
        let stdin = String::from_utf8(
            e.fake
                .calls_to("ssh")
                .iter()
                .find_map(|c| c.stdin.clone())
                .unwrap(),
        )
        .unwrap();
        assert!(stdin.starts_with("server_bin='gs --x'\n"));
    }

    #[test]
    fn recent_is_newest_first_deduplicated_and_capped() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        for n in 0..35 {
            remember_recent(&paths, &t(&format!("m{n}@h"))).unwrap();
        }
        remember_recent(&paths, &t("m20@h")).unwrap();
        let list: Vec<String> =
            serde_json::from_str(&std::fs::read_to_string(paths.recent_file()).unwrap()).unwrap();
        assert_eq!(list.len(), 30);
        assert_eq!(list[..3], ["m20@h", "m34@h", "m33@h"]);
        assert_eq!(list.iter().filter(|t| *t == "m20@h").count(), 1);
        // The roster's Recent source reads the same file.
        let from = crate::roster::sources::from_recent(&paths);
        assert_eq!(from.candidates.len(), 30);
        assert_eq!(from.candidates[0].target, "m20@h");
    }

    #[test]
    fn a_corrupt_recent_file_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        std::fs::create_dir_all(&paths.state_dir).unwrap();
        std::fs::write(paths.recent_file(), "{").unwrap();
        remember_recent(&paths, &t("a@b")).unwrap();
        assert_eq!(
            std::fs::read_to_string(paths.recent_file()).unwrap(),
            "[\"a@b\"]\n"
        );
    }

    #[test]
    fn error_display_joins_message_and_hint() {
        let e = ConnectError::new("x.", Some("do y"));
        assert_eq!(e.to_string(), "x. do y");
        assert_eq!(ConnectError::new("x.", None).to_string(), "x.");
    }
}
