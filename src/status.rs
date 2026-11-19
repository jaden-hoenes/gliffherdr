//! What a machine's row shows: state, rank, glyph, tone and text. Shared by `list` and the
//! picker so both always agree.

use crate::check::{CheckResult, Status};
use crate::roster::{Machine, OsHint};

/// The braille spinner frames.
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The state of one row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowState {
    /// A gliff window for the machine is open.
    Open,
    /// The last check found the machine ready.
    Ready,
    /// A check is running.
    Checking,
    /// There is no check result.
    NotChecked,
    /// The machine answers but needs something done (the status says what).
    NeedsSetup(Status),
    /// The machine did not answer, or the check failed (the status says why).
    Unreachable(Status),
    /// The machine runs something gliff cannot connect to (the OS name).
    Unsupported(String),
}

/// How a row is coloured. The picker maps tones to the terminal's 16 ANSI colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Green.
    Good,
    /// Cyan.
    Accent,
    /// Yellow, in motion.
    Busy,
    /// Dim.
    Muted,
    /// Yellow.
    Warn,
    /// Red.
    Bad,
}

/// The row state of `m`. Precedence: open window, check running, machine known not to run
/// Linux, no result, then the cached status.
pub fn row_state(
    m: &Machine,
    cached: Option<&CheckResult>,
    checking: bool,
    open: bool,
) -> RowState {
    if open {
        return RowState::Open;
    }
    if checking {
        return RowState::Checking;
    }
    if !m.checkable() {
        let os = match &m.os_hint {
            Some(OsHint::MacOs) => "macOS".to_string(),
            Some(OsHint::Windows) => "Windows".to_string(),
            Some(OsHint::Other(s)) => s.clone(),
            Some(OsHint::Linux) | None => "an unknown system".to_string(),
        };
        return RowState::Unsupported(os);
    }
    let Some(r) = cached else {
        return RowState::NotChecked;
    };
    match &r.status {
        Status::Ready => RowState::Ready,
        Status::NoHyprland | Status::NoServer | Status::Auth | Status::HostKey { .. } => {
            RowState::NeedsSetup(r.status.clone())
        }
        Status::Offline { .. } | Status::Error { .. } => RowState::Unreachable(r.status.clone()),
        Status::Unsupported { os } => RowState::Unsupported(os.clone()),
    }
}

/// Sort rank: lower is listed first.
pub fn rank(s: &RowState) -> u8 {
    match s {
        RowState::Open => 0,
        RowState::Ready => 1,
        RowState::Checking => 2,
        RowState::NotChecked => 3,
        RowState::NeedsSetup(_) => 4,
        RowState::Unreachable(_) => 5,
        RowState::Unsupported(_) => 6,
    }
}

/// The glyph of a row; `spinner_frame` selects the frame of the busy spinner.
pub fn glyph(s: &RowState, spinner_frame: usize) -> &'static str {
    match s {
        RowState::Open => "▣",
        RowState::Ready => "●",
        RowState::Checking => SPINNER[spinner_frame % SPINNER.len()],
        RowState::NotChecked => "○",
        RowState::NeedsSetup(_) => "▲",
        RowState::Unreachable(_) => "✕",
        RowState::Unsupported(_) => "–",
    }
}

/// The tone of a row.
pub fn tone(s: &RowState) -> Tone {
    match s {
        RowState::Open => Tone::Accent,
        RowState::Ready => Tone::Good,
        RowState::Checking => Tone::Busy,
        RowState::NotChecked | RowState::Unsupported(_) => Tone::Muted,
        RowState::NeedsSetup(_) => Tone::Warn,
        RowState::Unreachable(_) => Tone::Bad,
    }
}

/// A friendly name for an operating system as `uname -s` or Tailscale spells it.
pub fn os_name(os: &str) -> String {
    let lower = os.to_ascii_lowercase();
    if lower == "darwin" || lower == "macos" {
        "macOS".to_string()
    } else if lower.starts_with("windows") {
        "Windows".to_string()
    } else if os.trim().is_empty() {
        "an unknown system".to_string()
    } else {
        os.to_string()
    }
}

