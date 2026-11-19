//! The plugin manifest against herdr's rules and against our own code.
//!
//! Everything but the enum checks runs without `.vendor/`; those two read herdr's schema source
//! (`.vendor/herdr/src/api/schema/plugins.rs`) and are skipped with a printed reason when it is
//! missing locally (a failure when `CI=true`).

use std::collections::BTreeSet;
use std::path::PathBuf;

use clap::Parser;
use gliffherdr::cli::Cli;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn manifest() -> toml::Table {
    let text = std::fs::read_to_string(root().join("herdr-plugin.toml")).expect("manifest");
    text.parse().expect("herdr-plugin.toml parses as TOML")
}

fn tables<'a>(m: &'a toml::Table, key: &str) -> Vec<&'a toml::Table> {
    m.get(key)
        .and_then(toml::Value::as_array)
        .map(|a| a.iter().map(|v| v.as_table().expect("a table")).collect())
        .unwrap_or_default()
}

fn str_of<'a>(t: &'a toml::Table, key: &str) -> &'a str {
    t.get(key)
        .and_then(toml::Value::as_str)
        .unwrap_or_else(|| panic!("missing string {key}"))
}

fn command_of(t: &toml::Table) -> Vec<String> {
    t.get("command")
        .and_then(toml::Value::as_array)
        .expect("command is an array")
        .iter()
        .map(|v| v.as_str().expect("argv strings").to_string())
        .collect()
}

/// Every command the manifest declares, with a label for messages.
fn commands(m: &toml::Table) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    for kind in ["build", "startup", "actions", "panes", "events"] {
        for t in tables(m, kind) {
            let name = t.get("id").and_then(toml::Value::as_str).unwrap_or("-");
            out.push((format!("{kind}:{name}"), command_of(t)));
        }
    }
    out
}

fn plugin_id_ok(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '_' | '-'))
}

fn local_id_ok(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-'))
}

/// The variants of `pub enum <name>` in herdr's schema, in `snake_case` (the enums use
/// `rename_all = "snake_case"`), or `None` (after printing why) when `.vendor` is missing.
fn herdr_enum(name: &str) -> Option<BTreeSet<String>> {
    let path = root().join(".vendor/herdr/src/api/schema/plugins.rs");
    let Ok(src) = std::fs::read_to_string(&path) else {
        assert!(
            std::env::var("CI").as_deref() != Ok("true"),
            "CI=true but {} is missing: run scripts/vendor.sh",
            path.display()
        );
        println!(
            "skipped: {} is missing (run scripts/vendor.sh)",
            path.display()
        );
        return None;
    };
    let start = src
        .find(&format!("pub enum {name} {{"))
        .unwrap_or_else(|| panic!("enum {name} not found in {}", path.display()));
    let body = &src[start..];
    let body = &body[body.find('{').unwrap() + 1..];
    let body = &body[..body.find('}').unwrap()];
    let variants = body
        .lines()
        .map(str::trim)
        .filter(|l| l.chars().next().is_some_and(char::is_uppercase))
        .map(|l| {
            let ident = l.trim_end_matches(',');
            let mut snake = String::new();
            for (i, c) in ident.chars().enumerate() {
                if c.is_uppercase() && i > 0 {
                    snake.push('_');
                }
                snake.push(c.to_ascii_lowercase());
            }
            snake
        })
        .collect();
    Some(variants)
}

#[test]
fn the_manifest_parses_and_has_the_required_fields() {
    let m = manifest();
    for key in ["id", "name", "version", "min_herdr_version"] {
        assert_ne!(str_of(&m, key), "", "{key}");
    }
    assert_eq!(str_of(&m, "name"), "gliffherdr");
    assert_eq!(str_of(&m, "min_herdr_version"), "0.9.0");
    let platforms: Vec<&str> = m["platforms"]
        .as_array()
        .expect("platforms")
        .iter()
        .filter_map(toml::Value::as_str)
        .collect();
    assert_eq!(platforms, ["linux"]);
}

#[test]
fn the_version_is_the_crates() {
    assert_eq!(str_of(&manifest(), "version"), env!("CARGO_PKG_VERSION"));
}

#[test]
fn the_plugin_id_matches_the_code() {
    let id = str_of(&manifest(), "id").to_string();
    assert!(plugin_id_ok(&id), "{id}");
    assert_eq!(id, gliffherdr::paths::PLUGIN_ID);
    assert_eq!(id, "jaden-hoenes.gliffherdr");
}

#[test]
fn ids_follow_herdrs_rules_and_are_unique_per_kind() {
    let m = manifest();
    for kind in ["actions", "panes"] {
        let ids: Vec<&str> = tables(&m, kind).iter().map(|t| str_of(t, "id")).collect();
        assert!(!ids.is_empty(), "{kind}");
        for id in &ids {
            assert!(local_id_ok(id), "{kind} id {id:?} (no dots allowed)");
        }
        let unique: BTreeSet<&&str> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "duplicate {kind} ids: {ids:?}");
        for t in tables(&m, kind) {
            assert_ne!(str_of(t, "title"), "");
        }
    }
}

