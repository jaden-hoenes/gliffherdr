//! Hyprland: recovering the graphical environment, listing gliff windows and focusing one.
//!
//! A herdr server started over ssh has no Wayland session in its environment, so
//! [`graphical_env`] borrows it from the systemd user manager or from the running Hyprland
//! instance. Every `hyprctl` call carries that environment ([`Hypr`]).

use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::time::Duration;

use serde::Deserialize;
use thiserror::Error;

use crate::sys::{Cmd, Sys, cmd};
use crate::target::{Target, resolve};

/// The window class gliff sets (contract G2).
pub const GLIFF_CLASS: &str = "gliff";
/// What separates the machine from `Gliff` in a gliff window title (contract G3).
pub const TITLE_SEP: &str = " — ";
/// How long one `hyprctl` or `systemctl` call may take.
const TIMEOUT: Duration = Duration::from_secs(3);
/// The variables borrowed from the systemd user manager when the process has no Wayland display.
const SESSION_VARS: [&str; 5] = [
    "WAYLAND_DISPLAY",
    "HYPRLAND_INSTANCE_SIGNATURE",
    "DISPLAY",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_TYPE",
];

/// A reachable graphical session: the environment its programs need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Graphical {
    /// Variables to add to every command that talks to the desktop (`XDG_RUNTIME_DIR`,
    /// `WAYLAND_DISPLAY`, and those of `HYPRLAND_INSTANCE_SIGNATURE`, `DISPLAY`,
    /// `XDG_CURRENT_DESKTOP`, `XDG_SESSION_TYPE` that are known).
    pub env: Vec<(String, String)>,
    /// The Hyprland instance signature, when known; unknown means `hyprctl -i 0`.
    pub instance: Option<String>,
}

impl Graphical {
    /// The value of one of the session variables.
    #[must_use]
    pub fn var(&self, key: &str) -> Option<&str> {
        self.env
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// The uid of this process, read from the owner of `/proc/self` (no `libc` call needed).
fn current_uid() -> Option<u32> {
    std::fs::metadata("/proc/self").ok().map(|m| m.uid())
}

/// `KEY=value` lines of `systemctl --user show-environment` for [`SESSION_VARS`]; values that
/// systemd quotes as `$'…'` are skipped.
fn parse_session_env(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (k, v) = line.split_once('=')?;
            let v = v.trim();
            (SESSION_VARS.contains(&k) && !v.is_empty() && !v.starts_with("$'"))
                .then(|| (k.to_string(), v.to_string()))
        })
        .collect()
}

/// The first instance of `hyprctl instances`: its signature and Wayland socket name.
fn parse_instances(text: &str) -> (Option<String>, Option<String>) {
    let mut sig = None;
    let mut wl = None;
    for line in text.lines() {
        let line = line.trim();
        if sig.is_none() {
            if let Some(rest) = line.strip_prefix("instance ") {
                sig = Some(rest.trim_end_matches(':').trim().to_string()).filter(|s| !s.is_empty());
            }
        } else if wl.is_none() {
            if let Some(rest) = line.strip_prefix("wl socket:") {
                wl = Some(rest.trim().to_string()).filter(|s| !s.is_empty());
            }
        } else {
            break;
        }
    }
    (sig, wl)
}

