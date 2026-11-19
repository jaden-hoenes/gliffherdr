//! The roster: every machine the user can open, merged from all sources.
//!
//! [`collect`] gathers candidates from the sources in [`sources`], validates and resolves each
//! target, filters git hosts and hidden machines, and merges spellings of one machine so it is
//! listed once however many sources name it.

pub mod sources;
pub mod ssh_argv;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;

use crate::config::{Config, MachineConfig, Mode};
use crate::paths::Paths;
use crate::sys::Sys;
use crate::target::{Resolved, Target, explicit_user, fallback_resolve, resolve};

use sources::Candidate;

/// Where a machine was found. Declaration order is the collection order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Source {
    /// `[[machine]]` in `config.toml`.
    Config,
    /// herdr's saved machines.
    Herdr,
    /// Machines connected to before.
    Recent,
    /// gliff's own machine list.
    Gliff,
    /// `~/.ssh/config`.
    SshConfig,
    /// Online Tailscale peers.
    Tailscale,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Source::Config => "config",
            Source::Herdr => "herdr",
            Source::Recent => "recent",
            Source::Gliff => "gliff",
            Source::SshConfig => "ssh",
            Source::Tailscale => "tailscale",
        })
    }
}

/// The operating system a source claims a machine runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OsHint {
    /// Linux.
    Linux,
    /// macOS.
    MacOs,
    /// Windows.
    Windows,
    /// Anything else, as the source spelled it.
    Other(String),
}

/// Per-machine overrides from a `[[machine]]` entry.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MachineSettings {
    /// What opening shows.
    pub mode: Option<Mode>,
    /// Mirror this remote screen.
    pub output: Option<String>,
    /// The remote gliff-server command.
    pub server_bin: Option<String>,
    /// Extra gliff arguments.
    pub gliff_args: Option<Vec<String>>,
}

/// One listed machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Machine {
    /// The destination, as the first source that named it spelled it.
    pub target: Target,
    /// The best label: Config, then herdr, then Tailscale.
    pub label: Option<String>,
    /// Every source that names this machine, in collection order.
    pub sources: Vec<Source>,
    /// The operating system, when a source knows it.
    pub os_hint: Option<OsHint>,
    /// Overrides from the matching `[[machine]]` entry.
    pub settings: MachineSettings,
    /// What ssh resolves the target to.
    pub resolved: Resolved,
}

impl Machine {
    /// The label, or the target when there is none.
    pub fn display_label(&self) -> &str {
        self.label
            .as_deref()
            .unwrap_or_else(|| self.target.as_str())
    }

    /// Whether a gliff window for this machine is open, given [`crate::hypr::open_targets`]:
    /// its target is a window's title target, or what it resolves to is what a window's title
    /// target resolves to.
    #[must_use]
    pub fn is_open(&self, open: &HashSet<String>) -> bool {
        open.contains(self.target.as_str()) || open.contains(&self.resolved.full_key())
    }

    /// Whether an ssh check is worthwhile: the OS is unknown or Linux.
    pub fn checkable(&self) -> bool {
        matches!(self.os_hint, None | Some(OsHint::Linux))
    }
}

/// The merged machine list and the problems met while collecting it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Roster {
    /// The machines, in first-seen order.
    pub machines: Vec<Machine>,
    /// One line per source that failed or entry that was dropped as invalid.
    pub warnings: Vec<String>,
}

impl Roster {
    /// The machine whose target text is exactly `target`.
    pub fn find(&self, target: &Target) -> Option<&Machine> {
        self.machines.iter().find(|m| &m.target == target)
    }
}

/// The `[[machine]]` entry for a target: the one with the same target text, else the first
/// whose own text names the same user, host and port (no ssh call is made for entries).
fn config_entry<'a>(
    config: &'a Config,
    target: &Target,
    resolved: &Resolved,
) -> Option<&'a MachineConfig> {
    config
        .machines
        .iter()
        .find(|m| m.target == target.as_str())
        .or_else(|| {
            config.machines.iter().find(|m| {
                let own = fallback_resolve(&m.target);
                explicit_user(&m.target) == Some(resolved.user.as_str())
                    && own.host == resolved.host
                    && own.port == resolved.port
            })
        })
}

