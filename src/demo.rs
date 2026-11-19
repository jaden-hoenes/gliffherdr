//! Demo mode: seven fixture machines and scripted checks, for the README recording and the
//! snapshot tests. Nothing here touches the user's real configuration, state or windows: the
//! context is backed by [`FakeSys`] and its paths point into a directory created in the system
//! temp dir.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::check::{CheckEvent, CheckResult, Status};
use crate::config::Config;
use crate::connect::Ctx;
use crate::paths::Paths;
use crate::picker::app::Startup;
use crate::roster::{Machine, MachineSettings, OsHint, Source};
use crate::sys::Sys;
use crate::sys::fake::FakeSys;
use crate::target::{Resolved, Target};

/// The clock the snapshot tests use.
pub const NOW: u64 = 1_795_058_137;

/// How long the demo's connect waits between the progress states, in milliseconds.
pub const CONNECT_STEP_MS: u64 = 400;

/// One scripted check event, due `at_ms` after the script started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scripted {
    /// Milliseconds after the start of the script.
    pub at_ms: u64,
    /// What happens.
    pub event: CheckEvent,
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A demo context: `FakeSys` for the system and fresh temp directories for the paths. Call
/// [`cleanup`] when done.
pub fn ctx() -> Ctx {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("gliffherdr-demo-{}-{n}", std::process::id()));
    let fake = Arc::new(FakeSys::new());
    fake.set_now(NOW);
    let sys: Arc<dyn Sys> = fake;
    Ctx::new(sys, Paths::in_dir(&root), Config::default())
}

