//! Turning a failed ssh run into a [`Status`]. Pure: no I/O.

use super::{Status, overall_timeout_secs};
use crate::sys::Output;

/// Classify an ssh run that did not produce a report. `ssh_timeout_secs` is the configured
/// `check.ssh_timeout_secs`; the overall limit the check ran under is that plus 10 seconds.
///
/// Rules, first match wins: timed out; changed host key; unknown host key; permission denied;
/// unresolvable name; transport failures (timed out, no route, unreachable, refused, closed,
/// reset); anything else is an error carrying ssh's last stderr line.
pub fn classify(out: &Output, ssh_timeout_secs: u64) -> Status {
    if out.timed_out {
        return Status::Offline {
            detail: format!(
                "no answer within {}s",
                overall_timeout_secs(ssh_timeout_secs)
            ),
        };
    }
    let err = &out.stderr;
    let last = last_line(err);
    if err.contains("REMOTE HOST IDENTIFICATION HAS CHANGED") {
        Status::HostKey { changed: true }
    } else if err.contains("Host key verification failed") {
        Status::HostKey { changed: false }
    } else if err.contains("Permission denied") {
        Status::Auth
    } else if err.contains("Could not resolve hostname") {
        Status::Offline {
            detail: "cannot resolve the host name".to_string(),
        }
    } else if [
        "timed out",
        "No route to host",
        "Network is unreachable",
        "Connection refused",
        "Connection closed",
        "Connection reset",
    ]
    .iter()
    .any(|p| err.contains(p))
    {
        Status::Offline { detail: last }
    } else if !last.is_empty() {
        Status::Error { detail: last }
    } else {
        Status::Error {
            detail: out.code.map_or_else(
                || "ssh was stopped by a signal".to_string(),
                |c| format!("ssh exited with status {c}"),
            ),
        }
    }
}

/// The last non-blank line of `s`, trimmed, without carriage returns.
fn last_line(s: &str) -> String {
    s.lines()
        .rev()
        .map(|l| l.replace('\r', "").trim().to_string())
        .find(|l| !l.is_empty())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed(code: i32, stderr: &str) -> Output {
        Output {
            code: Some(code),
            stderr: stderr.to_string(),
            ..Output::default()
        }
    }

    fn offline(d: &str) -> Status {
        Status::Offline {
            detail: d.to_string(),
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)] // one table row per classify rule
    fn every_row_of_the_table() {
        let timed_out = Output {
            timed_out: true,
            ..Output::default()
        };
        let cases: Vec<(&str, Output, Status)> = vec![
            ("timeout", timed_out, offline("no answer within 15s")),
            (
                "changed key",
                failed(
                    255,
                    "@@@\nWARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @\nHost key verification failed.\n",
                ),
                Status::HostKey { changed: true },
            ),
            (
                "unknown key",
                failed(255, "Host key verification failed.\n"),
                Status::HostKey { changed: false },
            ),
            (
                "auth",
                failed(255, "j@box: Permission denied (publickey).\n"),
                Status::Auth,
            ),
            (
                "dns",
                failed(
                    255,
                    "ssh: Could not resolve hostname nope: Name or service not known\n",
                ),
                offline("cannot resolve the host name"),
            ),
            (
                "ssh timeout",
                failed(
                    255,
                    "ssh: connect to host h port 22: Connection timed out\n",
                ),
                offline("ssh: connect to host h port 22: Connection timed out"),
            ),
            (
                "no route",
                failed(255, "ssh: connect to host off port 22: No route to host\n"),
                offline("ssh: connect to host off port 22: No route to host"),
            ),
            (
                "unreachable",
                failed(255, "connect to host x port 22: Network is unreachable"),
                offline("connect to host x port 22: Network is unreachable"),
            ),
            (
                "refused",
                failed(255, "ssh: connect to host h port 1: Connection refused\n"),
                offline("ssh: connect to host h port 1: Connection refused"),
            ),
            (
                "closed",
                failed(255, "Connection closed by 10.0.0.1 port 22\n"),
                offline("Connection closed by 10.0.0.1 port 22"),
            ),
            (
                "reset",
                failed(
                    255,
                    "kex_exchange_identification: read: Connection reset by peer\n",
                ),
                offline("kex_exchange_identification: read: Connection reset by peer"),
            ),
            (
                "other",
                failed(1, "something else broke\n"),
                Status::Error {
                    detail: "something else broke".into(),
                },
            ),
            (
                "other, last line wins",
                failed(1, "first\n\nsecond line\r\n\n"),
                Status::Error {
                    detail: "second line".into(),
                },
            ),
            (
                "silent failure",
                failed(1, ""),
                Status::Error {
                    detail: "ssh exited with status 1".into(),
                },
            ),
            (
                "blank failure",
                failed(7, " \n\n"),
                Status::Error {
                    detail: "ssh exited with status 7".into(),
                },
            ),
            (
                "signal",
                Output::default(),
                Status::Error {
                    detail: "ssh was stopped by a signal".into(),
                },
            ),
        ];
        for (name, out, want) in cases {
            assert_eq!(classify(&out, 5), want, "{name}");
        }
    }

    #[test]
    fn order_matters_changed_key_beats_verification_failed_and_auth() {
        let both = failed(
            255,
            "REMOTE HOST IDENTIFICATION HAS CHANGED\nHost key verification failed.\nPermission denied",
        );
        assert_eq!(classify(&both, 5), Status::HostKey { changed: true });
        let verify_and_denied = failed(255, "Host key verification failed.\nPermission denied");
        assert_eq!(
            classify(&verify_and_denied, 5),
            Status::HostKey { changed: false }
        );
    }

    #[test]
    fn timeout_detail_follows_the_configured_timeout() {
        let t = Output {
            timed_out: true,
            ..Output::default()
        };
        assert_eq!(classify(&t, 1), offline("no answer within 11s"));
        assert_eq!(classify(&t, 60), offline("no answer within 70s"));
    }
}
