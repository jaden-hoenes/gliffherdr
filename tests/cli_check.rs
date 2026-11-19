//! `check` through the built binary: classification, startup/stale selection, the accept-new
//! rule, the cache, and the remote script itself (ports the earlier bash prototype's tests).

mod common;

use common::{READY, World};
use gliffherdr::check::{Status, parse_report, status_from_report};

/// The check output as `(target, status, detail)` triples.
fn lines(out: &str) -> Vec<(String, String, String)> {
    out.lines()
        .map(|l| {
            let mut it = l.splitn(3, ' ').map(str::trim);
            let (t, rest) = (
                it.next().unwrap().to_string(),
                l[l.find(' ').unwrap()..].trim(),
            );
            let (s, d) = rest.split_once(' ').unwrap_or((rest, ""));
            (t, s.to_string(), d.trim().to_string())
        })
        .collect()
}

fn status_of(rows: &[(String, String, String)], target: &str) -> (String, String) {
    let r = rows
        .iter()
        .find(|r| r.0 == target)
        .unwrap_or_else(|| panic!("no {target}: {rows:?}"));
    (r.1.clone(), r.2.clone())
}

#[test]
fn check_classifies_machines() {
    let w = World::new();
    w.host_answer("j@10.0.0.2", 0, READY, "");
    w.host_answer(
        "mac@10.0.0.3",
        0,
        "os=Darwin\nhyprland=0\ngliff_server=\n",
        "",
    );
    w.host_answer(
        "me@desk.local",
        0,
        "os=Linux\nhyprland=0\ngliff_server=/usr/bin/gliff-server\n",
        "",
    );
    w.host_answer("k@10.0.0.7", 0, "os=Linux\nhyprland=1\ngliff_server=\n", "");
    w.host_answer("box", 255, "", "j@box: Permission denied (publickey).");
    w.host_answer(
        "off",
        255,
        "",
        "ssh: connect to host off port 22: No route to host",
    );
    w.host_answer("hk", 255, "", "Host key verification failed.");
    w.host_answer(
        "changed",
        255,
        "",
        "@@@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @@@\nHost key verification failed.",
    );
    w.host_answer("weird", 1, "", "something else broke");
    let r = w.run(&[
        "check",
        "j@10.0.0.2",
        "mac@10.0.0.3",
        "me@desk.local",
        "k@10.0.0.7",
        "box",
        "off",
        "hk",
        "changed",
        "weird",
    ]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let rows = lines(&r.stdout);
    assert_eq!(rows.len(), 9);
    assert_eq!(status_of(&rows, "j@10.0.0.2").0, "ready");
    assert_eq!(
        status_of(&rows, "mac@10.0.0.3"),
        ("unsupported".into(), "Darwin".into())
    );
    assert_eq!(status_of(&rows, "me@desk.local").0, "no-hyprland");
    assert_eq!(status_of(&rows, "k@10.0.0.7").0, "no-server");
    assert_eq!(status_of(&rows, "box").0, "auth");
    let off = status_of(&rows, "off");
    assert_eq!(off.0, "offline");
    assert!(off.1.contains("No route to host"), "{off:?}");
    assert_eq!(
        status_of(&rows, "hk"),
        ("hostkey".into(), "host key unknown".into())
    );
    assert_eq!(
        status_of(&rows, "changed"),
        ("hostkey".into(), "host key changed".into())
    );
    assert_eq!(
        status_of(&rows, "weird"),
        ("error".into(), "something else broke".into())
    );
    assert_eq!(
        r.stdout
            .lines()
            .map(|l| l.split_whitespace().next().unwrap())
            .collect::<Vec<_>>(),
        [
            "j@10.0.0.2",
            "mac@10.0.0.3",
            "me@desk.local",
            "k@10.0.0.7",
            "box",
            "off",
            "hk",
            "changed",
            "weird"
        ],
        "output follows the order of the arguments"
    );
}

#[test]
fn check_aligns_columns_and_never_ends_a_line_with_spaces() {
    let w = World::new();
    w.host_answer("a", 0, READY, "");
    w.host_answer("longer-target", 0, "os=Darwin\n", "");
    let out = w.run(&["check", "a", "longer-target"]).stdout;
    assert_eq!(
        out,
        "a              ready\nlonger-target  unsupported   Darwin\n"
    );
}

#[test]
fn an_unscripted_ssh_failure_is_an_error_with_the_exit_status() {
    let w = World::new();
    let out = w.run(&["check", "nobody"]).stdout;
    assert_eq!(
        out.trim(),
        "nobody  error         ssh exited with status 255"
    );
}

#[test]
fn check_writes_the_cache_json() {
    let w = World::new();
    w.host_answer("j@10.0.0.2", 0, READY, "");
    w.run(&["check", "j@10.0.0.2"]);
    let files: Vec<_> = std::fs::read_dir(w.state.join("checks"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1);
    let name = files[0].file_name().unwrap().to_str().unwrap();
    assert!(
        name.starts_with("j@10.0.0.2-") && name.ends_with("json"),
        "{name}"
    );
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&files[0]).unwrap()).unwrap();
    assert_eq!(json["version"], 1);
    assert_eq!(json["target"], "j@10.0.0.2");
    assert_eq!(json["status"], serde_json::json!({"kind": "ready"}));
    assert_eq!(json["os"], "Linux");
    assert_eq!(json["outputs"], serde_json::json!(["DP-1", "HDMI-A-1"]));
    assert_eq!(json["server_version"], "gliff-server 0.4.3");
    assert!(json["checked_at"].as_u64().unwrap() > 1_700_000_000);
}

