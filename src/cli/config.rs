//! `gliffherdr config path|init|check`.

use super::{CliError, ConfigCommand, Io};
use crate::paths::Paths;
use crate::sys::Sys;

/// Run a `config` subcommand.
pub fn run(command: &ConfigCommand, sys: &dyn Sys, io: &mut Io<'_>) -> Result<(), CliError> {
    let paths = Paths::from_sys(sys);
    let file = paths.config_file();
    match command {
        ConfigCommand::Path => {
            let _ = writeln!(io.out, "{}", file.display());
            Ok(())
        }
        ConfigCommand::Init => {
            let written = crate::config::write_template_if_missing(&paths)
                .map_err(|e| CliError::failed(format!("cannot write {}: {e}", file.display())))?;
            if written {
                let _ = writeln!(io.out, "{}", file.display());
            } else {
                let _ = writeln!(io.out, "already exists: {}", file.display());
            }
            Ok(())
        }
        ConfigCommand::Check => {
            crate::config::load(&paths).map_err(|e| CliError::usage(e.to_string()))?;
            let _ = writeln!(io.out, "ok");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::fake::FakeSys;

    fn run_cmd(c: &ConfigCommand, root: &std::path::Path) -> (Result<(), CliError>, String) {
        let fake = FakeSys::new();
        fake.set_env("GLIFFHERDR_CONFIG_DIR", root.join("c").to_str().unwrap());
        fake.set_env("GLIFFHERDR_STATE_DIR", root.join("s").to_str().unwrap());
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let r = run(
            c,
            &fake,
            &mut Io {
                out: &mut out,
                err: &mut err,
                color: false,
            },
        );
        (r, String::from_utf8(out).unwrap())
    }

    #[test]
    fn path_init_check() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("c/config.toml");
        let shown = format!("{}\n", file.display());

        assert_eq!(run_cmd(&ConfigCommand::Path, dir.path()).1, shown);
        assert_eq!(
            run_cmd(&ConfigCommand::Check, dir.path()).1,
            "ok\n",
            "no file means defaults"
        );

        let (r, out) = run_cmd(&ConfigCommand::Init, dir.path());
        assert!(r.is_ok());
        assert_eq!(out, shown);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            crate::config::TEMPLATE
        );
        assert_eq!(run_cmd(&ConfigCommand::Check, dir.path()).1, "ok\n");

        let (r, out) = run_cmd(&ConfigCommand::Init, dir.path());
        assert!(r.is_ok());
        assert_eq!(out, format!("already exists: {}\n", file.display()));
    }

    #[test]
    fn check_reports_the_error_with_exit_2() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("c")).unwrap();
        std::fs::write(dir.path().join("c/config.toml"), "colour = 1\n").unwrap();
        let (r, out) = run_cmd(&ConfigCommand::Check, dir.path());
        let e = r.unwrap_err();
        assert_eq!(e.code, 2);
        assert!(
            e.message.contains("unknown field `colour`"),
            "{}",
            e.message
        );
        assert_eq!(out, "");
    }
}