/// The per-machine settings that apply to `target`.
pub fn settings_for(config: &Config, target: &Target, resolved: &Resolved) -> MachineSettings {
    config_entry(config, target, resolved).map_or_else(MachineSettings::default, |m| {
        MachineSettings {
            mode: m.mode,
            output: m.output.clone(),
            server_bin: m.server_bin.clone(),
            gliff_args: m.gliff_args.clone(),
        }
    })
}

/// A machine for a target that is not (or not known to be) in the roster: resolved with ssh,
/// with the settings and label of its `[[machine]]` entry when there is one.
pub fn bare_machine(sys: &dyn Sys, config: &Config, target: Target) -> Machine {
    let resolved = resolve(sys, &target);
    let entry = config_entry(config, &target, &resolved);
    Machine {
        label: entry.and_then(|m| m.label.clone()),
        sources: if entry.is_some() {
            vec![Source::Config]
        } else {
            Vec::new()
        },
        os_hint: None,
        settings: settings_for(config, &target, &resolved),
        resolved,
        target,
    }
}

/// Hosts that answer ssh but are never desktops.
fn is_git_host(host: &str) -> bool {
    const EXACT: [&str; 6] = [
        "github.com",
        "gitlab.com",
        "bitbucket.org",
        "codeberg.org",
        "git.sr.ht",
        "aur.archlinux.org",
    ];
    EXACT.contains(&host)
        || host.ends_with(".github.com")
        || host == "ssh.dev.azure.com"
        || host == "vs-ssh.visualstudio.com"
}

/// A machine while merging, with the names it is known by.
struct Entry {
    machine: Machine,
    host_keys: BTreeSet<String>,
    names: Vec<String>,
}

/// Collect, filter and merge every source into the roster. Sources run in the order
/// `Config`, `Herdr`, `Recent`, `Gliff`, `SshConfig`, `Tailscale`; `Config` is always on, the others follow
/// `config.sources`.
pub fn collect(sys: &dyn Sys, paths: &Paths, config: &Config) -> Roster {
    let on = &config.sources;
    let mut fetched = vec![sources::from_config(config)];
    if on.herdr {
        fetched.push(sources::from_herdr(sys));
    }
    if on.recent {
        fetched.push(sources::from_recent(paths));
    }
    if on.gliff {
        fetched.push(sources::from_gliff(paths));
    }
    if on.ssh_config {
        fetched.push(sources::from_ssh_config(paths));
    }
    if on.tailscale {
        fetched.push(sources::from_tailscale(sys));
    }

    let mut warnings = Vec::new();
    let mut entries: Vec<Entry> = Vec::new();
    let mut by_full_key: HashMap<String, usize> = HashMap::new();
    for f in fetched {
        warnings.extend(f.warning);
        for c in f.candidates {
            match Target::parse(&c.target) {
                Ok(t) => merge(sys, config, &mut entries, &mut by_full_key, t, &c),
                Err(e) => warnings.push(format!("{}: ignoring {:?}: {e}", c.source, c.target)),
            }
        }
    }

    let hidden: BTreeSet<String> = on
        .hide
        .iter()
        .map(|h| h.trim().to_lowercase())
        .filter(|h| !h.is_empty())
        .collect();
    entries.retain(|e| !e.names.iter().any(|n| hidden.contains(n)));
    Roster {
        machines: entries.into_iter().map(|e| e.machine).collect(),
        warnings,
    }
}

