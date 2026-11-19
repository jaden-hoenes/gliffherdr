//! A scripted [`Sys`] for unit tests. Hidden from the docs; public so integration tests and the
//! picker's demo mode can use it too.
//!
//! ```
//! use gliffherdr::sys::fake::FakeSys;
//! use gliffherdr::sys::{cmd, Sys};
//!
//! let fake = FakeSys::new();
//! fake.on("hyprctl", &["clients", "-j"]).stdout("[]");
//! let out = fake.run(&cmd("hyprctl", &["clients", "-j"])).unwrap();
//! assert_eq!(out.stdout, "[]");
//! assert_eq!(fake.call_lines(), ["hyprctl clients -j"]);
//! ```
//!
//! Matching: a rule is keyed by the program name plus an argument *prefix* (`on`), or by a
//! predicate over the whole [`Cmd`] (`on_where`, `on_arg`). The most specific rule wins: a
//! predicate beats any prefix, a longer prefix beats a shorter one, and among equals the rule
//! registered last wins (so a test can override an earlier default). An empty prefix is the
//! default response for the program. A call no rule matches fails with
//! [`io::ErrorKind::NotFound`], as a missing program would.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use super::{Cmd, Output, Sys};

type Predicate = Arc<dyn Fn(&Cmd) -> bool + Send + Sync>;
type Handler = Arc<dyn Fn(&Cmd) -> io::Result<Output> + Send + Sync>;

/// How a rule decides whether it applies to a command.
#[derive(Clone)]
enum Matcher {
    Prefix(Vec<String>),
    Where(Predicate),
}

/// What a rule answers.
#[derive(Clone)]
enum Answer {
    Output(Output),
    Error(io::ErrorKind),
    Handler(Handler),
}

#[derive(Clone)]
struct Rule {
    program: String,
    matcher: Matcher,
    /// Queued answers: each call pops the front while more than one remains; the last repeats.
    answers: VecDeque<Answer>,
}

impl Rule {
    fn matches(&self, cmd: &Cmd) -> bool {
        self.program == cmd.program
            && match &self.matcher {
                Matcher::Prefix(p) => cmd.args.starts_with(p),
                Matcher::Where(f) => f(cmd),
            }
    }

    fn specificity(&self) -> usize {
        match &self.matcher {
            Matcher::Prefix(p) => p.len(),
            Matcher::Where(_) => usize::MAX,
        }
    }
}

/// How a recorded call was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    /// [`Sys::run`].
    Run,
    /// [`Sys::run_interactive`].
    Interactive,
    /// [`Sys::spawn_detached`].
    Detached,
}

/// One recorded call to the fake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    /// Which `Sys` method was used.
    pub kind: CallKind,
    /// The command as passed.
    pub cmd: Cmd,
    /// The log path, for detached spawns.
    pub log: Option<PathBuf>,
}

#[derive(Default)]
struct State {
    rules: Vec<Rule>,
    calls: Vec<Call>,
    env: BTreeMap<String, String>,
    which: BTreeMap<String, PathBuf>,
    now_ms: u128,
    sleeps: Vec<Duration>,
}

/// A scripted, recording [`Sys`]. All methods take `&self`, so it can sit in an `Arc<dyn Sys>`
/// while the test keeps another `Arc<FakeSys>` handle to script and inspect it.
#[derive(Default)]
pub struct FakeSys {
    state: Mutex<State>,
}

/// A rule being scripted; every method returns the handle so calls chain.
pub struct RuleHandle<'a> {
    sys: &'a FakeSys,
    index: usize,
}

