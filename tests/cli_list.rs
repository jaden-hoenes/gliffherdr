//! `list` through the built binary (ports the earlier bash prototype's listing, source and dedupe tests).

mod common;

use common::{READY, World};
use serde_json::Value;

fn list_json(w: &World) -> Vec<Value> {
    let r = w.run(&["list", "--json"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    serde_json::from_str::<Vec<Value>>(&r.stdout).unwrap()
}

fn targets(rows: &[Value]) -> Vec<&str> {
    rows.iter().map(|r| r["target"].as_str().unwrap()).collect()
}

fn row<'a>(rows: &'a [Value], target: &str) -> &'a Value {
    rows.iter()
        .find(|r| r["target"] == target)
        .unwrap_or_else(|| panic!("no {target} in {rows:?}"))
}

#[test]
fn list_merges_sources_and_dedupes() {
    let w = World::new();
    w.roster_fixture();
    let rows = list_json(&w);
    let mut t = targets(&rows);
    t.sort_unstable();
    assert_eq!(
        t,
        [
            "box.tail.ts.net",
            "j@10.0.0.2",
            "k@10.0.0.7",
            "laptop.tail.ts.net",
            "mac@10.0.0.3",
            "me@desk.local"
        ]
    );
    let all = w.run(&["list"]).all();
    assert!(!all.contains("github"), "git hosts are not listed");
    assert!(
        !all.contains("devbox"),
        "devbox is the same machine as herdr's j"
    );
    assert!(!all.contains("phone"), "iOS peers are not listed");
    assert!(!all.contains("asleep"), "offline peers are not listed");
    let j = row(&rows, "j@10.0.0.2");
    assert_eq!(j["label"], "j");
}

