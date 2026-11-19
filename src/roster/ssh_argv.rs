//! Recovering an ssh destination from the argv of a running `ssh` process: "this pane's
//! machine".

use crate::target::Target;

/// ssh options that take a value (attached, `-p2222`, or separate, `-p 2222`).
const WITH_VALUE: &str = "BbcDEeFIiJLlmOoPpQRSWw";

/// The destination an `ssh` command line connects to, as a valid [`Target`] text.
///
/// `argv[0]` must be `ssh` (any directory). `--` ends the options; the destination is the first
/// argument that is not an option or an option's value. `-l <user>` and `-p <port>` are folded
/// in: `-l u -p 2222 h` gives `ssh://u@h:2222`, `-p 2222 h` gives `ssh://h:2222`, `-l u h` gives
/// `u@h`; a destination that already names a user or is an `ssh://` URI is kept as given.
/// `None` for `ssh -V`, `ssh` alone and anything that is not a valid target.
#[must_use]
pub fn ssh_destination(argv: &[String]) -> Option<String> {
    let (first, rest) = argv.split_first()?;
    if first.rsplit('/').next() != Some("ssh") {
        return None;
    }
    let mut user: Option<&str> = None;
    let mut port: Option<&str> = None;
    let mut dest: Option<&str> = None;
    let mut options_done = false;
    let mut i = 0;
    while i < rest.len() {
        let arg = rest[i].as_str();
        i += 1;
        if !options_done && arg == "--" {
            options_done = true;
        } else if !options_done && arg.starts_with('-') && arg.len() > 1 {
            // A cluster of flags, ending in an option that takes a value.
            for (at, flag) in arg.char_indices().skip(1) {
                if !WITH_VALUE.contains(flag) {
                    continue;
                }
                let attached = &arg[at + flag.len_utf8()..];
                let value = if attached.is_empty() {
                    let v = rest.get(i).map(String::as_str);
                    i += 1;
                    v
                } else {
                    Some(attached)
                };
                match flag {
                    'l' => user = value,
                    'p' => port = value,
                    _ => {}
                }
                break;
            }
        } else {
            dest = Some(arg);
            break;
        }
    }
    let dest = dest?;
    let text = if dest.starts_with("ssh://") {
        dest.to_string()
    } else {
        let host = match (dest.contains('@'), user) {
            (false, Some(u)) => format!("{u}@{dest}"),
            _ => dest.to_string(),
        };
        match port {
            Some(p) if p.parse::<u16>().is_ok() => format!("ssh://{host}:{p}"),
            Some(_) => return None,
            None => host,
        }
    };
    Target::parse(&text).ok().map(|t| t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dest(line: &str) -> Option<String> {
        let argv: Vec<String> = line.split_whitespace().map(str::to_string).collect();
        ssh_destination(&argv)
    }

    #[test]
    fn table() {
        let cases: &[(&str, Option<&str>)] = &[
            ("ssh j@h", Some("j@h")),
            ("ssh h", Some("h")),
            ("/usr/bin/ssh j@h", Some("j@h")),
            ("ssh -l u -p 2222 h", Some("ssh://u@h:2222")),
            ("ssh -p 2222 h", Some("ssh://h:2222")),
            ("ssh -p2222 h", Some("ssh://h:2222")),
            ("ssh -l u h", Some("u@h")),
            ("ssh -lu h", Some("u@h")),
            ("ssh -l other u@h", Some("u@h")),
            ("ssh -p 2222 u@h", Some("ssh://u@h:2222")),
            ("ssh -p 2222 -l u u2@h", Some("ssh://u2@h:2222")),
            ("ssh ssh://u@h:2200", Some("ssh://u@h:2200")),
            ("ssh -p 99 ssh://u@h:2200", Some("ssh://u@h:2200")),
            ("ssh -J bastion j@h", Some("j@h")),
            ("ssh -Jbastion j@h", Some("j@h")),
            ("ssh -o Foo=bar j@h", Some("j@h")),
            ("ssh -oFoo=bar j@h", Some("j@h")),
            ("ssh -t j@h", Some("j@h")),
            ("ssh -4 j@h", Some("j@h")),
            ("ssh -vvv j@h", Some("j@h")),
            ("ssh -tt -4 -i /k/id_ed25519 j@h", Some("j@h")),
            ("ssh -i /k/id -F /c/config devbox", Some("devbox")),
            ("ssh -L 8080:localhost:80 j@h", Some("j@h")),
            ("ssh -A -v -p 22 j@h", Some("ssh://j@h:22")),
            ("ssh -4tl u h", Some("u@h")),
            ("ssh -- j@h", Some("j@h")),
            ("ssh -p 2 -- j@h", Some("ssh://j@h:2")),
            ("ssh j@h ls -l /tmp", Some("j@h")),
            ("ssh j@h -p 99", Some("j@h")),
            ("ssh -V", None),
            ("ssh", None),
            ("ssh -p 2222", None),
            ("ssh -J", None),
            ("ssh -p x h", None),
            ("ssh -p 99999 h", None),
            ("ssh -- -oProxyCommand=x", None),
            ("ssh 'a;b'", None),
            ("ssh $(x)", None),
            ("ssh-add -l", None),
            ("sshd -D", None),
            ("mosh j@h", None),
            ("scp a j@h:b", None),
            ("", None),
        ];
        for (line, want) in cases {
            assert_eq!(dest(line).as_deref(), *want, "{line:?}");
        }
    }

    #[test]
    fn an_empty_argv_has_no_destination() {
        assert_eq!(ssh_destination(&[]), None);
    }

    #[test]
    fn invalid_targets_are_rejected_even_when_ssh_would_take_them() {
        let argv = |a: &[&str]| a.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert_eq!(ssh_destination(&argv(&["ssh", "a;b"])), None);
        assert_eq!(ssh_destination(&argv(&["ssh", "-l", "a b", "h"])), None);
        assert_eq!(ssh_destination(&argv(&["ssh", "[::1]"])), None);
        assert_eq!(ssh_destination(&argv(&["ssh", "h h"])), None);
    }
}