impl RuleHandle<'_> {
    fn edit(&self, f: impl FnOnce(&mut Rule)) -> &Self {
        f(&mut self.sys.lock().rules[self.index]);
        self
    }

    fn set(&self, answer: Answer) -> &Self {
        self.edit(|r| r.answers = VecDeque::from([answer]))
    }

    fn update_output(&self, f: impl FnOnce(&mut Output)) -> &Self {
        self.edit(|r| {
            if let Some(Answer::Output(o)) = r.answers.back_mut() {
                f(o);
            } else {
                let mut o = Output {
                    code: Some(0),
                    ..Output::default()
                };
                f(&mut o);
                r.answers = VecDeque::from([Answer::Output(o)]);
            }
        })
    }

    /// Answer with this stdout (exit status 0 unless changed).
    pub fn stdout(&self, text: &str) -> &Self {
        self.update_output(|o| o.stdout = text.to_string())
    }

    /// Answer with this stderr.
    pub fn stderr(&self, text: &str) -> &Self {
        self.update_output(|o| o.stderr = text.to_string())
    }

    /// Answer with this exit status.
    pub fn code(&self, code: i32) -> &Self {
        self.update_output(|o| o.code = Some(code))
    }

    /// Answer as if the command had been killed by its timeout.
    pub fn timed_out(&self) -> &Self {
        self.update_output(|o| {
            o.timed_out = true;
            o.code = None;
        })
    }

    /// Fail the call with this I/O error kind (for example `NotFound` for a missing program).
    pub fn io_error(&self, kind: io::ErrorKind) -> &Self {
        self.set(Answer::Error(kind))
    }

    /// Answer with a complete [`Output`].
    pub fn output(&self, output: Output) -> &Self {
        self.set(Answer::Output(output))
    }

    /// Answer by calling `f` with the command, for answers that depend on its stdin or env.
    pub fn handler(&self, f: impl Fn(&Cmd) -> io::Result<Output> + Send + Sync + 'static) -> &Self {
        self.set(Answer::Handler(Arc::new(f)))
    }

    /// Queue another answer after the current ones: successive calls get successive answers and
    /// the last one repeats. Use after `stdout`/`code`/… of the first answer:
    /// `fake.on("x", &[]).stdout("a").then().stdout("b")`.
    pub fn then(&self) -> &Self {
        self.edit(|r| {
            let next = r
                .answers
                .back()
                .cloned()
                .unwrap_or(Answer::Output(Output::default()));
            r.answers.push_back(next);
        })
    }
}

impl FakeSys {
    /// An empty fake: no rules, empty environment, clock at 0, nothing on `PATH`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn add_rule(&self, program: &str, matcher: Matcher) -> RuleHandle<'_> {
        let mut st = self.lock();
        st.rules.push(Rule {
            program: program.to_string(),
            matcher,
            answers: VecDeque::from([Answer::Output(Output {
                code: Some(0),
                ..Output::default()
            })]),
        });
        RuleHandle {
            sys: self,
            index: st.rules.len() - 1,
        }
    }

    /// Script `program` when its arguments start with `args_prefix`. Defaults to exit 0 with
    /// empty output until the handle's methods say otherwise.
    pub fn on(&self, program: &str, args_prefix: &[&str]) -> RuleHandle<'_> {
        let prefix = args_prefix.iter().map(|a| (*a).to_string()).collect();
        self.add_rule(program, Matcher::Prefix(prefix))
    }

    /// Script `program` when `pred` returns true for the whole command.
    pub fn on_where(
        &self,
        program: &str,
        pred: impl Fn(&Cmd) -> bool + Send + Sync + 'static,
    ) -> RuleHandle<'_> {
        self.add_rule(program, Matcher::Where(Arc::new(pred)))
    }

    /// Script `program` when any argument equals `arg` (handy for `ssh ... -- <target> ...`).
    pub fn on_arg(&self, program: &str, arg: &str) -> RuleHandle<'_> {
        let arg = arg.to_string();
        self.on_where(program, move |c| c.args.contains(&arg))
    }

    fn answer(&self, kind: CallKind, cmd: &Cmd, log: Option<&Path>) -> io::Result<Output> {
        let mut st = self.lock();
        st.calls.push(Call {
            kind,
            cmd: cmd.clone(),
            log: log.map(Path::to_path_buf),
        });
        let best = st
            .rules
            .iter()
            .enumerate()
            .filter(|(_, r)| r.matches(cmd))
            .max_by_key(|(i, r)| (r.specificity(), *i))
            .map(|(i, _)| i);
        let Some(i) = best else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("FakeSys: no rule for `{}`", cmd.line()),
            ));
        };
        let answers = &mut st.rules[i].answers;
        let answer = if answers.len() > 1 {
            answers.pop_front()
        } else {
            answers.front().cloned()
        };
        drop(st);
        match answer.expect("a rule always has an answer") {
            Answer::Output(o) => Ok(o),
            Answer::Error(kind) => Err(io::Error::new(kind, "FakeSys: scripted error")),
            Answer::Handler(f) => f(cmd),
        }
    }

    /// Every call so far, in order, across `run`, `run_interactive` and `spawn_detached`.
    #[must_use]
    pub fn calls(&self) -> Vec<Call> {
        self.lock().calls.clone()
    }

    /// The commands of every call so far.
    #[must_use]
    pub fn commands(&self) -> Vec<Cmd> {
        self.lock().calls.iter().map(|c| c.cmd.clone()).collect()
    }

    /// The commands of calls to `program`, in order.
    #[must_use]
    pub fn calls_to(&self, program: &str) -> Vec<Cmd> {
        self.commands()
            .into_iter()
            .filter(|c| c.program == program)
            .collect()
    }

    /// Every call as a `program arg arg` line (see [`Cmd::line`]).
    #[must_use]
    pub fn call_lines(&self) -> Vec<String> {
        self.commands().iter().map(Cmd::line).collect()
    }

    /// Forget the recorded calls (rules stay).
    pub fn clear_calls(&self) {
        self.lock().calls.clear();
    }

    /// Set an environment variable.
    pub fn set_env(&self, key: &str, value: &str) {
        self.lock().env.insert(key.to_string(), value.to_string());
    }

    /// Remove an environment variable.
    pub fn unset_env(&self, key: &str) {
        self.lock().env.remove(key);
    }

    /// Make `which(program)` succeed with `/usr/bin/<program>`.
    pub fn add_which(&self, program: &str) {
        self.add_which_at(program, &format!("/usr/bin/{program}"));
    }

    /// Make `which(program)` succeed with this path.
    pub fn add_which_at(&self, program: &str, path: &str) {
        self.lock()
            .which
            .insert(program.to_string(), PathBuf::from(path));
    }

    /// Set the clock, in seconds since the epoch.
    pub fn set_now(&self, secs: u64) {
        self.lock().now_ms = u128::from(secs) * 1000;
    }

    /// Advance the clock.
    pub fn advance(&self, d: Duration) {
        self.lock().now_ms += d.as_millis();
    }

    /// Every duration passed to `sleep`, in order (each also advanced the clock).
    #[must_use]
    pub fn sleeps(&self) -> Vec<Duration> {
        self.lock().sleeps.clone()
    }

    /// Programs `which` can currently find.
    #[must_use]
    pub fn which_set(&self) -> BTreeSet<String> {
        self.lock().which.keys().cloned().collect()
    }
}

