//! The production [`Sys`]: real processes, real environment, real clock.
//!
//! This file (and `tests/`) is the only place allowed to call `std::process::Command::new`.
#![allow(clippy::disallowed_methods)] // RealSys is the one place that builds processes.

use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{Cmd, Output, Sys};

/// How often a running child is polled for exit while a timeout is pending.
const POLL: Duration = Duration::from_millis(20);
/// How long to wait for the output pipes to close after the child exited. A grandchild that
/// inherited the pipes (an ssh control master, say) must not hang the caller.
const PIPE_GRACE: Duration = Duration::from_secs(2);

/// The real system: processes, environment, clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealSys;

impl RealSys {
    fn command(cmd: &Cmd) -> Command {
        let mut c = Command::new(&cmd.program);
        c.args(&cmd.args);
        for (k, v) in &cmd.env {
            c.env(k, v);
        }
        c
    }
}

/// Read `r` to the end into `buf`, then signal on the returned channel.
fn drain<R: Read + Send + 'static>(mut r: R, buf: Arc<Mutex<Vec<u8>>>) -> Receiver<()> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        while let Ok(n) = r.read(&mut chunk) {
            if n == 0 {
                break;
            }
            if let Ok(mut b) = buf.lock() {
                b.extend_from_slice(&chunk[..n]);
            }
        }
        let _ = tx.send(());
    });
    rx
}

fn take_text(buf: &Mutex<Vec<u8>>) -> String {
    let bytes = buf.lock().map(|b| b.clone()).unwrap_or_default();
    String::from_utf8_lossy(&bytes).into_owned()
}

impl Sys for RealSys {
    fn run(&self, cmd: &Cmd) -> io::Result<Output> {
        let mut c = Self::command(cmd);
        c.stdin(if cmd.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        let mut child = c.spawn()?;

        if let (Some(mut pipe), Some(data)) = (child.stdin.take(), cmd.stdin.clone()) {
            // Closing the pipe (drop at the end of the thread) is the child's EOF.
            thread::spawn(move || {
                let _ = pipe.write_all(&data);
            });
        }
        let out_buf = Arc::new(Mutex::new(Vec::new()));
        let err_buf = Arc::new(Mutex::new(Vec::new()));
        let out_done = child.stdout.take().map(|p| drain(p, Arc::clone(&out_buf)));
        let err_done = child.stderr.take().map(|p| drain(p, Arc::clone(&err_buf)));

        let deadline = cmd.timeout.map(|t| Instant::now() + t);
        let mut timed_out = false;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if deadline.is_some_and(|d| Instant::now() >= d) {
                timed_out = true;
                let _ = child.kill();
                break child.wait()?;
            }
            thread::sleep(POLL);
        };

        let grace_end = Instant::now() + PIPE_GRACE;
        for done in [out_done, err_done].into_iter().flatten() {
            let _ = done.recv_timeout(grace_end.saturating_duration_since(Instant::now()));
        }
        Ok(Output {
            code: if timed_out { None } else { status.code() },
            stdout: take_text(&out_buf),
            stderr: take_text(&err_buf),
            timed_out,
        })
    }

    fn run_interactive(&self, cmd: &Cmd) -> io::Result<i32> {
        let status = Self::command(cmd).status()?;
        Ok(status.code().unwrap_or(-1))
    }

