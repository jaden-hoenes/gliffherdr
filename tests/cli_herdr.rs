//! The herdr entry points through the built binary with a stub `herdr`: `open-picker`,
//! `connect --from-pane`, `check --all --notify`, and the startup check.

mod common;

use common::{READY, World};

const T: &str = "j@10.0.0.2";

fn herdr_log(w: &World) -> Vec<String> {
    w.read_data("herdr.log")
        .lines()
        .map(str::to_string)
        .collect()
}

fn process_info(argvs: &[&[&str]]) -> String {
    let procs: Vec<String> = argvs
        .iter()
        .map(|a| {
            let quoted: Vec<String> = a.iter().map(|s| format!("{s:?}")).collect();
            format!(r#"{{"argv":[{}],"name":"x","pid":1}}"#, quoted.join(","))
        })
        .collect();
    format!(
        r#"{{"id":"cli:pane:process_info","result":{{"process_info":{{"foreground_processes":[{}],"pane_id":"w1:p1"}},"type":"pane_process_info"}}}}"#,
        procs.join(",")
    )
}

// ---- open-picker --------------------------------------------------------------------------

#[test]
fn open_picker_asks_herdr() {
    let mut w = World::new();
    w.set_env("HERDR_PLUGIN_ID", "me.test");
    let r = w.run(&["open-picker"]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert_eq!(
        herdr_log(&w),
        ["plugin pane open --plugin me.test --entrypoint picker"]
    );
}

#[test]
fn open_picker_defaults_to_the_plugin_id() {
    let w = World::new();
    assert_eq!(w.run(&["open-picker"]).code, 0);
    assert_eq!(
        herdr_log(&w),
        ["plugin pane open --plugin jaden-hoenes.gliffherdr --entrypoint picker"]
    );
}

#[test]
fn open_picker_failure_is_shown_as_a_notification() {
    let w = World::new();
    w.write_stub(
        "herdr",
        r#"echo "$*" >>"$STUB/herdr.log"
if [ "$1" = plugin ]; then
  echo '{"id":"cli","error":{"code":"ui_busy","message":"a popup pane is already open"}}' >&2
  exit 1
fi
exit 0
"#,
    );
    let r = w.run(&["open-picker"]);
    assert_eq!(r.code, 1);
    assert_eq!(r.stderr, "gliffherdr: a popup pane is already open\n");
    let log = herdr_log(&w);
    assert_eq!(log.len(), 2, "{log:?}");
    assert_eq!(
        log[1],
        "notification show Remote desktop --body a popup pane is already open"
    );
}

#[test]
fn open_picker_without_herdr_fails_with_a_clear_message() {
    let mut w = World::new();
    w.set_env("HERDR_BIN_PATH", "/nonexistent/herdr");
    let r = w.run(&["open-picker"]);
    assert_eq!(r.code, 1);
    assert_eq!(r.stderr, "gliffherdr: herdr is not installed\n");
}

// ---- connect --from-pane ------------------------------------------------------------------

#[test]
fn from_pane_connects_to_the_ssh_destination_of_the_pane() {
    let mut w = World::new();
    w.host_answer(T, 0, READY, "");
    w.set_env("HERDR_PANE_ID", "w1:p1");
    w.write_data(
        "process-info.json",
        &process_info(&[&["fish"], &["ssh", T]]),
    );
    let r = w.run(&["connect", "--from-pane"]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert_eq!(r.stdout, format!("Opened {T}.\n"));
    assert_eq!(w.wait_for_data("gliff.argv").lines().last(), Some(T));
    let log = herdr_log(&w);
    assert_eq!(log[0], "pane process-info --pane w1:p1");
    assert!(
        !log.iter().any(|l| l.starts_with("notification")),
        "no notification on success: {log:?}"
    );
}

#[test]
fn from_pane_folds_user_and_port_options_into_the_target() {
    let mut w = World::new();
    w.set_env("HERDR_PANE_ID", "p");
    w.write_data(
        "process-info.json",
        &process_info(&[&["/usr/bin/ssh", "-p", "2222", "-l", "u", "box.lan"]]),
    );
    w.host_answer("ssh://u@box.lan:2222", 0, READY, "");
    let r = w.run(&["connect", "--from-pane"]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert_eq!(
        w.wait_for_data("gliff.argv").lines().last(),
        Some("ssh://u@box.lan:2222")
    );
}

#[test]
fn from_pane_reads_the_pane_from_the_plugin_context_json() {
    let mut w = World::new();
    w.host_answer(T, 0, READY, "");
    w.set_env(
        "HERDR_PLUGIN_CONTEXT_JSON",
        r#"{"workspace_id":"w1","focused_pane_id":"w1:p9"}"#,
    );
    w.write_data("process-info.json", &process_info(&[&["ssh", T]]));
    assert_eq!(w.run(&["connect", "--from-pane"]).code, 0);
    assert_eq!(herdr_log(&w)[0], "pane process-info --pane w1:p9");
}

#[test]
fn from_pane_accepts_an_explicit_pane_id() {
    let mut w = World::new();
    w.host_answer(T, 0, READY, "");
    w.set_env("HERDR_PANE_ID", "from-env");
    w.write_data("process-info.json", &process_info(&[&["ssh", T]]));
    assert_eq!(w.run(&["connect", "--from-pane", "explicit"]).code, 0);
    assert_eq!(herdr_log(&w)[0], "pane process-info --pane explicit");
}

#[test]
fn from_pane_failure_is_a_notification_with_message_and_hint() {
    let mut w = World::new();
    w.host_answer(T, 0, "os=Linux\nhyprland=1\ngliff_server=\n", "");
    w.set_env("HERDR_PANE_ID", "p");
    w.write_data("process-info.json", &process_info(&[&["ssh", T]]));
    let r = w.run(&["connect", "--from-pane"]);
    assert_eq!(r.code, 1);
    let log = herdr_log(&w);
    let note = log
        .iter()
        .find(|l| l.starts_with("notification show"))
        .unwrap();
    assert!(
        note.starts_with(&format!(
            "notification show Remote desktop --body {T}: gliff-server is not installed there. Run `gliffherdr fix {T}` to install it"
        )),
        "{note}"
    );
}

#[test]
fn from_pane_without_an_ssh_process_opens_the_picker() {
    let mut w = World::new();
    w.set_env("HERDR_PANE_ID", "p");
    w.write_data(
        "process-info.json",
        &process_info(&[&["fish"], &["ssh", "-V"]]),
    );
    let r = w.run(&["connect", "--from-pane"]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert_eq!(
        herdr_log(&w),
        [
            "pane process-info --pane p".to_string(),
            "plugin pane open --plugin jaden-hoenes.gliffherdr --entrypoint picker".to_string(),
        ]
    );
    assert!(w.ssh_calls().is_empty() && !w.data.join("gliff.argv").exists());
}

#[test]
fn from_pane_without_any_pane_opens_the_picker() {
    let w = World::new();
    assert_eq!(w.run(&["connect", "--from-pane"]).code, 0);
    assert_eq!(
        herdr_log(&w),
        ["plugin pane open --plugin jaden-hoenes.gliffherdr --entrypoint picker"]
    );
}

#[test]
fn from_pane_when_herdr_cannot_describe_the_pane_notifies() {
    let mut w = World::new();
    w.set_env("HERDR_PANE_ID", "gone");
    w.write_stub(
        "herdr",
        r#"echo "$*" >>"$STUB/herdr.log"
if [ "$1" = pane ]; then echo '{"error":{"code":"pane_not_found","message":"pane not found"}}' >&2; exit 1; fi
exit 0
"#,
    );
    let r = w.run(&["connect", "--from-pane"]);
    assert_eq!(r.code, 1);
    assert_eq!(r.stderr, "gliffherdr: pane not found\n");
    assert!(
        herdr_log(&w)
            .iter()
            .any(|l| l == "notification show Remote desktop --body pane not found")
    );
}

// ---- check --notify / startup ---------------------------------------------------------------

#[test]
fn recheck_notifies_the_summary() {
    let w = World::new();
    w.write_config(
        "[sources]\nherdr = false\nrecent = false\ngliff = false\nssh_config = false\ntailscale = false\n\
         [[machine]]\ntarget = \"j@10.0.0.2\"\n[[machine]]\ntarget = \"k@10.0.0.7\"\n[[machine]]\ntarget = \"l@10.0.0.8\"\n[[machine]]\ntarget = \"m@10.0.0.9\"\n",
    );
    w.host_answer("j@10.0.0.2", 0, READY, "");
    w.host_answer("k@10.0.0.7", 0, "os=Linux\nhyprland=1\ngliff_server=\n", "");
    w.host_answer(
        "l@10.0.0.8",
        255,
        "",
        "ssh: connect to host l port 22: No route to host",
    );
    w.host_answer("m@10.0.0.9", 0, READY, "");
    let r = w.run(&["check", "--all", "--notify"]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(
        r.stdout.contains("j@10.0.0.2"),
        "still prints the table: {}",
        r.stdout
    );
    assert_eq!(
        herdr_log(&w),
        ["notification show Remote desktop --body 2 ready · 1 need setup · 1 unreachable"]
    );
}

#[test]
fn recheck_with_no_machines_says_nothing_to_check() {
    let w = World::new();
    w.write_config("[sources]\nherdr = false\nrecent = false\ngliff = false\nssh_config = false\ntailscale = false\n");
    let r = w.run(&["check", "--all", "--notify"]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert_eq!(
        herdr_log(&w),
        ["notification show Remote desktop --body nothing to check"]
    );
}

#[test]
fn recheck_survives_a_failing_notification_with_exit_1() {
    let w = World::new();
    w.host_answer(T, 0, READY, "");
    w.write_stub("herdr", "echo 'no server' >&2\nexit 1\n");
    let r = w.run(&["check", T, "--notify"]);
    assert_eq!(r.code, 1);
    assert!(
        r.stderr
            .contains("could not show the notification: no server"),
        "{}",
        r.stderr
    );
}

#[test]
fn check_without_notify_never_notifies() {
    let w = World::new();
    w.host_answer(T, 0, READY, "");
    assert_eq!(w.run(&["check", T]).code, 0);
    assert!(
        !herdr_log(&w).iter().any(|l| l.starts_with("notification")),
        "{:?}",
        herdr_log(&w)
    );
}

#[test]
fn the_startup_hook_prints_nothing_and_notifies_nothing() {
    let w = World::new();
    w.write_config("[sources]\nherdr = false\nrecent = false\ngliff = false\nssh_config = false\ntailscale = false\n[[machine]]\ntarget = \"j@10.0.0.2\"\n");
    w.host_answer(
        T,
        255,
        "",
        "ssh: connect to host j port 22: Connection refused",
    );
    let r = w.run(&["check", "--startup", "--quiet"]);
    assert_eq!(
        r.code,
        0,
        "unreachable machines do not fail the hook: {}",
        r.all()
    );
    assert_eq!((r.stdout.as_str(), r.stderr.as_str()), ("", ""));
    assert_eq!(w.ssh_checked(), [T]);
    assert!(!herdr_log(&w).iter().any(|l| l.starts_with("notification")));
}
