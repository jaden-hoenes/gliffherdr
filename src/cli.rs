//! The command line: the clap command tree and the dispatcher that runs each command.
//!
//! Commands print to the [`Io`] they are given (stdout and stderr in production, buffers in
//! unit tests) and return a [`CliError`] on failure; [`run`] turns that into the exit code:
//! 0 success, 1 the operation failed, 2 usage error or invalid config.

pub mod check;
pub mod config;
pub mod connect;
pub mod doctor;
pub mod fix;
pub mod list;
pub mod open_picker;

use std::io::{IsTerminal, Write};
use std::process::ExitCode;
use std::sync::Arc;

use clap::{ArgGroup, Args, Parser, Subcommand};

use crate::config::Config;
use crate::paths::Paths;
use crate::sys::{RealSys, Sys};

/// Open a gliff remote desktop to any machine you can ssh to.
#[derive(Debug, Parser)]
#[command(name = "gliffherdr", version, about)]
pub struct Cli {
    /// What to do.
    #[command(subcommand)]
    pub command: Command,
}

/// The subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Open the machine picker (`GLIFFHERDR_DEMO=1` shows demo data).
    Pick,
    /// Ask herdr to open the picker popup.
    OpenPicker,
    /// Open a machine in gliff, or focus its open window.
    Connect(ConnectArgs),
    /// List the known machines with their status.
    List(ListArgs),
    /// Check machines over ssh and cache the result.
    Check(CheckArgs),
    /// Fix a machine that needs setup (install gliff-server, copy the ssh key, accept the host key).
    Fix(FixArgs),
    /// Check this machine's setup.
    Doctor(DoctorArgs),
    /// Show, create or validate the configuration file.
    Config {
        /// What to do with the configuration file.
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

/// Arguments of `connect`.
#[derive(Debug, Args)]
#[command(group(ArgGroup::new("screen").args(["mirror", "headless", "output", "choose"])))]
pub struct ConnectArgs {
    /// The ssh destination (user@host, an ssh alias, ssh://user@host:port).
    #[arg(required_unless_present = "from_pane", conflicts_with = "from_pane")]
    pub target: Option<String>,
    /// Mirror the remote's focused screen.
    #[arg(long)]
    pub mirror: bool,
    /// Open a private screen sized to the window.
    #[arg(long)]
    pub headless: bool,
    /// Mirror this remote screen.
    #[arg(long, value_name = "NAME")]
    pub output: Option<String>,
    /// Choose the screen interactively.
    #[arg(long)]
    pub choose: bool,
    /// Connect to the machine the focused herdr pane is ssh'd into (herdr passes the pane).
    #[arg(long, value_name = "PANE_ID", num_args = 0..=1)]
    pub from_pane: Option<Option<String>>,
}

/// Arguments of `list`.
#[derive(Debug, Args)]
pub struct ListArgs {
    /// Print JSON instead of a table.
    #[arg(long)]
    pub json: bool,
    /// Check every machine first.
    #[arg(long)]
    pub check: bool,
    /// Check machines whose result is missing or stale first.
    #[arg(long)]
    pub check_stale: bool,
}

/// Arguments of `check`.
// Four independent switches by design.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Args)]
pub struct CheckArgs {
    /// Machines to check (they need not be in the roster).
    pub targets: Vec<String>,
    /// Check every machine that can run gliff.
    #[arg(long)]
    pub all: bool,
    /// Check machines whose result is missing or stale.
    #[arg(long)]
    pub stale: bool,
    /// Check stale non-Tailscale machines, if enabled in the config (herdr startup hook).
    #[arg(long)]
    pub startup: bool,
    /// Print nothing.
    #[arg(long, short)]
    pub quiet: bool,
    /// Report the outcome as a herdr notification.
    #[arg(long)]
    pub notify: bool,
}

/// Arguments of `fix`.
#[derive(Debug, Args)]
pub struct FixArgs {
    /// The machine to fix (default: `GLIFFHERDR_FIX_TARGET`).
    pub target: Option<String>,
}

/// Arguments of `doctor`.
#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Print JSON.
    #[arg(long)]
    pub json: bool,
}

/// Subcommands of `config`.
#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print the path of the configuration file.
    Path,
    /// Write the commented template if there is no configuration file yet.
    Init,
    /// Validate the configuration file.
    Check,
}

/// Where a command writes.
pub struct Io<'a> {
    /// Standard output.
    pub out: &'a mut dyn Write,
    /// Standard error.
    pub err: &'a mut dyn Write,
    /// Whether stdout takes ANSI colours (a terminal, and `NO_COLOR` unset).
    pub color: bool,
}

/// A failed command: printed as `gliffherdr: <message>` and, indented, the hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    /// What went wrong.
    pub message: String,
    /// What to do about it.
    pub hint: Option<String>,
    /// The process exit code.
    pub code: u8,
}

impl CliError {
    /// Exit code 2: bad usage or invalid configuration.
    pub fn usage(message: impl Into<String>) -> CliError {
        CliError {
            message: message.into(),
            hint: None,
            code: 2,
        }
    }

    /// Exit code 1: the operation failed.
    pub fn failed(message: impl Into<String>) -> CliError {
        CliError {
            message: message.into(),
            hint: None,
            code: 1,
        }
    }

    /// Add a hint line.
    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> CliError {
        self.hint = Some(hint.into());
        self
    }

    /// Exit with `code` and print nothing: the command has already said what it had to say.
    pub fn quiet(code: u8) -> CliError {
        CliError {
            message: String::new(),
            hint: None,
            code,
        }
    }
}

