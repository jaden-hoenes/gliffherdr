//! Tests of the test harness itself: isolation and the stub tools.

mod common;

use common::{World, isolated_env};
use std::ffi::OsString;

fn env_of(world: &World, program: &str, args: &[&str]) -> String {
    common::ran(world.command_for(program).args(args)).stdout
}

#[test]
fn command_strips_a_planted_herdr_socket_path() {
    let mut w = World::new();
    w.simulate_parent_env("HERDR_SOCKET_PATH", "/live/herdr.sock");
    w.simulate_parent_env("HERDR_ENV", "1");
    w.simulate_parent_env("HERDR_PANE_ID", "p_1");
    w.simulate_parent_env("HERDR_PLUGIN_STATE_DIR", "/live/state");
    w.simulate_parent_env("GLIFFHERDR_DEMO", "1");
    w.simulate_parent_env("GLIFFHERDR_E2E_SSH", "1");
    w.simulate_parent_env("WAYLAND_DISPLAY_BACKUP", "kept");
    w.simulate_parent_env("XDG_DATA_HOME", "/live/data");
    w.simulate_parent_env("SSH_AUTH_SOCK", "/live/agent");
    w.simulate_parent_env("DISPLAY", ":0");
    let out = env_of(&w, "env", &[]);
    for gone in [
        "HERDR_SOCKET_PATH",
        "HERDR_ENV=",
        "HERDR_PANE_ID",
        "HERDR_PLUGIN_STATE_DIR",
        "GLIFFHERDR_DEMO",
        "GLIFFHERDR_E2E_SSH",
        "XDG_DATA_HOME",
        "SSH_AUTH_SOCK",
        "DISPLAY=:0",
        "/live/",
    ] {
        assert!(!out.contains(gone), "{gone} leaked into the child:\n{out}");
    }
    assert!(
        out.contains("WAYLAND_DISPLAY_BACKUP=kept"),
        "unrelated variables are inherited"
    );
    // The binary itself sees the same environment.
    let w2 = w.run(&["config", "path"]);
    assert_eq!(
        w2.stdout.trim(),
        w.config.join("config.toml").to_str().unwrap()
    );
}

#[test]
fn the_world_sets_its_own_environment() {
    let w = World::new();
    let out = env_of(&w, "env", &[]);
    let line = |k: &str| {
        out.lines()
            .find_map(|l| l.strip_prefix(&format!("{k}=")))
            .map(str::to_string)
    };
    assert_eq!(line("HOME").as_deref(), w.home.to_str());
    assert_eq!(line("GLIFFHERDR_CONFIG_DIR").as_deref(), w.config.to_str());
    assert_eq!(line("GLIFFHERDR_STATE_DIR").as_deref(), w.state.to_str());
    assert_eq!(
        line("XDG_STATE_HOME").unwrap(),
        w.home.join(".local/state").to_str().unwrap()
    );
    assert_eq!(
        line("HERDR_BIN_PATH").unwrap(),
        w.stubs.join("herdr").to_str().unwrap()
    );
    assert_eq!(line("WAYLAND_DISPLAY").as_deref(), Some("wayland-1"));
    assert_eq!(
        line("HYPRLAND_INSTANCE_SIGNATURE").as_deref(),
        Some("testsig")
    );
    assert_eq!(line("GLIFFHERDR_WINDOW_WAIT_MS").as_deref(), Some("300"));
    assert!(line("PATH").unwrap().starts_with(w.stubs.to_str().unwrap()));
}

#[test]
fn set_and_unset_env() {
    let mut w = World::new();
    w.unset_env("WAYLAND_DISPLAY");
    w.set_env("GLIFFHERDR_FOO", "bar");
    let out = env_of(&w, "env", &[]);
    assert!(!out.contains("WAYLAND_DISPLAY="));
    assert!(out.contains("GLIFFHERDR_FOO=bar"));
}