#[test]
fn startup_check_skips_tailscale_only_machines_and_fresh_results() {
    let w = World::new();
    w.roster_fixture();
    w.host_answer("j@10.0.0.2", 0, READY, "");
    w.run(&["check", "j@10.0.0.2"]);
    std::fs::write(w.data.join("ssh.log"), "").unwrap();
    let r = w.run(&["check", "--startup"]);
    assert_eq!(
        (r.code, r.stdout.as_str()),
        (0, ""),
        "startup prints nothing"
    );
    let mut checked = w.ssh_checked();
    checked.sort();
    // j is fresh; box is a Tailscale peer and waits for the picker; laptop (macOS) is never
    // checked. mac@10.0.0.3 is a herdr machine with no OS hint, so it is checked.
    assert_eq!(checked, ["k@10.0.0.7", "mac@10.0.0.3", "me@desk.local"]);
}

#[test]
fn startup_check_can_be_turned_off() {
    let w = World::new();
    w.roster_fixture();
    w.config_file_machines("[check]\non_startup = false\n");
    let r = w.run(&["check", "--startup"]);
    assert_eq!((r.code, r.stdout.as_str(), r.stderr.as_str()), (0, "", ""));
    assert_eq!(w.ssh_checked().len(), 0, "{:?}", w.ssh_checked());
}

#[test]
fn startup_check_exits_zero_even_when_machines_are_unreachable() {
    let w = World::new();
    w.config_file_machines("[[machine]]\ntarget = \"down@h\"\n");
    w.host_answer(
        "down@h",
        255,
        "",
        "ssh: connect to host h port 22: Connection refused",
    );
    let r = w.run(&["check", "--startup", "--quiet"]);
    assert_eq!((r.code, r.stdout.as_str(), r.stderr.as_str()), (0, "", ""));
    assert_eq!(w.ssh_checked(), ["down@h"]);
}

