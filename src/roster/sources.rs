//! The six places machines come from. Each function turns one source into [`Candidate`]s; none
//! of them merges or filters (that is [`super::collect`]). A source that fails yields a
//! one-line warning and no candidates.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use super::{OsHint, Source};
use crate::config::Config;
use crate::herdr::{Herdr, HerdrError};
use crate::paths::Paths;
use crate::sys::{Sys, cmd};

/// How long tailscale may take to report its peers.
const TAILSCALE_TIMEOUT: Duration = Duration::from_secs(3);

/// A machine as one source names it, before merging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The source that named it.
    pub source: Source,
    /// The ssh destination, not yet validated.
    pub target: String,
    /// The label the source gives it.
    pub label: Option<String>,
    /// The operating system, when the source knows it.
    pub os_hint: Option<OsHint>,
    /// Further host names and addresses the machine is known by (lowercase), used to merge a
    /// Tailscale peer with an entry from another source.
    pub host_keys: Vec<String>,
}

impl Candidate {
    fn new(source: Source, target: &str, label: Option<&str>) -> Candidate {
        Candidate {
            source,
            target: target.to_string(),
            label: label
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string),
            os_hint: None,
            host_keys: Vec::new(),
        }
    }
}

/// What one source produced.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Fetched {
    /// The machines it named.
    pub candidates: Vec<Candidate>,
    /// A one-line problem report, when the source failed.
    pub warning: Option<String>,
}

impl Fetched {
    fn ok(candidates: Vec<Candidate>) -> Fetched {
        Fetched {
            candidates,
            warning: None,
        }
    }

    fn warn(source: Source, why: impl std::fmt::Display) -> Fetched {
        Fetched {
            candidates: Vec::new(),
            warning: Some(format!("{source}: {why}")),
        }
    }
}

/// `[[machine]]` entries of the config file.
pub fn from_config(config: &Config) -> Fetched {
    Fetched::ok(
        config
            .machines
            .iter()
            .map(|m| Candidate::new(Source::Config, &m.target, m.label.as_deref()))
            .collect(),
    )
}

/// herdr's saved machines: `herdr machine list --json`. A missing herdr is not a problem.
pub fn from_herdr(sys: &dyn Sys) -> Fetched {
    match Herdr::from_sys(sys).machines(sys) {
        Err(HerdrError::NotInstalled) => Fetched::default(),
        Err(e @ HerdrError::Failed(_)) => {
            Fetched::warn(Source::Herdr, format!("machine list failed: {e}"))
        }
        Err(e) => Fetched::warn(Source::Herdr, e),
        Ok(list) => Fetched::ok(herdr_candidates(&list)),
    }
}

fn herdr_candidates(list: &[(String, Option<String>)]) -> Vec<Candidate> {
    list.iter()
        .map(|(target, label)| Candidate::new(Source::Herdr, target, label.as_deref()))
        .collect()
}

/// Parse `herdr machine list --json`: an array (or an object with a `machines` array) of
/// `{target, label}`; entries without a target are skipped.
pub fn parse_herdr_machines(json: &str) -> Result<Vec<Candidate>, String> {
    crate::herdr::parse_machines(json)
        .map(|l| herdr_candidates(&l))
        .map_err(|e| e.to_string())
}

/// Machines connected to before: `<state>/recent.json`, most recent first.
pub fn from_recent(paths: &Paths) -> Fetched {
    let file = paths.recent_file();
    match std::fs::read_to_string(&file) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Fetched::default(),
        Err(e) => Fetched::warn(
            Source::Recent,
            format!("cannot read {}: {e}", file.display()),
        ),
        Ok(text) => match serde_json::from_str::<Vec<String>>(&text) {
            Ok(list) => Fetched::ok(
                list.iter()
                    .map(|t| Candidate::new(Source::Recent, t, None))
                    .collect(),
            ),
            Err(e) => Fetched::warn(
                Source::Recent,
                format!("{} is not a list of targets: {e}", file.display()),
            ),
        },
    }
}

/// Machines gliff remembers: the `machines` array of `<config home>/gliff/config.toml`.
pub fn from_gliff(paths: &Paths) -> Fetched {
    let file = paths.real_config_home.join("gliff/config.toml");
    match std::fs::read_to_string(&file) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Fetched::default(),
        Err(e) => Fetched::warn(
            Source::Gliff,
            format!("cannot read {}: {e}", file.display()),
        ),
        Ok(text) => match parse_gliff_machines(&text) {
            Ok(c) => Fetched::ok(c),
            Err(e) => Fetched::warn(Source::Gliff, format!("{}: {e}", file.display())),
        },
    }
}

