//! `connect` through the built binary with the stub chain (`uwsm-app` -> `env` -> stub `gliff`):
//! real detached launches, focus, environment recovery and failures. Ports the earlier bash prototype's tests.

mod common;

use common::{READY, World};

const T: &str = "j@10.0.0.2";
const WINDOW: &str = r#"[{"class":"gliff","title":"j@10.0.0.2 — Gliff","address":"0xabc"}]"#;

fn ready_world() -> World {
    let w = World::new();
    w.host_answer(T, 0, READY, "");
    w
}

/// The lines of a detached launch's argv file, waiting for the launch to happen.
fn gliff_argv(w: &World) -> Vec<String> {
    w.wait_for_data("gliff.argv")
        .lines()
        .map(str::to_string)
        .collect()
}

fn env_line(w: &World, key: &str) -> String {
    w.read_data("gliff.env")
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{key}=")).map(str::to_string))
        .unwrap_or_default()
}

fn no_launch(w: &World) {
    assert!(
        !w.data.join("gliff.argv").exists() && !w.data.join("gliff.env").exists(),
        "gliff was launched"
    );
}

#[test]
fn connect_rejects_option_injection() {
    let w = World::new();
    for bad in ["-oProxyCommand=touch /tmp/x", "a;b", "a b", "$(x)"] {
        let r = w.run(&["connect", "--", bad]);
        assert_eq!(r.code, 2, "{bad}: {}", r.all());
        assert!(r.stderr.contains("not an ssh destination"), "{}", r.stderr);
    }
    // Without `--` clap itself refuses an option-looking target.
    assert_eq!(w.run(&["connect", "-oProxyCommand=touch /tmp/x"]).code, 2);
    no_launch(&w);
    assert!(
        !w.data.join("ssh.argv").exists(),
        "ssh ran: {}",
        w.read_data("ssh.argv")
    );
}

