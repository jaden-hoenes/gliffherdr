//! The herdr CLI, and nothing else of herdr: saved machines, plugin panes, notifications and the
//! processes of a pane. Every call goes through `HERDR_BIN_PATH` (else `herdr`) with a 5 s
//! timeout; herdr's JSON errors are turned into one readable message.

use std::time::Duration;

use serde_json::Value;
use thiserror::Error;

use crate::paths::PLUGIN_ID;
use crate::sys::{Cmd, Sys, cmd};

/// How long one herdr call may take.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Why a herdr call failed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HerdrError {
    /// There is no `herdr` to run.
    #[error("herdr is not installed")]
    NotInstalled,
    /// herdr could not be started.
    #[error("cannot run herdr: {0}")]
    Spawn(String),
    /// herdr did not answer in time.
    #[error("herdr did not answer within 5s")]
    TimedOut,
    /// herdr ran and reported a problem (its message, or the last line it printed).
    #[error("{0}")]
    Failed(String),
    /// herdr answered with something unexpected.
    #[error("{0}")]
    Unreadable(String),
}

/// The herdr command line of this session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Herdr {
    bin: String,
    plugin_id: String,
}

impl Herdr {
    /// `HERDR_BIN_PATH`, else `herdr` from `PATH`; the plugin id is `HERDR_PLUGIN_ID`, else
    /// [`PLUGIN_ID`].
    pub fn from_sys(sys: &dyn Sys) -> Herdr {
        let var = |k: &str| sys.env(k).filter(|v| !v.is_empty());
        Herdr {
            bin: var("HERDR_BIN_PATH").unwrap_or_else(|| "herdr".to_string()),
            plugin_id: var("HERDR_PLUGIN_ID").unwrap_or_else(|| PLUGIN_ID.to_string()),
        }
    }

    /// The program that is run.
    #[must_use]
    pub fn bin(&self) -> &str {
        &self.bin
    }

    /// The id passed as `--plugin`.
    #[must_use]
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    /// Whether the process was started by herdr (`HERDR_ENV=1`).
    pub fn in_herdr(sys: &dyn Sys) -> bool {
        sys.env("HERDR_ENV").as_deref() == Some("1")
    }

    fn command(&self, args: &[&str]) -> Cmd {
        cmd(&self.bin, args).with_timeout(TIMEOUT)
    }

    /// Run herdr and return its stdout, or the reason it failed.
    fn run(&self, sys: &dyn Sys, args: &[&str]) -> Result<String, HerdrError> {
        let out = sys.run(&self.command(args)).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                HerdrError::NotInstalled
            } else {
                HerdrError::Spawn(e.to_string())
            }
        })?;
        if out.timed_out {
            return Err(HerdrError::TimedOut);
        }
        if !out.success() {
            return Err(HerdrError::Failed(error_message(&out.stdout, &out.stderr)));
        }
        Ok(out.stdout)
    }

    /// The first line of `herdr --version`.
    pub fn version(&self, sys: &dyn Sys) -> Result<String, HerdrError> {
        let text = self.run(sys, &["--version"])?;
        Ok(text.lines().next().unwrap_or("").trim().to_string())
    }

    /// The machines saved in herdr: `(target, label)`. Entries without a target are skipped.
    /// Accepts an array, or an object with a `machines` array.
    pub fn machines(&self, sys: &dyn Sys) -> Result<Vec<(String, Option<String>)>, HerdrError> {
        let text = self.run(sys, &["machine", "list", "--json"])?;
        parse_machines(&text)
    }

    /// Open a pane of this plugin: `plugin pane open --plugin <id> --entrypoint <e>`, with an
    /// optional `--placement`, `--env K=V` pairs and `--focus`.
    pub fn open_pane(
        &self,
        sys: &dyn Sys,
        entrypoint: &str,
        placement: Option<&str>,
        env: &[(&str, &str)],
        focus: bool,
    ) -> Result<(), HerdrError> {
        let mut args = vec![
            "plugin",
            "pane",
            "open",
            "--plugin",
            &self.plugin_id,
            "--entrypoint",
            entrypoint,
        ];
        if let Some(p) = placement {
            args.extend(["--placement", p]);
        }
        let pairs: Vec<String> = env.iter().map(|(k, v)| format!("{k}={v}")).collect();
        for pair in &pairs {
            args.extend(["--env", pair]);
        }
        if focus {
            args.push("--focus");
        }
        self.run(sys, &args).map(drop)
    }

    /// Show a herdr notification: `notification show <title> --body <body>`.
    pub fn notify(&self, sys: &dyn Sys, title: &str, body: &str) -> Result<(), HerdrError> {
        self.run(sys, &["notification", "show", title, "--body", body])
            .map(drop)
    }

    /// The argv of every foreground process of a pane (`pane process-info --pane <id>`).
    pub fn pane_processes(
        &self,
        sys: &dyn Sys,
        pane_id: &str,
    ) -> Result<Vec<Vec<String>>, HerdrError> {
        let text = self.run(sys, &["pane", "process-info", "--pane", pane_id])?;
        parse_processes(&text)
    }
}