/// Parse gliff's config for its `machines` array (other keys are ignored).
pub fn parse_gliff_machines(text: &str) -> Result<Vec<Candidate>, String> {
    let table: toml::Table = text
        .parse()
        .map_err(|e: toml::de::Error| e.message().to_string())?;
    let Some(machines) = table.get("machines") else {
        return Ok(Vec::new());
    };
    let list = machines.as_array().ok_or("`machines` is not a list")?;
    Ok(list
        .iter()
        .filter_map(toml::Value::as_str)
        .map(|t| Candidate::new(Source::Gliff, t, None))
        .collect())
}

/// Host aliases from `~/.ssh/config` and the files its `Include` lines name (one level).
pub fn from_ssh_config(paths: &Paths) -> Fetched {
    let ssh_dir = paths.home.join(".ssh");
    let main = ssh_dir.join("config");
    let Ok(text) = std::fs::read_to_string(&main) else {
        return Fetched::default();
    };
    let (mut hosts, includes) = parse_ssh_config(&text);
    for pattern in includes {
        for file in expand_include(&pattern, &ssh_dir, &paths.home) {
            if let Ok(text) = std::fs::read_to_string(&file) {
                hosts.extend(parse_ssh_config(&text).0);
            }
        }
    }
    let mut seen = HashSet::new();
    hosts.retain(|h| seen.insert(h.clone()));
    Fetched::ok(
        hosts
            .iter()
            .map(|h| Candidate::new(Source::SshConfig, h, None))
            .collect(),
    )
}

/// Split a config line into keyword and arguments (`Host a b`, `Host=a b`, `Host = a`).
fn keyword_line(line: &str) -> Option<(String, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let end = line
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(line.len());
    let rest = line[end..]
        .trim_start()
        .trim_start_matches('=')
        .trim_start();
    Some((line[..end].to_ascii_lowercase(), rest))
}

/// Arguments of a config line: whitespace separated, optionally double-quoted, up to a `#`.
fn arguments(rest: &str) -> Vec<String> {
    let mut out = Vec::new();
    for tok in rest.split_whitespace() {
        if tok.starts_with('#') {
            break;
        }
        out.push(tok.trim_matches('"').to_string());
    }
    out
}

/// Concrete host names from `Host` lines (patterns with `*`, `?` or `!` are skipped) and the
/// patterns of `Include` lines, in file order.
pub fn parse_ssh_config(text: &str) -> (Vec<String>, Vec<String>) {
    let (mut hosts, mut includes) = (Vec::new(), Vec::new());
    for line in text.lines() {
        match keyword_line(line) {
            Some((kw, rest)) if kw == "host" => hosts.extend(
                arguments(rest)
                    .into_iter()
                    .filter(|h| !h.contains(['*', '?', '!']) && !h.is_empty()),
            ),
            Some((kw, rest)) if kw == "include" => includes.extend(arguments(rest)),
            _ => {}
        }
    }
    (hosts, includes)
}

/// Files an `Include` pattern names: relative patterns are under `ssh_dir`, `~` is the home
/// directory, and `*` / `?` in the last path component match file names in that directory.
pub fn expand_include(pattern: &str, ssh_dir: &Path, home: &Path) -> Vec<PathBuf> {
    let full = if let Some(rest) = pattern.strip_prefix("~/") {
        home.join(rest)
    } else if pattern.starts_with('/') {
        PathBuf::from(pattern)
    } else {
        ssh_dir.join(pattern)
    };
    let (Some(dir), Some(name)) = (full.parent(), full.file_name().and_then(|n| n.to_str())) else {
        return Vec::new();
    };
    if dir.to_string_lossy().contains(['*', '?']) {
        return Vec::new();
    }
    if !name.contains(['*', '?']) {
        return if full.is_file() {
            vec![full]
        } else {
            Vec::new()
        };
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_str().is_some_and(|n| glob_match(name, n)))
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    files
}