    #[allow(unsafe_code)] // the crate's only unsafe: setsid() between fork and exec, see below
    fn spawn_detached(&self, cmd: &Cmd, log: &Path) -> io::Result<()> {
        let out = OpenOptions::new().create(true).append(true).open(log)?;
        let err = out.try_clone()?;
        let mut c = Self::command(cmd);
        c.stdin(Stdio::null()).stdout(out).stderr(err);
        // SAFETY: the closure runs in the forked child before exec and only calls `setsid`,
        // which is async-signal-safe, touches no memory of ours and takes no locks.
        unsafe {
            c.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        let mut child = c.spawn()?;
        // Reap the child when it ends so a long-lived caller leaves no zombie; never waited on
        // by the caller.
        thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }

    fn which(&self, program: &str) -> Option<PathBuf> {
        if program.contains('/') {
            let p = PathBuf::from(program);
            return is_executable(&p).then_some(p);
        }
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .filter(|d| !d.as_os_str().is_empty())
            .map(|d| d.join(program))
            .find(|p| is_executable(p))
    }

    fn env(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }

    fn now_unix(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    }

    fn sleep(&self, d: Duration) {
        thread::sleep(d);
    }
}

fn is_executable(p: &Path) -> bool {
    p.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::cmd;

    #[test]
    fn run_captures_stdout_stderr_and_code() {
        let out = RealSys
            .run(&cmd("sh", &["-c", "echo out; echo err >&2; exit 3"]))
            .unwrap();
        assert_eq!(out.stdout, "out\n");
        assert_eq!(out.stderr, "err\n");
        assert_eq!(out.code, Some(3));
        assert!(!out.timed_out);
    }

    #[test]
    fn run_feeds_stdin_and_adds_env() {
        let c = cmd("sh", &["-c", "cat; printf %s \"$GLIFFHERDR_TEST_VAR\""])
            .with_stdin("piped ")
            .with_env("GLIFFHERDR_TEST_VAR", "env");
        assert_eq!(RealSys.run(&c).unwrap().stdout, "piped env");
    }

    #[test]
    fn run_gives_dev_null_when_no_stdin() {
        let out = RealSys.run(&cmd("cat", &[])).unwrap();
        assert!(out.success());
        assert_eq!(out.stdout, "");
    }

    #[test]
    fn run_large_stdin_and_output_do_not_deadlock() {
        let data = "x".repeat(1 << 20);
        let out = RealSys
            .run(&cmd("cat", &[]).with_stdin(data.clone()))
            .unwrap();
        assert_eq!(out.stdout.len(), data.len());
    }

    #[test]
    fn run_times_out_and_kills_the_child() {
        let start = Instant::now();
        let out = RealSys
            .run(&cmd("sleep", &["5"]).with_timeout(Duration::from_millis(200)))
            .unwrap();
        assert!(out.timed_out);
        assert_eq!(out.code, None);
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn run_missing_program_is_not_found() {
        let err = RealSys
            .run(&cmd("gliffherdr-no-such-program", &[]))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn run_returns_partial_output_when_a_grandchild_keeps_the_pipe() {
        // The background sleep inherits stdout; the shell itself exits at once.
        let start = Instant::now();
        let out = RealSys
            .run(&cmd("sh", &["-c", "echo hi; sleep 4 &"]))
            .unwrap();
        assert_eq!(out.stdout, "hi\n");
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn which_finds_executables_only() {
        assert!(RealSys.which("sh").is_some());
        assert!(RealSys.which("gliffherdr-no-such-program").is_none());
        assert!(RealSys.which("/bin/sh").is_some() || RealSys.which("/usr/bin/sh").is_some());
        assert!(RealSys.which("/etc/passwd").is_none());
    }

    #[test]
    fn env_and_clock() {
        assert!(RealSys.env("PATH").is_some());
        assert!(RealSys.env("GLIFFHERDR_SURELY_UNSET_VARIABLE").is_none());
        assert!(RealSys.now_unix() > 1_700_000_000);
    }

    fn wait_for(path: &Path, needle: &str) -> String {
        for _ in 0..100 {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            if text.contains(needle) {
                return text;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("{} never contained {needle:?}", path.display());
    }

    #[test]
    fn spawn_detached_appends_to_the_log_and_starts_a_new_session() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("gliff.log");
        std::fs::write(&log, "header\n").unwrap();
        // awk's stat: field 1 is its pid, 4 its parent (the shell, unless the shell exec'd awk),
        // 6 its session id; after setsid the session id is the shell's pid.
        let script = "echo started; echo oops >&2; \
                      awk '{print \"sid\", ($6 == $4 || $6 == $1)}' /proc/self/stat";
        RealSys
            .spawn_detached(&cmd("sh", &["-c", script]), &log)
            .unwrap();
        let text = wait_for(&log, "sid");
        assert!(text.starts_with("header\n"), "log was truncated: {text:?}");
        assert!(text.contains("started") && text.contains("oops"));
        assert!(
            text.contains("sid 1"),
            "child is not a session leader: {text:?}"
        );
    }

    #[test]
    fn spawn_detached_gives_dev_null_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("l");
        RealSys
            .spawn_detached(&cmd("sh", &["-c", "cat; echo eof"]), &log)
            .unwrap();
        wait_for(&log, "eof");
    }

    #[test]
    fn spawn_detached_reports_a_missing_program() {
        let dir = tempfile::tempdir().unwrap();
        let err = RealSys
            .spawn_detached(
                &cmd("gliffherdr-no-such-program", &[]),
                &dir.path().join("l"),
            )
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn run_interactive_returns_the_exit_code() {
        assert_eq!(
            RealSys
                .run_interactive(&cmd("sh", &["-c", "exit 4"]))
                .unwrap(),
            4
        );
    }
}