/// The text of herdr's error: `error.message` of a JSON error on either stream, else the last
/// non-empty line printed.
fn error_message(stdout: &str, stderr: &str) -> String {
    for text in [stderr, stdout] {
        if let Ok(v) = serde_json::from_str::<Value>(text.trim()) {
            let msg = v.pointer("/error/message").and_then(Value::as_str);
            let code = v.pointer("/error/code").and_then(Value::as_str);
            match (msg, code) {
                (Some(m), _) if !m.is_empty() => return m.to_string(),
                (None, Some(c)) => return c.to_string(),
                _ => {}
            }
        }
    }
    stderr
        .lines()
        .rev()
        .chain(stdout.lines().rev())
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("no details")
        .to_string()
}

/// Parse `machine list --json`.
pub fn parse_machines(json: &str) -> Result<Vec<(String, Option<String>)>, HerdrError> {
    let v: Value = serde_json::from_str(json)
        .map_err(|e| HerdrError::Unreadable(format!("unreadable machine list: {e}")))?;
    let items = match &v {
        Value::Array(a) => a,
        Value::Object(o) => match o.get("machines") {
            Some(Value::Array(a)) => a,
            _ => {
                return Err(HerdrError::Unreadable(
                    "unexpected machine list shape".to_string(),
                ));
            }
        },
        _ => {
            return Err(HerdrError::Unreadable(
                "unexpected machine list shape".to_string(),
            ));
        }
    };
    Ok(items
        .iter()
        .filter_map(|m| {
            let target = m.get("target")?.as_str()?.to_string();
            let label = m.get("label").and_then(Value::as_str).map(str::to_string);
            Some((target, label))
        })
        .collect())
}

/// Parse `.result.process_info.foreground_processes[].argv` of `pane process-info`.
fn parse_processes(json: &str) -> Result<Vec<Vec<String>>, HerdrError> {
    let v: Value = serde_json::from_str(json)
        .map_err(|e| HerdrError::Unreadable(format!("unreadable process info: {e}")))?;
    let list = v
        .pointer("/result/process_info/foreground_processes")
        .and_then(Value::as_array)
        .ok_or_else(|| HerdrError::Unreadable("unexpected process info shape".to_string()))?;
    Ok(list
        .iter()
        .filter_map(|p| {
            let argv = p.get("argv")?.as_array()?;
            Some(
                argv.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect(),
            )
        })
        .collect())
}

/// The focused pane of an invocation: `HERDR_PANE_ID`, else the pane named in
/// `HERDR_PLUGIN_CONTEXT_JSON` (`focused_pane_id`, `pane.pane_id`, `pane.id` or `pane_id`;
/// popups get no `HERDR_PANE_ID`). Anything missing or malformed is `None`.
pub fn pane_from_env(sys: &dyn Sys) -> Option<String> {
    let var = |k: &str| sys.env(k).filter(|v| !v.trim().is_empty());
    if let Some(id) = var("HERDR_PANE_ID") {
        return Some(id.trim().to_string());
    }
    pane_from_context(&var("HERDR_PLUGIN_CONTEXT_JSON")?)
}