fn merge(
    sys: &dyn Sys,
    config: &Config,
    entries: &mut Vec<Entry>,
    by_full_key: &mut HashMap<String, usize>,
    target: Target,
    c: &Candidate,
) {
    let resolved = resolve(sys, &target);
    if c.source == Source::SshConfig && (resolved.user == "git" || is_git_host(&resolved.host)) {
        return;
    }
    let full_key = resolved.full_key();
    let mut host_keys: BTreeSet<String> = c.host_keys.iter().cloned().collect();
    host_keys.insert(resolved.host.clone());

    let existing = if c.source == Source::Tailscale {
        entries
            .iter()
            .position(|e| host_keys.iter().any(|k| e.host_keys.contains(k)))
    } else {
        by_full_key.get(&full_key).copied()
    };
    let mut names = vec![target.as_str().to_lowercase()];
    names.extend(c.label.iter().map(|l| l.to_lowercase()));

    if let Some(i) = existing {
        let e = &mut entries[i];
        if !e.machine.sources.contains(&c.source) {
            e.machine.sources.push(c.source);
        }
        if e.machine.label.is_none() {
            e.machine.label.clone_from(&c.label);
        }
        if e.machine.os_hint.is_none() {
            e.machine.os_hint.clone_from(&c.os_hint);
        }
        e.host_keys.extend(host_keys);
        e.names.extend(names);
        by_full_key.entry(full_key).or_insert(i);
    } else {
        by_full_key.insert(full_key, entries.len());
        entries.push(Entry {
            machine: Machine {
                settings: settings_for(config, &target, &resolved),
                target,
                label: c.label.clone(),
                sources: vec![c.source],
                os_hint: c.os_hint.clone(),
                resolved,
            },
            host_keys,
            names,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::fake::FakeSys;

    fn t(s: &str) -> Target {
        Target::parse(s).unwrap()
    }

    fn ssh_g(fake: &FakeSys, dest: &str, user: &str, host: &str, port: u16) {
        fake.on("ssh", &["-G", "--", dest])
            .stdout(&format!("user {user}\nhostname {host}\nport {port}\n"));
    }

    struct Env {
        dir: tempfile::TempDir,
        paths: Paths,
        fake: FakeSys,
    }

    fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        Env {
            dir,
            paths,
            fake: FakeSys::new(),
        }
    }

    fn config(toml: &str) -> Config {
        toml::from_str(toml).unwrap()
    }

    fn summary(r: &Roster) -> Vec<(String, Option<String>, Vec<String>)> {
        r.machines
            .iter()
            .map(|m| {
                (
                    m.target.to_string(),
                    m.label.clone(),
                    m.sources.iter().map(ToString::to_string).collect(),
                )
            })
            .collect()
    }

    fn row(t: &str, l: Option<&str>, s: &[&str]) -> (String, Option<String>, Vec<String>) {
        (
            t.into(),
            l.map(Into::into),
            s.iter().map(|x| (*x).into()).collect(),
        )
    }

    /// A roster fixture with machines from every source, some of them the same machine.
    fn v01_fixture(e: &Env) {
        let paths = &e.paths;
        e.fake.on("herdr", &["machine", "list", "--json"]).stdout(
            r#"[{"id":"a","label":"j","target":"j@10.0.0.2"},{"id":"b","label":"Q","target":"mac@10.0.0.3"}]"#,
        );
        e.fake.on("tailscale", &["status", "--json"]).stdout(
            r#"{"Self":{"HostName":"me"},"Peer":{
  "k1":{"HostName":"j","DNSName":"j.tail.ts.net.","OS":"linux","Online":true,"TailscaleIPs":["10.0.0.2"]},
  "k2":{"HostName":"box","DNSName":"box.tail.ts.net.","OS":"linux","Online":true,"TailscaleIPs":["100.64.0.9"]},
  "k3":{"HostName":"laptop","DNSName":"laptop.tail.ts.net.","OS":"macOS","Online":true,"TailscaleIPs":["100.64.0.10"]},
  "k4":{"HostName":"phone","DNSName":"phone.tail.ts.net.","OS":"iOS","Online":true,"TailscaleIPs":["100.64.0.11"]},
  "k5":{"HostName":"asleep","DNSName":"asleep.tail.ts.net.","OS":"linux","Online":false,"TailscaleIPs":["100.64.0.12"]}}}"#,
        );
        std::fs::create_dir_all(paths.home.join(".ssh")).unwrap();
        std::fs::write(
            paths.home.join(".ssh/config"),
            "Host github.com\n  User git\nHost gitbox\n  User git\nHost devbox\n  HostName 10.0.0.2\n  User j\nHost *\n  ServerAliveInterval 30\n",
        )
        .unwrap();
        ssh_g(&e.fake, "github.com", "git", "github.com", 22);
        ssh_g(&e.fake, "gitbox", "git", "gitbox.lan", 22);
        ssh_g(&e.fake, "devbox", "j", "10.0.0.2", 22);
        std::fs::create_dir_all(paths.real_config_home.join("gliff")).unwrap();
        std::fs::write(
            paths.real_config_home.join("gliff/config.toml"),
            "machines = [\"k@10.0.0.7\", \"j@10.0.0.2\"]\nopen = []\n",
        )
        .unwrap();
    }

    #[test]
    fn merges_sources_dedupes_and_filters_like_v01() {
        let e = env();
        v01_fixture(&e);
        let cfg = config("[[machine]]\ntarget = \"me@desk.local\"\nlabel = \"desk\"\n");
        let r = collect(&e.fake, &e.paths, &cfg);
        assert_eq!(r.warnings, Vec::<String>::new());
        assert_eq!(
            summary(&r),
            [
                row("me@desk.local", Some("desk"), &["config"]),
                row(
                    "j@10.0.0.2",
                    Some("j"),
                    &["herdr", "gliff", "ssh", "tailscale"]
                ),
                row("mac@10.0.0.3", Some("Q"), &["herdr"]),
                row("k@10.0.0.7", None, &["gliff"]),
                row("box.tail.ts.net", Some("box"), &["tailscale"]),
                row("laptop.tail.ts.net", Some("laptop"), &["tailscale"]),
            ]
        );
        let laptop = r.machines.last().unwrap();
        assert_eq!(laptop.os_hint, Some(OsHint::MacOs));
        assert!(!laptop.checkable());
        assert!(r.machines[1].checkable());
        assert_eq!(
            r.machines[1].os_hint,
            Some(OsHint::Linux),
            "tailscale tells us j runs Linux"
        );
    }

    #[test]
    fn sources_can_be_switched_off() {
        let e = env();
        v01_fixture(&e);
        let cfg = config(
            "[sources]\ntailscale = false\nssh_config = false\nherdr = false\ngliff = false\nrecent = false\n[[machine]]\ntarget = \"a@b\"\n",
        );
        let r = collect(&e.fake, &e.paths, &cfg);
        assert_eq!(summary(&r), [row("a@b", None, &["config"])]);
        assert_eq!(e.fake.calls_to("herdr").len(), 0);
        assert_eq!(e.fake.calls_to("tailscale").len(), 0);
    }

    #[test]
    fn recent_comes_third_and_merges_by_full_key() {
        let e = env();
        std::fs::create_dir_all(&e.paths.state_dir).unwrap();
        std::fs::write(e.paths.recent_file(), r#"["new@h","a@h"]"#).unwrap();
        let cfg = config("[[machine]]\ntarget = \"a@h\"\nlabel = \"A\"\n");
        let r = collect(&e.fake, &e.paths, &cfg);
        assert_eq!(
            summary(&r),
            [
                row("a@h", Some("A"), &["config", "recent"]),
                row("new@h", None, &["recent"])
            ]
        );
    }

    #[test]
    fn two_spellings_of_one_machine_merge_by_resolved_key() {
        let e = env();
        ssh_g(&e.fake, "alias", "u", "10.1.1.1", 22);
        ssh_g(&e.fake, "u@10.1.1.1", "u", "10.1.1.1", 22);
        ssh_g(&e.fake, "u@10.1.1.1:2200", "u", "10.1.1.1", 2200);
        let cfg = config(
            "[[machine]]\ntarget = \"alias\"\nlabel = \"first\"\n[[machine]]\ntarget = \"u@10.1.1.1\"\n[[machine]]\ntarget = \"ssh://u@10.1.1.1:2200\"\n",
        );
        ssh_g(&e.fake, "ssh://u@10.1.1.1:2200", "u", "10.1.1.1", 2200);
        let r = collect(&e.fake, &e.paths, &cfg);
        assert_eq!(
            summary(&r),
            [
                row("alias", Some("first"), &["config"]),
                row("ssh://u@10.1.1.1:2200", None, &["config"]),
            ],
            "a different port is a different machine"
        );
    }

    #[test]
    fn label_precedence_is_config_then_herdr_then_tailscale() {
        let e = env();
        ssh_g(&e.fake, "h1", "u", "10.0.0.5", 22);
        e.fake.on("herdr", &[]).stdout(r#"[{"target":"u@10.0.0.5","label":"herdr label"},{"target":"u@10.0.0.6","label":"H6"}]"#);
        e.fake.on("tailscale", &[]).stdout(r#"{"Peer":{"a":{"HostName":"tsname","DNSName":"x.ts.net.","OS":"linux","Online":true,"TailscaleIPs":["10.0.0.5"]},"b":{"HostName":"six","DNSName":"six.ts.net.","OS":"linux","Online":true,"TailscaleIPs":["10.0.0.6"]}}}"#);
        let cfg = config("[[machine]]\ntarget = \"h1\"\n");
        let r = collect(&e.fake, &e.paths, &cfg);
        // h1 resolves to u@10.0.0.5 and merges with herdr's spelling; no config label, so
        // herdr's label wins over Tailscale's.
        assert_eq!(
            summary(&r),
            [
                row("h1", Some("herdr label"), &["config", "herdr", "tailscale"]),
                row("u@10.0.0.6", Some("H6"), &["herdr", "tailscale"]),
            ]
        );
        let cfg = config("[[machine]]\ntarget = \"h1\"\nlabel = \"mine\"\n");
        let r = collect(&e.fake, &e.paths, &cfg);
        assert_eq!(r.machines[0].label.as_deref(), Some("mine"));
    }

    #[test]
    fn tailscale_matches_on_host_name_and_ips_but_not_on_user() {
        let e = env();
        e.fake
            .on("herdr", &[])
            .stdout(r#"[{"target":"someuser@box","label":"b"}]"#);
        ssh_g(&e.fake, "someuser@box", "someuser", "box", 22);
        e.fake.on("tailscale", &[]).stdout(r#"{"Peer":{"a":{"HostName":"Box","DNSName":"box.ts.net.","OS":"linux","Online":true,"TailscaleIPs":["100.1.1.1"]}}}"#);
        let r = collect(&e.fake, &e.paths, &Config::default());
        assert_eq!(
            summary(&r),
            [row("someuser@box", Some("b"), &["herdr", "tailscale"])]
        );
    }

    #[test]
    fn git_hosts_are_dropped_only_from_ssh_config() {
        let e = env();
        std::fs::create_dir_all(e.paths.home.join(".ssh")).unwrap();
        std::fs::write(
            e.paths.home.join(".ssh/config"),
            "Host gh\nHost gl\nHost gu\nHost az\nHost ok\nHost sub\n",
        )
        .unwrap();
        ssh_g(&e.fake, "gh", "x", "github.com", 22);
        ssh_g(&e.fake, "gl", "x", "gitlab.com", 22);
        ssh_g(&e.fake, "gu", "git", "mybox", 22);
        ssh_g(&e.fake, "az", "x", "ssh.dev.azure.com", 22);
        ssh_g(&e.fake, "sub", "x", "ghe.github.com", 22);
        ssh_g(&e.fake, "ok", "x", "notgithub.com.example", 22);
        let cfg = config("[[machine]]\ntarget = \"git@github.com\"\n");
        let r = collect(&e.fake, &e.paths, &cfg);
        assert_eq!(
            summary(&r),
            [
                row("git@github.com", None, &["config"]),
                row("ok", None, &["ssh"])
            ]
        );
    }

    #[test]
    fn git_host_list() {
        for h in [
            "github.com",
            "foo.github.com",
            "gitlab.com",
            "bitbucket.org",
            "codeberg.org",
            "git.sr.ht",
            "aur.archlinux.org",
            "ssh.dev.azure.com",
            "vs-ssh.visualstudio.com",
        ] {
            assert!(is_git_host(h), "{h}");
        }
        for h in [
            "notgithub.com",
            "github.com.evil.example",
            "gitlab.org",
            "box.lan",
        ] {
            assert!(!is_git_host(h), "{h}");
        }
    }

    #[test]
    fn hide_matches_target_or_label_case_insensitively_including_merged_names() {
        let e = env();
        e.fake.on("herdr", &[]).stdout(r#"[{"target":"j@10.0.0.2","label":"Desk"},{"target":"k@h","label":"keep"},{"target":"Hidden@Host"}]"#);
        std::fs::create_dir_all(e.paths.home.join(".ssh")).unwrap();
        std::fs::write(e.paths.home.join(".ssh/config"), "Host devbox\n").unwrap();
        ssh_g(&e.fake, "devbox", "j", "10.0.0.2", 22);
        let cfg = config("[sources]\nhide = [\"desk\", \" HIDDEN@host \", \"\"]\n");
        let r = collect(&e.fake, &e.paths, &cfg);
        assert_eq!(summary(&r), [row("k@h", Some("keep"), &["herdr"])]);
        // Hiding by the ssh alias also removes the herdr entry for the same machine.
        let cfg = config("[sources]\nhide = [\"DevBox\"]\n");
        let r = collect(&e.fake, &e.paths, &cfg);
        assert!(r.machines.iter().all(|m| m.target.as_str() != "j@10.0.0.2"));
    }

    #[test]
    fn hide_applies_to_machine_entries_too() {
        let e = env();
        let cfg = config(
            "[sources]\nhide = [\"desk\", \"b@h\"]\n\n[[machine]]\ntarget = \"me@desk.local\"\nlabel = \"desk\"\n\n[[machine]]\ntarget = \"b@h\"\n\n[[machine]]\ntarget = \"c@h\"\n",
        );
        let r = collect(&e.fake, &e.paths, &cfg);
        assert_eq!(summary(&r), [row("c@h", None, &["config"])]);
    }

    #[test]
    fn invalid_targets_are_dropped_with_a_warning() {
        let e = env();
        e.fake
            .on("herdr", &[])
            .stdout(r#"[{"target":"-oProxyCommand=x"},{"target":"ok@h"},{"target":"a b"}]"#);
        let r = collect(&e.fake, &e.paths, &Config::default());
        assert_eq!(summary(&r), [row("ok@h", None, &["herdr"])]);
        assert_eq!(r.warnings.len(), 2);
        assert!(
            r.warnings[0].starts_with("herdr: ignoring \"-oProxyCommand=x\""),
            "{:?}",
            r.warnings
        );
    }

    #[test]
    fn source_failures_are_warnings_and_do_not_stop_the_rest() {
        let e = env();
        e.fake.on("herdr", &[]).code(2).stderr("boom\n");
        e.fake.on("tailscale", &[]).stdout("garbage");
        std::fs::create_dir_all(&e.paths.state_dir).unwrap();
        std::fs::write(e.paths.recent_file(), r#"["r@h"]"#).unwrap();
        let r = collect(&e.fake, &e.paths, &Config::default());
        assert_eq!(summary(&r), [row("r@h", None, &["recent"])]);
        assert_eq!(r.warnings.len(), 2);
        assert!(r.warnings.iter().any(|w| w.starts_with("herdr: ")));
        assert!(r.warnings.iter().any(|w| w.starts_with("tailscale: ")));
    }

    #[test]
    fn settings_come_from_the_matching_machine_entry() {
        let cfg = config(
            "[[machine]]\ntarget = \"j@10.0.0.2\"\nmode = \"headless\"\noutput = \"DP-1\"\nserver_bin = \"gs\"\ngliff_args = [\"--x\"]\n[[machine]]\ntarget = \"ssh://k@kbox:2200\"\nmode = \"mirror\"\n",
        );
        let by_text = settings_for(
            &cfg,
            &t("j@10.0.0.2"),
            &Resolved {
                user: "j".into(),
                host: "10.0.0.2".into(),
                port: 22,
            },
        );
        assert_eq!(
            by_text,
            MachineSettings {
                mode: Some(Mode::Headless),
                output: Some("DP-1".into()),
                server_bin: Some("gs".into()),
                gliff_args: Some(vec!["--x".into()]),
            }
        );
        // Another spelling of the same user, host and port.
        let by_key = settings_for(
            &cfg,
            &t("kbox-alias"),
            &Resolved {
                user: "k".into(),
                host: "kbox".into(),
                port: 2200,
            },
        );
        assert_eq!(by_key.mode, Some(Mode::Mirror));
        let none = settings_for(
            &cfg,
            &t("other"),
            &Resolved {
                user: "j".into(),
                host: "10.9.9.9".into(),
                port: 22,
            },
        );
        assert_eq!(none, MachineSettings::default());
    }

    #[test]
    fn machines_carry_their_settings() {
        let e = env();
        let cfg = config("[[machine]]\ntarget = \"j@h\"\nmode = \"headless\"\n");
        e.fake.on("herdr", &[]).stdout(r#"[{"target":"j@h"}]"#);
        let r = collect(&e.fake, &e.paths, &cfg);
        assert_eq!(r.machines[0].settings.mode, Some(Mode::Headless));
        assert_eq!(r.machines.len(), 1);
    }

    #[test]
    fn bare_machine_and_find() {
        let e = env();
        ssh_g(&e.fake, "devbox", "j", "10.0.0.2", 22);
        let cfg = config("[[machine]]\ntarget = \"devbox\"\nlabel = \"dev\"\noutput = \"DP-1\"\n");
        let m = bare_machine(&e.fake, &cfg, t("devbox"));
        assert_eq!(m.label.as_deref(), Some("dev"));
        assert_eq!(m.settings.output.as_deref(), Some("DP-1"));
        assert_eq!(m.resolved.host, "10.0.0.2");
        let plain = bare_machine(&e.fake, &Config::default(), t("x@y"));
        assert_eq!(
            (plain.label, plain.sources.len(), plain.os_hint),
            (None, 0, None)
        );

        let r = collect(&e.fake, &e.paths, &cfg);
        assert!(r.find(&t("devbox")).is_some());
        assert!(r.find(&t("nope")).is_none());
        drop(e.dir);
    }

    #[test]
    fn display_label_and_checkable() {
        let e = env();
        let mut m = bare_machine(&e.fake, &Config::default(), t("j@h"));
        assert_eq!(m.display_label(), "j@h");
        m.label = Some("desk".into());
        assert_eq!(m.display_label(), "desk");
        for (hint, ok) in [
            (None, true),
            (Some(OsHint::Linux), true),
            (Some(OsHint::MacOs), false),
            (Some(OsHint::Windows), false),
            (Some(OsHint::Other("freebsd".into())), false),
        ] {
            m.os_hint = hint;
            assert_eq!(m.checkable(), ok);
        }
    }

    #[test]
    fn source_display_names() {
        let all = [
            Source::Config,
            Source::Herdr,
            Source::Recent,
            Source::Gliff,
            Source::SshConfig,
            Source::Tailscale,
        ];
        let names: Vec<_> = all.iter().map(ToString::to_string).collect();
        assert_eq!(
            names,
            ["config", "herdr", "recent", "gliff", "ssh", "tailscale"]
        );
        assert!(
            all.windows(2).all(|w| w[0] < w[1]),
            "declaration order is collection order"
        );
    }
}