#[test]
fn check_all_includes_linux_tailscale_peers_but_not_macos() {
    let w = World::new();
    w.roster_fixture();
    let r = w.run(&["check", "--all"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let checked = w.ssh_checked().join(" ");
    assert!(checked.contains("box.tail.ts.net"), "{checked}");
    assert!(!checked.contains("laptop"), "{checked}");
    assert!(!checked.contains("phone"), "{checked}");
    assert_eq!(w.ssh_checked().len(), 5);
}

#[test]
fn check_stale_checks_only_missing_or_old_results() {
    let w = World::new();
    w.config_file_machines("[[machine]]\ntarget = \"a@h\"\n[[machine]]\ntarget = \"b@h\"\n[check]\nfresh_for_secs = 3600\n");
    w.host_answer("a@h", 0, READY, "");
    w.host_answer("b@h", 0, READY, "");
    w.run(&["check", "a@h"]);
    std::fs::write(w.data.join("ssh.log"), "").unwrap();
    let r = w.run(&["check", "--stale"]);
    assert_eq!(w.ssh_checked(), ["b@h"]);
    assert_eq!(r.stdout.lines().count(), 1);
    // Age a@h's result past the freshness window: now it is stale as well.
    let cache = std::fs::read_dir(w.state.join("checks"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.file_name().unwrap().to_str().unwrap().starts_with("a@h"))
        .unwrap();
    let text = std::fs::read_to_string(&cache).unwrap().replace(
        &format!(
            "\"checked_at\":{}",
            serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&cache).unwrap())
                .unwrap()["checked_at"]
        ),
        "\"checked_at\":1000",
    );
    std::fs::write(&cache, text).unwrap();
    std::fs::write(w.data.join("ssh.log"), "").unwrap();
    w.run(&["check", "--stale"]);
    assert_eq!(w.ssh_checked(), ["a@h"]);
}

#[test]
fn explicit_targets_need_not_be_in_the_roster_and_are_deduplicated() {
    let w = World::new();
    w.host_answer("stranger@elsewhere", 0, READY, "");
    let r = w.run(&["check", "stranger@elsewhere", "stranger@elsewhere"]);
    assert_eq!(r.stdout.lines().count(), 1);
    assert_eq!(w.ssh_checked(), ["stranger@elsewhere"]);
    assert_eq!(
        w.read_data("herdr.log"),
        "",
        "no roster was collected for explicit targets"
    );
}

#[test]
fn explicit_targets_and_all_do_not_check_a_machine_twice() {
    let w = World::new();
    w.config_file_machines("[[machine]]\ntarget = \"a@h\"\n");
    w.run(&["check", "a@h", "--all"]);
    assert_eq!(w.ssh_checked(), ["a@h"]);
}

#[test]
fn check_rejects_option_injection_and_shell_syntax() {
    let w = World::new();
    // clap itself refuses a leading dash; after `--` our own validation does.
    assert_eq!(
        w.run(&["check", "-oProxyCommand=touch /tmp/gliffherdr-injection-x"])
            .code,
        2
    );
    for bad in [
        "-oProxyCommand=touch /tmp/gliffherdr-injection-x",
        "a;b",
        "a b",
        "$(x)",
        "",
    ] {
        let r = w.run(&["check", "--", bad]);
        assert_eq!(r.code, 2, "{bad:?}");
        assert!(
            r.stderr.contains("not an ssh destination"),
            "{bad:?}: {}",
            r.stderr
        );
    }
    assert_eq!(w.ssh_calls().len(), 0, "nothing reached ssh");
    assert!(!std::path::Path::new("/tmp/gliffherdr-injection-x").exists());
}

#[test]
fn check_with_nothing_to_check_says_so_on_stderr() {
    let w = World::new();
    let r = w.run(&["check"]);
    assert_eq!((r.code, r.stdout.as_str()), (0, ""));
    assert_eq!(r.stderr, "nothing to check\n");
    assert_eq!(w.run(&["check", "--quiet"]).stderr, "");
}

#[test]
fn quiet_prints_nothing_but_still_caches() {
    let w = World::new();
    w.host_answer("a@h", 0, READY, "");
    let r = w.run(&["check", "--quiet", "a@h"]);
    assert_eq!((r.code, r.stdout.as_str(), r.stderr.as_str()), (0, "", ""));
    assert_eq!(
        std::fs::read_dir(w.state.join("checks")).unwrap().count(),
        1
    );
}

#[test]
fn accept_new_host_keys_exactly_when_ssh_would_ask() {
    let w = World::new();
    w.host_answer("asks", 0, READY, "");
    w.host_answer("strict", 0, READY, "");
    w.ssh_g_raw(
        "asks",
        "user u\nhostname asks\nport 22\nstricthostkeychecking ask\n",
    );
    w.ssh_g_raw(
        "strict",
        "user u\nhostname strict\nport 22\nstricthostkeychecking true\n",
    );
    w.run(&["check", "asks", "strict"]);
    let calls = w.ssh_calls();
    let check_call = |t: &str| {
        calls
            .iter()
            .find(|c| c.contains(&format!("-- {t} sh -s")))
            .unwrap()
            .clone()
    };
    assert!(
        check_call("asks").contains("-o StrictHostKeyChecking=accept-new"),
        "{calls:?}"
    );
    assert!(!check_call("strict").contains("accept-new"), "{calls:?}");
    for t in ["asks", "strict"] {
        let c = check_call(t);
        assert!(
            c.starts_with("-T -o BatchMode=yes -o ConnectTimeout=5 -o LogLevel=ERROR"),
            "{c}"
        );
    }
}

#[test]
fn the_ssh_timeout_comes_from_the_config() {
    let w = World::new();
    w.config_file_machines("[check]\nssh_timeout_secs = 2\n");
    w.host_answer("a", 0, READY, "");
    w.run(&["check", "a"]);
    assert!(w.ssh_calls().iter().any(|c| c.contains("ConnectTimeout=2")));
}

#[test]
fn server_bin_is_fed_to_the_remote_script_quoted() {
    let w = World::new();
    w.config_file_machines("[connect]\nserver_bin = \"gliff-server --low-bandwidth\"\n[[machine]]\ntarget = \"own@h\"\nserver_bin = \"it's mine\"\n");
    w.host_answer("a", 0, READY, "");
    w.host_answer("own@h", 0, READY, "");
    w.run(&["check", "a", "own@h"]);
    assert_eq!(
        w.ssh_stdin("a").lines().next().unwrap(),
        "server_bin='gliff-server --low-bandwidth'"
    );
    assert_eq!(
        w.ssh_stdin("own@h").lines().next().unwrap(),
        "server_bin='it'\\''s mine'"
    );
    assert_eq!(w.ssh_stdin("a").lines().nth(1).unwrap(), "#!/bin/sh");
}

#[test]
fn without_server_bin_the_script_is_sent_alone() {
    let w = World::new();
    w.host_answer("a", 0, READY, "");
    w.run(&["check", "a"]);
    assert_eq!(w.ssh_stdin("a"), gliffherdr::check::REMOTE_SCRIPT);
}

// ---- the remote script, run for real with `sh -s` against the stub tools --------------------

/// Programs the remote script needs besides the stubs.
const SCRIPT_TOOLS: &[&str] = &["sh", "uname", "id", "sed", "tr", "head", "cat"];

fn run_script(w: &World, prefix: &str) -> String {
    let input = format!("{prefix}{}", gliffherdr::check::REMOTE_SCRIPT);
    let sh = common::find_tool("sh");
    let r = common::ran(
        w.command_for(sh)
            .env("PATH", w.minimal_path(SCRIPT_TOOLS))
            .arg("-s")
            .write_stdin(input),
    );
    assert_eq!(r.code, 0, "{}", r.stderr);
    r.stdout
}

#[test]
fn remote_script_reports_hyprland_and_outputs() {
    let mut w = World::new();
    w.unset_env("HYPRLAND_INSTANCE_SIGNATURE");
    let out = run_script(&w, "");
    let uname = common::ran(w.command_for("uname").arg("-s")).stdout;
    assert!(out.contains(&format!("os={}\n", uname.trim())), "{out}");
    assert!(out.contains("hyprland=1\n"), "{out}");
    assert!(out.contains("outputs=DP-1 HDMI-A-1 \n"), "{out}");
    assert!(
        w.read_data("hyprctl.log").contains("|-i 0 monitors"),
        "no signature means -i 0: {}",
        w.read_data("hyprctl.log")
    );
    assert!(out.contains("gliff_server=\n"), "{out}");
    assert!(!out.contains("server_version"), "{out}");
    let report = parse_report(&out);
    assert_eq!(report.outputs, ["DP-1", "HDMI-A-1"]);
    assert_eq!(
        status_from_report(&report),
        if report.os == "Linux" {
            Status::NoServer
        } else {
            Status::Unsupported {
                os: report.os.clone(),
            }
        }
    );
}

#[test]
fn remote_script_uses_the_signature_when_there_is_one() {
    let w = World::new(); // HYPRLAND_INSTANCE_SIGNATURE=testsig
    run_script(&w, "");
    let log = w.read_data("hyprctl.log");
    assert!(log.contains("HIS=testsig|version"), "{log}");
    assert!(!log.contains("-i 0"), "{log}");
}

#[test]
fn remote_script_finds_gliff_server_and_its_version() {
    let w = World::new();
    w.write_stub(
        "gliff-server",
        "echo 'gliff-server 0.4.3'\necho second line\n",
    );
    let out = run_script(&w, "");
    assert!(
        out.contains(&format!(
            "gliff_server={}\n",
            w.stubs.join("gliff-server").display()
        )),
        "{out}"
    );
    assert!(out.contains("server_version=gliff-server 0.4.3\n"), "{out}");
    let report = parse_report(&out);
    assert_eq!(report.server_version.as_deref(), Some("gliff-server 0.4.3"));
}

#[test]
fn remote_script_trusts_a_custom_server_command() {
    let w = World::new();
    w.write_stub("gliff-server", "echo should-not-run\n");
    let out = run_script(&w, "server_bin='x y'\n");
    assert!(out.contains("gliff_server=custom\n"), "{out}");
    assert!(
        !out.contains("should-not-run") && !out.contains("server_version"),
        "{out}"
    );
}

#[test]
fn remote_script_commands_cannot_eat_the_script_from_stdin() {
    // A hyprctl that reads stdin must not swallow the rest of the script.
    let w = World::new();
    w.write_stub(
        "hyprctl",
        "cat >/dev/null\ncase $1 in version) echo v ;; esac\nexit 0\n",
    );
    let out = run_script(&w, "");
    assert!(
        out.contains("hyprland=1") && out.contains("gliff_server="),
        "{out}"
    );
}

#[test]
fn remote_script_without_hyprland() {
    let w = World::new();
    w.write_stub("hyprctl", "exit 1\n");
    let out = run_script(&w, "");
    // pgrep may find a real Hyprland on the developer's machine; only the version call is stubbed.
    assert!(out.contains("outputs=\n"), "{out}");
}