/// The directory a demo context created, so the caller can delete it.
pub fn root_of(ctx: &Ctx) -> PathBuf {
    ctx.paths
        .config_dir
        .parent()
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// Remove the temp directory of a demo context (best effort).
pub fn cleanup(ctx: &Ctx) {
    let root = root_of(ctx);
    if root.starts_with(std::env::temp_dir()) {
        let _ = std::fs::remove_dir_all(root);
    }
}

fn machine(
    label: &str,
    target: &str,
    user: &str,
    host: &str,
    sources: &[Source],
    os_hint: Option<OsHint>,
) -> Machine {
    Machine {
        target: Target::parse(target).expect("demo targets are valid"),
        label: Some(label.to_string()),
        sources: sources.to_vec(),
        os_hint,
        settings: MachineSettings::default(),
        resolved: Resolved {
            user: user.to_string(),
            host: host.to_string(),
            port: 22,
        },
    }
}

fn result(target: &str, status: Status, outputs: &[&str], now: u64) -> CheckResult {
    let ready_like = !matches!(status, Status::Offline { .. } | Status::Unsupported { .. });
    CheckResult {
        target: target.to_string(),
        os: ready_like.then(|| "Linux".to_string()),
        outputs: outputs.iter().map(ToString::to_string).collect(),
        server_version: matches!(status, Status::Ready).then(|| "gliff-server 0.4.3".to_string()),
        checked_at: now,
        status,
    }
}

/// The seven demo machines, in collection order.
pub fn machines() -> Vec<Machine> {
    vec![
        machine(
            "studio",
            "studio.lan",
            "me",
            "studio.lan",
            &[Source::Herdr, Source::Tailscale],
            None,
        ),
        machine("j", "j@10.0.0.2", "j", "10.0.0.2", &[Source::Herdr], None),
        machine(
            "render",
            "render@10.0.0.7",
            "render",
            "10.0.0.7",
            &[Source::Config],
            None,
        ),
        machine(
            "kiosk",
            "kiosk.tail.ts.net",
            "me",
            "kiosk.tail.ts.net",
            &[Source::Tailscale],
            Some(OsHint::Linux),
        ),
        machine(
            "lab",
            "lab@10.0.0.9",
            "lab",
            "10.0.0.9",
            &[Source::SshConfig],
            None,
        ),
        machine("nas", "nas.lan", "me", "nas.lan", &[Source::Recent], None),
        machine(
            "air",
            "air.tail.ts.net",
            "me",
            "air.tail.ts.net",
            &[Source::Tailscale],
            Some(OsHint::MacOs),
        ),
    ]
}

/// What the demo shows when it opens: the roster, the cached results (everything but `render`,
/// `kiosk`, `nas` and `air` has one, checked two minutes before `now`), the open window.
pub fn startup(now: u64) -> Startup {
    let ago = now.saturating_sub(120);
    let caches: HashMap<String, CheckResult> = [
        result("studio.lan", Status::Ready, &["DP-1", "DP-2"], ago),
        result("j@10.0.0.2", Status::Ready, &["HDMI-A-1"], ago),
        result("lab@10.0.0.9", Status::NoServer, &["DP-1"], ago),
    ]
    .into_iter()
    .map(|r| (r.target.clone(), r))
    .collect();
    Startup {
        config: Config::default(),
        machines: machines(),
        caches,
        open: HashSet::from(["studio.lan".to_string()]),
        banner: None,
        unreadable_sources: 0,
        in_herdr: false,
    }
}

fn finished(at_ms: u64, r: CheckResult) -> Scripted {
    Scripted {
        at_ms,
        event: CheckEvent::Finished(r),
    }
}

/// The checks that settle after the demo opens: `kiosk` after 0.8 s, `render` after 1.2 s,
/// `nas` after 2.0 s.
pub fn script(now: u64) -> Vec<Scripted> {
    vec![
        finished(
            800,
            result("kiosk.tail.ts.net", Status::NoHyprland, &[], now),
        ),
        finished(
            1200,
            result("render@10.0.0.7", Status::Ready, &["DP-1", "DP-2"], now),
        ),
        finished(
            2000,
            result(
                "nas.lan",
                Status::Offline {
                    detail: "no answer within 15s".to_string(),
                },
                &[],
                now,
            ),
        ),
    ]
}

/// What `Ctrl+R` shows in the demo: every checkable machine answers again, one after the other,
/// with the same results it has at the end of [`script`].
pub fn recheck_script(now: u64) -> Vec<Scripted> {
    vec![
        finished(
            300,
            result("studio.lan", Status::Ready, &["DP-1", "DP-2"], now),
        ),
        finished(500, result("j@10.0.0.2", Status::Ready, &["HDMI-A-1"], now)),
        finished(
            800,
            result("kiosk.tail.ts.net", Status::NoHyprland, &[], now),
        ),
        finished(
            1000,
            result("render@10.0.0.7", Status::Ready, &["DP-1", "DP-2"], now),
        ),
        finished(
            1200,
            result("lab@10.0.0.9", Status::NoServer, &["DP-1"], now),
        ),
        finished(
            1600,
            result(
                "nas.lan",
                Status::Offline {
                    detail: "no answer within 15s".to_string(),
                },
                &[],
                now,
            ),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seven_machines_with_the_documented_sources() {
        let m = machines();
        assert_eq!(m.len(), 7);
        let line = |m: &Machine| {
            format!(
                "{} {} {}",
                m.display_label(),
                m.target,
                m.sources
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("+")
            )
        };
        assert_eq!(
            m.iter().map(line).collect::<Vec<_>>(),
            [
                "studio studio.lan herdr+tailscale",
                "j j@10.0.0.2 herdr",
                "render render@10.0.0.7 config",
                "kiosk kiosk.tail.ts.net tailscale",
                "lab lab@10.0.0.9 ssh",
                "nas nas.lan recent",
                "air air.tail.ts.net tailscale",
            ]
        );
    }

    #[test]
    fn the_script_settles_every_checking_machine_at_its_time() {
        let s = script(NOW);
        let at = |t: &str| {
            s.iter()
                .find_map(|e| match &e.event {
                    CheckEvent::Finished(r) if r.target == t => Some(e.at_ms),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(at("render@10.0.0.7"), 1200);
        assert_eq!(at("kiosk.tail.ts.net"), 800);
        assert_eq!(at("nas.lan"), 2000);
    }

    #[test]
    fn the_demo_context_is_isolated_and_cleaned_up() {
        let c = ctx();
        let root = root_of(&c);
        assert!(root.starts_with(std::env::temp_dir()));
        std::fs::create_dir_all(&root).unwrap();
        cleanup(&c);
        assert!(!root.exists());
    }
}