#[test]
fn connect_launches_isolated_window() {
    let w = ready_world();
    let fontconfig = w.home.join(".config/fontconfig");
    std::fs::create_dir_all(&fontconfig).unwrap();
    let r = w.run(&["connect", T]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert_eq!(r.stdout, format!("Opened {T}.\n"));
    let argv = gliff_argv(&w);
    assert_eq!(argv, ["--", T], "no flags for a mirror launch");

    let cfg = env_line(&w, "XDG_CONFIG_HOME");
    let prefix = format!("{}/windows/{T}-", w.state.display());
    assert!(cfg.starts_with(&prefix), "{cfg}");
    assert!(cfg.ends_with("/config"), "{cfg}");
    let linked = std::fs::read_link(format!("{cfg}/fontconfig")).unwrap();
    assert_eq!(
        linked, fontconfig,
        "fontconfig is linked from the real config home"
    );
    assert!(!std::path::Path::new(&format!("{cfg}/gtk-4.0")).exists());

    assert_eq!(
        std::fs::read_to_string(w.state.join("recent.json")).unwrap(),
        format!("[\"{T}\"]\n")
    );
    let log = std::fs::read_to_string(
        std::fs::read_dir(w.state.join("windows"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path()
            .join("gliff.log"),
    )
    .unwrap();
    let first = log.lines().next().unwrap();
    assert!(
        first.contains('T') && first.ends_with(&format!("gliff -- {T}")),
        "{first}"
    );
    assert!(first.contains(" env XDG_CONFIG_HOME="), "{first}");
    assert!(
        !w.data.join("uwsm-app.argv").exists(),
        "no uwsm unless active"
    );
}

#[test]
fn connect_headless_and_output() {
    let w = ready_world();
    assert_eq!(w.run(&["connect", "--headless", T]).code, 0);
    assert_eq!(gliff_argv(&w), ["--headless", "--", T]);

    // Forget the first window and launch again with a named output.
    std::fs::remove_file(w.data.join("gliff.argv")).unwrap();
    w.write_data("clients.json", "[]");
    let r = w.run(&["connect", "--output", "HDMI-A-1", T]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert_eq!(gliff_argv(&w), ["--output", "HDMI-A-1", "--", T]);
    // The second connect found a fresh Ready result: one check in total.
    assert_eq!(w.ssh_checked(), [T]);
}

#[test]
fn connect_mirror_flag_and_flag_conflicts() {
    let w = ready_world();
    w.write_config("[connect]\nmode = \"headless\"\n");
    assert_eq!(w.run(&["connect", "--mirror", T]).code, 0);
    assert_eq!(
        gliff_argv(&w),
        ["--", T],
        "--mirror overrides the headless default"
    );

    assert_eq!(w.run(&["connect", "--mirror", "--headless", T]).code, 2);
    let r = w.run(&["connect", "--output", "a b", T]);
    assert_eq!(r.code, 2, "{}", r.all());
}

#[test]
fn connect_choose_needs_a_terminal() {
    let w = ready_world();
    let r = w.run(&["connect", "--choose", T]);
    assert_eq!(r.code, 2);
    assert_eq!(r.stderr, "gliffherdr: --choose needs a terminal\n");
    no_launch(&w);
}

#[test]
fn connect_focuses_open_window() {
    let w = ready_world();
    w.write_data("clients.json", WINDOW);
    let r = w.run(&["connect", T]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert_eq!(r.stdout, format!("Focused the open window for {T}.\n"));
    no_launch(&w);
    let log = w.read_data("hyprctl.log");
    assert!(
        log.contains(r#"HIS=testsig|dispatch hl.dsp.focus({ window = "address:0xabc" })"#),
        "{log}"
    );
    assert!(
        w.ssh_calls().is_empty(),
        "checked although the window was open: {:?}",
        w.ssh_calls()
    );
    assert_eq!(
        std::fs::read_to_string(w.state.join("recent.json")).unwrap(),
        format!("[\"{T}\"]\n")
    );
}

#[test]
fn connect_finds_a_window_opened_under_another_spelling() {
    let w = ready_world();
    // The window was opened for the ssh alias `j`, which ssh resolves to j@10.0.0.2:22.
    w.ssh_g("j", "j", "10.0.0.2");
    w.write_data(
        "clients.json",
        r#"[{"class":"gliff","title":"j — Gliff","address":"0xabc"}]"#,
    );
    let r = w.run(&["connect", T]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert_eq!(r.stdout, format!("Focused the open window for {T}.\n"));
    no_launch(&w);
    assert!(
        w.read_data("hyprctl.log")
            .contains(r#"dispatch hl.dsp.focus({ window = "address:0xabc" })"#)
    );
}

#[test]
fn a_window_of_another_machine_is_not_taken_for_this_one() {
    let w = ready_world();
    w.ssh_g("other", "j", "10.0.0.99");
    w.write_data(
        "clients.json",
        r#"[{"class":"gliff","title":"other — Gliff","address":"0xabc"}]"#,
    );
    let r = w.run(&["connect", T]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(
        !r.stdout.contains("Focused"),
        "focused the wrong window: {}",
        r.all()
    );
    assert_eq!(gliff_argv(&w).last().map(String::as_str), Some(T));
}

#[test]
fn connect_falls_back_to_the_legacy_dispatcher_and_reports_a_refusal() {
    let w = ready_world();
    w.write_data("clients.json", WINDOW);
    w.write_data("dispatch.reply", "Error: nope\n");
    let r = w.run(&["connect", T]);
    assert_eq!(r.code, 1);
    assert!(
        r.stderr
            .contains("Could not focus the open gliff window for j@10.0.0.2."),
        "{}",
        r.stderr
    );
    let log = w.read_data("hyprctl.log");
    assert!(
        log.contains("dispatch focuswindow address:0xabc"),
        "both forms tried: {log}"
    );
}

#[test]
fn connect_refuses_machine_without_hyprland() {
    let w = World::new();
    w.host_answer(
        "me@desk.local",
        0,
        "os=Linux\nhyprland=0\ngliff_server=/usr/bin/gliff-server\n",
        "",
    );
    let r = w.run(&["connect", "me@desk.local"]);
    assert_eq!(r.code, 1);
    assert!(
        r.stderr.contains("Hyprland is not running there"),
        "{}",
        r.stderr
    );
    no_launch(&w);
}

#[test]
fn connect_failure_hints_name_the_fix_command_not_the_picker_key() {
    let w = World::new();
    w.host_answer("k@10.0.0.7", 0, "os=Linux\nhyprland=1\ngliff_server=\n", "");
    w.host_answer("box", 255, "", "box: Permission denied (publickey).");
    let r = w.run(&["connect", "k@10.0.0.7"]);
    assert_eq!(r.code, 1);
    assert!(
        r.stderr.contains("gliff-server is not installed there."),
        "{}",
        r.stderr
    );
    assert!(
        r.stderr
            .contains("Run `gliffherdr fix k@10.0.0.7` to install it"),
        "{}",
        r.stderr
    );
    assert!(!r.stderr.contains("ctrl-f"));
    let r = w.run(&["connect", "box"]);
    assert!(
        r.stderr.contains("box refused the ssh key."),
        "{}",
        r.stderr
    );
    assert!(r.stderr.contains("gliffherdr fix box"), "{}", r.stderr);
}

#[test]
fn connect_rechecks_a_cached_failure() {
    let w = World::new();
    w.host_answer("k@10.0.0.7", 0, "os=Linux\nhyprland=1\ngliff_server=\n", "");
    assert_eq!(w.run(&["check", "k@10.0.0.7"]).code, 0);
    w.host_answer("k@10.0.0.7", 0, READY, ""); // gliff-server installed since
    let r = w.run(&["connect", "k@10.0.0.7"]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert_eq!(gliff_argv(&w), ["--", "k@10.0.0.7"]);
    assert_eq!(w.ssh_checked().len(), 2);
}

#[test]
fn connect_through_uwsm_when_active() {
    let w = ready_world();
    w.write_data("uwsm-active", "");
    let r = w.run(&["connect", T]);
    assert_eq!(r.code, 0, "{}", r.all());
    let argv = w.wait_for_data("uwsm-app.argv");
    let words: Vec<&str> = argv.lines().collect();
    assert_eq!(
        words[..6],
        ["-a", "gliff", "-d", "Gliff: j@10.0.0.2", "--", "env"]
    );
    assert!(words[6].starts_with("XDG_CONFIG_HOME="));
    assert_eq!(words[7..], ["gliff", "--", T]);
    assert_eq!(gliff_argv(&w), ["--", T], "the stub chain ran gliff");
}

#[test]
fn connect_passes_gliff_args_and_server_bin() {
    let w = ready_world();
    w.write_config(
        "[connect]\ngliff_args = [\"--no-clipboard\", \"--video\", \"cpu\"]\nserver_bin = \"gliff-server --low-bandwidth\"\n",
    );
    let r = w.run(&["connect", T]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert_eq!(
        gliff_argv(&w),
        [
            "--no-clipboard",
            "--video",
            "cpu",
            "--server-bin",
            "gliff-server --low-bandwidth",
            "--",
            T
        ]
    );
    assert_eq!(
        w.ssh_stdin(T).lines().next().unwrap(),
        "server_bin='gliff-server --low-bandwidth'"
    );
}

#[test]
fn connect_uses_the_machines_own_settings() {
    let w = ready_world();
    w.write_config(
        "[connect]\nmode = \"headless\"\n[[machine]]\ntarget = \"j@10.0.0.2\"\noutput = \"DP-1\"\ngliff_args = [\"--no-clipboard\"]\n",
    );
    assert_eq!(w.run(&["connect", T]).code, 0);
    assert_eq!(
        gliff_argv(&w),
        ["--no-clipboard", "--output", "DP-1", "--", T]
    );
}

#[test]
fn connect_recovers_wayland_env_from_systemd() {
    let mut w = ready_world();
    w.unset_env("WAYLAND_DISPLAY");
    w.unset_env("HYPRLAND_INSTANCE_SIGNATURE");
    w.write_data(
        "systemd-env.txt",
        "HOME=/x\nWAYLAND_DISPLAY=wayland-9\nHYPRLAND_INSTANCE_SIGNATURE=fromsystemd\nHYPRLAND_CMD=$'Hyprland --x'\n",
    );
    let r = w.run(&["connect", T]);
    assert_eq!(r.code, 0, "{}", r.all());
    gliff_argv(&w);
    assert_eq!(env_line(&w, "WAYLAND_DISPLAY"), "wayland-9");
    let log = w.read_data("hyprctl.log");
    assert!(log.contains("HIS=fromsystemd|clients -j"), "{log}");
}

#[test]
fn connect_recovers_env_from_hyprctl_instances() {
    let mut w = ready_world();
    w.unset_env("WAYLAND_DISPLAY");
    w.unset_env("HYPRLAND_INSTANCE_SIGNATURE");
    w.write_data(
        "instances.txt",
        "instance abc_123:\n\ttime: 1\n\tpid: 2\n\twl socket: wayland-7\n\n",
    );
    let r = w.run(&["connect", T]);
    assert_eq!(r.code, 0, "{}", r.all());
    gliff_argv(&w);
    assert_eq!(env_line(&w, "WAYLAND_DISPLAY"), "wayland-7");
    let log = w.read_data("hyprctl.log");
    assert!(log.contains("HIS=abc_123|clients -j"), "{log}");
}

#[test]
fn connect_without_a_signature_asks_instance_zero() {
    let mut w = ready_world();
    w.unset_env("HYPRLAND_INSTANCE_SIGNATURE");
    assert_eq!(w.run(&["connect", T]).code, 0);
    gliff_argv(&w);
    let log = w.read_data("hyprctl.log");
    assert!(log.contains("HIS=|-i 0 clients -j"), "{log}");
}

#[test]
fn connect_without_a_desktop_session_fails_before_any_check() {
    let mut w = ready_world();
    w.unset_env("WAYLAND_DISPLAY");
    w.unset_env("HYPRLAND_INSTANCE_SIGNATURE");
    let r = w.run(&["connect", T]);
    assert_eq!(r.code, 1);
    assert_eq!(
        r.stderr,
        "gliffherdr: No Hyprland session found to open a window in.\n  herdr runs plugin commands on its own machine; gliff needs a desktop there.\n"
    );
    assert_eq!(w.ssh_checked().len(), 0);
    no_launch(&w);
}

#[test]
fn connect_without_gliff_installed_says_how_to_install_it() {
    let w = ready_world();
    std::fs::remove_file(w.stubs.join("gliff")).unwrap();
    // The developer's own PATH may have gliff: use only the stubs and the few tools they need.
    let path = w.minimal_path(&["sh", "cat", "tr", "mkdir", "cp", "env"]);
    let r = common::ran(w.command().env("PATH", path).args(["connect", T]));
    assert_eq!(r.code, 1);
    assert_eq!(
        r.stderr,
        "gliffherdr: gliff is not installed on this machine.\n  omarchy pkg add gliff\n"
    );
}

#[test]
fn connect_reports_missing_window() {
    let w = ready_world();
    w.write_data("no-window", "");
    let r = w.run(&["connect", T]);
    assert_eq!(r.code, 1);
    assert!(
        r.stderr
            .contains("gliff did not open a window for j@10.0.0.2."),
        "{}",
        r.stderr
    );
    assert!(r.stderr.contains("gliff.log"), "{}", r.stderr);
    assert!(r.stderr.contains("log: "), "{}", r.stderr);
    // The log tail (the header line) is in the hint.
    assert!(r.stderr.contains("env XDG_CONFIG_HOME="), "{}", r.stderr);
    gliff_argv(&w);
}

#[test]
fn connect_does_not_pass_the_parents_herdr_environment_to_gliff() {
    // The harness strips HERDR_* from the child, so gliff never sees a planted one.
    let mut w = ready_world();
    w.simulate_parent_env("HERDR_SOCKET_PATH", "/tmp/live-herdr.sock");
    assert_eq!(w.run(&["connect", T]).code, 0);
    gliff_argv(&w);
    assert!(!w.read_data("gliff.env").contains("HERDR"));
}

#[test]
fn connect_shows_progress_only_on_a_terminal() {
    // The binary's stderr is a pipe here, so no progress lines are printed.
    let w = ready_world();
    let r = w.run(&["connect", T]);
    assert_eq!(r.stderr, "");
}

#[test]
fn connect_honours_the_window_wait_override() {
    let mut w = ready_world();
    w.write_data("no-window", "");
    w.set_env("GLIFFHERDR_WINDOW_WAIT_MS", "50");
    let started = std::time::Instant::now();
    let r = w.run(&["connect", T]);
    assert_eq!(r.code, 1);
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn list_marks_machines_with_an_open_window() {
    let world = World::new();
    world.write_config(
        "[[machine]]\ntarget = \"j@10.0.0.2\"\n[[machine]]\ntarget = \"k@10.0.0.7\"\n",
    );
    world.host_answer(T, 0, READY, "");
    assert_eq!(world.run(&["check", T]).code, 0);
    world.write_data("clients.json", WINDOW);
    let ran = world.run(&["list", "--json"]);
    assert_eq!(ran.code, 0, "{}", ran.all());
    let list: serde_json::Value = serde_json::from_str(&ran.stdout).unwrap();
    let open = list
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["target"] == T)
        .unwrap();
    assert_eq!(open["window_open"], true);
    assert_eq!(open["state"], "open");
    let closed = list
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["target"] == "k@10.0.0.7")
        .unwrap();
    assert_eq!(closed["window_open"], false);
    let table = world.run(&["list"]).stdout;
    assert!(
        table.lines().next().unwrap().contains("window open"),
        "{table}"
    );
}

#[test]
fn list_without_a_desktop_session_has_no_open_windows() {
    let mut w = World::new();
    w.unset_env("WAYLAND_DISPLAY");
    w.unset_env("HYPRLAND_INSTANCE_SIGNATURE");
    w.write_config("[[machine]]\ntarget = \"j@10.0.0.2\"\n");
    w.write_data("clients.json", WINDOW);
    let r = w.run(&["list", "--json"]);
    assert_eq!(r.code, 0, "{}", r.all());
    let v: serde_json::Value = serde_json::from_str(&r.stdout).unwrap();
    assert_eq!(v[0]["window_open"], false);
}
