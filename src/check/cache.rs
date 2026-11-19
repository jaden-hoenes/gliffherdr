//! The on-disk cache of check results: one JSON file per machine under
//! `<state>/checks/<slug>.json`.
//!
//! ```json
//! {"version":1,"target":"j@h","status":{"kind":"ready"},"os":"Linux","outputs":["DP-1"],
//!  "server_version":"gliff-server 0.4.3","checked_at":1795058137}
//! ```

use std::io;

use serde::{Deserialize, Serialize};

use super::{CheckResult, Status};
use crate::paths::{Paths, write_atomic};
use crate::target::Target;

/// The only cache format version this build reads and writes.
pub const CACHE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Stored {
    version: u32,
    target: String,
    status: Status,
    #[serde(default)]
    os: Option<String>,
    #[serde(default)]
    outputs: Vec<String>,
    #[serde(default)]
    server_version: Option<String>,
    checked_at: u64,
}

/// The cached result for `target`, if there is a readable one of the current version.
pub fn load_cached(paths: &Paths, target: &Target) -> Option<CheckResult> {
    let text = std::fs::read_to_string(paths.check_file(target)).ok()?;
    parse(&text).filter(|r| r.target == target.as_str())
}

/// Parse cache file text; unknown version or malformed text is `None`.
pub fn parse(text: &str) -> Option<CheckResult> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    if value.get("version")?.as_u64()? != u64::from(CACHE_VERSION) {
        return None;
    }
    let s: Stored = serde_json::from_value(value).ok()?;
    Some(CheckResult {
        target: s.target,
        status: s.status,
        os: s.os,
        outputs: s.outputs,
        server_version: s.server_version,
        checked_at: s.checked_at,
    })
}

/// Write `result` to the cache file of `target`, atomically.
pub fn save(paths: &Paths, target: &Target, result: &CheckResult) -> io::Result<()> {
    let stored = Stored {
        version: CACHE_VERSION,
        target: result.target.clone(),
        status: result.status.clone(),
        os: result.os.clone(),
        outputs: result.outputs.clone(),
        server_version: result.server_version.clone(),
        checked_at: result.checked_at,
    };
    let json = serde_json::to_string(&stored).map_err(io::Error::other)?;
    write_atomic(&paths.check_file(target), json.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(status: Status) -> CheckResult {
        CheckResult {
            target: "j@h".into(),
            status,
            os: Some("Linux".into()),
            outputs: vec!["DP-1".into(), "HDMI-A-1".into()],
            server_version: Some("gliff-server 0.4.3".into()),
            checked_at: 1_795_058_137,
        }
    }

    #[test]
    fn exact_json_shape() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        let t = Target::parse("j@h").unwrap();
        save(&paths, &t, &result(Status::Ready)).unwrap();
        let text = std::fs::read_to_string(paths.check_file(&t)).unwrap();
        assert_eq!(
            text,
            r#"{"version":1,"target":"j@h","status":{"kind":"ready"},"os":"Linux","outputs":["DP-1","HDMI-A-1"],"server_version":"gliff-server 0.4.3","checked_at":1795058137}"#
        );
    }

    #[test]
    fn every_status_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        let t = Target::parse("j@h").unwrap();
        let all = [
            (Status::Ready, r#"{"kind":"ready"}"#),
            (Status::NoHyprland, r#"{"kind":"no_hyprland"}"#),
            (Status::NoServer, r#"{"kind":"no_server"}"#),
            (
                Status::Unsupported {
                    os: "Darwin".into(),
                },
                r#"{"kind":"unsupported","os":"Darwin"}"#,
            ),
            (
                Status::Offline { detail: "d".into() },
                r#"{"kind":"offline","detail":"d"}"#,
            ),
            (Status::Auth, r#"{"kind":"auth"}"#),
            (
                Status::HostKey { changed: true },
                r#"{"kind":"host_key","changed":true}"#,
            ),
            (
                Status::Error { detail: "e".into() },
                r#"{"kind":"error","detail":"e"}"#,
            ),
        ];
        for (status, json) in all {
            assert_eq!(serde_json::to_string(&status).unwrap(), json);
            let r = result(status);
            save(&paths, &t, &r).unwrap();
            assert_eq!(load_cached(&paths, &t), Some(r));
        }
    }

    #[test]
    fn missing_unknown_version_and_garbage_are_no_cache() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        let t = Target::parse("j@h").unwrap();
        assert_eq!(load_cached(&paths, &t), None);
        std::fs::create_dir_all(paths.checks_dir()).unwrap();
        for text in [
            "",
            "not json",
            "[]",
            r#"{"version":2,"target":"j@h","status":{"kind":"ready"},"checked_at":1}"#,
            r#"{"target":"j@h","status":{"kind":"ready"},"checked_at":1}"#,
            r#"{"version":1,"target":"j@h","status":{"kind":"mystery"},"checked_at":1}"#,
            r#"{"version":1,"target":"j@h","status":{"kind":"ready"}}"#,
            r#"{"version":1,"target":"someone-else","status":{"kind":"ready"},"checked_at":1}"#,
        ] {
            std::fs::write(paths.check_file(&t), text).unwrap();
            assert_eq!(load_cached(&paths, &t), None, "{text}");
        }
    }

    #[test]
    fn optional_fields_may_be_absent() {
        let r =
            parse(r#"{"version":1,"target":"x","status":{"kind":"auth"},"checked_at":5}"#).unwrap();
        assert_eq!((r.os, r.outputs, r.server_version), (None, vec![], None));
    }
}
