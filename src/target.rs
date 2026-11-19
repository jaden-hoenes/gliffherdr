//! Validated ssh destinations.
//!
//! A [`Target`] is the only way a destination reaches ssh: it cannot start with `-` (which ssh
//! would read as an option) and contains no whitespace or shell syntax. Targets are still always
//! passed after `--`.

use std::fmt;
use std::time::Duration;

use thiserror::Error;

use crate::sys::{Sys, cmd};

/// Longest accepted target, in bytes.
const MAX_LEN: usize = 255;
/// How long `ssh -G` may take while resolving a target.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(3);

/// Why a string is not a valid [`Target`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TargetError {
    /// Nothing but whitespace.
    #[error("the ssh destination is empty")]
    Empty,
    /// More than 255 bytes.
    #[error("the ssh destination is longer than 255 characters")]
    TooLong,
    /// A character ssh destinations may not start with or contain here.
    #[error("{0:?} is not allowed in an ssh destination")]
    InvalidChar(char),
}

/// A validated ssh destination: `host`, `user@host`, an `~/.ssh/config` alias or
/// `ssh://user@host:port`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Target(String);

impl Target {
    /// Trim surrounding whitespace and validate: 1 to 255 bytes, the first character in
    /// `[A-Za-z0-9_]`, every other in `[A-Za-z0-9._@%:/+-]`.
    pub fn parse(s: &str) -> Result<Target, TargetError> {
        let s = s.trim();
        if s.is_empty() {
            return Err(TargetError::Empty);
        }
        if s.len() > MAX_LEN {
            return Err(TargetError::TooLong);
        }
        for (i, c) in s.chars().enumerate() {
            let ok = if i == 0 {
                c.is_ascii_alphanumeric() || c == '_'
            } else {
                c.is_ascii_alphanumeric() || "._@%:/+-".contains(c)
            };
            if !ok {
                return Err(TargetError::InvalidChar(c));
            }
        }
        Ok(Target(s.to_string()))
    }

    /// The validated text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for Target {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// What ssh will actually connect to for a [`Target`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Resolved {
    /// The login user, or `-` when unknown.
    pub user: String,
    /// The host name or address, lowercased.
    pub host: String,
    /// The port.
    pub port: u16,
}

impl Resolved {
    /// `user@host:port`, the key two spellings of one machine share.
    #[must_use]
    pub fn full_key(&self) -> String {
        format!("{}@{}:{}", self.user, self.host, self.port)
    }
}

/// Resolve `t` as ssh would (`ssh -G -- <target>`, 3 s timeout). When ssh fails, derive
/// what is derivable from the text: strip `ssh://`, take what follows the last `@` as the host
/// (with a numeric `:port` suffix as the port), the user written before the `@` (else `-`),
/// port 22.
pub fn resolve(sys: &dyn Sys, t: &Target) -> Resolved {
    let fallback = fallback_resolve(t.as_str());
    let c = cmd("ssh", &["-G", "--", t.as_str()]).with_timeout(RESOLVE_TIMEOUT);
    let Ok(out) = sys.run(&c) else {
        return fallback;
    };
    if !out.success() {
        return fallback;
    }
    let (mut user, mut host, mut port) = (None, None, None);
    for line in out.stdout.lines() {
        let mut it = line.split_whitespace();
        match (it.next(), it.next()) {
            (Some("user"), Some(v)) => user = Some(v.to_string()),
            (Some("hostname"), Some(v)) => host = Some(v.to_ascii_lowercase()),
            (Some("port"), Some(v)) => port = v.parse::<u16>().ok(),
            _ => {}
        }
    }
    Resolved {
        user: user.unwrap_or(fallback.user),
        host: host.unwrap_or(fallback.host),
        port: port.unwrap_or(fallback.port),
    }
}

/// The host and port derivable from a target's text alone.
pub(crate) fn fallback_resolve(t: &str) -> Resolved {
    let rest = t.strip_prefix("ssh://").unwrap_or(t).trim_end_matches('/');
    let rest = rest.rsplit_once('@').map_or(rest, |(_, h)| h);
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && p.parse::<u16>().is_ok() => {
            (h, p.parse::<u16>().unwrap_or(22))
        }
        _ => (rest, 22),
    };
    Resolved {
        user: explicit_user(t).unwrap_or("-").to_string(),
        host: host.to_ascii_lowercase(),
        port,
    }
}