/// Match `name` against a pattern where `*` is any run of characters and `?` any one.
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    let (mut pi, mut ni) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) && p[pi] != '*' {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// Online Tailscale peers: `tailscale status --json`. A missing tailscale is not a problem.
pub fn from_tailscale(sys: &dyn Sys) -> Fetched {
    let c = cmd("tailscale", &["status", "--json"]).with_timeout(TAILSCALE_TIMEOUT);
    match sys.run(&c) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Fetched::default(),
        Err(e) => Fetched::warn(Source::Tailscale, format!("cannot run tailscale: {e}")),
        Ok(out) if out.timed_out => Fetched::warn(Source::Tailscale, "no answer within 3s"),
        Ok(out) if !out.success() => Fetched::warn(
            Source::Tailscale,
            format!("status failed: {}", last_line(&out.stderr)),
        ),
        Ok(out) => match parse_tailscale(&out.stdout) {
            Ok(c) => Fetched::ok(c),
            Err(e) => Fetched::warn(Source::Tailscale, e),
        },
    }
}

/// Map a Tailscale `OS` string to an [`OsHint`]; empty means unknown.
pub fn os_hint(os: &str) -> Option<OsHint> {
    match os.trim().to_ascii_lowercase().as_str() {
        "" => None,
        "linux" => Some(OsHint::Linux),
        "macos" | "darwin" => Some(OsHint::MacOs),
        "windows" => Some(OsHint::Windows),
        _ => Some(OsHint::Other(os.trim().to_string())),
    }
}

/// Parse `tailscale status --json`: every `Peer` with `Online == true`, except iOS and Android
/// devices, Funnel ingress nodes (tag `tag:ingress`) and peers with neither DNS name nor OS. The target is the DNS name without its trailing dot, else the first Tailscale IP.
pub fn parse_tailscale(json: &str) -> Result<Vec<Candidate>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("unreadable status: {e}"))?;
    let Some(peers) = v.get("Peer").and_then(Value::as_object) else {
        return Ok(Vec::new());
    };
    let text = |p: &Value, key: &str| p.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    let mut out = Vec::new();
    for peer in peers.values() {
        if peer.get("Online").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        let os = text(peer, "OS");
        if matches!(os.to_ascii_lowercase().as_str(), "ios" | "android") {
            continue;
        }
        let dns = text(peer, "DNSName").trim_end_matches('.').to_string();
        // Tailscale's own Funnel ingress nodes are not machines.
        let ingress_tag = peer
            .get("Tags")
            .and_then(Value::as_array)
            .is_some_and(|t| t.iter().any(|t| t.as_str() == Some("tag:ingress")));
        if ingress_tag || (dns.is_empty() && os.trim().is_empty()) {
            continue;
        }
        let ips: Vec<String> = peer
            .get("TailscaleIPs")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let host_name = text(peer, "HostName");
        let Some(target) = Some(dns.clone())
            .filter(|d| !d.is_empty())
            .or_else(|| ips.first().cloned())
        else {
            continue;
        };
        let mut c = Candidate::new(Source::Tailscale, &target, Some(&host_name));
        c.os_hint = os_hint(&os);
        c.host_keys = std::iter::once(dns)
            .chain(std::iter::once(host_name))
            .chain(ips)
            .filter(|k| !k.is_empty())
            .map(|k| k.to_ascii_lowercase())
            .collect();
        out.push(c);
    }
    out.sort_by(|a, b| a.target.cmp(&b.target));
    Ok(out)
}

