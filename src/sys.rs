//! The external-command seam.
//!
//! Production code never calls `std::process::Command` outside [`real`]; every external command,
//! environment lookup, clock read and sleep goes through the [`Sys`] trait. Unit tests use
//! [`fake::FakeSys`], CLI integration tests put stub executables on `PATH` and use [`RealSys`].

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[doc(hidden)]
pub mod fake;
pub mod real;

pub use real::RealSys;

/// One external command to run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Cmd {
    /// Program name, looked up on `PATH` unless it contains a `/`.
    pub program: String,
    /// Arguments, passed without a shell.
    pub args: Vec<String>,
    /// Variables added to the inherited environment.
    pub env: Vec<(String, String)>,
    /// Bytes written to the child's stdin; `None` means `/dev/null`.
    pub stdin: Option<Vec<u8>>,
    /// Kill the child after this long; `None` waits forever.
    pub timeout: Option<Duration>,
}

impl Cmd {
    /// Add one environment variable.
    #[must_use]
    pub fn with_env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.to_string(), value.to_string()));
        self
    }

    /// Add several environment variables.
    #[must_use]
    pub fn with_envs(mut self, vars: &[(String, String)]) -> Self {
        self.env.extend(vars.iter().cloned());
        self
    }

    /// Feed `bytes` to the child's stdin.
    #[must_use]
    pub fn with_stdin(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(bytes.into());
        self
    }

    /// Kill the child after `timeout`.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// The command as one display line (`program arg arg`), for logs and test assertions.
    /// Arguments are joined with single spaces and are not quoted.
    #[must_use]
    pub fn line(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// What a finished command produced.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Output {
    /// Exit status; `None` when the child was killed (timeout or signal).
    pub code: Option<i32>,
    /// Standard output, lossy UTF-8.
    pub stdout: String,
    /// Standard error, lossy UTF-8.
    pub stderr: String,
    /// The command was killed because its timeout elapsed.
    pub timed_out: bool,
}

impl Output {
    /// True when the command exited with status 0 and did not time out.
    #[must_use]
    pub fn success(&self) -> bool {
        self.code == Some(0) && !self.timed_out
    }
}

/// Everything the program does to the outside world.
pub trait Sys: Send + Sync {
    /// Run a command to completion, capturing stdout and stderr (lossy UTF-8).
    fn run(&self, cmd: &Cmd) -> io::Result<Output>;
    /// Run a command with the terminal inherited (used by `fix` and the editor); returns the
    /// exit status, or `-1` when the child was killed by a signal.
    fn run_interactive(&self, cmd: &Cmd) -> io::Result<i32>;
    /// Start a command in its own session with stdin from `/dev/null` and stdout and stderr
    /// appended to `log` (created when missing, never truncated); do not wait for it.
    fn spawn_detached(&self, cmd: &Cmd, log: &Path) -> io::Result<()>;
    /// Find an executable on `PATH` (or at the given path when `program` contains a `/`).
    fn which(&self, program: &str) -> Option<PathBuf>;
    /// Read an environment variable.
    fn env(&self, key: &str) -> Option<String>;
    /// Seconds since the Unix epoch.
    fn now_unix(&self) -> u64;
    /// Block for `d`.
    fn sleep(&self, d: Duration);
}

/// Build a [`Cmd`] from a program and its arguments.
#[must_use]
pub fn cmd(program: &str, args: &[&str]) -> Cmd {
    Cmd {
        program: program.to_string(),
        args: args.iter().map(|a| (*a).to_string()).collect(),
        ..Cmd::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmd_builds_with_defaults() {
        let c = cmd("ssh", &["-G", "--", "host"]);
        assert_eq!(c.program, "ssh");
        assert_eq!(c.args, ["-G", "--", "host"]);
        assert!(c.env.is_empty() && c.stdin.is_none() && c.timeout.is_none());
        assert_eq!(c.line(), "ssh -G -- host");
    }

    #[test]
    fn builders_add_env_stdin_timeout() {
        let c = cmd("x", &[])
            .with_env("A", "1")
            .with_envs(&[("B".into(), "2".into())])
            .with_stdin("in")
            .with_timeout(Duration::from_secs(3));
        assert_eq!(c.env, [("A".into(), "1".into()), ("B".into(), "2".into())]);
        assert_eq!(c.stdin.as_deref(), Some(&b"in"[..]));
        assert_eq!(c.timeout, Some(Duration::from_secs(3)));
    }

    #[test]
    fn output_success_needs_code_zero_and_no_timeout() {
        let ok = Output {
            code: Some(0),
            ..Output::default()
        };
        assert!(ok.success());
        assert!(
            !Output {
                code: Some(1),
                ..Output::default()
            }
            .success()
        );
        assert!(
            !Output {
                code: None,
                ..Output::default()
            }
            .success()
        );
        assert!(
            !Output {
                code: Some(0),
                timed_out: true,
                ..Output::default()
            }
            .success()
        );
    }
}
