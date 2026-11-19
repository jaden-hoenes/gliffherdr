//! `gliffherdr doctor`: check that this machine can open gliff windows.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::{CliError, DoctorArgs, Io};
use crate::config;
use crate::herdr::Herdr;
use crate::hypr::{Hypr, graphical_env};
use crate::launch::uwsm_active;
use crate::paths::Paths;
use crate::roster::{Source, collect};
use crate::sys::{Sys, cmd};

/// How bad a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Fine.
    Ok,
    /// Works, but something is off.
    Warn,
    /// Broken: `doctor` exits 1.
    Fail,
    /// Just information.
    Info,
}

impl Level {
    /// The name used in text and JSON.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Level::Ok => "ok",
            Level::Warn => "warn",
            Level::Fail => "fail",
            Level::Info => "info",
        }
    }
}

/// One line of the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// What was checked.
    pub check: &'static str,
    /// The outcome.
    pub level: Level,
    /// One line about it.
    pub message: String,
}

fn finding(check: &'static str, level: Level, message: impl Into<String>) -> Finding {
    Finding {
        check,
        level,
        message: message.into(),
    }
}

/// Run every check and return the findings in report order.
#[must_use]
pub fn diagnose(sys: &dyn Sys) -> Vec<Finding> {
    let mut f = Vec::new();

    match sys.which("gliff") {
        None => f.push(finding(
            "gliff",
            Level::Fail,
            "not found on PATH (omarchy pkg add gliff)",
        )),
        Some(path) => {
            let version = sys
                .run(&cmd("gliff", &["--version"]).with_timeout(Duration::from_secs(5)))
                .ok()
                .filter(crate::sys::Output::success)
                .and_then(|o| o.stdout.lines().next().map(|l| l.trim().to_string()))
                .filter(|l| !l.is_empty());
            f.push(finding(
                "gliff",
                Level::Ok,
                format!(
                    "{} at {}",
                    version.as_deref().unwrap_or("version unknown"),
                    path.display()
                ),
            ));
        }
    }

    match sys.which("ssh") {
        None => f.push(finding("ssh", Level::Fail, "not found on PATH")),
        Some(path) => f.push(finding("ssh", Level::Ok, path.display().to_string())),
    }

    let graphical = graphical_env(sys);
    match &graphical {
        None => f.push(finding(
            "hyprland",
            Level::Fail,
            "no Hyprland session found (no WAYLAND_DISPLAY, systemd user environment or hyprctl instance)",
        )),
        Some(g) => match Hypr::new(sys, g).version() {
            Some(v) => f.push(finding("hyprland", Level::Ok, format!("Hyprland {v} reachable"))),
            None => f.push(finding("hyprland", Level::Fail, "hyprctl does not answer")),
        },
    }

    f.push(finding(
        "launch",
        Level::Info,
        if uwsm_active(sys) {
            "uwsm-app"
        } else {
            "setsid"
        },
    ));

    let herdr = Herdr::from_sys(sys);
    match herdr.version(sys) {
        Ok(v) => f.push(finding(
            "herdr",
            Level::Ok,
            format!(
                "{} ({})",
                if v.is_empty() { "version unknown" } else { &v },
                herdr.bin()
            ),
        )),
        Err(e) => f.push(finding("herdr", Level::Warn, e.to_string())),
    }

    let paths = Paths::from_sys(sys);
    let config = match config::load(&paths) {
        Ok(c) => {
            let message = if paths.config_file().exists() {
                paths.config_file().display().to_string()
            } else {
                "defaults, no file".to_string()
            };
            f.push(finding("config", Level::Info, message));
            Some(c)
        }
        Err(e) => {
            f.push(finding("config", Level::Fail, e.summary()));
            None
        }
    };

    f.push(state_dir_finding(&paths));

    if let Some(c) = config.or_else(|| Some(config::Config::default())) {
        let roster = collect(sys, &paths, &c);
        f.push(finding(
            "roster",
            Level::Info,
            roster_summary(&roster.machines),
        ));
        for w in &roster.warnings {
            f.push(finding("roster", Level::Warn, w.clone()));
        }
    }

    f.push(finding(
        "inside herdr",
        Level::Info,
        if Herdr::in_herdr(sys) { "yes" } else { "no" },
    ));
    f
}

fn state_dir_finding(paths: &Paths) -> Finding {
    let dir = &paths.state_dir;
    let probe = dir.join(format!(".doctor-{}", std::process::id()));
    let result = std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&probe, b"ok"))
        .and_then(|()| std::fs::remove_file(&probe));
    match result {
        Ok(()) => finding("state", Level::Ok, format!("{} is writable", dir.display())),
        Err(e) => finding(
            "state",
            Level::Fail,
            format!("{} is not writable: {e}", dir.display()),
        ),
    }
}