fn last_line(s: &str) -> String {
    s.lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("no details")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::fake::FakeSys;
    use std::io::ErrorKind;

    fn targets(f: &Fetched) -> Vec<&str> {
        f.candidates.iter().map(|c| c.target.as_str()).collect()
    }

    #[test]
    fn config_source_lists_machine_entries_with_labels() {
        let config: Config = toml::from_str(
            "[[machine]]\ntarget = \"j@h\"\nlabel = \"desk\"\n[[machine]]\ntarget = \"k\"\n",
        )
        .unwrap();
        let f = from_config(&config);
        assert_eq!(targets(&f), ["j@h", "k"]);
        assert_eq!(f.candidates[0].label.as_deref(), Some("desk"));
        assert_eq!(f.candidates[1].label, None);
        assert_eq!(f.candidates[0].source, Source::Config);
    }

    #[test]
    fn herdr_json_shapes() {
        let c = parse_herdr_machines(
            r#"[{"id":"a","label":"j","target":"j@10.0.0.2","enabled":true},{"id":"b","label":"","target":"mac"},{"label":"no target"},{"target":5}]"#,
        )
        .unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].label.as_deref(), Some("j"));
        assert_eq!(c[1].label, None, "empty label is no label");
        assert_eq!(
            parse_herdr_machines(r#"{"machines":[{"target":"x"}]}"#)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(parse_herdr_machines("[]").unwrap().len(), 0);
        assert!(parse_herdr_machines("nope").is_err());
        assert!(parse_herdr_machines("{}").is_err());
        assert!(parse_herdr_machines("3").is_err());
    }

    #[test]
    fn herdr_source_uses_herdr_bin_path_and_tolerates_absence() {
        let fake = FakeSys::new();
        fake.set_env("HERDR_BIN_PATH", "/opt/herdr");
        fake.on("/opt/herdr", &["machine", "list", "--json"])
            .stdout(r#"[{"target":"a@b"}]"#);
        let f = from_herdr(&fake);
        assert_eq!(targets(&f), ["a@b"]);
        assert_eq!(fake.calls()[0].cmd.timeout, Some(Duration::from_secs(5)));

        let none = FakeSys::new();
        assert_eq!(from_herdr(&none), Fetched::default(), "no herdr: silent");
        let on_path = FakeSys::new();
        on_path.on("herdr", &[]).stdout("[]");
        assert!(from_herdr(&on_path).warning.is_none());
    }

    #[test]
    fn herdr_failures_become_warnings() {
        let fail = FakeSys::new();
        fail.on("herdr", &[]).code(1).stderr("unknown command\n");
        let w = from_herdr(&fail).warning.unwrap();
        assert!(
            w.starts_with("herdr: ") && w.contains("unknown command"),
            "{w}"
        );
        let bad = FakeSys::new();
        bad.on("herdr", &[]).stdout("not json");
        assert!(from_herdr(&bad).warning.unwrap().contains("unreadable"));
        let denied = FakeSys::new();
        denied
            .on("herdr", &[])
            .io_error(ErrorKind::PermissionDenied);
        assert!(
            from_herdr(&denied)
                .warning
                .unwrap()
                .contains("cannot run herdr")
        );
    }

    fn paths_in(dir: &tempfile::TempDir) -> Paths {
        Paths::in_dir(dir.path())
    }

    #[test]
    fn recent_source() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(&dir);
        assert_eq!(from_recent(&paths), Fetched::default());
        std::fs::create_dir_all(&paths.state_dir).unwrap();
        std::fs::write(paths.recent_file(), r#"["b@h","a@h"]"#).unwrap();
        assert_eq!(targets(&from_recent(&paths)), ["b@h", "a@h"]);
        std::fs::write(paths.recent_file(), "{").unwrap();
        assert!(from_recent(&paths).warning.unwrap().starts_with("recent: "));
    }

    #[test]
    fn gliff_source_reads_machines_and_ignores_other_keys() {
        let c = parse_gliff_machines(
            "machines = [\"k@10.0.0.7\", \"j@10.0.0.2\"]\nopen = []\nshown = 3\n",
        )
        .unwrap();
        assert_eq!(
            c.iter().map(|c| c.target.as_str()).collect::<Vec<_>>(),
            ["k@10.0.0.7", "j@10.0.0.2"]
        );
        assert_eq!(parse_gliff_machines("open = []").unwrap().len(), 0);
        assert!(parse_gliff_machines("machines = 3").is_err());
        assert!(parse_gliff_machines("machines = [").is_err());

        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(&dir);
        assert_eq!(from_gliff(&paths), Fetched::default());
        std::fs::create_dir_all(paths.real_config_home.join("gliff")).unwrap();
        std::fs::write(
            paths.real_config_home.join("gliff/config.toml"),
            "machines = [\"x\"]\n",
        )
        .unwrap();
        assert_eq!(targets(&from_gliff(&paths)), ["x"]);
        std::fs::write(paths.real_config_home.join("gliff/config.toml"), "= =").unwrap();
        assert!(from_gliff(&paths).warning.unwrap().starts_with("gliff: "));
    }

    #[test]
    fn ssh_config_parsing() {
        let (hosts, inc) = parse_ssh_config(
            "# comment\nHost github.com\n  User git\nhost devbox  Dev2 # trailing\nHost=eq\nHost = spaced\nHost *\nHost web-* !bad ok?\nHOST \"quoted\"\nInclude conf.d/*\nInclude ~/x/y  /abs/z\nMatch host foo\n",
        );
        assert_eq!(
            hosts,
            ["github.com", "devbox", "Dev2", "eq", "spaced", "quoted"]
        );
        assert_eq!(inc, ["conf.d/*", "~/x/y", "/abs/z"]);
    }

    #[test]
    fn glob_matching_table() {
        let cases = [
            ("*", "anything", true),
            ("*.conf", "a.conf", true),
            ("*.conf", "a.conf.bak", false),
            ("a?c", "abc", true),
            ("a?c", "ac", false),
            ("a*b*c", "aXXbYYc", true),
            ("a*b*c", "aXXbYY", false),
            ("", "", true),
            ("", "a", false),
            ("**", "", true),
            ("config", "config", true),
            ("config", "config2", false),
        ];
        for (p, n, want) in cases {
            assert_eq!(glob_match(p, n), want, "{p} vs {n}");
        }
    }

    #[test]
    fn ssh_config_source_follows_includes_one_level() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(&dir);
        let ssh = paths.home.join(".ssh");
        std::fs::create_dir_all(ssh.join("conf.d")).unwrap();
        std::fs::write(
            ssh.join("config"),
            "Host main\nInclude conf.d/*.conf\nInclude extra\nInclude missing\nHost last\n",
        )
        .unwrap();
        std::fs::write(ssh.join("conf.d/b.conf"), "Host from-b\n").unwrap();
        std::fs::write(
            ssh.join("conf.d/a.conf"),
            "Host from-a\nInclude nested\nHost main\n",
        )
        .unwrap();
        std::fs::write(ssh.join("conf.d/skip.txt"), "Host skipped\n").unwrap();
        std::fs::write(ssh.join("extra"), "Host extra\n").unwrap();
        std::fs::write(ssh.join("nested"), "Host nested\n").unwrap();
        let f = from_ssh_config(&paths);
        assert_eq!(targets(&f), ["main", "last", "from-a", "from-b", "extra"]);
    }

    #[test]
    fn ssh_config_missing_is_silent() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(from_ssh_config(&paths_in(&dir)), Fetched::default());
    }

    #[test]
    fn include_paths_resolve_relative_to_ssh_dir_and_home() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let ssh = home.join(".ssh");
        std::fs::create_dir_all(home.join("cfg")).unwrap();
        std::fs::create_dir_all(&ssh).unwrap();
        std::fs::write(home.join("cfg/one"), "").unwrap();
        std::fs::write(ssh.join("rel"), "").unwrap();
        assert_eq!(
            expand_include("~/cfg/one", &ssh, &home),
            [home.join("cfg/one")]
        );
        assert_eq!(expand_include("rel", &ssh, &home), [ssh.join("rel")]);
        let abs = home.join("cfg/one");
        assert_eq!(expand_include(abs.to_str().unwrap(), &ssh, &home).len(), 1);
        assert_eq!(expand_include("nope", &ssh, &home).len(), 0);
        assert!(
            expand_include("*/x", &ssh, &home).is_empty(),
            "wildcards in directories are not expanded"
        );
    }

    const TAILSCALE: &str = r#"{"Self":{"HostName":"me","Online":true},
 "Peer":{
  "k1":{"HostName":"j","DNSName":"j.tail.ts.net.","OS":"linux","Online":true,"TailscaleIPs":["10.0.0.2"]},
  "k2":{"HostName":"Box","DNSName":"box.tail.ts.net.","OS":"linux","Online":true,"TailscaleIPs":["100.64.0.9"]},
  "k3":{"HostName":"laptop","DNSName":"laptop.tail.ts.net.","OS":"macOS","Online":true,"TailscaleIPs":["100.64.0.10"]},
  "k4":{"HostName":"phone","DNSName":"phone.tail.ts.net.","OS":"iOS","Online":true,"TailscaleIPs":["100.64.0.11"]},
  "k5":{"HostName":"asleep","DNSName":"asleep.tail.ts.net.","OS":"linux","Online":false,"TailscaleIPs":["100.64.0.12"]},
  "k6":{"HostName":"droid","DNSName":"droid.tail.ts.net.","OS":"Android","Online":true,"TailscaleIPs":["100.64.0.13"]},
  "k7":{"HostName":"noname","DNSName":"","OS":"windows","Online":true,"TailscaleIPs":["100.64.0.14","fd7a::1"]},
  "k8":{"HostName":"nothing","DNSName":"","OS":"linux","Online":true,"TailscaleIPs":[]}}}"#;

    #[test]
    fn tailscale_online_peers_without_phones() {
        let peers = parse_tailscale(TAILSCALE).unwrap();
        let pairs: Vec<_> = peers
            .iter()
            .map(|p| (p.target.as_str(), p.label.as_deref()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("100.64.0.14", Some("noname")),
                ("box.tail.ts.net", Some("Box")),
                ("j.tail.ts.net", Some("j")),
                ("laptop.tail.ts.net", Some("laptop")),
            ]
        );
        let named = |label: &str| {
            peers
                .iter()
                .find(|p| p.label.as_deref() == Some(label))
                .unwrap()
        };
        assert_eq!(named("j").host_keys, ["j.tail.ts.net", "j", "10.0.0.2"]);
        assert_eq!(named("j").os_hint, Some(OsHint::Linux));
        assert!(
            named("Box").host_keys.contains(&"box".to_string()),
            "host name keys are lowercase"
        );
        assert_eq!(named("laptop").os_hint, Some(OsHint::MacOs));
        assert_eq!(named("noname").os_hint, Some(OsHint::Windows));
    }

    #[test]
    fn tailscale_funnel_ingress_nodes_are_not_machines() {
        let json = r#"{"Peer":{
          "i1":{"HostName":"funnel-ingress-node","DNSName":"","OS":"","Online":true,"ShareeNode":true,"Tags":["tag:ingress"],"TailscaleIPs":["100.1.2.3"]},
          "i2":{"HostName":"tagged","DNSName":"tagged.ts.net.","OS":"linux","Online":true,"Tags":["tag:ingress"],"TailscaleIPs":["100.1.2.4"]},
          "i3":{"HostName":"bare","DNSName":"","OS":"","Online":true,"TailscaleIPs":["100.1.2.5"]},
          "ok":{"HostName":"real","DNSName":"real.ts.net.","OS":"linux","Online":true,"Tags":["tag:server"],"TailscaleIPs":["100.1.2.6"]},
          "ok2":{"HostName":"nodns","DNSName":"","OS":"linux","Online":true,"TailscaleIPs":["100.1.2.7"]}}}"#;
        let peers = parse_tailscale(json).unwrap();
        let targets: Vec<_> = peers.iter().map(|p| p.target.as_str()).collect();
        assert_eq!(targets, ["100.1.2.7", "real.ts.net"]);
    }

    #[test]
    fn tailscale_parse_edges() {
        assert_eq!(parse_tailscale(r#"{"Self":{}}"#).unwrap().len(), 0);
        assert!(parse_tailscale("x").is_err());
        let c = parse_tailscale(r#"{"Peer":{"a":{"HostName":"h","DNSName":"h.ts.net.","Online":true,"TailscaleIPs":["1.1.1.1"]}}}"#).unwrap();
        assert_eq!(c[0].os_hint, None, "no OS means unknown, still checkable");
    }

    #[test]
    fn os_hint_mapping() {
        assert_eq!(os_hint("linux"), Some(OsHint::Linux));
        assert_eq!(os_hint("Linux"), Some(OsHint::Linux));
        assert_eq!(os_hint("macOS"), Some(OsHint::MacOs));
        assert_eq!(os_hint("darwin"), Some(OsHint::MacOs));
        assert_eq!(os_hint("windows"), Some(OsHint::Windows));
        assert_eq!(os_hint("freebsd"), Some(OsHint::Other("freebsd".into())));
        assert_eq!(os_hint(""), None);
    }

    #[test]
    fn tailscale_source_failures() {
        let fake = FakeSys::new();
        assert_eq!(
            from_tailscale(&fake),
            Fetched::default(),
            "not installed: silent"
        );
        fake.on("tailscale", &["status", "--json"])
            .stdout(TAILSCALE);
        assert_eq!(from_tailscale(&fake).candidates.len(), 4);
        assert_eq!(fake.calls()[1].cmd.timeout, Some(Duration::from_secs(3)));

        let stopped = FakeSys::new();
        stopped
            .on("tailscale", &[])
            .code(1)
            .stderr("Tailscale is stopped.\n");
        assert!(
            from_tailscale(&stopped)
                .warning
                .unwrap()
                .contains("Tailscale is stopped.")
        );
        let slow = FakeSys::new();
        slow.on("tailscale", &[]).timed_out();
        assert!(from_tailscale(&slow).warning.unwrap().contains("3s"));
        let junk = FakeSys::new();
        junk.on("tailscale", &[]).stdout("<html>");
        assert!(
            from_tailscale(&junk)
                .warning
                .unwrap()
                .starts_with("tailscale: ")
        );
    }
}
