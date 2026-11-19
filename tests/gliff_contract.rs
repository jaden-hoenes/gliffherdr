//! The gliff contract (docs/design/03-desktop.md, rows G1 to G9).
//!
//! Every test reads the gliff source in `.vendor/gliff` (pinned to 0.4.3 by `scripts/vendor.sh`)
//! and asserts the facts gliffherdr relies on. A failure names the row: gliff changed something
//! gliffherdr depends on, and the matching code in `launch`, `hypr`, `check` or `roster` must be
//! reviewed. Without `.vendor/gliff` the tests print why and pass, except when `CI=true`.

use std::path::{Path, PathBuf};

/// The gliff commit the contract was written against (gliff 0.4.3).
const PINNED_SHA: &str = "9d93732ae3fd4319e071feeecf38ab2cfc19edfc";

fn vendor_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(".vendor/gliff")
}

/// The vendored gliff tree, or `None` (after printing why) when it is missing locally.
/// Missing in CI is a failure.
fn gliff() -> Option<PathBuf> {
    let dir = vendor_dir();
    if dir.join("crates/gliff/src/main.rs").is_file() {
        return Some(dir);
    }
    assert!(
        std::env::var("CI").as_deref() != Ok("true"),
        ".vendor/gliff is missing in CI: run scripts/vendor.sh before the tests"
    );
    eprintln!("skipped: .vendor/gliff is missing (run scripts/vendor.sh)");
    None
}