/// Load the configuration; an invalid file is a usage error (exit 2).
pub fn load_config(paths: &Paths) -> Result<Config, CliError> {
    crate::config::load(paths).map_err(|e| CliError::usage(e.to_string()))
}

/// Run a parsed command line and return the exit code.
pub fn run(cli: Cli, sys: &Arc<dyn Sys>, io: &mut Io<'_>) -> u8 {
    let result = match cli.command {
        Command::List(args) => list::run(&args, sys, io),
        Command::Check(args) => check::run(&args, sys, io),
        Command::Config { command } => config::run(&command, sys.as_ref(), io),
        Command::Pick => crate::picker::pick(sys),
        Command::OpenPicker => open_picker::run(sys.as_ref(), io),
        Command::Connect(args) => connect::run(&args, sys, io),
        Command::Fix(args) => fix::run(&args, sys, io),
        Command::Doctor(args) => doctor::run(&args, sys, io),
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            if !e.message.is_empty() {
                let _ = writeln!(io.err, "gliffherdr: {}", e.message);
            }
            if let Some(h) = &e.hint {
                let _ = writeln!(io.err, "  {h}");
            }
            e.code
        }
    }
}

/// The program entry: parse, run against the real system, exit.
pub fn main() -> ExitCode {
    let cli = Cli::parse();
    let sys: Arc<dyn Sys> = Arc::new(RealSys);
    let stdout = std::io::stdout();
    let color = stdout.is_terminal() && sys.env("NO_COLOR").is_none_or(|v| v.is_empty());
    let (mut out, mut err) = (stdout.lock(), std::io::stderr().lock());
    let code = run(
        cli,
        &sys,
        &mut Io {
            out: &mut out,
            err: &mut err,
            color,
        },
    );
    let _ = out.flush();
    ExitCode::from(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("gliffherdr").chain(args.iter().copied()))
    }

    #[test]
    fn every_command_and_flag_of_the_spec_parses() {
        let ok: &[&[&str]] = &[
            &["pick"],
            &["open-picker"],
            &["connect", "j@h"],
            &["connect", "--mirror", "j@h"],
            &["connect", "--headless", "j@h"],
            &["connect", "--output", "DP-1", "j@h"],
            &["connect", "--choose", "j@h"],
            &["connect", "--from-pane"],
            &["connect", "--from-pane", "p_1"],
            &["list"],
            &["list", "--json", "--check", "--check-stale"],
            &["check"],
            &["check", "a", "b@c"],
            &[
                "check",
                "--all",
                "--stale",
                "--startup",
                "--quiet",
                "--notify",
            ],
            &["check", "-q"],
            &["fix"],
            &["fix", "j@h"],
            &["doctor"],
            &["doctor", "--json"],
            &["config", "path"],
            &["config", "init"],
            &["config", "check"],
        ];
        for args in ok {
            assert!(parse(args).is_ok(), "{args:?}: {:?}", parse(args).err());
        }
    }

    #[test]
    fn the_manifest_argv_parses() {
        for args in [
            &["check", "--startup", "--quiet"][..],
            &["pick"],
            &["fix"],
            &["open-picker"],
            &["connect", "--from-pane"],
            &["check", "--all", "--notify"],
        ] {
            assert!(parse(args).is_ok(), "{args:?}");
        }
    }

    #[test]
    fn invalid_combinations_are_usage_errors() {
        let bad: &[&[&str]] = &[
            &[],
            &["nonsense"],
            &["connect"],
            &["connect", "--mirror", "--headless", "j@h"],
            &["connect", "--headless", "--output", "DP-1", "j@h"],
            &["connect", "--choose", "--mirror", "j@h"],
            &["connect", "--from-pane", "--", "j@h"],
            &["config"],
            &["config", "frobnicate"],
            &["list", "--frobnicate"],
        ];
        for args in bad {
            let e = parse(args).unwrap_err();
            assert_eq!(e.exit_code(), 2, "{args:?}");
        }
    }

    #[test]
    fn from_pane_value_is_optional() {
        let Command::Connect(a) = parse(&["connect", "--from-pane"]).unwrap().command else {
            panic!("not connect")
        };
        assert_eq!(a.from_pane, Some(None));
        let Command::Connect(a) = parse(&["connect", "--from-pane", "p"]).unwrap().command else {
            panic!("not connect")
        };
        assert_eq!(a.from_pane, Some(Some("p".to_string())));
        let Command::Connect(a) = parse(&["connect", "h"]).unwrap().command else {
            panic!("not connect")
        };
        assert_eq!((a.from_pane, a.target.as_deref()), (None, Some("h")));
    }

    #[test]
    fn help_and_version_work() {
        assert_eq!(
            parse(&["--help"]).unwrap_err().kind(),
            clap::error::ErrorKind::DisplayHelp
        );
        assert_eq!(
            parse(&["--version"]).unwrap_err().kind(),
            clap::error::ErrorKind::DisplayVersion
        );
    }

    #[test]
    fn errors_print_the_message_then_the_indented_hint() {
        use crate::sys::fake::FakeSys;
        let sys: Arc<dyn Sys> = Arc::new(FakeSys::new());
        let cli = parse(&["fix"]).unwrap();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(
            cli,
            &sys,
            &mut Io {
                out: &mut out,
                err: &mut err,
                color: false,
            },
        );
        assert_eq!(code, 2);
        assert_eq!(
            String::from_utf8(err).unwrap(),
            "gliffherdr: no machine to fix\n  Usage: gliffherdr fix <user@host>\n"
        );

        let e = CliError::failed("x").with_hint("do y");
        assert_eq!((e.code, e.hint.as_deref()), (1, Some("do y")));
    }
}