#[test]
fn isolated_env_is_a_pure_function() {
    let parent = vec![
        (OsString::from("HERDR_X"), OsString::from("1")),
        (OsString::from("HOME"), OsString::from("/real")),
        (OsString::from("LANG"), OsString::from("C")),
    ];
    let env = isolated_env(parent, &[("HOME".into(), "/fake".into())]);
    let keys: Vec<_> = env
        .iter()
        .map(|(k, v)| format!("{}={}", k.to_string_lossy(), v.to_string_lossy()))
        .collect();
    assert_eq!(keys, ["LANG=C", "HOME=/fake"]);
}

#[test]
fn ssh_stub_answers_g_and_checks() {
    let w = World::new();
    w.host_answer("j@h", 0, "os=Linux\n", "");
    let g = common::ran(w.command_for("ssh").args(["-G", "--", "u@box"]));
    assert_eq!(g.stdout, "user u\nhostname box\nport 22\n");
    w.ssh_g("devbox", "j", "10.0.0.2");
    let g = common::ran(w.command_for("ssh").args(["-G", "--", "devbox"]));
    assert_eq!(g.stdout, "user j\nhostname 10.0.0.2\nport 22\n");
    let c = common::ran(
        w.command_for("ssh")
            .args(["-T", "-o", "BatchMode=yes", "--", "j@h", "sh", "-s"])
            .write_stdin("script"),
    );
    assert_eq!((c.code, c.stdout.as_str()), (0, "os=Linux\n"));
    assert_eq!(w.ssh_checked(), ["j@h"]);
    assert_eq!(w.ssh_stdin("j@h"), "script");
    assert_eq!(w.ssh_calls().len(), 3);
    let unscripted = common::ran(w.command_for("ssh").args(["--", "nobody", "sh", "-s"]));
    assert_eq!(unscripted.code, 255);
}

#[test]
fn launch_chain_stubs_work_end_to_end() {
    // What a launch does: uwsm-app -> env -> gliff, then the window shows in hyprctl.
    let w = World::new();
    let r = common::ran(w.command_for("uwsm-app").args([
        "-a",
        "gliff",
        "-d",
        "Gliff: j@h",
        "--",
        "env",
        "XDG_CONFIG_HOME=/x/cfg",
        "gliff",
        "--headless",
        "--",
        "j@h",
    ]));
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(w.wait_for_data("gliff.argv"), "--headless\n--\nj@h\n");
    assert_eq!(
        w.read_data("gliff.env"),
        "XDG_CONFIG_HOME=/x/cfg\nWAYLAND_DISPLAY=wayland-1\n"
    );
    let clients = common::ran(w.command_for("hyprctl").args(["clients", "-j"]));
    assert!(clients.stdout.contains("j@h — Gliff"), "{}", clients.stdout);
    assert!(
        w.read_data("hyprctl.log")
            .contains("HIS=testsig|clients -j")
    );
    assert!(w.read_data("uwsm-app.argv").starts_with("-a\ngliff\n"));
}

#[test]
fn other_stubs_behave() {
    let w = World::new();
    assert_eq!(
        common::ran(w.command_for("uwsm").args(["check", "is-active"])).code,
        1
    );
    w.write_data("uwsm-active", "");
    assert_eq!(
        common::ran(w.command_for("uwsm").args(["check", "is-active"])).code,
        0
    );
    assert_eq!(
        env_of(&w, "tailscale", &["status", "--json"]).trim(),
        r#"{"Peer":{}}"#
    );
    assert_eq!(
        env_of(&w, "herdr", &["machine", "list", "--json"]).trim(),
        "[]"
    );
    assert_eq!(w.read_data("herdr.log"), "machine list --json\n");
    w.write_data("systemd-env.txt", "A=b\n");
    assert_eq!(
        env_of(&w, "systemctl", &["--user", "show-environment"]),
        "A=b\n"
    );
    assert_eq!(
        env_of(&w, "hyprctl", &["-i", "0", "dispatch", "x"]).trim(),
        "ok"
    );
    assert!(env_of(&w, "hyprctl", &["monitors"]).contains("Monitor DP-1 (ID 0):"));
    assert_eq!(env_of(&w, "gliff", &["--version"]).trim(), "gliff 0.4.3");
}
