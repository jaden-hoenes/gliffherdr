//! `gliffherdr fix`: run the remedy for a machine that needs setup (the herdr `fix` pane, or a
//! shell).

use std::io::IsTerminal;
use std::sync::Arc;

use super::{CliError, FixArgs, Io, load_config};
use crate::fix::run as run_fix;
use crate::paths::Paths;
use crate::roster::bare_machine;
use crate::sys::Sys;
use crate::target::Target;

/// Run `fix`: the target is the argument, else `GLIFFHERDR_FIX_TARGET` (set by the picker when it
/// opens the fix pane). Exit 0 when the machine ends `Ready`, else 1.
pub fn run(args: &FixArgs, sys: &Arc<dyn Sys>, io: &mut Io<'_>) -> Result<(), CliError> {
    run_with(args, sys, io, std::io::stdin().is_terminal())
}

/// [`run`] with the "Press Enter to close." wait switched explicitly (tests).
pub fn run_with(
    args: &FixArgs,
    sys: &Arc<dyn Sys>,
    io: &mut Io<'_>,
    wait: bool,
) -> Result<(), CliError> {
    let text = args
        .target
        .clone()
        .or_else(|| {
            sys.env("GLIFFHERDR_FIX_TARGET")
                .filter(|t| !t.trim().is_empty())
        })
        .ok_or_else(|| {
            CliError::usage("no machine to fix").with_hint("Usage: gliffherdr fix <user@host>")
        })?;
    let target = Target::parse(&text)
        .map_err(|e| CliError::usage(format!("not an ssh destination: {text}: {e}")))?;
    let paths = Paths::from_sys(sys.as_ref());
    let config = load_config(&paths)?;
    let machine = bare_machine(sys.as_ref(), &config, target);
    let report = run_fix(sys.as_ref(), &paths, &config, &machine, io.out, wait);
    if report.ready() {
        Ok(())
    } else {
        Err(CliError::quiet(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::fake::FakeSys;

    fn go(
        target: Option<&str>,
        env_target: Option<&str>,
        fake: &Arc<FakeSys>,
    ) -> (Result<(), CliError>, String) {
        let dir = tempfile::tempdir().unwrap();
        fake.set_env(
            "GLIFFHERDR_CONFIG_DIR",
            dir.path().join("c").to_str().unwrap(),
        );
        fake.set_env(
            "GLIFFHERDR_STATE_DIR",
            dir.path().join("s").to_str().unwrap(),
        );
        if let Some(t) = env_target {
            fake.set_env("GLIFFHERDR_FIX_TARGET", t);
        }
        let sys: Arc<dyn Sys> = fake.clone();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let r = run_with(
            &FixArgs {
                target: target.map(str::to_string),
            },
            &sys,
            &mut Io {
                out: &mut out,
                err: &mut err,
                color: false,
            },
            false,
        );
        (r, String::from_utf8(out).unwrap())
    }

    #[test]
    fn target_from_the_argument_or_the_environment() {
        let fake = Arc::new(FakeSys::new());
        fake.on_arg("ssh", "sh")
            .stdout("os=Linux\nhyprland=1\ngliff_server=/x\n");
        let (r, out) = go(Some("a@b"), Some("c@d"), &fake);
        assert!(r.is_ok());
        assert_eq!(out, "a@b is ready.\n");
        let fake = Arc::new(FakeSys::new());
        fake.on_arg("ssh", "sh")
            .stdout("os=Linux\nhyprland=1\ngliff_server=/x\n");
        let (_, out) = go(None, Some("c@d"), &fake);
        assert_eq!(out, "c@d is ready.\n");
    }

    #[test]
    fn no_target_is_a_usage_error() {
        let fake = Arc::new(FakeSys::new());
        let (r, _) = go(None, None, &fake);
        assert_eq!(r.unwrap_err().code, 2);
        let (r, _) = go(Some("a;b"), None, &fake);
        assert_eq!(r.unwrap_err().code, 2);
        assert_eq!(fake.calls().len(), 0);
    }

    #[test]
    fn a_machine_that_stays_broken_exits_1_silently() {
        let fake = Arc::new(FakeSys::new());
        fake.on_arg("ssh", "sh").stdout("os=Linux\nhyprland=0\n");
        let (r, out) = go(Some("a@b"), None, &fake);
        let e = r.unwrap_err();
        assert_eq!((e.code, e.message.as_str()), (1, ""));
        assert!(out.contains("Hyprland is not running"));
    }
}
