//! Starting gliff: the launch plan, the isolated config home, the detached spawn and the wait
//! for the window.
//!
//! gliff reopens every machine its config lists as open (contract G5), so each machine gets a
//! config home of its own and a window never shows tabs of other machines.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config::{Config, Mode};
use crate::hypr::{Client, Graphical, Hypr};
use crate::paths::Paths;
use crate::roster::MachineSettings;
use crate::sys::{Cmd, Sys, cmd};
use crate::target::Target;

/// How often [`wait_for_window`] looks for the window.
const POLL: Duration = Duration::from_millis(250);
/// What is linked from the real config home into a window's own (fonts, GTK and dconf settings;
/// the theme comes from `$HOME`, contract G6/G7).
const LINKED: [&str; 4] = ["fontconfig", "gtk-4.0", "gtk-3.0", "dconf"];

/// What the opened window shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenChoice {
    /// The remote's focused screen.
    Mirror,
    /// A private screen sized to the window (`--headless`).
    Headless,
    /// One named remote screen (`--output NAME`).
    Output(String),
}

impl ScreenChoice {
    /// What `mode` means as a choice.
    #[must_use]
    pub fn from_mode(mode: Mode) -> ScreenChoice {
        match mode {
            Mode::Mirror => ScreenChoice::Mirror,
            Mode::Headless => ScreenChoice::Headless,
        }
    }
}

/// Whether `name` can be a remote screen name: letters, digits, `.`, `_` and `-`.
#[must_use]
pub fn valid_output_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

/// Everything needed to start one gliff window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    /// The command: an optional `uwsm-app -a gliff -d … --`, then `env XDG_CONFIG_HOME=… gliff …`.
    pub argv: Vec<String>,
    /// Environment added to the command (the graphical session's).
    pub env: Vec<(String, String)>,
    /// The window's own `XDG_CONFIG_HOME`.
    pub config_home: PathBuf,
    /// The real `XDG_CONFIG_HOME`, from which fonts and GTK settings are linked.
    pub real_config_home: PathBuf,
    /// Where gliff's output goes.
    pub log: PathBuf,
}

/// Build the launch plan (no side effects). `uwsm` wraps the command in `uwsm-app` so the window
/// gets its own systemd scope, as Omarchy launches apps.
#[must_use]
pub fn plan(
    paths: &Paths,
    config: &Config,
    settings: &MachineSettings,
    target: &Target,
    choice: &ScreenChoice,
    uwsm: bool,
    g: &Graphical,
) -> LaunchPlan {
    let dir = paths.window_dir(target);
    let config_home = dir.join("config");
    let mut argv: Vec<String> = Vec::new();
    if uwsm {
        argv.extend(
            ["uwsm-app", "-a", "gliff", "-d"]
                .iter()
                .map(ToString::to_string),
        );
        argv.push(format!("Gliff: {target}"));
        argv.push("--".to_string());
    }
    argv.push("env".to_string());
    argv.push(format!("XDG_CONFIG_HOME={}", config_home.display()));
    argv.push("gliff".to_string());
    let gliff_args = settings
        .gliff_args
        .as_ref()
        .unwrap_or(&config.connect.gliff_args);
    argv.extend(gliff_args.iter().cloned());
    if let Some(bin) = settings
        .server_bin
        .as_ref()
        .or(config.connect.server_bin.as_ref())
    {
        argv.push("--server-bin".to_string());
        argv.push(bin.clone());
    }
    match choice {
        ScreenChoice::Mirror => {}
        ScreenChoice::Headless => argv.push("--headless".to_string()),
        ScreenChoice::Output(name) => {
            argv.push("--output".to_string());
            argv.push(name.clone());
        }
    }
    argv.push("--".to_string());
    argv.push(target.to_string());
    LaunchPlan {
        argv,
        env: g.env.clone(),
        config_home,
        real_config_home: paths.real_config_home.clone(),
        log: dir.join("gliff.log"),
    }
}