/// The status column text. For an unreachable machine with an error this is the error detail,
/// which the caller truncates to its column.
pub fn short_text(s: &RowState) -> String {
    match s {
        RowState::Open => "window open".to_string(),
        RowState::Ready => "ready".to_string(),
        RowState::Checking => "checking…".to_string(),
        RowState::NotChecked => "not checked".to_string(),
        RowState::NeedsSetup(status) => match status {
            Status::NoHyprland => "Hyprland not running",
            Status::NoServer => "gliff-server missing",
            Status::Auth => "ssh key refused",
            Status::HostKey { changed: true } => "host key changed",
            Status::HostKey { changed: false } => "unknown host key",
            _ => "needs setup",
        }
        .to_string(),
        RowState::Unreachable(status) => match status {
            Status::Error { detail } => detail.clone(),
            _ => "offline".to_string(),
        },
        RowState::Unsupported(os) => format!("{}: needs Hyprland", os_name(os)),
    }
}

/// The host part of a target (`ssh://user@host:port` gives `host`), case preserved.
fn host_of(target: &str) -> &str {
    let rest = target
        .strip_prefix("ssh://")
        .unwrap_or(target)
        .trim_end_matches('/');
    let rest = rest.rsplit_once('@').map_or(rest, |(_, h)| h);
    match rest.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && p.parse::<u16>().is_ok() => h,
        _ => rest,
    }
}

/// The sentence for a status and what to do about it: `(message, hint)`. Hints name keys of the
/// picker (`ctrl-f`); the command line replaces that with `gliffherdr fix <target>`.
pub fn sentence(status: &Status, target: &str) -> (String, Option<String>) {
    let t = target;
    let (message, hint): (String, Option<String>) = match status {
        Status::NoHyprland => (
            format!("{t}: Hyprland is not running there."),
            Some("gliff shows a running Hyprland session; log in on that machine first.".into()),
        ),
        Status::NoServer => (
            format!("{t}: gliff-server is not installed there."),
            Some("Press ctrl-f to install it, or set server_bin in config.toml.".into()),
        ),
        Status::Unsupported { os } => (
            format!("{t} runs {}; gliff connects to Hyprland only.", os_name(os)),
            None,
        ),
        Status::Offline { detail } => (
            format!("{t} is not reachable: {}.", detail.trim_end_matches('.')),
            None,
        ),
        Status::Auth => (
            format!("{t} refused the ssh key."),
            Some(
                "gliff needs a key login without a prompt. Press ctrl-f to run ssh-copy-id.".into(),
            ),
        ),
        Status::HostKey { changed: false } => (
            format!("{t} has a host key ssh does not know."),
            Some("Press ctrl-f to check and accept it.".into()),
        ),
        Status::HostKey { changed: true } => (
            format!("{t}'s host key has changed."),
            Some(format!(
                "This can mean an attack. If the machine was reinstalled, remove the old key: ssh-keygen -R {}",
                host_of(t)
            )),
        ),
        Status::Error { detail } => (format!("{t}: {detail}"), None),
        Status::Ready => (format!("{t} is ready."), None),
    };
    (message, hint)
}