/// The pane id inside a `HERDR_PLUGIN_CONTEXT_JSON` document.
#[must_use]
pub fn pane_from_context(json: &str) -> Option<String> {
    let v: Value = serde_json::from_str(json).ok()?;
    ["/focused_pane_id", "/pane/pane_id", "/pane/id", "/pane_id"]
        .iter()
        .filter_map(|p| v.pointer(p).and_then(Value::as_str))
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::fake::FakeSys;
    use std::io::ErrorKind;

    fn herdr() -> (FakeSys, Herdr) {
        let fake = FakeSys::new();
        fake.set_env("HERDR_BIN_PATH", "/opt/herdr");
        let h = Herdr::from_sys(&fake);
        (fake, h)
    }

    #[test]
    fn from_sys_reads_bin_and_plugin_id() {
        let fake = FakeSys::new();
        let h = Herdr::from_sys(&fake);
        assert_eq!((h.bin(), h.plugin_id()), ("herdr", PLUGIN_ID));
        fake.set_env("HERDR_BIN_PATH", "/x/herdr");
        fake.set_env("HERDR_PLUGIN_ID", "me.test");
        let h = Herdr::from_sys(&fake);
        assert_eq!((h.bin(), h.plugin_id()), ("/x/herdr", "me.test"));
        fake.set_env("HERDR_BIN_PATH", "");
        assert_eq!(Herdr::from_sys(&fake).bin(), "herdr", "empty means unset");
    }

    #[test]
    fn in_herdr_needs_env_one() {
        let fake = FakeSys::new();
        assert!(!Herdr::in_herdr(&fake));
        fake.set_env("HERDR_ENV", "0");
        assert!(!Herdr::in_herdr(&fake));
        fake.set_env("HERDR_ENV", "1");
        assert!(Herdr::in_herdr(&fake));
    }

    #[test]
    fn machines_parse_arrays_and_objects() {
        let (fake, h) = herdr();
        fake.on("/opt/herdr", &["machine", "list", "--json"]).stdout(
            r#"[{"target":"j@h","label":"j"},{"target":"k@h"},{"label":"no target"},{"target":"x","label":7}]"#,
        );
        assert_eq!(
            h.machines(&fake).unwrap(),
            [
                ("j@h".to_string(), Some("j".to_string())),
                ("k@h".to_string(), None),
                ("x".to_string(), None),
            ]
        );
        assert_eq!(fake.calls()[0].cmd.timeout, Some(TIMEOUT));
        let obj = FakeSys::new();
        obj.on("herdr", &[])
            .stdout(r#"{"machines":[{"target":"a@b"}]}"#);
        assert_eq!(Herdr::from_sys(&obj).machines(&obj).unwrap().len(), 1);
    }

    #[test]
    fn machines_errors() {
        let (fake, h) = herdr();
        assert_eq!(h.machines(&fake), Err(HerdrError::NotInstalled));
        fake.on("/opt/herdr", &[]).stdout("<html>");
        assert!(
            matches!(h.machines(&fake), Err(HerdrError::Unreadable(m)) if m.contains("unreadable"))
        );
        fake.on("/opt/herdr", &[]).stdout(r#"{"x":1}"#);
        assert!(matches!(h.machines(&fake), Err(HerdrError::Unreadable(_))));
        fake.on("/opt/herdr", &[]).stdout("3");
        assert!(matches!(h.machines(&fake), Err(HerdrError::Unreadable(_))));
        fake.on("/opt/herdr", &[])
            .io_error(ErrorKind::PermissionDenied);
        assert!(matches!(h.machines(&fake), Err(HerdrError::Spawn(_))));
        fake.on("/opt/herdr", &[]).timed_out();
        assert_eq!(h.machines(&fake), Err(HerdrError::TimedOut));
    }

    #[test]
    fn json_errors_are_surfaced_from_either_stream() {
        let (fake, h) = herdr();
        fake.on("/opt/herdr", &[]).code(1).stderr(
            "{\"id\":\"cli\",\"error\":{\"code\":\"ui_busy\",\"message\":\"a popup pane is already open\"}}\n",
        );
        assert_eq!(
            h.open_pane(&fake, "picker", None, &[], false),
            Err(HerdrError::Failed("a popup pane is already open".into()))
        );
        fake.on("/opt/herdr", &[])
            .code(1)
            .stdout(r#"{"error":{"code":"not_linked"}}"#);
        assert_eq!(
            h.notify(&fake, "t", "b"),
            Err(HerdrError::Failed("not_linked".into()))
        );
        fake.on("/opt/herdr", &[])
            .code(2)
            .stderr("boom\nunknown command: machine\n");
        assert_eq!(
            h.machines(&fake),
            Err(HerdrError::Failed("unknown command: machine".into()))
        );
        fake.on("/opt/herdr", &[]).code(2);
        assert_eq!(
            h.machines(&fake),
            Err(HerdrError::Failed("no details".into()))
        );
    }

    #[test]
    fn open_pane_command_shapes() {
        let (fake, h) = herdr();
        fake.on("/opt/herdr", &[]).stdout("{}");
        h.open_pane(&fake, "picker", None, &[], false).unwrap();
        h.open_pane(
            &fake,
            "fix",
            Some("split"),
            &[("GLIFFHERDR_FIX_TARGET", "j@h"), ("A", "b c")],
            true,
        )
        .unwrap();
        assert_eq!(
            fake.call_lines(),
            [
                format!("/opt/herdr plugin pane open --plugin {PLUGIN_ID} --entrypoint picker"),
                format!(
                    "/opt/herdr plugin pane open --plugin {PLUGIN_ID} --entrypoint fix --placement split --env GLIFFHERDR_FIX_TARGET=j@h --env A=b c --focus"
                ),
            ]
        );
        fake.set_env("HERDR_PLUGIN_ID", "me.test");
        let h = Herdr::from_sys(&fake);
        h.open_pane(&fake, "picker", None, &[], false).unwrap();
        assert!(fake.call_lines()[2].contains("--plugin me.test --entrypoint picker"));
    }

    #[test]
    fn version_is_the_first_line() {
        let (fake, h) = herdr();
        fake.on("/opt/herdr", &["--version"])
            .stdout("herdr 0.9.5\nmore\n");
        assert_eq!(h.version(&fake).unwrap(), "herdr 0.9.5");
    }

    #[test]
    fn notify_command_shape() {
        let (fake, h) = herdr();
        fake.on("/opt/herdr", &[]).stdout("");
        h.notify(&fake, "Remote desktop", "j@h is not reachable.")
            .unwrap();
        let c = &fake.calls()[0].cmd;
        assert_eq!(
            c.args,
            [
                "notification",
                "show",
                "Remote desktop",
                "--body",
                "j@h is not reachable."
            ]
        );
        assert_eq!(c.timeout, Some(TIMEOUT));
    }

    #[test]
    fn pane_processes_parse_the_real_shape() {
        let (fake, h) = herdr();
        fake.on("/opt/herdr", &["pane", "process-info", "--pane", "w6:pB"]).stdout(
            r#"{"id":"cli:pane:process_info","result":{"process_info":{"foreground_process_group_id":4105539,
            "foreground_processes":[{"argv":["/usr/bin/ssh","-p","2222","h"],"name":"ssh","pid":1},
            {"name":"nameless","pid":2},{"argv":["fish"],"pid":3}],"pane_id":"w6:pB","shell_pid":4}}}"#,
        );
        assert_eq!(
            h.pane_processes(&fake, "w6:pB").unwrap(),
            [vec!["/usr/bin/ssh", "-p", "2222", "h"], vec!["fish"]]
        );
        fake.on("/opt/herdr", &[]).stdout(r#"{"result":{}}"#);
        assert!(matches!(
            h.pane_processes(&fake, "x"),
            Err(HerdrError::Unreadable(_))
        ));
        fake.on("/opt/herdr", &[]).stdout("nope");
        assert!(matches!(
            h.pane_processes(&fake, "x"),
            Err(HerdrError::Unreadable(_))
        ));
        fake.on("/opt/herdr", &[])
            .code(1)
            .stderr(r#"{"error":{"message":"pane not found"}}"#);
        assert_eq!(
            h.pane_processes(&fake, "x"),
            Err(HerdrError::Failed("pane not found".into()))
        );
    }

    #[test]
    fn context_json_parsing_tolerates_anything() {
        let cases: [(&str, Option<&str>); 9] = [
            (r#"{"focused_pane_id":"w1:p2"}"#, Some("w1:p2")),
            (r#"{"pane":{"pane_id":"p9"}}"#, Some("p9")),
            (r#"{"pane":{"id":"p8"}}"#, Some("p8")),
            (r#"{"pane_id":" p7 "}"#, Some("p7")),
            (
                r#"{"focused_pane_id":"","pane":{"pane_id":"p6"}}"#,
                Some("p6"),
            ),
            (r#"{"workspace_id":"w1"}"#, None),
            (r#"{"focused_pane_id":5}"#, None),
            ("not json", None),
            ("", None),
        ];
        for (json, want) in cases {
            assert_eq!(pane_from_context(json).as_deref(), want, "{json}");
        }
    }

    #[test]
    fn pane_from_env_prefers_the_pane_variable() {
        let fake = FakeSys::new();
        assert_eq!(pane_from_env(&fake), None);
        fake.set_env("HERDR_PLUGIN_CONTEXT_JSON", r#"{"focused_pane_id":"ctx"}"#);
        assert_eq!(pane_from_env(&fake).as_deref(), Some("ctx"));
        fake.set_env("HERDR_PANE_ID", "w1:p1");
        assert_eq!(pane_from_env(&fake).as_deref(), Some("w1:p1"));
        fake.set_env("HERDR_PANE_ID", "  ");
        assert_eq!(pane_from_env(&fake).as_deref(), Some("ctx"));
    }
}