/// Find the graphical session to open windows in, or `None` when there is none.
///
/// Starts from the process environment; `XDG_RUNTIME_DIR` defaults to `/run/user/<uid>`. When
/// `WAYLAND_DISPLAY` is unset it reads the systemd user environment, then `hyprctl instances`.
pub fn graphical_env(sys: &dyn Sys) -> Option<Graphical> {
    let set = |k: &str| sys.env(k).filter(|v| !v.is_empty());
    let runtime_dir =
        set("XDG_RUNTIME_DIR").or_else(|| current_uid().map(|u| format!("/run/user/{u}")));
    let mut vars: Vec<(String, String)> = SESSION_VARS
        .iter()
        .filter_map(|k| set(k).map(|v| ((*k).to_string(), v)))
        .collect();
    let put = |vars: &mut Vec<(String, String)>, k: &str, v: String| {
        vars.retain(|(ek, _)| ek != k);
        vars.push((k.to_string(), v));
    };
    let with_runtime = |c: Cmd| match &runtime_dir {
        Some(d) => c.with_env("XDG_RUNTIME_DIR", d),
        None => c,
    };
    let known = |vars: &[(String, String)], k: &str| vars.iter().any(|(ek, _)| ek == k);

    if !known(&vars, "WAYLAND_DISPLAY") {
        let c =
            with_runtime(cmd("systemctl", &["--user", "show-environment"]).with_timeout(TIMEOUT));
        if let Ok(out) = sys.run(&c)
            && out.success()
        {
            for (k, v) in parse_session_env(&out.stdout) {
                put(&mut vars, &k, v);
            }
        }
    }
    if !known(&vars, "WAYLAND_DISPLAY") {
        let c = with_runtime(cmd("hyprctl", &["instances"]).with_timeout(TIMEOUT));
        if let Ok(out) = sys.run(&c)
            && out.success()
        {
            let (sig, wl) = parse_instances(&out.stdout);
            if let Some(sig) = sig {
                put(&mut vars, "HYPRLAND_INSTANCE_SIGNATURE", sig);
            }
            if let Some(wl) = wl {
                put(&mut vars, "WAYLAND_DISPLAY", wl);
            }
        }
    }
    if !known(&vars, "WAYLAND_DISPLAY") {
        return None;
    }
    let instance = vars
        .iter()
        .find(|(k, _)| k == "HYPRLAND_INSTANCE_SIGNATURE")
        .map(|(_, v)| v.clone());
    let mut env = Vec::new();
    if let Some(d) = runtime_dir {
        env.push(("XDG_RUNTIME_DIR".to_string(), d));
    }
    env.extend(vars);
    Some(Graphical { env, instance })
}

/// The targets of every open gliff window (see [`Hypr::open_targets`]), or an empty set when
/// there is no Hyprland session to ask. Cheap enough to call every couple of seconds (the picker
/// does).
pub fn open_targets(sys: &dyn Sys) -> HashSet<String> {
    graphical_env(sys).map_or_else(HashSet::new, |g| Hypr::new(sys, &g).open_targets())
}

/// The target a gliff window title names: the text before ` — `, when there is any.
fn title_target(title: &str) -> Option<&str> {
    title
        .split_once(TITLE_SEP)
        .map(|(t, _)| t)
        .filter(|t| !t.is_empty())
}

/// One window of `hyprctl clients -j`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct Client {
    /// Hyprland's window address (`0x…`).
    #[serde(default)]
    pub address: String,
    /// The window class.
    #[serde(default)]
    pub class: String,
    /// The window title.
    #[serde(default)]
    pub title: String,
    /// The process id.
    #[serde(default)]
    pub pid: i64,
}

/// Why a window could not be focused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HyprError {
    /// The address is not `0x` followed by lowercase hex digits.
    #[error("not a window address: {0:?}")]
    BadAddress(String),
    /// Hyprland refused both dispatch forms; the text is what each said.
    #[error("Hyprland would not focus the window: {0}")]
    Focus(String),
}