/// `N machines (a herdr, b config, …)`: one part per source that names any.
fn roster_summary(machines: &[crate::roster::Machine]) -> String {
    let sources = [
        Source::Config,
        Source::Herdr,
        Source::Recent,
        Source::Gliff,
        Source::SshConfig,
        Source::Tailscale,
    ];
    let parts: Vec<String> = sources
        .iter()
        .filter_map(|s| {
            let n = machines.iter().filter(|m| m.sources.contains(s)).count();
            (n > 0).then(|| format!("{n} {s}"))
        })
        .collect();
    let total = machines.len();
    let noun = if total == 1 { "machine" } else { "machines" };
    if parts.is_empty() {
        format!("{total} {noun}")
    } else {
        format!("{total} {noun} ({})", parts.join(", "))
    }
}

fn paint(level: Level, text: &str, color: bool) -> String {
    if !color {
        return text.to_string();
    }
    let code = match level {
        Level::Ok => "32",
        Level::Warn => "33",
        Level::Fail => "31",
        Level::Info => "2",
    };
    format!("\x1b[{code}m{text}\x1b[0m")
}

/// Run `doctor`: exit 1 when any check fails.
pub fn run(args: &DoctorArgs, sys: &Arc<dyn Sys>, io: &mut Io<'_>) -> Result<(), CliError> {
    let findings = diagnose(sys.as_ref());
    if args.json {
        let values: Vec<_> = findings
            .iter()
            .map(|f| json!({"check": f.check, "level": f.level.name(), "message": f.message}))
            .collect();
        let text =
            serde_json::to_string_pretty(&values).map_err(|e| CliError::failed(e.to_string()))?;
        let _ = writeln!(io.out, "{text}");
    } else {
        let width = findings.iter().map(|f| f.check.len()).max().unwrap_or(0);
        for f in &findings {
            let label = if f.level == Level::Fail {
                "FAIL"
            } else {
                f.level.name()
            };
            let _ = writeln!(
                io.out,
                "{}  {:width$}  {}",
                paint(f.level, &format!("{label:<4}"), io.color),
                f.check,
                f.message
            );
        }
    }
    if findings.iter().any(|f| f.level == Level::Fail) {
        Err(CliError::quiet(1))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::fake::FakeSys;

    fn healthy() -> (tempfile::TempDir, FakeSys) {
        let dir = tempfile::tempdir().unwrap();
        let fake = FakeSys::new();
        fake.set_env(
            "GLIFFHERDR_CONFIG_DIR",
            dir.path().join("c").to_str().unwrap(),
        );
        fake.set_env(
            "GLIFFHERDR_STATE_DIR",
            dir.path().join("s").to_str().unwrap(),
        );
        fake.set_env("HOME", dir.path().join("h").to_str().unwrap());
        fake.set_env("WAYLAND_DISPLAY", "wayland-1");
        fake.set_env("HYPRLAND_INSTANCE_SIGNATURE", "sig");
        fake.set_env("XDG_RUNTIME_DIR", "/run/user/1");
        fake.set_env("HERDR_BIN_PATH", "/h");
        fake.add_which_at("gliff", "/usr/bin/gliff");
        fake.add_which_at("ssh", "/usr/bin/ssh");
        fake.on("gliff", &["--version"]).stdout("gliff 0.4.3\n");
        fake.on("hyprctl", &["version", "-j"])
            .stdout(r#"{"version":"0.56.2"}"#);
        fake.on("/h", &["--version"]).stdout("herdr 0.9.5\n");
        fake.on("/h", &["machine"])
            .stdout(r#"[{"target":"j@h","label":"j"}]"#);
        (dir, fake)
    }

    fn level_of(f: &[Finding], check: &str) -> Vec<Level> {
        f.iter()
            .filter(|x| x.check == check)
            .map(|x| x.level)
            .collect()
    }

    fn message_of<'a>(f: &'a [Finding], check: &str) -> &'a str {
        &f.iter().find(|x| x.check == check).unwrap().message
    }

    #[test]
    fn a_healthy_machine_has_no_failure() {
        let (_dir, fake) = healthy();
        let f = diagnose(&fake);
        assert!(f.iter().all(|x| x.level != Level::Fail), "{f:?}");
        assert_eq!(message_of(&f, "gliff"), "gliff 0.4.3 at /usr/bin/gliff");
        assert_eq!(message_of(&f, "hyprland"), "Hyprland 0.56.2 reachable");
        assert_eq!(message_of(&f, "launch"), "setsid");
        assert_eq!(message_of(&f, "herdr"), "herdr 0.9.5 (/h)");
        assert_eq!(message_of(&f, "config"), "defaults, no file");
        assert!(message_of(&f, "state").ends_with("is writable"));
        assert_eq!(message_of(&f, "roster"), "1 machine (1 herdr)");
        assert_eq!(message_of(&f, "inside herdr"), "no");
        let order: Vec<_> = f.iter().map(|x| x.check).collect();
        assert_eq!(
            order,
            [
                "gliff",
                "ssh",
                "hyprland",
                "launch",
                "herdr",
                "config",
                "state",
                "roster",
                "inside herdr"
            ]
        );
    }

    #[test]
    fn uwsm_and_herdr_env_are_reported() {
        let (_dir, fake) = healthy();
        fake.add_which("uwsm-app");
        fake.on("uwsm", &["check", "is-active"]).code(0);
        fake.set_env("HERDR_ENV", "1");
        let f = diagnose(&fake);
        assert_eq!(message_of(&f, "launch"), "uwsm-app");
        assert_eq!(message_of(&f, "inside herdr"), "yes");
    }

    #[test]
    fn missing_tools_and_desktop_fail() {
        let fake = FakeSys::new();
        let dir = tempfile::tempdir().unwrap();
        fake.set_env(
            "GLIFFHERDR_CONFIG_DIR",
            dir.path().join("c").to_str().unwrap(),
        );
        fake.set_env(
            "GLIFFHERDR_STATE_DIR",
            dir.path().join("s").to_str().unwrap(),
        );
        let f = diagnose(&fake);
        assert_eq!(level_of(&f, "gliff"), [Level::Fail]);
        assert_eq!(level_of(&f, "ssh"), [Level::Fail]);
        assert_eq!(level_of(&f, "hyprland"), [Level::Fail]);
        assert_eq!(
            level_of(&f, "herdr"),
            [Level::Warn],
            "missing herdr is only a warning"
        );
    }

    #[test]
    fn hyprctl_that_does_not_answer_fails() {
        let (_dir, fake) = healthy();
        fake.on("hyprctl", &["version", "-j"]).code(1);
        assert_eq!(level_of(&diagnose(&fake), "hyprland"), [Level::Fail]);
    }

    #[test]
    fn an_invalid_config_fails_and_the_roster_still_runs() {
        let (_dir, fake) = healthy();
        let dir = std::path::PathBuf::from(fake.env("GLIFFHERDR_CONFIG_DIR").unwrap());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), "colour = 1\n").unwrap();
        let f = diagnose(&fake);
        assert_eq!(level_of(&f, "config"), [Level::Fail]);
        assert!(
            message_of(&f, "config").contains("config.toml line 1"),
            "{}",
            message_of(&f, "config")
        );
        assert_eq!(level_of(&f, "roster"), [Level::Info]);
    }

    #[test]
    fn an_unwritable_state_dir_fails() {
        let (_dir, fake) = healthy();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a-file");
        std::fs::write(&file, "x").unwrap();
        fake.set_env("GLIFFHERDR_STATE_DIR", file.join("sub").to_str().unwrap());
        assert_eq!(level_of(&diagnose(&fake), "state"), [Level::Fail]);
    }

    #[test]
    fn roster_warnings_are_listed() {
        let (_dir, fake) = healthy();
        fake.on("/h", &["machine"]).code(1).stderr("boom\n");
        let f = diagnose(&fake);
        assert_eq!(level_of(&f, "roster"), [Level::Info, Level::Warn]);
        assert!(
            f.iter()
                .any(|x| x.level == Level::Warn && x.message.starts_with("herdr: "))
        );
    }

    #[test]
    fn roster_summary_counts_per_source() {
        use crate::config::Config;
        use crate::roster::bare_machine;
        use crate::target::Target;
        let fake = FakeSys::new();
        let mut a = bare_machine(&fake, &Config::default(), Target::parse("a@h").unwrap());
        a.sources = vec![Source::Config, Source::Herdr];
        let mut b = a.clone();
        b.sources = vec![Source::Herdr, Source::Tailscale];
        assert_eq!(
            roster_summary(&[a, b]),
            "2 machines (1 config, 2 herdr, 1 tailscale)"
        );
        assert_eq!(roster_summary(&[]), "0 machines");
    }

    fn render(args: &DoctorArgs, fake: FakeSys) -> (Result<(), CliError>, String) {
        let sys: Arc<dyn Sys> = Arc::new(fake);
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
        (r, String::from_utf8(out).unwrap())
    }

    #[test]
    fn text_output_and_exit_code() {
        let (r, out) = render(&DoctorArgs { json: false }, healthy().1);
        assert!(r.is_ok());
        let first = out.lines().next().unwrap();
        assert_eq!(first, "ok    gliff         gliff 0.4.3 at /usr/bin/gliff");
        let (r, out) = render(&DoctorArgs { json: false }, FakeSys::new());
        assert_eq!(r.unwrap_err().code, 1);
        assert!(
            out.lines().next().unwrap().starts_with("FAIL  gliff "),
            "{out}"
        );
    }

    #[test]
    fn json_output() {
        let (r, out) = render(&DoctorArgs { json: true }, healthy().1);
        assert!(r.is_ok());
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let first = &v[0];
        assert_eq!(first["check"], "gliff");
        assert_eq!(first["level"], "ok");
        assert_eq!(first["message"], "gliff 0.4.3 at /usr/bin/gliff");
        assert_eq!(v.as_array().unwrap().len(), 9);
    }

    #[test]
    fn colour_paints_the_level_only() {
        assert_eq!(paint(Level::Fail, "FAIL", true), "\x1b[31mFAIL\x1b[0m");
        assert_eq!(paint(Level::Ok, "ok", false), "ok");
    }
}