/// The user written in a target's text (`user@host`, `ssh://user@host:port`), if any.
pub(crate) fn explicit_user(t: &str) -> Option<&str> {
    let rest = t.strip_prefix("ssh://").unwrap_or(t);
    rest.rsplit_once('@')
        .map(|(u, _)| u)
        .filter(|u| !u.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::fake::FakeSys;

    #[test]
    fn parse_accepts_the_usual_destinations() {
        for ok in [
            "host",
            "j@192.0.2.10",
            "user@host.example.com",
            "ssh://User@Host:2222",
            "_alias",
            "a",
            "u@fe80%eth0",
            "dev-box.lan",
            "x/y+z",
            &"a".repeat(255),
        ] {
            assert_eq!(Target::parse(ok).unwrap().as_str(), ok, "{ok}");
        }
    }

    #[test]
    fn parse_trims_whitespace() {
        assert_eq!(Target::parse("  j@host\n").unwrap().as_str(), "j@host");
    }

    #[test]
    fn parse_rejects_with_the_right_error() {
        let cases: &[(&str, TargetError)] = &[
            ("", TargetError::Empty),
            ("   ", TargetError::Empty),
            ("-oProxyCommand=x", TargetError::InvalidChar('-')),
            ("-", TargetError::InvalidChar('-')),
            (".hidden", TargetError::InvalidChar('.')),
            ("@host", TargetError::InvalidChar('@')),
            ("a;b", TargetError::InvalidChar(';')),
            ("a b", TargetError::InvalidChar(' ')),
            ("a$(x)", TargetError::InvalidChar('$')),
            ("a`x`", TargetError::InvalidChar('`')),
            ("a\nb", TargetError::InvalidChar('\n')),
            ("a'b", TargetError::InvalidChar('\'')),
            ("é", TargetError::InvalidChar('é')),
        ];
        for (input, want) in cases {
            assert_eq!(
                Target::parse(input).as_ref().unwrap_err(),
                want,
                "{input:?}"
            );
        }
        assert_eq!(
            Target::parse(&"a".repeat(256)).unwrap_err(),
            TargetError::TooLong
        );
    }

    #[test]
    fn error_messages_are_readable() {
        assert_eq!(
            TargetError::InvalidChar(';').to_string(),
            "';' is not allowed in an ssh destination"
        );
        assert!(TargetError::Empty.to_string().contains("empty"));
    }

    #[test]
    fn display_and_as_ref() {
        let t = Target::parse("j@h").unwrap();
        assert_eq!(t.to_string(), "j@h");
        assert_eq!(t.as_ref() as &str, "j@h");
    }

    #[test]
    fn resolve_reads_ssh_g() {
        let fake = FakeSys::new();
        fake.on("ssh", &["-G", "--", "devbox"])
            .stdout("host devbox\nuser j\nhostname 10.0.0.2\nport 2200\nidentityfile x\n");
        let t = Target::parse("devbox").unwrap();
        let r = resolve(&fake, &t);
        assert_eq!(
            r,
            Resolved {
                user: "j".into(),
                host: "10.0.0.2".into(),
                port: 2200
            }
        );
        assert_eq!(r.full_key(), "j@10.0.0.2:2200");
        let call = &fake.calls()[0].cmd;
        assert_eq!(call.timeout, Some(Duration::from_secs(3)));
    }

    #[test]
    fn resolve_lowercases_the_host() {
        let fake = FakeSys::new();
        fake.on("ssh", &["-G"])
            .stdout("user u\nhostname Box.Example.COM\nport 22\n");
        let r = resolve(&fake, &Target::parse("u@Box.Example.COM").unwrap());
        assert_eq!(r.host, "box.example.com");
    }

    #[test]
    fn resolve_falls_back_when_ssh_fails() {
        let table = [
            ("j@Host.Lan", "j", "host.lan", 22),
            ("host", "-", "host", 22),
            ("ssh://u@h:2222", "u", "h", 2222),
            ("ssh://h/", "-", "h", 22),
            ("a@b@c", "a@b", "c", 22),
            ("h:abc", "-", "h:abc", 22),
        ];
        for fail in [
            FakeSys::new(),
            {
                let f = FakeSys::new();
                f.on("ssh", &[]).code(255);
                f
            },
            {
                let f = FakeSys::new();
                f.on("ssh", &[]).timed_out();
                f
            },
        ] {
            for (t, user, host, port) in table {
                let r = resolve(&fail, &Target::parse(t).unwrap());
                assert_eq!(
                    (r.user.as_str(), r.host.as_str(), r.port),
                    (user, host, port),
                    "{t}"
                );
            }
        }
    }

    #[test]
    fn explicit_user_reads_the_text() {
        assert_eq!(explicit_user("j@h"), Some("j"));
        assert_eq!(explicit_user("ssh://u@h:22"), Some("u"));
        assert_eq!(explicit_user("h"), None);
        assert_eq!(explicit_user("ssh://h"), None);
    }

    #[test]
    fn resolve_fills_gaps_from_the_fallback() {
        let fake = FakeSys::new();
        fake.on("ssh", &["-G"]).stdout("user j\n");
        let r = resolve(&fake, &Target::parse("x@Box:22").unwrap());
        assert_eq!((r.user.as_str(), r.host.as_str(), r.port), ("j", "box", 22));
    }
}
