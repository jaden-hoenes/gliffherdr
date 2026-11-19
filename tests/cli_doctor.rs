//! `doctor` through the built binary.

mod common;

use common::World;

fn json(w: &World) -> (i32, Vec<serde_json::Value>) {
    let r = w.run(&["doctor", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&r.stdout).unwrap_or_else(|e| {
        panic!("not JSON: {e}\n{}", r.all());
    });
    (r.code, v.as_array().unwrap().clone())
}

fn find<'a>(v: &'a [serde_json::Value], check: &str) -> &'a serde_json::Value {
    v.iter()
        .find(|f| f["check"] == check)
        .unwrap_or_else(|| panic!("no {check} in {v:?}"))
}

#[test]
fn doctor_is_all_clear_in_a_working_world() {
    let w = World::new();
    w.roster_fixture();
    let r = w.run(&["doctor"]);
    assert_eq!(r.code, 0, "{}", r.all());
    let lines: Vec<&str> = r.stdout.lines().collect();
    assert_eq!(lines.len(), 9, "{}", r.stdout);
    assert!(lines[0].starts_with("ok    gliff "), "{}", lines[0]);
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Hyprland 0.0.0-stub reachable")),
        "{}",
        r.stdout
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("info  launch") && l.ends_with("setsid")),
        "{}",
        r.stdout
    );
    assert!(
        lines.iter().any(|l| l.contains("herdr 0.0.0-stub")),
        "{}",
        r.stdout
    );
    assert!(
        lines.iter().any(|l| l.contains("config.toml")),
        "{}",
        r.stdout
    );
    assert!(
        !r.stdout.contains('\x1b'),
        "no colour when stdout is not a terminal"
    );
}

#[test]
fn doctor_json_shape_and_roster_counts() {
    let w = World::new();
    w.roster_fixture();
    let (code, v) = json(&w);
    assert_eq!(code, 0);
    let names: Vec<_> = v.iter().map(|f| f["check"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "gliff",
            "ssh",
            "hyprland",
            "launch",
            "herdr",
            "config",
            "state",
            "roster",
            "inside herdr"
        ]
    );
    for f in &v {
        assert!(["ok", "warn", "fail", "info"].contains(&f["level"].as_str().unwrap()));
        assert!(f["message"].is_string());
    }
    let roster = find(&v, "roster")["message"].as_str().unwrap();
    assert!(
        roster.contains("machines (") && roster.contains("herdr") && roster.contains("config"),
        "{roster}"
    );
    assert_eq!(find(&v, "inside herdr")["message"], "no");
}

#[test]
fn doctor_reports_uwsm_and_herdr_context() {
    let mut w = World::new();
    w.write_data("uwsm-active", "");
    w.set_env("HERDR_ENV", "1");
    let (_, v) = json(&w);
    assert_eq!(find(&v, "launch")["message"], "uwsm-app");
    assert_eq!(find(&v, "inside herdr")["message"], "yes");
}

#[test]
fn doctor_fails_without_gliff_and_exits_1() {
    let w = World::new();
    std::fs::remove_file(w.stubs.join("gliff")).unwrap();
    let path = w.minimal_path(&["sh", "cat", "tr", "mkdir", "cp", "env", "uname"]);
    let r = common::ran(w.command().env("PATH", &path).args(["doctor", "--json"]));
    assert_eq!(r.code, 1, "{}", r.all());
    let v: Vec<serde_json::Value> = serde_json::from_str(&r.stdout).unwrap();
    assert_eq!(find(&v, "gliff")["level"], "fail");
    let text = common::ran(w.command().env("PATH", &path).arg("doctor"));
    assert_eq!(text.code, 1);
    assert!(
        text.stdout
            .lines()
            .next()
            .unwrap()
            .starts_with("FAIL  gliff"),
        "{}",
        text.stdout
    );
    assert_eq!(text.stderr, "", "no error line after the report");
}

#[test]
fn doctor_fails_without_a_hyprland_session() {
    let mut w = World::new();
    w.unset_env("WAYLAND_DISPLAY");
    w.unset_env("HYPRLAND_INSTANCE_SIGNATURE");
    let (code, v) = json(&w);
    assert_eq!(code, 1);
    assert_eq!(find(&v, "hyprland")["level"], "fail");
    assert_eq!(find(&v, "gliff")["level"], "ok");
}

#[test]
fn doctor_fails_on_an_invalid_config_but_still_lists_the_rest() {
    let w = World::new();
    w.write_config("[connect]\ncolour = \"red\"\n");
    let (code, v) = json(&w);
    assert_eq!(code, 1);
    let c = find(&v, "config");
    assert_eq!(c["level"], "fail");
    assert!(
        c["message"]
            .as_str()
            .unwrap()
            .contains("config.toml line 2"),
        "{c}"
    );
    assert_eq!(find(&v, "roster")["level"], "info");
}

#[test]
fn doctor_warns_about_a_broken_source_and_a_missing_herdr() {
    let w = World::new();
    w.write_stub("herdr", "echo oops >&2\nexit 3\n");
    let (code, v) = json(&w);
    assert_eq!(code, 0, "warnings do not fail doctor");
    assert_eq!(find(&v, "herdr")["level"], "warn");
    assert!(
        v.iter().any(|f| f["check"] == "roster"
            && f["level"] == "warn"
            && f["message"].as_str().unwrap().starts_with("herdr: ")),
        "{v:?}"
    );
}

#[test]
fn doctor_fails_when_the_state_dir_is_not_writable() {
    let mut w = World::new();
    let file = w.dir.path().join("a-file");
    std::fs::write(&file, "x").unwrap();
    w.set_env("GLIFFHERDR_STATE_DIR", file.join("state").to_str().unwrap());
    let (code, v) = json(&w);
    assert_eq!(code, 1);
    assert_eq!(find(&v, "state")["level"], "fail");
}