/// Whether `address` is `0x` followed by one or more lowercase hex digits.
fn valid_address(address: &str) -> bool {
    address.strip_prefix("0x").is_some_and(|h| {
        !h.is_empty()
            && h.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// `hyprctl` against one graphical session.
#[derive(Clone, Copy)]
pub struct Hypr<'a> {
    sys: &'a dyn Sys,
    g: &'a Graphical,
}

impl<'a> Hypr<'a> {
    /// A handle on the session `g`.
    #[must_use]
    pub fn new(sys: &'a dyn Sys, g: &'a Graphical) -> Self {
        Hypr { sys, g }
    }

    /// `hyprctl [-i 0] <args>` with the session's environment and a 3 s timeout.
    fn hyprctl(&self, args: &[&str]) -> Cmd {
        let mut all: Vec<&str> = Vec::new();
        if self.g.instance.is_none() {
            all.extend(["-i", "0"]);
        }
        all.extend(args);
        cmd("hyprctl", &all)
            .with_envs(&self.g.env)
            .with_timeout(TIMEOUT)
    }

    /// Every window (`hyprctl clients -j`); empty on any failure.
    #[must_use]
    pub fn clients(&self) -> Vec<Client> {
        match self.sys.run(&self.hyprctl(&["clients", "-j"])) {
            Ok(out) if out.success() => serde_json::from_str(&out.stdout).unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// The gliff window showing `target` (class `gliff`, title `<target> — …`), if one is open.
    ///
    /// A window whose title spells the target exactly wins. Otherwise a gliff window whose title
    /// target resolves (`ssh -G`) to the same `user@host:port` counts too, so `j` and
    /// `j@100.64.0.2` find one window. Only gliff windows that did not match exactly are
    /// resolved (usually none, one or two).
    #[must_use]
    pub fn find_window(&self, target: &str) -> Option<Client> {
        let prefix = format!("{target}{TITLE_SEP}");
        let gliff: Vec<Client> = self
            .clients()
            .into_iter()
            .filter(|c| c.class == GLIFF_CLASS)
            .collect();
        if let Some(c) = gliff.iter().find(|c| c.title.starts_with(&prefix)) {
            return Some(c.clone());
        }
        let wanted = Target::parse(target)
            .ok()
            .map(|t| resolve(self.sys, &t).full_key())?;
        gliff.into_iter().find(|c| {
            title_target(&c.title)
                .and_then(|t| Target::parse(t).ok())
                .is_some_and(|t| resolve(self.sys, &t).full_key() == wanted)
        })
    }

    /// The targets of every open gliff window: the title before ` — `, plus the resolved
    /// `user@host:port` of each, so a machine is open whichever way its target is spelled
    /// (see [`Machine::is_open`](crate::roster::Machine::is_open)).
    #[must_use]
    pub fn open_targets(&self) -> HashSet<String> {
        let mut open = HashSet::new();
        for c in self
            .clients()
            .into_iter()
            .filter(|c| c.class == GLIFF_CLASS)
        {
            let Some(title) = title_target(&c.title) else {
                continue;
            };
            open.insert(title.to_string());
            if let Ok(t) = Target::parse(title) {
                open.insert(resolve(self.sys, &t).full_key());
            }
        }
        open
    }

    /// Focus a window: the Lua dispatcher of Hyprland 0.56 first, then the legacy
    /// `focuswindow` dispatcher; each succeeds when it prints `ok`.
    pub fn focus(&self, address: &str) -> Result<(), HyprError> {
        if !valid_address(address) {
            return Err(HyprError::BadAddress(address.to_string()));
        }
        let lua = format!("hl.dsp.focus({{ window = \"address:{address}\" }})");
        let legacy = format!("address:{address}");
        let attempts: [&[&str]; 2] = [&["dispatch", &lua], &["dispatch", "focuswindow", &legacy]];
        let mut said = Vec::new();
        for args in attempts {
            match self.sys.run(&self.hyprctl(args)) {
                Ok(out) if out.stdout.trim() == "ok" => return Ok(()),
                Ok(out) => said.push(first_line(&out.stdout, &out.stderr)),
                Err(e) => said.push(e.to_string()),
            }
        }
        Err(HyprError::Focus(said.join("; ")))
    }

    /// The Hyprland version (`hyprctl version -j`), or `None` when it does not answer.
    #[must_use]
    pub fn version(&self) -> Option<String> {
        let out = self.sys.run(&self.hyprctl(&["version", "-j"])).ok()?;
        if !out.success() {
            return None;
        }
        let v: serde_json::Value = serde_json::from_str(&out.stdout).ok()?;
        v.get("version")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    }
}

/// The first non-empty line of what a command printed, for error messages.
fn first_line(stdout: &str, stderr: &str) -> String {
    stdout
        .lines()
        .chain(stderr.lines())
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("no answer")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::fake::FakeSys;

    fn graphical(instance: Option<&str>) -> Graphical {
        let mut env = vec![("WAYLAND_DISPLAY".to_string(), "wayland-1".to_string())];
        if let Some(i) = instance {
            env.push(("HYPRLAND_INSTANCE_SIGNATURE".into(), i.into()));
        }
        Graphical {
            env,
            instance: instance.map(str::to_string),
        }
    }

    const CLIENTS: &str = r#"[
      {"address":"0x1","class":"steam","title":"Steam","pid":3},
      {"address":"0xabc","class":"gliff","title":"j@10.0.0.2 — Gliff","pid":4},
      {"address":"0xdef","class":"gliff","title":"j@10.0.0.22 — Gliff","pid":5},
      {"address":"0x2","class":"firefox","title":"j@10.0.0.9 — Gliff","pid":6},
      {"address":"0x3","class":"gliff","title":"Gliff","pid":7}]"#;

    #[test]
    fn graphical_env_from_the_process_environment() {
        let fake = FakeSys::new();
        fake.set_env("WAYLAND_DISPLAY", "wayland-1");
        fake.set_env("HYPRLAND_INSTANCE_SIGNATURE", "sig1");
        fake.set_env("XDG_RUNTIME_DIR", "/run/user/7");
        let g = graphical_env(&fake).unwrap();
        assert_eq!(g.instance.as_deref(), Some("sig1"));
        assert_eq!(g.var("XDG_RUNTIME_DIR"), Some("/run/user/7"));
        assert_eq!(g.var("WAYLAND_DISPLAY"), Some("wayland-1"));
        assert_eq!(fake.call_lines().len(), 0, "nothing needs recovering");
    }

    #[test]
    fn runtime_dir_defaults_to_run_user_uid_from_proc_self() {
        let fake = FakeSys::new();
        fake.set_env("WAYLAND_DISPLAY", "wayland-1");
        let g = graphical_env(&fake).unwrap();
        let uid = std::fs::metadata("/proc/self").unwrap().uid();
        assert_eq!(
            g.var("XDG_RUNTIME_DIR").map(str::to_string),
            Some(format!("/run/user/{uid}"))
        );
        assert_eq!(g.instance, None, "no signature known: hyprctl -i 0");
    }

    #[test]
    fn graphical_env_from_systemd() {
        let fake = FakeSys::new();
        fake.set_env("XDG_RUNTIME_DIR", "/run/user/7");
        fake.on("systemctl", &["--user", "show-environment"]).stdout(
            "HOME=/x\nWAYLAND_DISPLAY=wayland-9\nHYPRLAND_INSTANCE_SIGNATURE=fromsystemd\nDISPLAY=:0\n\
             HYPRLAND_CMD=$'Hyprland --watchdog-fd 4'\nXDG_CURRENT_DESKTOP=$'Hyprland'\nXDG_SESSION_TYPE=wayland\n",
        );
        let g = graphical_env(&fake).unwrap();
        assert_eq!(g.var("WAYLAND_DISPLAY"), Some("wayland-9"));
        assert_eq!(g.instance.as_deref(), Some("fromsystemd"));
        assert_eq!(g.var("DISPLAY"), Some(":0"));
        assert_eq!(g.var("XDG_SESSION_TYPE"), Some("wayland"));
        assert_eq!(
            g.var("XDG_CURRENT_DESKTOP"),
            None,
            "$'…' values are skipped"
        );
        assert_eq!(g.var("HOME"), None, "only the session variables are taken");
        let c = &fake.calls_to("systemctl")[0];
        assert!(
            c.env
                .contains(&("XDG_RUNTIME_DIR".into(), "/run/user/7".into()))
        );
    }

    #[test]
    fn systemd_overrides_a_stale_signature_but_a_known_display_skips_systemd() {
        let fake = FakeSys::new();
        fake.set_env("HYPRLAND_INSTANCE_SIGNATURE", "stale");
        fake.on("systemctl", &[])
            .stdout("WAYLAND_DISPLAY=wayland-2\nHYPRLAND_INSTANCE_SIGNATURE=fresh\n");
        assert_eq!(
            graphical_env(&fake).unwrap().instance.as_deref(),
            Some("fresh")
        );
    }

    #[test]
    fn graphical_env_from_hyprctl_instances() {
        let fake = FakeSys::new();
        fake.on("systemctl", &[]).stdout("HOME=/x\n");
        fake.on("hyprctl", &["instances"]).stdout(
            "instance abc_123:\n\ttime: 1\n\tpid: 2\n\twl socket: wayland-7\n\ninstance zzz_9:\n\twl socket: wayland-8\n",
        );
        let g = graphical_env(&fake).unwrap();
        assert_eq!(g.var("WAYLAND_DISPLAY"), Some("wayland-7"));
        assert_eq!(g.instance.as_deref(), Some("abc_123"));
    }

    #[test]
    fn graphical_env_none_when_nothing_knows_a_display() {
        let fake = FakeSys::new();
        assert_eq!(graphical_env(&fake), None);
        fake.on("systemctl", &[]).code(1);
        fake.on("hyprctl", &["instances"]).stdout("no instances\n");
        assert_eq!(graphical_env(&fake), None);
    }

    #[test]
    fn instances_parsing() {
        assert_eq!(
            parse_instances("instance a:\n\twl socket: w1\n"),
            (Some("a".into()), Some("w1".into()))
        );
        assert_eq!(parse_instances(""), (None, None));
        assert_eq!(parse_instances("instance a:\n"), (Some("a".into()), None));
    }

    #[test]
    fn finds_gliff_windows_by_title_prefix_and_class() {
        let fake = FakeSys::new();
        fake.on("hyprctl", &["clients", "-j"]).stdout(CLIENTS);
        let g = graphical(Some("sig"));
        let h = Hypr::new(&fake, &g);
        assert_eq!(h.clients().len(), 5);
        assert_eq!(h.find_window("j@10.0.0.2").unwrap().address, "0xabc");
        assert_eq!(h.find_window("j@10.0.0.22").unwrap().address, "0xdef");
        assert_eq!(h.find_window("j@10.0.0.9"), None, "wrong class");
        assert_eq!(
            h.find_window("j@10.0.0"),
            None,
            "prefix needs the separator"
        );
        let open = h.open_targets();
        let mut open: Vec<_> = open.iter().map(String::as_str).collect();
        open.sort_unstable();
        assert_eq!(
            open,
            [
                "j@10.0.0.2",
                "j@10.0.0.22",
                "j@10.0.0.22:22",
                "j@10.0.0.2:22"
            ],
            "every title target and what it resolves to"
        );
        let call = &fake.calls_to("hyprctl")[0];
        assert!(
            call.env
                .contains(&("HYPRLAND_INSTANCE_SIGNATURE".into(), "sig".into()))
        );
    }

    #[test]
    fn a_window_is_found_under_any_spelling_that_resolves_to_the_same_machine() {
        let fake = FakeSys::new();
        let clients = r#"[
          {"address":"0xa","class":"gliff","title":"alias — Gliff","pid":1},
          {"address":"0xb","class":"gliff","title":"k@10.0.0.7 — Gliff","pid":2},
          {"address":"0xc","class":"firefox","title":"other — Gliff","pid":3}]"#;
        fake.on("hyprctl", &["clients", "-j"]).stdout(clients);
        fake.on("ssh", &["-G", "--", "alias"])
            .stdout("user j\nhostname 10.0.0.2\nport 22\n");
        fake.on("ssh", &["-G", "--", "j@10.0.0.2"])
            .stdout("user j\nhostname 10.0.0.2\nport 22\n");
        fake.on("ssh", &["-G", "--", "other"])
            .stdout("user j\nhostname 10.0.0.2\nport 22\n");
        let g = graphical(Some("sig"));
        let h = Hypr::new(&fake, &g);
        assert_eq!(h.find_window("j@10.0.0.2").unwrap().address, "0xa");
        assert_eq!(
            h.find_window("k@10.0.0.7").unwrap().address,
            "0xb",
            "an exact title wins"
        );
        assert_eq!(h.find_window("j@10.0.0.3"), None, "another machine");
        assert_eq!(h.find_window("a b"), None, "not a target");
        let exact = FakeSys::new();
        exact.on("hyprctl", &["clients", "-j"]).stdout(clients);
        let _ = Hypr::new(&exact, &g).find_window("k@10.0.0.7");
        assert_eq!(
            exact.calls_to("ssh").len(),
            0,
            "an exact match resolves nothing"
        );
        let open = h.open_targets();
        assert!(
            open.contains("alias") && open.contains("j@10.0.0.2:22"),
            "{open:?}"
        );
    }

    #[test]
    fn the_free_function_asks_the_recovered_session() {
        let fake = FakeSys::new();
        assert_eq!(
            open_targets(&fake).len(),
            0,
            "no session: empty, no hyprctl clients call"
        );
        fake.set_env("WAYLAND_DISPLAY", "wayland-1");
        fake.set_env("HYPRLAND_INSTANCE_SIGNATURE", "s");
        fake.on("hyprctl", &["clients", "-j"]).stdout(CLIENTS);
        assert_eq!(
            open_targets(&fake).len(),
            4,
            "two windows, each as written and resolved"
        );
    }

    #[test]
    fn clients_is_empty_on_any_failure() {
        let g = graphical(Some("sig"));
        let none = FakeSys::new();
        assert_eq!(Hypr::new(&none, &g).clients(), vec![]);
        let bad = FakeSys::new();
        bad.on("hyprctl", &[]).stdout("not json");
        assert_eq!(Hypr::new(&bad, &g).clients(), vec![]);
        let failed = FakeSys::new();
        failed.on("hyprctl", &[]).code(1).stdout("[]");
        assert_eq!(Hypr::new(&failed, &g).find_window("x"), None);
        let slow = FakeSys::new();
        slow.on("hyprctl", &[]).timed_out();
        assert_eq!(Hypr::new(&slow, &g).open_targets().len(), 0);
    }

    #[test]
    fn without_a_signature_hyprctl_gets_instance_zero() {
        let fake = FakeSys::new();
        fake.on("hyprctl", &["-i", "0", "clients", "-j"])
            .stdout("[]");
        let g = graphical(None);
        let _ = Hypr::new(&fake, &g).clients();
        assert_eq!(fake.call_lines(), ["hyprctl -i 0 clients -j"]);
    }

    #[test]
    fn focus_lua_ok() {
        let fake = FakeSys::new();
        fake.on("hyprctl", &["dispatch"]).stdout("ok\n");
        let g = graphical(Some("s"));
        assert_eq!(Hypr::new(&fake, &g).focus("0xabc"), Ok(()));
        assert_eq!(
            fake.call_lines(),
            [r#"hyprctl dispatch hl.dsp.focus({ window = "address:0xabc" })"#]
        );
    }

    #[test]
    fn focus_lua_error_then_legacy_ok() {
        let fake = FakeSys::new();
        fake.on("hyprctl", &["dispatch"])
            .stdout("Error: nope")
            .code(7);
        fake.on("hyprctl", &["dispatch", "focuswindow"])
            .stdout("ok");
        let g = graphical(Some("s"));
        assert_eq!(Hypr::new(&fake, &g).focus("0xabc"), Ok(()));
        assert_eq!(
            fake.call_lines()[1],
            "hyprctl dispatch focuswindow address:0xabc"
        );
    }

    #[test]
    fn focus_fails_when_both_forms_fail() {
        let fake = FakeSys::new();
        fake.on("hyprctl", &["dispatch"])
            .stdout("Error: nope")
            .code(7);
        let g = graphical(Some("s"));
        let e = Hypr::new(&fake, &g).focus("0xabc").unwrap_err();
        assert_eq!(e, HyprError::Focus("Error: nope; Error: nope".into()));
        let missing = FakeSys::new();
        assert!(matches!(
            Hypr::new(&missing, &g).focus("0xabc"),
            Err(HyprError::Focus(_))
        ));
    }

    #[test]
    fn addresses_are_validated_before_any_call() {
        let fake = FakeSys::new();
        let g = graphical(Some("s"));
        for bad in [
            "",
            "0x",
            "abc",
            "0xABC",
            "0xabc; reboot",
            "0x1 2",
            "0xnew",
            "-1",
        ] {
            assert_eq!(
                Hypr::new(&fake, &g).focus(bad),
                Err(HyprError::BadAddress(bad.to_string())),
                "{bad:?}"
            );
        }
        assert_eq!(fake.calls().len(), 0);
        assert!(valid_address("0x560d53f176d0"));
    }

    #[test]
    fn version_reads_hyprctl_version_json() {
        let fake = FakeSys::new();
        fake.on("hyprctl", &["version", "-j"])
            .stdout(r#"{"branch":"v0.56.2","version":"0.56.2"}"#);
        let g = graphical(Some("s"));
        assert_eq!(Hypr::new(&fake, &g).version().as_deref(), Some("0.56.2"));
        let none = FakeSys::new();
        assert_eq!(Hypr::new(&none, &g).version(), None);
    }
}