#[test]
fn the_entry_points_are_the_ones_the_code_opens() {
    let m = manifest();
    let ids = |kind| -> BTreeSet<String> {
        tables(&m, kind)
            .iter()
            .map(|t| str_of(t, "id").to_string())
            .collect()
    };
    // `herdr plugin pane open --entrypoint picker|fix` in src/cli/open_picker.rs and src/picker.rs.
    assert_eq!(ids("panes"), ["fix", "picker"].map(String::from).into());
    assert_eq!(
        ids("actions"),
        ["connect", "connect-pane", "recheck"]
            .map(String::from)
            .into()
    );
}

#[test]
fn every_command_runs_our_binary_or_sh() {
    let m = manifest();
    let all = commands(&m);
    assert_eq!(
        all.len(),
        7,
        "1 build, 1 startup, 3 actions, 2 panes: {all:?}"
    );
    for (label, argv) in all {
        assert!(!argv.is_empty(), "{label}");
        assert!(
            argv[0] == "bin/gliffherdr" || argv[0] == "sh",
            "{label}: {argv:?}"
        );
        if argv[0] == "sh" {
            assert!(
                root().join(&argv[1]).is_file(),
                "{label}: {} does not exist",
                argv[1]
            );
        }
    }
}

#[test]
fn every_subcommand_exists_in_the_clap_definition() {
    for (label, argv) in commands(&manifest()) {
        if argv[0] != "bin/gliffherdr" {
            continue;
        }
        let mut words = vec!["gliffherdr".to_string()];
        words.extend(argv[1..].iter().cloned());
        if let Err(e) = Cli::try_parse_from(&words) {
            panic!("{label}: {words:?} does not parse: {e}");
        }
    }
}

#[test]
fn the_build_and_startup_commands_are_what_the_spec_says() {
    let m = manifest();
    assert_eq!(
        command_of(tables(&m, "build")[0]),
        ["sh", "scripts/install.sh"]
    );
    assert_eq!(
        command_of(tables(&m, "startup")[0]),
        ["bin/gliffherdr", "check", "--startup", "--quiet"]
    );
}

#[test]
fn pane_placements_and_sizes_are_valid() {
    let m = manifest();
    let allowed = herdr_enum("PluginPanePlacement");
    for t in tables(&m, "panes") {
        let placement = str_of(t, "placement");
        match &allowed {
            Some(set) => assert!(set.contains(placement), "{placement} not in {set:?}"),
            None => assert!(["overlay", "popup", "split", "tab", "zoomed"].contains(&placement)),
        }
        for dim in ["width", "height"] {
            if let Some(v) = t.get(dim) {
                let s = v.as_str().expect("percent string");
                let n = s.strip_suffix('%').expect("a percentage");
                assert!(n.parse::<u8>().is_ok_and(|n| (1..=100).contains(&n)), "{s}");
            }
        }
    }
    let picker = tables(&m, "panes")
        .into_iter()
        .find(|t| str_of(t, "id") == "picker")
        .unwrap();
    assert_eq!(str_of(picker, "placement"), "popup");
    let fix = tables(&m, "panes")
        .into_iter()
        .find(|t| str_of(t, "id") == "fix")
        .unwrap();
    assert_eq!(str_of(fix, "placement"), "split");
}

#[test]
fn action_contexts_are_in_herdrs_enum() {
    let m = manifest();
    let mut used = BTreeSet::new();
    for t in tables(&m, "actions") {
        let contexts = t["contexts"].as_array().expect("contexts");
        assert!(!contexts.is_empty(), "{}", str_of(t, "id"));
        for c in contexts {
            used.insert(c.as_str().expect("string").to_string());
        }
    }
    assert_eq!(
        used,
        ["global", "pane"].map(String::from).into(),
        "connect and recheck are global, connect-pane is pane"
    );
    if let Some(allowed) = herdr_enum("PluginActionContext") {
        assert!(
            allowed.is_superset(&used),
            "contexts {used:?} not all in herdr's {allowed:?}"
        );
        for expected in ["global", "workspace", "tab", "pane"] {
            assert!(allowed.contains(expected), "{expected} vanished from herdr");
        }
    }
}

#[test]
fn only_known_manifest_keys_are_used() {
    // herdr ignores unknown keys silently, so a typo would vanish; keep to the documented set.
    let m = manifest();
    let top: BTreeSet<&str> = [
        "id",
        "name",
        "version",
        "min_herdr_version",
        "description",
        "platforms",
        "build",
        "startup",
        "actions",
        "panes",
    ]
    .into();
    for k in m.keys() {
        assert!(top.contains(k.as_str()), "unexpected top-level key {k}");
    }
    let pane_keys: BTreeSet<&str> =
        ["id", "title", "placement", "width", "height", "command"].into();
    for t in tables(&m, "panes") {
        for k in t.keys() {
            assert!(pane_keys.contains(k.as_str()), "unexpected pane key {k}");
        }
    }
    let action_keys: BTreeSet<&str> = ["id", "title", "contexts", "command"].into();
    for t in tables(&m, "actions") {
        for k in t.keys() {
            assert!(
                action_keys.contains(k.as_str()),
                "unexpected action key {k}"
            );
        }
    }
}