/// Create the window's config home and link the fonts, GTK and dconf settings of
/// `real_config_home` into it (those that exist and are not there yet).
pub fn prepare_config_home(real_config_home: &Path, dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for name in LINKED {
        let from = real_config_home.join(name);
        let to = dir.join(name);
        if from.exists() && to.symlink_metadata().is_err() {
            std::os::unix::fs::symlink(&from, &to)?;
        }
    }
    Ok(())
}

/// Whether windows should be started through `uwsm-app`: it is installed and the session is
/// managed by uwsm (`uwsm check is-active` exits 0).
pub fn uwsm_active(sys: &dyn Sys) -> bool {
    sys.which("uwsm-app").is_some()
        && sys
            .run(&cmd("uwsm", &["check", "is-active"]).with_timeout(Duration::from_secs(3)))
            .is_ok_and(|o| o.success())
}

/// `secs` since the epoch as an RFC 3339 UTC time (`2026-11-18T09:30:00Z`).
#[must_use]
pub fn rfc3339(secs: u64) -> String {
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Start the window detached: prepare the config home, write the log's first line
/// (`<time> <argv>`), then spawn the command in its own session without waiting for it.
pub fn launch(sys: &dyn Sys, plan: &LaunchPlan) -> io::Result<()> {
    let Some((program, args)) = plan.argv.split_first() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "empty launch command",
        ));
    };
    prepare_config_home(&plan.real_config_home, &plan.config_home)?;
    if let Some(dir) = plan.log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(
        &plan.log,
        format!("{} {}\n", rfc3339(sys.now_unix()), plan.argv.join(" ")),
    )?;
    let command = Cmd {
        program: program.clone(),
        args: args.to_vec(),
        env: plan.env.clone(),
        ..Cmd::default()
    };
    sys.spawn_detached(&command, &plan.log)
}