fn read(root: &Path, rel: &str) -> String {
    let path = root.join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// Collapse every run of whitespace to one space.
fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` without `//` comment lines (doc comments included), so prose cannot satisfy or break an
/// assertion.
fn code_only(text: &str) -> String {
    text.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The code from the line containing `start` through the first line that is exactly `end`
/// (usually `}`), whitespace-squashed. Panics with the row when `start` is gone.
fn block(row: &str, text: &str, start: &str, end: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let first = lines
        .iter()
        .position(|l| l.contains(start))
        .unwrap_or_else(|| panic!("{row}: `{start}` no longer appears in gliff's source"));
    let last = lines[first..]
        .iter()
        .position(|l| *l == end)
        .unwrap_or_else(|| panic!("{row}: the block starting at `{start}` has no closing `{end}`"));
    squash(&code_only(&lines[first..=first + last].join("\n")))
}

/// Split the body of a struct into its fields at top-level commas, each squashed.
fn fields(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for ch in body.chars() {
        match ch {
            '(' | '[' | '<' => depth += 1,
            ')' | ']' | '>' => depth -= 1,
            _ => {}
        }
        if ch == ',' && depth == 0 {
            out.push(squash(&cur));
            cur.clear();
        } else {
            cur.push(ch);
        }
    }
    if !squash(&cur).is_empty() {
        out.push(squash(&cur));
    }
    out
}

/// Assert `haystack` contains every needle, naming the row and the missing fact.
fn assert_has(row: &str, haystack: &str, needles: &[&str]) {
    for needle in needles {
        assert!(
            haystack.contains(needle),
            "{row}: gliff's source no longer contains `{needle}`\n--- searched ---\n{haystack}"
        );
    }
}

/// Assert the needles appear in `haystack` in the given order.
fn assert_order(row: &str, haystack: &str, needles: &[&str]) {
    let mut from = 0;
    for needle in needles {
        match haystack[from..].find(needle) {
            Some(i) => from += i + needle.len(),
            None => panic!(
                "{row}: `{needle}` is missing or out of order (after `{}`)\n--- searched ---\n{haystack}",
                &haystack[..from]
            ),
        }
    }
}

/// Where gliff's `.git/HEAD` points, without running git.
fn head_sha(dir: &Path) -> String {
    let git = dir.join(".git");
    let git_dir = if git.is_file() {
        let text = std::fs::read_to_string(&git).expect("read .vendor/gliff/.git");
        let rel = text
            .trim()
            .strip_prefix("gitdir:")
            .expect("gitdir: line")
            .trim();
        dir.join(rel)
    } else {
        git
    };
    let head = std::fs::read_to_string(git_dir.join("HEAD")).expect("read .git/HEAD");
    let head = head.trim();
    let Some(reference) = head.strip_prefix("ref:") else {
        return head.to_string();
    };
    let reference = reference.trim();
    if let Ok(sha) = std::fs::read_to_string(git_dir.join(reference)) {
        return sha.trim().to_string();
    }
    let packed = std::fs::read_to_string(git_dir.join("packed-refs")).unwrap_or_default();
    packed
        .lines()
        .find_map(|l| l.strip_suffix(reference).map(|s| s.trim().to_string()))
        .unwrap_or_else(|| panic!("cannot resolve {reference} in .vendor/gliff"))
}

#[test]
fn vendored_gliff_is_the_pinned_commit() {
    let Some(g) = gliff() else { return };
    assert_eq!(
        head_sha(&g),
        PINNED_SHA,
        ".vendor/gliff is not gliff 0.4.3 (9d93732): run scripts/vendor.sh; the G1 to G9 rows were \
         written against that commit"
    );
    assert_has("pin", &read(&g, "Cargo.toml"), &["version = \"0.4.3\""]);
}

#[test]
fn g1_binary_flags_and_one_optional_positional_host() {
    let Some(g) = gliff() else { return };
    let main = read(&g, "crates/gliff/src/main.rs");
    let cli = block("G1", &main, "struct Cli {", "}");
    assert_has("G1", &cli, &["struct Cli {"]);
    let body = cli
        .strip_prefix("struct Cli {")
        .unwrap()
        .trim_end_matches('}')
        .to_string();
    let items = fields(&body);

    // Every flag gliffherdr passes, with its exact attribute: long-only, defaults, conflicts.
    let expected = [
        "#[arg(long)] no_clipboard: bool",
        "#[arg(long, default_value = \"gliff-server\")] server_bin: String",
        "#[arg(long, conflicts_with = \"output\")] headless: bool",
        "#[arg(long)] output: Option<String>",
        "#[arg(long, default_value = \"shift+escape\")] release_hotkey: String",
        "#[arg(long, value_parser = [\"gpu\", \"cpu\"])] video: Option<String>",
    ];
    for want in expected {
        assert!(
            items.iter().any(|i| i == want),
            "G1: struct Cli lost the field `{want}`; has {items:#?}"
        );
    }

    // Exactly one positional (a field without #[arg]): the optional host, first.
    let positional: Vec<&String> = items.iter().filter(|i| !i.starts_with("#[arg")).collect();
    assert_eq!(
        positional,
        ["host: Option<String>"],
        "G1: gliff's only positional must be `host: Option<String>`; has {items:#?}"
    );
    assert_eq!(
        items[0], "host: Option<String>",
        "G1: `host` must stay the first field"
    );

    // The binary name.
    assert_has("G1", &squash(&main), &["name = APP_NAME"]);
}

#[test]
fn g2_window_class_is_gliff() {
    let Some(g) = gliff() else { return };
    let main = squash(&code_only(&read(&g, "crates/gliff/src/main.rs")));
    assert_has(
        "G2",
        &main,
        &[
            "const APP_NAME: &str = \"gliff\";",
            "glib::set_prgname(Some(APP_NAME));",
        ],
    );
}

#[test]
fn g3_window_title_is_machine_em_dash_gliff() {
    let Some(g) = gliff() else { return };
    let main = read(&g, "crates/gliff/src/main.rs");
    let title = block("G3", &main, "fn window_title(", "}");
    // " — " with U+2014 EM DASH, spelled out so a look-alike dash cannot slip through.
    let want = format!(
        "format!(\"{{}} {} Gliff\", machine_name(endpoint))",
        '\u{2014}'
    );
    assert_has("G3", &title, &[&want]);
    // machine_name is the host string as given.
    let name = block("G3", &main, "fn machine_name(", "}");
    assert_has(
        "G3",
        &name,
        &[
            "Endpoint::Ssh(t) => t.host.as_str(),",
            "Endpoint::Tcp(addr) => addr.as_str(),",
        ],
    );
}

#[test]
fn g4_no_application_id_so_every_launch_is_a_new_window() {
    let Some(g) = gliff() else { return };
    let main = read(&g, "crates/gliff/src/main.rs");
    let entry = block("G4", &main, "fn main() -> glib::ExitCode {", "}");
    assert_has("G4", &entry, &["adw::Application::builder().build();"]);

    let dir = g.join("crates/gliff/src");
    for entry in std::fs::read_dir(&dir).expect("read crates/gliff/src") {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "rs") {
            let text = code_only(&std::fs::read_to_string(&path).unwrap());
            assert!(
                !text.contains("application_id"),
                "G4: {} sets an application id; gliff would become single-instance and \
                 gliffherdr's one-window-per-machine launches would merge",
                path.display()
            );
        }
    }
}

#[test]
fn g5_open_machines_are_reconnected_then_cli_hosts() {
    let Some(g) = gliff() else { return };
    let main = read(&g, "crates/gliff/src/main.rs");
    let ui = squash(&code_only(&main));
    assert_order(
        "G5",
        &ui,
        &[
            "let config = Config::load(&ui.config_path);",
            "for machine in &config.open { open_session(&ui, ssh_endpoint(cli, machine)); }",
            "for endpoint in from_cli.iter().cloned() { if find_session(&ui, machine_name(&endpoint)).is_none() { open_session(&ui, endpoint); } }",
        ],
    );
}

#[test]
fn g6_config_path_and_keys() {
    let Some(g) = gliff() else { return };
    let recent = read(&g, "crates/gliff/src/recent.rs");
    let code = squash(&code_only(&recent));
    assert_has(
        "G6",
        &code,
        &[
            "gtk4::glib::user_config_dir() .join(\"gliff\") .join(\"config.toml\")",
            "#[serde(default, alias = \"recent_machines\")] pub machines: Vec<String>,",
            "#[serde(default)] pub open: Vec<String>,",
            "pub shown: Option<String>,",
        ],
    );
}

#[test]
fn g7_theme_comes_from_home_not_xdg_config_home() {
    let Some(g) = gliff() else { return };
    let theme = read(&g, "crates/gliff/src/theme.rs");
    let code = squash(&code_only(&theme));
    assert_has(
        "G7",
        &code,
        &[
            "std::env::var_os(\"HOME\").map(|h| Path::new(&h).join(\".local/state/omarchy/current\"))",
        ],
    );
    assert!(
        !code.contains("XDG_CONFIG_HOME") && !code.contains("user_config_dir"),
        "G7: gliff's theme now reads XDG_CONFIG_HOME; launch::prepare_config_home may no longer \
         need to link the theme state"
    );
}

#[test]
fn g8_ssh_argv_double_dash_and_accept_new_rule() {
    let Some(g) = gliff() else { return };
    let ssh = read(&g, "crates/gliff-transport/src/ssh.rs");
    let cmd = block("G8", &ssh, "fn ssh_command(", "}");
    assert_order(
        "G8",
        &cmd,
        &[
            "Command::new(\"ssh\")",
            "cmd.arg(\"-T\")",
            ".args(&target.ssh_args);",
            "if host_key_checking_is_default(target) {",
            "cmd.arg(\"-o\").arg(\"StrictHostKeyChecking=accept-new\");",
            "cmd.arg(\"--\")",
            ".arg(&target.host)",
            ".arg(&target.server_bin)",
            ".arg(\"--stdio\")",
            ".args(&target.server_args);",
        ],
    );
    let rule = block("G8", &ssh, "fn host_key_checking_is_default(", "}");
    assert_order(
        "G8",
        &rule,
        &[
            "Command::new(\"ssh\")",
            ".arg(\"-G\")",
            ".args(&target.ssh_args)",
            ".arg(\"--\")",
            ".arg(&target.host)",
            ".any(|l| l == \"stricthostkeychecking ask\")",
        ],
    );
}

#[test]
fn g9_gliff_server_version_flag() {
    let Some(g) = gliff() else { return };
    let main = read(&g, "crates/gliff-server/src/main.rs");
    let lines: Vec<&str> = main.lines().collect();
    let at = lines
        .iter()
        .position(|l| l.trim() == "struct Cli {")
        .unwrap_or_else(|| panic!("G9: gliff-server has no `struct Cli`"));
    // The derive and #[command(...)] block directly above it.
    let head = squash(
        &lines[..at]
            .iter()
            .rev()
            .take(8)
            .rev()
            .copied()
            .collect::<Vec<_>>()
            .join("\n"),
    );
    assert_has(
        "G9",
        &head,
        &[
            "#[derive(Parser)]",
            "#[command( name = \"gliff-server\", version,",
        ],
    );
    // clap prints `<name> <version>` and the version is the workspace's.
    let manifest = read(&g, "crates/gliff-server/Cargo.toml");
    assert_has("G9", &manifest, &["version.workspace = true"]);
}