#[test]
fn every_source_of_a_machine_is_shown() {
    let w = World::new();
    w.roster_fixture();
    let rows = list_json(&w);
    let sources = |t: &str| -> Vec<String> {
        row(&rows, t)["sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        sources("j@10.0.0.2"),
        ["herdr", "gliff", "ssh", "tailscale"]
    );
    assert_eq!(sources("me@desk.local"), ["config"]);
    assert_eq!(sources("k@10.0.0.7"), ["gliff"]);
    assert_eq!(sources("box.tail.ts.net"), ["tailscale"]);
    let table = w.run(&["list"]).stdout;
    let line = table.lines().find(|l| l.contains("j@10.0.0.2")).unwrap();
    assert!(line.ends_with("herdr gliff ssh tailscale"), "{line}");
}

#[test]
fn macos_peers_are_listed_as_unsupported_without_an_ssh_attempt() {
    let w = World::new();
    w.roster_fixture();
    let table = w.run(&["list"]).stdout;
    let line = table.lines().find(|l| l.contains("laptop")).unwrap();
    assert!(line.contains("macOS: needs Hyprland"), "{line}");
    let rows = list_json(&w);
    assert_eq!(row(&rows, "laptop.tail.ts.net")["state"], "unsupported");
    assert_eq!(row(&rows, "laptop.tail.ts.net")["os"], "macOS");
    assert_eq!(w.ssh_checked().len(), 0);
}

#[test]
fn list_without_tailscale_or_ssh_config() {
    let w = World::new();
    w.roster_fixture();
    w.config_file_machines("[sources]\ntailscale = false\nssh_config = false\n");
    let all = w.run(&["list"]).all();
    assert!(!all.contains("tail.ts.net"), "{all}");
    assert!(all.contains("j@10.0.0.2") && all.contains("k@10.0.0.7"));
    assert_eq!(w.read_data("herdr.log"), "machine list --json\n");
}

#[test]
fn list_puts_ready_machines_first() {
    let w = World::new();
    w.roster_fixture();
    w.host_answer("mac@10.0.0.3", 0, "os=Darwin\n", "");
    w.host_answer("k@10.0.0.7", 0, READY, "");
    assert_eq!(w.run(&["check", "mac@10.0.0.3", "k@10.0.0.7"]).code, 0);
    let table = w.run(&["list"]).stdout;
    let first = table.lines().next().unwrap();
    assert!(
        first.contains("k@10.0.0.7") && first.contains('●') && first.contains("ready"),
        "{first}"
    );
    let last = table.lines().last().unwrap();
    assert!(last.contains("–"), "unsupported rows sort last: {last}");
    let rows = list_json(&w);
    assert_eq!(targets(&rows)[0], "k@10.0.0.7");
    assert_eq!(rows[0]["state"], "ready");
    // Ties keep roster order: the not-checked machines follow config, herdr, gliff, tailscale.
    let order: Vec<_> = targets(&rows).iter().map(|t| (*t).to_string()).collect();
    assert_eq!(
        order[1..4],
        ["me@desk.local", "j@10.0.0.2", "box.tail.ts.net"]
    );
}

#[test]
fn a_window_opened_under_another_spelling_counts_as_open() {
    let w = World::new();
    w.config_file_machines("[[machine]]\ntarget = \"j@10.0.0.2\"\nlabel = \"j\"\n");
    w.ssh_g("j", "j", "10.0.0.2");
    w.write_data(
        "clients.json",
        r#"[{"class":"gliff","title":"j — Gliff","address":"0xabc"}]"#,
    );
    let rows = list_json(&w);
    let j = row(&rows, "j@10.0.0.2");
    assert_eq!(j["state"], "open", "{j}");
    assert_eq!(j["window_open"], true);
}

#[test]
fn json_shape_for_a_checked_machine() {
    let w = World::new();
    w.config_file_machines("[[machine]]\ntarget = \"j@10.0.0.2\"\nlabel = \"j\"\n");
    w.host_answer("j@10.0.0.2", 0, READY, "");
    w.run(&["check", "j@10.0.0.2"]);
    let rows = list_json(&w);
    let j = row(&rows, "j@10.0.0.2");
    assert_eq!(j["label"], "j");
    assert_eq!(j["sources"], serde_json::json!(["config"]));
    assert_eq!(j["state"], "ready");
    assert_eq!(j["status"], serde_json::json!({"kind": "ready"}));
    assert_eq!(j["detail"], Value::Null);
    assert_eq!(j["os"], "Linux");
    assert_eq!(j["outputs"], serde_json::json!(["DP-1", "HDMI-A-1"]));
    assert_eq!(j["server_version"], "gliff-server 0.4.3");
    assert!(j["checked_at"].as_u64().unwrap() > 1_700_000_000);
    assert_eq!(j["window_open"], false);
    let keys: Vec<_> = j.as_object().unwrap().keys().cloned().collect();
    let mut want = [
        "target",
        "label",
        "sources",
        "state",
        "status",
        "detail",
        "os",
        "outputs",
        "server_version",
        "checked_at",
        "window_open",
    ]
    .map(String::from)
    .to_vec();
    want.sort();
    let mut keys = keys;
    keys.sort();
    assert_eq!(keys, want);
}

#[test]
fn json_for_an_unchecked_machine_and_a_failure() {
    let w = World::new();
    w.config_file_machines("[[machine]]\ntarget = \"a@h\"\n[[machine]]\ntarget = \"b@h\"\n");
    w.host_answer("b@h", 255, "", "b@h: Permission denied (publickey).\n");
    w.run(&["check", "b@h"]);
    let rows = list_json(&w);
    let a = row(&rows, "a@h");
    assert_eq!(a["state"], "not_checked");
    assert_eq!(a["status"], Value::Null);
    assert_eq!(a["label"], Value::Null);
    assert_eq!(a["outputs"], serde_json::json!([]));
    assert_eq!(a["checked_at"], Value::Null);
    let b = row(&rows, "b@h");
    assert_eq!(b["state"], "needs_setup");
    assert_eq!(b["status"], serde_json::json!({"kind": "auth"}));
    assert_eq!(b["detail"], "ssh key not accepted");
}

#[test]
fn an_unlabelled_machine_does_not_repeat_its_target_in_the_label_column() {
    let w = World::new();
    w.config_file_machines(
        "[[machine]]\ntarget = \"lab@10.0.0.9\"\n\n[[machine]]\ntarget = \"j@10.0.0.2\"\nlabel = \"j\"\n",
    );
    let out = w.run(&["list"]).stdout;
    let line = out.lines().find(|l| l.contains("lab@10.0.0.9")).unwrap();
    assert_eq!(line.matches("lab@10.0.0.9").count(), 1, "{out}");
    assert!(
        line.starts_with("  ○   ") || line.starts_with("  ○    "),
        "blank label column: {line:?}"
    );
    assert!(out.lines().any(|l| l.contains(" j  j@10.0.0.2")), "{out}");
    let json = list_json(&w);
    assert_eq!(row(&json, "lab@10.0.0.9")["label"], Value::Null);
}

#[test]
fn table_has_no_colour_when_not_a_terminal() {
    let w = World::new();
    w.roster_fixture();
    let out = w.run(&["list"]).stdout;
    assert!(!out.contains('\x1b'));
    assert!(out.lines().all(|l| l.starts_with("  ")), "{out}");
}

#[test]
fn list_check_checks_every_checkable_machine_first() {
    let w = World::new();
    w.roster_fixture();
    w.host_answer("j@10.0.0.2", 0, READY, "");
    let rows = {
        let r = w.run(&["list", "--json", "--check"]);
        assert_eq!(r.code, 0, "{}", r.stderr);
        serde_json::from_str::<Vec<Value>>(&r.stdout).unwrap()
    };
    let mut checked = w.ssh_checked();
    checked.sort();
    assert_eq!(
        checked,
        [
            "box.tail.ts.net",
            "j@10.0.0.2",
            "k@10.0.0.7",
            "mac@10.0.0.3",
            "me@desk.local"
        ]
    );
    assert_eq!(row(&rows, "j@10.0.0.2")["state"], "ready");
    assert_eq!(
        row(&rows, "k@10.0.0.7")["state"],
        "unreachable",
        "stub answers 255 with no message"
    );
}

#[test]
fn list_check_stale_skips_fresh_results() {
    let w = World::new();
    w.config_file_machines("[[machine]]\ntarget = \"a@h\"\n[[machine]]\ntarget = \"b@h\"\n");
    w.host_answer("a@h", 0, READY, "");
    w.host_answer("b@h", 0, READY, "");
    w.run(&["check", "a@h"]);
    let before = w.ssh_checked().len();
    assert_eq!(w.run(&["list", "--check-stale"]).code, 0);
    assert_eq!(w.ssh_checked()[before..], ["b@h"]);
}

#[test]
fn source_failures_are_warnings_on_stderr() {
    let w = World::new();
    w.write_stub("herdr", "echo 'no such command' >&2\nexit 3\n");
    w.config_file_machines("[[machine]]\ntarget = \"a@h\"\n");
    let r = w.run(&["list"]);
    assert_eq!(r.code, 0);
    assert!(r.stdout.contains("a@h"));
    assert!(
        r.stderr
            .contains("gliffherdr: herdr: machine list failed: no such command"),
        "{}",
        r.stderr
    );
}

#[test]
fn empty_roster_prints_a_hint_on_stderr_only() {
    let w = World::new();
    let r = w.run(&["list"]);
    assert_eq!((r.code, r.stdout.as_str()), (0, ""));
    assert!(r.stderr.contains("no machines found"));
    let j = w.run(&["list", "--json"]);
    assert_eq!(j.stdout.trim(), "[]");
}

#[test]
fn recent_and_hide() {
    let w = World::new();
    std::fs::write(w.state.join("recent.json"), r#"["r1@h","r2@h"]"#).unwrap();
    w.config_file_machines("[sources]\nhide = [\"R2@H\"]\n");
    let rows = list_json(&w);
    assert_eq!(targets(&rows), ["r1@h"]);
    assert_eq!(rows[0]["sources"], serde_json::json!(["recent"]));
}