/// Poll for the window of `target` every 250 ms until `within` has passed.
pub fn wait_for_window(
    sys: &dyn Sys,
    hypr: &Hypr<'_>,
    target: &str,
    within: Duration,
) -> Option<Client> {
    let started = sys.now_unix();
    let mut waited = Duration::ZERO;
    loop {
        if let Some(c) = hypr.find_window(target) {
            return Some(c);
        }
        // Both clocks: the sum of the sleeps, and the wall clock when a hyprctl call hangs.
        let late = sys.now_unix().saturating_sub(started) > within.as_secs() + 1;
        if waited >= within || late {
            return None;
        }
        let step = POLL.min(within.saturating_sub(waited));
        sys.sleep(step);
        waited += step;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::sys::fake::{CallKind, FakeSys};

    fn target() -> Target {
        Target::parse("j@10.0.0.2").unwrap()
    }

    fn graphical() -> Graphical {
        Graphical {
            env: vec![("WAYLAND_DISPLAY".into(), "wayland-9".into())],
            instance: Some("sig".into()),
        }
    }

    fn plan_for(
        config: &Config,
        settings: &MachineSettings,
        choice: &ScreenChoice,
        uwsm: bool,
    ) -> LaunchPlan {
        let paths = Paths::in_dir(Path::new("/s"));
        plan(
            &paths,
            config,
            settings,
            &target(),
            choice,
            uwsm,
            &graphical(),
        )
    }

    fn words(p: &LaunchPlan) -> Vec<&str> {
        p.argv.iter().map(String::as_str).collect()
    }

    const CFG: &str = "/s/state/windows/j@10.0.0.2-";

    #[test]
    fn plan_mirror_without_uwsm() {
        let p = plan_for(
            &Config::default(),
            &MachineSettings::default(),
            &ScreenChoice::Mirror,
            false,
        );
        let w = words(&p);
        assert_eq!(w[0], "env");
        assert!(
            w[1].starts_with(&format!("XDG_CONFIG_HOME={CFG}")),
            "{}",
            w[1]
        );
        assert!(w[1].ends_with("/config"));
        assert_eq!(&w[2..], ["gliff", "--", "j@10.0.0.2"]);
        assert_eq!(p.env, graphical().env);
        assert!(p.config_home.ends_with("config"));
        assert!(p.log.ends_with("gliff.log"));
        assert_eq!(p.log.parent(), p.config_home.parent());
        assert_eq!(p.real_config_home, Path::new("/s/home/.config"));
    }

    #[test]
    fn plan_headless_and_output() {
        let c = Config::default();
        let s = MachineSettings::default();
        let h = plan_for(&c, &s, &ScreenChoice::Headless, false);
        assert_eq!(&words(&h)[2..], ["gliff", "--headless", "--", "j@10.0.0.2"]);
        let o = plan_for(&c, &s, &ScreenChoice::Output("HDMI-A-1".into()), false);
        assert_eq!(
            &words(&o)[2..],
            ["gliff", "--output", "HDMI-A-1", "--", "j@10.0.0.2"]
        );
    }

    #[test]
    fn plan_through_uwsm() {
        let p = plan_for(
            &Config::default(),
            &MachineSettings::default(),
            &ScreenChoice::Mirror,
            true,
        );
        let w = words(&p);
        assert_eq!(
            &w[..7],
            [
                "uwsm-app",
                "-a",
                "gliff",
                "-d",
                "Gliff: j@10.0.0.2",
                "--",
                "env"
            ]
        );
        assert_eq!(&w[8..], ["gliff", "--", "j@10.0.0.2"]);
    }

    #[test]
    fn plan_uses_global_then_machine_gliff_args_and_server_bin() {
        let mut c = Config::default();
        c.connect.gliff_args = vec!["--no-clipboard".into(), "--video".into(), "cpu".into()];
        c.connect.server_bin = Some("gliff-server --low-bandwidth".into());
        let g = plan_for(
            &c,
            &MachineSettings::default(),
            &ScreenChoice::Headless,
            false,
        );
        assert_eq!(
            &words(&g)[2..],
            [
                "gliff",
                "--no-clipboard",
                "--video",
                "cpu",
                "--server-bin",
                "gliff-server --low-bandwidth",
                "--headless",
                "--",
                "j@10.0.0.2"
            ]
        );
        let m = MachineSettings {
            gliff_args: Some(vec![]),
            server_bin: Some("/opt/gs".into()),
            ..MachineSettings::default()
        };
        let m = plan_for(&c, &m, &ScreenChoice::Mirror, false);
        assert_eq!(
            &words(&m)[2..],
            ["gliff", "--server-bin", "/opt/gs", "--", "j@10.0.0.2"],
            "a machine's empty gliff_args replaces the global ones"
        );
    }

    #[test]
    fn choice_from_mode_and_output_names() {
        assert_eq!(ScreenChoice::from_mode(Mode::Mirror), ScreenChoice::Mirror);
        assert_eq!(
            ScreenChoice::from_mode(Mode::Headless),
            ScreenChoice::Headless
        );
        for ok in ["DP-1", "HDMI-A-1", "eDP_1", "a.b"] {
            assert!(valid_output_name(ok), "{ok}");
        }
        for bad in ["", "a b", "a;b", "-x y", "é", "a/b"] {
            assert!(!valid_output_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn prepare_links_only_what_exists_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        let own = dir.path().join("own/config");
        std::fs::create_dir_all(real.join("fontconfig")).unwrap();
        std::fs::create_dir_all(real.join("gtk-4.0")).unwrap();
        std::fs::create_dir_all(real.join("unrelated")).unwrap();
        prepare_config_home(&real, &own).unwrap();
        assert!(own.join("fontconfig").is_symlink());
        assert_eq!(
            std::fs::read_link(own.join("gtk-4.0")).unwrap(),
            real.join("gtk-4.0")
        );
        assert!(!own.join("gtk-3.0").exists() && !own.join("dconf").exists());
        assert!(!own.join("unrelated").exists());
        prepare_config_home(&real, &own).unwrap();
        // A real directory in the window's home is left alone.
        std::fs::create_dir_all(real.join("dconf")).unwrap();
        std::fs::create_dir_all(own.join("dconf")).unwrap();
        prepare_config_home(&real, &own).unwrap();
        assert!(!own.join("dconf").is_symlink());
    }

    #[test]
    fn uwsm_active_needs_the_program_and_a_managed_session() {
        let fake = FakeSys::new();
        assert!(!uwsm_active(&fake), "no uwsm-app");
        fake.add_which("uwsm-app");
        fake.on("uwsm", &["check", "is-active"]).code(1);
        assert!(!uwsm_active(&fake), "uwsm says no");
        let yes = FakeSys::new();
        yes.add_which("uwsm-app");
        yes.on("uwsm", &["check", "is-active"]).code(0);
        assert!(uwsm_active(&yes));
        let nouwsm = FakeSys::new();
        nouwsm.add_which("uwsm-app");
        assert!(!uwsm_active(&nouwsm), "uwsm itself missing");
    }

    #[test]
    fn rfc3339_known_dates() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_795_058_137), "2026-11-19T03:15:37Z");
        assert_eq!(rfc3339(1_709_251_199), "2024-02-29T23:59:59Z");
    }

    #[test]
    fn launch_prepares_logs_and_spawns_detached() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        std::fs::create_dir_all(paths.real_config_home.join("fontconfig")).unwrap();
        let fake = FakeSys::new();
        fake.set_now(1_795_058_137);
        fake.on("env", &[]);
        let p = plan(
            &paths,
            &Config::default(),
            &MachineSettings::default(),
            &target(),
            &ScreenChoice::Mirror,
            false,
            &graphical(),
        );
        // A stale log is replaced, not appended to.
        std::fs::create_dir_all(p.log.parent().unwrap()).unwrap();
        std::fs::write(&p.log, "old\n").unwrap();
        launch(&fake, &p).unwrap();
        let calls = fake.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].kind, CallKind::Detached);
        assert_eq!(calls[0].log.as_deref(), Some(p.log.as_path()));
        assert_eq!(calls[0].cmd.program, "env");
        assert_eq!(
            calls[0].cmd.args.last().map(String::as_str),
            Some("j@10.0.0.2")
        );
        assert_eq!(calls[0].cmd.env, graphical().env);
        let log = std::fs::read_to_string(&p.log).unwrap();
        assert_eq!(log, format!("2026-11-19T03:15:37Z {}\n", p.argv.join(" ")));
        assert!(p.config_home.join("fontconfig").is_symlink());
    }

    #[test]
    fn launch_reports_a_missing_program() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        let p = plan(
            &paths,
            &Config::default(),
            &MachineSettings::default(),
            &target(),
            &ScreenChoice::Mirror,
            false,
            &graphical(),
        );
        let fake = FakeSys::new(); // `env` is not scripted: NotFound
        assert_eq!(
            launch(&fake, &p).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    fn open_window_after(fake: &FakeSys, polls: usize) {
        let empty = "[]";
        let window = r#"[{"address":"0x1","class":"gliff","title":"j@10.0.0.2 — Gliff"}]"#;
        let rule = fake.on("hyprctl", &["clients", "-j"]);
        rule.stdout(empty);
        for _ in 1..polls {
            rule.then().stdout(empty);
        }
        rule.then().stdout(window);
    }

    #[test]
    fn wait_finds_a_window_that_appears_later() {
        let fake = FakeSys::new();
        open_window_after(&fake, 3);
        let g = graphical();
        let hypr = Hypr::new(&fake, &g);
        let c = wait_for_window(&fake, &hypr, "j@10.0.0.2", Duration::from_secs(8)).unwrap();
        assert_eq!(c.address, "0x1");
        assert_eq!(fake.sleeps(), vec![POLL; 3]);
    }

    #[test]
    fn wait_gives_up_after_the_deadline() {
        let fake = FakeSys::new();
        fake.on("hyprctl", &["clients", "-j"]).stdout("[]");
        let g = graphical();
        let hypr = Hypr::new(&fake, &g);
        assert_eq!(
            wait_for_window(&fake, &hypr, "j@10.0.0.2", Duration::from_millis(300)),
            None
        );
        assert_eq!(
            fake.sleeps(),
            vec![Duration::from_millis(250), Duration::from_millis(50)]
        );
        let none = FakeSys::new();
        assert_eq!(
            wait_for_window(&none, &hypr_of(&none, &g), "x", Duration::ZERO),
            None
        );
        assert_eq!(none.sleeps().len(), 0, "zero wait looks once");
    }

    fn hypr_of<'a>(fake: &'a FakeSys, g: &'a Graphical) -> Hypr<'a> {
        Hypr::new(fake, g)
    }
}
