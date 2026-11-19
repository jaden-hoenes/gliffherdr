//! `fix` through the built binary: the remedy runs interactively (with stub `ssh` and
//! `ssh-copy-id`), the machine is checked again, the exit code says whether it ended ready.

mod common;

use common::{READY, World};

const T: &str = "j@10.0.0.2";

/// A stateful `ssh`: a remedy call (`-t`, or `-o StrictHostKeyChecking=ask`) creates
/// `$STUB/fixed`; a check answers from `$STUB/broken.{out,err,rc}` until then, afterwards with a
/// ready report. Every call's argv is appended to `$STUB/ssh.argv`.
const SSH: &str = r#"echo "$*" >>"$STUB/ssh.argv"
[ "$1" = -G ] && { printf 'user j\nhostname h\nport 22\n'; exit 0; }
case "$*" in
  *StrictHostKeyChecking=ask*|"-t "*) touch "$STUB/fixed"; exit 0 ;;
esac
cat >/dev/null
if [ -f "$STUB/fixed" ]; then
  printf 'os=Linux\nhyprland=1\ngliff_server=/usr/bin/gliff-server\n'
  exit 0
fi
cat "$STUB/broken.out" 2>/dev/null
cat "$STUB/broken.err" >&2 2>/dev/null
exit "$(cat "$STUB/broken.rc" 2>/dev/null || echo 255)"
"#;

fn world_with(out: &str, err: &str, rc: i32) -> World {
    let w = World::new();
    w.write_stub("ssh", SSH);
    w.write_data("broken.out", out);
    w.write_data("broken.err", err);
    w.write_data("broken.rc", &format!("{rc}\n"));
    w
}

#[test]
fn fix_of_a_ready_machine_changes_nothing() {
    let w = World::new();
    w.host_answer(T, 0, READY, "");
    let r = w.run(&["fix", T]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert_eq!(r.stdout, format!("{T} is ready.\n"));
}

#[test]
fn fix_installs_gliff_server_over_ssh_and_rechecks() {
    let w = world_with("os=Linux\nhyprland=1\ngliff_server=\n", "", 0);
    let r = w.run(&["fix", T]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(
        r.stdout.starts_with(&format!(
            "Install gliff on {T} (runs omarchy pkg add gliff there)\n"
        )),
        "{}",
        r.stdout
    );
    assert!(
        r.stdout.ends_with(&format!("{T} is ready.\n")),
        "{}",
        r.stdout
    );
    let calls = w.ssh_calls();
    let remedy = calls
        .iter()
        .find(|l| l.starts_with("-t "))
        .expect("remedy ran");
    assert!(
        remedy.starts_with(&format!("-t -- {T} if command -v omarchy"))
            && remedy.contains("omarchy pkg add gliff"),
        "{remedy}"
    );
    // Check, remedy, check (each check is preceded by an `ssh -G` look-up).
    assert_eq!(
        calls.iter().filter(|l| l.contains("sh -s")).count(),
        2,
        "{calls:?}"
    );
}

#[test]
fn fix_copies_the_key_when_it_was_refused() {
    let w = world_with("", "j@h: Permission denied (publickey).", 255);
    w.write_stub(
        "ssh-copy-id",
        "echo \"$*\" >>\"$STUB/ssh-copy-id.argv\"\ntouch \"$STUB/fixed\"\n",
    );
    let r = w.run(&["fix", T]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(
        r.stdout.starts_with(&format!("Copy your ssh key to {T}\n")),
        "{}",
        r.stdout
    );
    assert_eq!(w.read_data("ssh-copy-id.argv"), format!("{T}\n"));
}

#[test]
fn fix_accepts_a_new_host_key_with_an_interactive_ssh() {
    let w = world_with("", "Host key verification failed.", 255);
    let r = w.run(&["fix", T]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(
        r.stdout
            .starts_with(&format!("Check {T}'s host key and accept it\n")),
        "{}",
        r.stdout
    );
    assert!(
        w.ssh_calls()
            .iter()
            .any(|l| l == &format!("-o StrictHostKeyChecking=ask -- {T} true")),
        "{:?}",
        w.ssh_calls()
    );
}

#[test]
fn fix_that_does_not_help_exits_1_and_says_why() {
    let w = world_with("", "j@h: Permission denied (publickey).", 255);
    w.write_stub("ssh-copy-id", "exit 1\n");
    let r = w.run(&["fix", T]);
    assert_eq!(r.code, 1, "{}", r.all());
    assert!(
        r.stdout.contains("ssh-copy-id exited with status 1."),
        "{}",
        r.stdout
    );
    assert!(
        r.stdout.contains(&format!("{T} refused the ssh key.")),
        "{}",
        r.stdout
    );
    assert_eq!(
        r.stderr, "",
        "the outcome is on stdout, no extra error line"
    );
}

#[test]
fn fix_does_not_run_anything_for_a_changed_host_key() {
    let w = world_with(
        "",
        "WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!\nHost key verification failed.",
        255,
    );
    let r = w.run(&["fix", T]);
    assert_eq!(r.code, 1);
    assert!(r.stdout.contains("host key has changed"), "{}", r.stdout);
    assert!(r.stdout.contains("ssh-keygen -R 10.0.0.2"), "{}", r.stdout);
    assert!(!w.data.join("fixed").exists());
}

#[test]
fn fix_without_a_remedy_explains_the_problem() {
    let w = World::new();
    w.host_answer(T, 0, "os=Linux\nhyprland=0\ngliff_server=/x\n", "");
    let r = w.run(&["fix", T]);
    assert_eq!(r.code, 1);
    assert!(
        r.stdout.contains("Hyprland is not running there."),
        "{}",
        r.stdout
    );
}

#[test]
fn fix_takes_its_target_from_the_environment() {
    let mut w = World::new();
    w.host_answer(T, 0, READY, "");
    w.set_env("GLIFFHERDR_FIX_TARGET", T);
    let r = w.run(&["fix"]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert_eq!(r.stdout, format!("{T} is ready.\n"));
    assert_eq!(w.ssh_checked(), [T]);
    // An argument beats the environment.
    w.host_answer("k@10.0.0.7", 0, READY, "");
    let r = w.run(&["fix", "k@10.0.0.7"]);
    assert_eq!(r.stdout, "k@10.0.0.7 is ready.\n");
}

#[test]
fn fix_needs_a_valid_target() {
    let w = World::new();
    let r = w.run(&["fix"]);
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("no machine to fix"), "{}", r.stderr);
    let r = w.run(&["fix", "--", "a;b"]);
    assert_eq!(r.code, 2);
    assert_eq!(w.ssh_calls().len(), 0);
}

#[test]
fn fix_does_not_wait_for_enter_without_a_terminal() {
    // stdin is /dev/null here: a wait would end at once; a hang would fail the test run. The
    // prompt text must not appear because stdin is not a terminal.
    let w = World::new();
    w.host_answer(T, 0, READY, "");
    let r = w.run(&["fix", T]);
    assert!(!r.stdout.contains("Press Enter"));
}