/// A [`sentence`] hint for the picker when the fix key's own line is shown next to it: the
/// `Press ctrl-f to …` sentence is dropped and whatever else the hint says stays (`Press ctrl-f
/// to install it, or set server_bin in config.toml.` becomes `Or set server_bin in config.toml.`).
/// `None` when nothing else is left.
#[must_use]
pub fn without_fix_sentence(hint: &str) -> Option<String> {
    const SENTENCE: &str = "Press ctrl-f to ";
    let Some(at) = hint.find(SENTENCE) else {
        return Some(hint.to_string());
    };
    let (before, rest) = (hint[..at].trim_end(), &hint[at + SENTENCE.len()..]);
    let after = if let Some((_, or)) = rest.split_once(", or ") {
        format!("Or {or}")
    } else {
        rest.split_once(". ")
            .map(|(_, tail)| tail.to_string())
            .unwrap_or_default()
    };
    let joined = [before, after.trim()]
        .iter()
        .filter(|p| !p.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    (!joined.is_empty()).then_some(joined)
}

/// Short machine-readable name of a status, as printed by `gliffherdr check`.
pub fn status_name(status: &Status) -> &'static str {
    match status {
        Status::Ready => "ready",
        Status::NoHyprland => "no-hyprland",
        Status::NoServer => "no-server",
        Status::Unsupported { .. } => "unsupported",
        Status::Offline { .. } => "offline",
        Status::Auth => "auth",
        Status::HostKey { .. } => "hostkey",
        Status::Error { .. } => "error",
    }
}