impl Sys for FakeSys {
    fn run(&self, cmd: &Cmd) -> io::Result<Output> {
        self.answer(CallKind::Run, cmd, None)
    }

    fn run_interactive(&self, cmd: &Cmd) -> io::Result<i32> {
        let out = self.answer(CallKind::Interactive, cmd, None)?;
        Ok(out.code.unwrap_or(-1))
    }

    fn spawn_detached(&self, cmd: &Cmd, log: &Path) -> io::Result<()> {
        self.answer(CallKind::Detached, cmd, Some(log)).map(|_| ())
    }

    fn which(&self, program: &str) -> Option<PathBuf> {
        self.lock().which.get(program).cloned()
    }

    fn env(&self, key: &str) -> Option<String> {
        self.lock().env.get(key).cloned()
    }

    fn now_unix(&self) -> u64 {
        u64::try_from(self.lock().now_ms / 1000).unwrap_or(u64::MAX)
    }

    fn sleep(&self, d: Duration) {
        let mut st = self.lock();
        st.sleeps.push(d);
        st.now_ms += d.as_millis();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::cmd;

    #[test]
    fn unscripted_calls_are_not_found_but_still_recorded() {
        let fake = FakeSys::new();
        let err = fake.run(&cmd("hyprctl", &["version"])).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert_eq!(fake.call_lines(), ["hyprctl version"]);
    }

    #[test]
    fn longest_prefix_wins_and_empty_prefix_is_the_default() {
        let fake = FakeSys::new();
        fake.on("hyprctl", &[]).stdout("default");
        fake.on("hyprctl", &["clients"]).stdout("clients");
        fake.on("hyprctl", &["clients", "-j"]).stdout("json");
        let run = |a: &[&str]| fake.run(&cmd("hyprctl", a)).unwrap().stdout;
        assert_eq!(run(&["clients", "-j"]), "json");
        assert_eq!(run(&["clients"]), "clients");
        assert_eq!(run(&["monitors"]), "default");
    }

    #[test]
    fn later_rules_override_equal_ones() {
        let fake = FakeSys::new();
        fake.on("x", &["a"]).stdout("first");
        fake.on("x", &["a"]).stdout("second");
        assert_eq!(fake.run(&cmd("x", &["a"])).unwrap().stdout, "second");
    }

    #[test]
    fn predicates_beat_prefixes_and_on_arg_matches_anywhere() {
        let fake = FakeSys::new();
        fake.on("ssh", &[]).stdout("check");
        fake.on("ssh", &["-G"]).stdout("config");
        fake.on_arg("ssh", "j@host")
            .code(255)
            .stderr("Permission denied");
        let out = fake
            .run(&cmd("ssh", &["-T", "--", "j@host", "sh", "-s"]))
            .unwrap();
        assert_eq!(
            (out.code, out.stderr.as_str()),
            (Some(255), "Permission denied")
        );
        assert_eq!(
            fake.run(&cmd("ssh", &["-G", "--", "other"]))
                .unwrap()
                .stdout,
            "config"
        );
    }

    #[test]
    fn code_stderr_timeout_and_errors() {
        let fake = FakeSys::new();
        fake.on("a", &[]).stdout("o").stderr("e").code(2);
        fake.on("b", &[]).timed_out();
        fake.on("c", &[]).io_error(io::ErrorKind::PermissionDenied);
        let a = fake.run(&cmd("a", &[])).unwrap();
        assert_eq!(
            (a.stdout.as_str(), a.stderr.as_str(), a.code),
            ("o", "e", Some(2))
        );
        let b = fake.run(&cmd("b", &[])).unwrap();
        assert!(b.timed_out && b.code.is_none());
        assert_eq!(
            fake.run(&cmd("c", &[])).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn answers_can_be_queued() {
        let fake = FakeSys::new();
        fake.on("x", &[])
            .stdout("1")
            .then()
            .stdout("2")
            .then()
            .stdout("3");
        let outs: Vec<_> = (0..5)
            .map(|_| fake.run(&cmd("x", &[])).unwrap().stdout)
            .collect();
        assert_eq!(outs, ["1", "2", "3", "3", "3"]);
    }

    #[test]
    fn handlers_see_the_command() {
        let fake = FakeSys::new();
        fake.on("cat", &[]).handler(|c| {
            Ok(Output {
                code: Some(0),
                stdout: String::from_utf8_lossy(c.stdin.as_deref().unwrap_or_default())
                    .into_owned(),
                ..Output::default()
            })
        });
        let out = fake.run(&cmd("cat", &[]).with_stdin("echoed")).unwrap();
        assert_eq!(out.stdout, "echoed");
    }

    #[test]
    fn records_kinds_logs_env_and_timeouts() {
        let fake = FakeSys::new();
        fake.on("gliff", &[]);
        fake.on("sh", &[]).code(7);
        fake.spawn_detached(&cmd("gliff", &["h"]).with_env("A", "1"), Path::new("/l"))
            .unwrap();
        let code = fake
            .run_interactive(&cmd("sh", &[]).with_timeout(Duration::from_secs(1)))
            .unwrap();
        assert_eq!(code, 7);
        let calls = fake.calls();
        assert_eq!(calls[0].kind, CallKind::Detached);
        assert_eq!(calls[0].log.as_deref(), Some(Path::new("/l")));
        assert_eq!(calls[0].cmd.env, [("A".to_string(), "1".to_string())]);
        assert_eq!(calls[1].kind, CallKind::Interactive);
        assert_eq!(calls[1].cmd.timeout, Some(Duration::from_secs(1)));
        assert_eq!(fake.calls_to("gliff").len(), 1);
        fake.clear_calls();
        assert_eq!(fake.calls().len(), 0);
    }

    #[test]
    fn env_which_and_clock() {
        let fake = FakeSys::new();
        assert_eq!(fake.env("HOME"), None);
        fake.set_env("HOME", "/h");
        assert_eq!(fake.env("HOME").as_deref(), Some("/h"));
        fake.unset_env("HOME");
        assert_eq!(fake.env("HOME"), None);

        assert_eq!(fake.which("gliff"), None);
        fake.add_which("gliff");
        fake.add_which_at("uwsm-app", "/opt/uwsm-app");
        assert_eq!(fake.which("gliff"), Some(PathBuf::from("/usr/bin/gliff")));
        assert_eq!(fake.which("uwsm-app"), Some(PathBuf::from("/opt/uwsm-app")));
        assert_eq!(fake.which_set().len(), 2);

        assert_eq!(fake.now_unix(), 0);
        fake.set_now(1000);
        fake.sleep(Duration::from_millis(1500));
        fake.sleep(Duration::from_millis(600));
        assert_eq!(fake.now_unix(), 1002);
        fake.advance(Duration::from_secs(10));
        assert_eq!(fake.now_unix(), 1012);
        assert_eq!(fake.sleeps().len(), 2);
    }

    #[test]
    fn works_behind_an_arc_dyn() {
        let fake = Arc::new(FakeSys::new());
        fake.on("x", &[]).stdout("hi");
        let sys: Arc<dyn Sys> = fake.clone();
        assert_eq!(sys.run(&cmd("x", &[])).unwrap().stdout, "hi");
        assert_eq!(fake.calls().len(), 1);
    }
}