/// One-line detail of a status, printed after [`status_name`] by `gliffherdr check`.
pub fn status_detail(status: &Status) -> String {
    match status {
        Status::Ready => String::new(),
        Status::NoHyprland => "Hyprland is not running there".to_string(),
        Status::NoServer => "gliff-server is not installed there".to_string(),
        Status::Unsupported { os } => os.clone(),
        Status::Offline { detail } | Status::Error { detail } => detail.clone(),
        Status::Auth => "ssh key not accepted".to_string(),
        Status::HostKey { changed: false } => "host key unknown".to_string(),
        Status::HostKey { changed: true } => "host key changed".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::roster::bare_machine;
    use crate::sys::fake::FakeSys;
    use crate::target::Target;

    fn machine(hint: Option<OsHint>) -> Machine {
        let mut m = bare_machine(
            &FakeSys::new(),
            &Config::default(),
            Target::parse("j@h").unwrap(),
        );
        m.os_hint = hint;
        m
    }

    fn cached(status: Status) -> CheckResult {
        CheckResult {
            target: "j@h".into(),
            status,
            os: None,
            outputs: vec![],
            server_version: None,
            checked_at: 1,
        }
    }

    fn off() -> Status {
        Status::Offline {
            detail: "no route".into(),
        }
    }

    fn err() -> Status {
        Status::Error {
            detail: "boom".into(),
        }
    }

    #[test]
    fn row_state_from_status() {
        let m = machine(None);
        let cases = [
            (Status::Ready, RowState::Ready),
            (Status::NoHyprland, RowState::NeedsSetup(Status::NoHyprland)),
            (Status::NoServer, RowState::NeedsSetup(Status::NoServer)),
            (Status::Auth, RowState::NeedsSetup(Status::Auth)),
            (
                Status::HostKey { changed: true },
                RowState::NeedsSetup(Status::HostKey { changed: true }),
            ),
            (off(), RowState::Unreachable(off())),
            (err(), RowState::Unreachable(err())),
            (
                Status::Unsupported {
                    os: "Darwin".into(),
                },
                RowState::Unsupported("Darwin".into()),
            ),
        ];
        for (status, want) in cases {
            assert_eq!(
                row_state(&m, Some(&cached(status.clone())), false, false),
                want,
                "{status:?}"
            );
        }
        assert_eq!(row_state(&m, None, false, false), RowState::NotChecked);
    }

    #[test]
    fn row_state_precedence() {
        let ready = cached(Status::Ready);
        let mac = machine(Some(OsHint::MacOs));
        let linux = machine(Some(OsHint::Linux));
        assert_eq!(row_state(&linux, Some(&ready), true, true), RowState::Open);
        assert_eq!(row_state(&mac, Some(&ready), true, true), RowState::Open);
        assert_eq!(
            row_state(&linux, Some(&ready), true, false),
            RowState::Checking
        );
        assert_eq!(
            row_state(&mac, Some(&ready), true, false),
            RowState::Checking
        );
        assert_eq!(
            row_state(&mac, Some(&ready), false, false),
            RowState::Unsupported("macOS".into())
        );
        assert_eq!(
            row_state(&mac, None, false, false),
            RowState::Unsupported("macOS".into())
        );
        assert_eq!(
            row_state(&machine(Some(OsHint::Windows)), None, false, false),
            RowState::Unsupported("Windows".into())
        );
        assert_eq!(
            row_state(
                &machine(Some(OsHint::Other("freebsd".into()))),
                None,
                false,
                false
            ),
            RowState::Unsupported("freebsd".into())
        );
        assert_eq!(row_state(&linux, None, false, false), RowState::NotChecked);
    }

    #[test]
    fn the_row_table() {
        // (state, rank, glyph, tone, short text)
        let rows = [
            (RowState::Open, 0, "▣", Tone::Accent, "window open"),
            (RowState::Ready, 1, "●", Tone::Good, "ready"),
            (RowState::Checking, 2, "⠋", Tone::Busy, "checking…"),
            (RowState::NotChecked, 3, "○", Tone::Muted, "not checked"),
            (
                RowState::NeedsSetup(Status::NoHyprland),
                4,
                "▲",
                Tone::Warn,
                "Hyprland not running",
            ),
            (
                RowState::NeedsSetup(Status::NoServer),
                4,
                "▲",
                Tone::Warn,
                "gliff-server missing",
            ),
            (
                RowState::NeedsSetup(Status::Auth),
                4,
                "▲",
                Tone::Warn,
                "ssh key refused",
            ),
            (
                RowState::NeedsSetup(Status::HostKey { changed: false }),
                4,
                "▲",
                Tone::Warn,
                "unknown host key",
            ),
            (
                RowState::NeedsSetup(Status::HostKey { changed: true }),
                4,
                "▲",
                Tone::Warn,
                "host key changed",
            ),
            (RowState::Unreachable(off()), 5, "✕", Tone::Bad, "offline"),
            (RowState::Unreachable(err()), 5, "✕", Tone::Bad, "boom"),
            (
                RowState::Unsupported("Darwin".into()),
                6,
                "–",
                Tone::Muted,
                "macOS: needs Hyprland",
            ),
            (
                RowState::Unsupported("macOS".into()),
                6,
                "–",
                Tone::Muted,
                "macOS: needs Hyprland",
            ),
            (
                RowState::Unsupported("FreeBSD".into()),
                6,
                "–",
                Tone::Muted,
                "FreeBSD: needs Hyprland",
            ),
        ];
        for (state, r, g, t, text) in rows {
            assert_eq!(rank(&state), r, "{state:?}");
            assert_eq!(glyph(&state, 0), g, "{state:?}");
            assert_eq!(tone(&state), t, "{state:?}");
            assert_eq!(short_text(&state), text, "{state:?}");
        }
    }

    #[test]
    fn spinner_cycles_through_ten_frames() {
        let frames: Vec<_> = (0..10).map(|i| glyph(&RowState::Checking, i)).collect();
        assert_eq!(frames.concat(), "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏");
        assert_eq!(glyph(&RowState::Checking, 10), "⠋");
        assert_eq!(glyph(&RowState::Checking, 23), "⠸");
        assert_eq!(glyph(&RowState::Ready, 3), "●", "only checking spins");
    }

    #[test]
    fn os_names() {
        for (raw, want) in [
            ("Darwin", "macOS"),
            ("macOS", "macOS"),
            ("windows", "Windows"),
            ("Windows_NT", "Windows"),
            ("", "an unknown system"),
            ("FreeBSD", "FreeBSD"),
        ] {
            assert_eq!(os_name(raw), want);
        }
    }

    #[test]
    fn the_fix_sentence_is_dropped_and_the_rest_kept() {
        for (hint, want) in [
            (
                "Press ctrl-f to install it, or set server_bin in config.toml.",
                Some("Or set server_bin in config.toml."),
            ),
            (
                "gliff needs a key login without a prompt. Press ctrl-f to run ssh-copy-id.",
                Some("gliff needs a key login without a prompt."),
            ),
            ("Press ctrl-f to check and accept it.", None),
            ("Press ctrl-f to install it.", None),
            ("Press ctrl-f to a. Then b.", Some("Then b.")),
            (
                "gliff shows a running Hyprland session; log in on that machine first.",
                Some("gliff shows a running Hyprland session; log in on that machine first."),
            ),
        ] {
            assert_eq!(without_fix_sentence(hint).as_deref(), want, "{hint}");
        }
        // Every hint `sentence` produces for a status with a fix survives or empties sensibly.
        for status in [
            Status::NoServer,
            Status::Auth,
            Status::HostKey { changed: false },
        ] {
            let hint = sentence(&status, "j@h").1.unwrap();
            let short = without_fix_sentence(&hint).unwrap_or_default();
            assert!(!short.contains("ctrl-f"), "{short}");
        }
    }

    #[test]
    fn sentences() {
        let t = "j@h";
        let cases: Vec<(Status, &str, Option<&str>)> = vec![
            (
                Status::NoHyprland,
                "j@h: Hyprland is not running there.",
                Some("gliff shows a running Hyprland session; log in on that machine first."),
            ),
            (
                Status::NoServer,
                "j@h: gliff-server is not installed there.",
                Some("Press ctrl-f to install it, or set server_bin in config.toml."),
            ),
            (
                Status::Unsupported {
                    os: "Darwin".into(),
                },
                "j@h runs macOS; gliff connects to Hyprland only.",
                None,
            ),
            (off(), "j@h is not reachable: no route.", None),
            (
                Status::Offline {
                    detail: "Connection refused.".into(),
                },
                "j@h is not reachable: Connection refused.",
                None,
            ),
            (
                Status::Auth,
                "j@h refused the ssh key.",
                Some("gliff needs a key login without a prompt. Press ctrl-f to run ssh-copy-id."),
            ),
            (
                Status::HostKey { changed: false },
                "j@h has a host key ssh does not know.",
                Some("Press ctrl-f to check and accept it."),
            ),
            (
                Status::HostKey { changed: true },
                "j@h's host key has changed.",
                Some(
                    "This can mean an attack. If the machine was reinstalled, remove the old key: ssh-keygen -R h",
                ),
            ),
            (err(), "j@h: boom", None),
            (Status::Ready, "j@h is ready.", None),
        ];
        for (status, msg, hint) in cases {
            assert_eq!(
                sentence(&status, t),
                (msg.to_string(), hint.map(str::to_string)),
                "{status:?}"
            );
        }
    }

    #[test]
    fn host_key_hint_names_the_host_only() {
        for (target, host) in [
            ("j@Box.Lan", "Box.Lan"),
            ("ssh://u@box:2222", "box"),
            ("box", "box"),
            ("ssh://box/", "box"),
        ] {
            let (_, hint) = sentence(&Status::HostKey { changed: true }, target);
            assert!(
                hint.unwrap().ends_with(&format!("ssh-keygen -R {host}")),
                "{target}"
            );
        }
    }

    #[test]
    fn names_and_details_for_the_check_command() {
        let all = [
            (Status::Ready, "ready", ""),
            (
                Status::NoHyprland,
                "no-hyprland",
                "Hyprland is not running there",
            ),
            (
                Status::NoServer,
                "no-server",
                "gliff-server is not installed there",
            ),
            (
                Status::Unsupported {
                    os: "Darwin".into(),
                },
                "unsupported",
                "Darwin",
            ),
            (off(), "offline", "no route"),
            (Status::Auth, "auth", "ssh key not accepted"),
            (
                Status::HostKey { changed: false },
                "hostkey",
                "host key unknown",
            ),
            (
                Status::HostKey { changed: true },
                "hostkey",
                "host key changed",
            ),
            (err(), "error", "boom"),
        ];
        for (s, name, detail) in all {
            assert_eq!(status_name(&s), name);
            assert_eq!(status_detail(&s), detail);
        }
    }
}
