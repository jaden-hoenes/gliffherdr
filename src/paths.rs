//! Where gliffherdr keeps its config and state.
//!
//! Inside herdr the plugin dirs arrive in `HERDR_PLUGIN_CONFIG_DIR` / `HERDR_PLUGIN_STATE_DIR`.
//! When those are absent (run from a shell) the same directories are computed with herdr's own
//! rules, so a shell and the plugin share one cache. The path-component encoding is a
//! re-implementation of herdr v0.9.5 `src/plugin_paths.rs` `plugin_config_path_component`
//! (Apache-2.0), written from its behaviour and test vectors.

use std::io;
use std::path::{Path, PathBuf};

use crate::sys::Sys;
use crate::target::Target;

/// The herdr plugin id.
pub const PLUGIN_ID: &str = "jaden-hoenes.gliffherdr";
/// The application name, used for fallback directory names.
pub const APP: &str = "gliffherdr";
/// herdr's per-user directory name under the XDG config and state homes (release builds).
const HERDR_DIR: &str = "herdr";
/// herdr's cap on one plugin path component, in characters.
const COMPONENT_MAX_CHARS: usize = 120;

/// The directories gliffherdr reads and writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// Holds `config.toml`.
    pub config_dir: PathBuf,
    /// Holds `checks/`, `recent.json`, `windows/`.
    pub state_dir: PathBuf,
    /// `${XDG_CONFIG_HOME:-$HOME/.config}`: where gliff and the desktop keep their config.
    pub real_config_home: PathBuf,
    /// `$HOME`.
    pub home: PathBuf,
}

/// An environment value that is set and not empty.
fn var(sys: &dyn Sys, key: &str) -> Option<String> {
    sys.env(key).filter(|v| !v.is_empty())
}

impl Paths {
    /// Resolve the directories from the environment: `GLIFFHERDR_*_DIR`, then herdr's
    /// `HERDR_PLUGIN_*_DIR`, then herdr's own default location for the plugin.
    pub fn from_sys(sys: &dyn Sys) -> Paths {
        let home = var(sys, "HOME").map_or_else(|| std::env::temp_dir().join(APP), PathBuf::from);
        let real_config_home =
            var(sys, "XDG_CONFIG_HOME").map_or_else(|| home.join(".config"), PathBuf::from);
        let state_home =
            var(sys, "XDG_STATE_HOME").map_or_else(|| home.join(".local/state"), PathBuf::from);
        let id = var(sys, "HERDR_PLUGIN_ID").unwrap_or_else(|| PLUGIN_ID.to_string());
        let component = herdr_component(&id);

        let config_dir = var(sys, "GLIFFHERDR_CONFIG_DIR")
            .or_else(|| var(sys, "HERDR_PLUGIN_CONFIG_DIR"))
            .map_or_else(
                || {
                    real_config_home
                        .join(HERDR_DIR)
                        .join("plugins/config")
                        .join(&component)
                },
                PathBuf::from,
            );
        let state_dir = var(sys, "GLIFFHERDR_STATE_DIR")
            .or_else(|| var(sys, "HERDR_PLUGIN_STATE_DIR"))
            .map_or_else(
                || state_home.join(HERDR_DIR).join("plugins").join(&component),
                PathBuf::from,
            );
        Paths {
            config_dir,
            state_dir,
            real_config_home,
            home,
        }
    }

    /// Directories under one root, for tests and demo mode: `config/`, `state/`, `home/`.
    pub fn in_dir(root: &Path) -> Paths {
        Paths {
            config_dir: root.join("config"),
            state_dir: root.join("state"),
            real_config_home: root.join("home/.config"),
            home: root.join("home"),
        }
    }

    /// `<config_dir>/config.toml`.
    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    /// `<state_dir>/checks`.
    pub fn checks_dir(&self) -> PathBuf {
        self.state_dir.join("checks")
    }

    /// `<checks_dir>/<slug>.json`.
    pub fn check_file(&self, target: &Target) -> PathBuf {
        self.checks_dir()
            .join(format!("{}.json", slug(target.as_str())))
    }

    /// `<state_dir>/recent.json`.
    pub fn recent_file(&self) -> PathBuf {
        self.state_dir.join("recent.json")
    }

    /// `<state_dir>/windows/<slug>`: one machine's isolated gliff config home and log.
    pub fn window_dir(&self, target: &Target) -> PathBuf {
        self.state_dir.join("windows").join(slug(target.as_str()))
    }
}

/// Write `contents` to `path` atomically: parent directories are created, the data goes to
/// `<path>.tmp.<pid>` and is renamed into place.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp.{}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let result = std::fs::write(&tmp, contents).and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// The directory name herdr uses for a plugin id under its plugin config and state dirs
/// (herdr v0.9.5 `plugin_config_path_component`): lowercase ASCII letters, digits, `.`, `_` and
/// `-` are kept, every other byte becomes `%XX` (uppercase hex); a trailing `.` becomes `%2E`;
/// an empty id becomes `%plugin`; a Windows reserved device stem gets a `%` prefix; a component
/// over 120 characters is cut and suffixed with `-` and 12 hex digits of the id's SHA-256.
pub fn herdr_component(id: &str) -> String {
    use std::fmt::Write as _;

    let mut component = String::new();
    for byte in id.bytes() {
        if byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        {
            component.push(char::from(byte));
        } else {
            let _ = write!(component, "%{byte:02X}");
        }
    }
    if component.ends_with('.') {
        component.pop();
        component.push_str("%2E");
    }
    if component.is_empty() {
        return "%plugin".to_string();
    }
    if has_windows_reserved_stem(&component) {
        component = format!("%{component}");
    }
    if component.chars().count() > COMPONENT_MAX_CHARS {
        let hash = short_hash(id);
        let prefix_len = COMPONENT_MAX_CHARS - hash.len() - 1;
        let prefix: String = component.chars().take(prefix_len).collect();
        return format!("{prefix}-{hash}");
    }
    component
}

fn has_windows_reserved_stem(value: &str) -> bool {
    let stem = value
        .split('.')
        .next()
        .unwrap_or(value)
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|p| {
            stem.strip_prefix(p)
                .is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"))
        })
}

/// First 12 hex digits (6 bytes) of the SHA-256 of `value`.
fn short_hash(value: &str) -> String {
    use std::fmt::Write as _;
    sha256(value.as_bytes())[..6]
        .iter()
        .fold(String::new(), |mut acc, b| {
            let _ = write!(acc, "{b:02x}");
            acc
        })
}

/// SHA-256 (FIPS 180-4); only used for the rare over-long plugin id.
#[allow(clippy::too_many_lines, clippy::chunks_exact_to_as_chunks)] // one straight-line algorithm
fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a_2f98,
        0x7137_4491,
        0xb5c0_fbcf,
        0xe9b5_dba5,
        0x3956_c25b,
        0x59f1_11f1,
        0x923f_82a4,
        0xab1c_5ed5,
        0xd807_aa98,
        0x1283_5b01,
        0x2431_85be,
        0x550c_7dc3,
        0x72be_5d74,
        0x80de_b1fe,
        0x9bdc_06a7,
        0xc19b_f174,
        0xe49b_69c1,
        0xefbe_4786,
        0x0fc1_9dc6,
        0x240c_a1cc,
        0x2de9_2c6f,
        0x4a74_84aa,
        0x5cb0_a9dc,
        0x76f9_88da,
        0x983e_5152,
        0xa831_c66d,
        0xb003_27c8,
        0xbf59_7fc7,
        0xc6e0_0bf3,
        0xd5a7_9147,
        0x06ca_6351,
        0x1429_2967,
        0x27b7_0a85,
        0x2e1b_2138,
        0x4d2c_6dfc,
        0x5338_0d13,
        0x650a_7354,
        0x766a_0abb,
        0x81c2_c92e,
        0x9272_2c85,
        0xa2bf_e8a1,
        0xa81a_664b,
        0xc24b_8b70,
        0xc76c_51a3,
        0xd192_e819,
        0xd699_0624,
        0xf40e_3585,
        0x106a_a070,
        0x19a4_c116,
        0x1e37_6c08,
        0x2748_774c,
        0x34b0_bcb5,
        0x391c_0cb3,
        0x4ed8_aa4a,
        0x5b9c_ca4f,
        0x682e_6ff3,
        0x748f_82ee,
        0x78a5_636f,
        0x84c8_7814,
        0x8cc7_0208,
        0x90be_fffa,
        0xa450_6ceb,
        0xbef9_a3f7,
        0xc671_78f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09_e667,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
    ];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7]
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [
                t1.wrapping_add(t2),
                v[0],
                v[1],
                v[2],
                v[3].wrapping_add(t1),
                v[4],
                v[5],
                v[6],
            ];
        }
        for (hv, vv) in h.iter_mut().zip(v) {
            *hv = hv.wrapping_add(vv);
        }
    }
    let mut out = [0u8; 32];
    for (chunk, word) in out.chunks_exact_mut(4).zip(h) {
        chunk.copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// 64-bit FNV-1a hash.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// A file-name-safe, collision-resistant name for a target: the lowercased text with every
/// character outside `[a-z0-9._@-]` replaced by `_`, cut to 60 characters, then `-` and the
/// first 8 hex digits of the FNV-1a hash of the original text.
pub fn slug(target: &str) -> String {
    let readable: String = target
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || "._@-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .take(60)
        .collect();
    let hash = format!("{:016x}", fnv1a64(target.as_bytes()));
    format!("{readable}-{}", &hash[..8])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::fake::FakeSys;

    // The first block is herdr v0.9.5's own test vectors.
    #[test]
    fn herdr_component_matches_herdr_vectors() {
        assert_eq!(
            herdr_component("examples.agent-telegram-notify"),
            "examples.agent-telegram-notify"
        );
        assert_eq!(herdr_component("example:a"), "example%3Aa");
        assert_eq!(herdr_component("Example"), "%45xample");
        assert_ne!(herdr_component("example:a"), herdr_component("example-a"));
        assert_ne!(herdr_component("Example"), herdr_component("example"));
        assert_ne!(
            herdr_component(&"A".repeat(120)),
            herdr_component(&"B".repeat(120))
        );
        assert!(herdr_component(&"A".repeat(120)).len() <= 120);
        assert_eq!(herdr_component("con"), "%con");
        assert_eq!(herdr_component("example."), "example%2E");
    }

    #[test]
    fn herdr_component_edge_cases() {
        assert_eq!(herdr_component(PLUGIN_ID), "jaden-hoenes.gliffherdr");
        assert_eq!(herdr_component(""), "%plugin");
        assert_eq!(herdr_component("a b/é"), "a%20b%2F%C3%A9");
        assert_eq!(herdr_component("com1.tool"), "%com1.tool");
        assert_eq!(herdr_component("lpt9"), "%lpt9");
        assert_eq!(herdr_component("com0"), "com0");
        let long = herdr_component(&"A".repeat(120));
        assert_eq!(long.chars().count(), 120);
        assert_eq!(long, format!("{}-a4f4256159ea", "%41".repeat(35) + "%4"));
    }

    #[test]
    fn sha256_matches_known_vectors() {
        assert_eq!(short_hash("abc"), "ba7816bf8f01");
        assert_eq!(short_hash(""), "e3b0c44298fc");
        assert_eq!(short_hash(&"a".repeat(1000)), short_hash(&"a".repeat(1000)));
    }

    #[test]
    fn fnv_matches_reference_vectors() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn slug_vectors() {
        assert_eq!(slug("j@192.0.2.10"), "j@192.0.2.10-eeb05b1c");
        assert_eq!(
            slug("ssh://User@Host:2222"),
            "ssh___user@host_2222-f9faef29"
        );
        assert_eq!(slug("a"), "a-af63dc4c");
    }

    #[test]
    fn slug_is_distinct_for_case_and_bounded() {
        assert_ne!(slug("Host"), slug("host"));
        let s = slug(&"x".repeat(200));
        assert_eq!(s.len(), 60 + 1 + 8);
        assert!(
            s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._@-".contains(c))
        );
    }

    fn fake(env: &[(&str, &str)]) -> FakeSys {
        let f = FakeSys::new();
        for (k, v) in env {
            f.set_env(k, v);
        }
        f
    }

    #[test]
    fn defaults_follow_herdrs_own_rules() {
        let p = Paths::from_sys(&fake(&[("HOME", "/home/u")]));
        assert_eq!(p.home, Path::new("/home/u"));
        assert_eq!(p.real_config_home, Path::new("/home/u/.config"));
        assert_eq!(
            p.config_dir,
            Path::new("/home/u/.config/herdr/plugins/config/jaden-hoenes.gliffherdr")
        );
        assert_eq!(
            p.state_dir,
            Path::new("/home/u/.local/state/herdr/plugins/jaden-hoenes.gliffherdr")
        );
    }

    #[test]
    fn xdg_homes_are_honoured() {
        let p = Paths::from_sys(&fake(&[
            ("HOME", "/home/u"),
            ("XDG_CONFIG_HOME", "/cfg"),
            ("XDG_STATE_HOME", "/st"),
        ]));
        assert_eq!(p.real_config_home, Path::new("/cfg"));
        assert_eq!(
            p.config_dir,
            Path::new("/cfg/herdr/plugins/config/jaden-hoenes.gliffherdr")
        );
        assert_eq!(
            p.state_dir,
            Path::new("/st/herdr/plugins/jaden-hoenes.gliffherdr")
        );
    }

    #[test]
    fn herdr_plugin_dirs_beat_defaults_and_gliffherdr_dirs_beat_both() {
        let env = [
            ("HOME", "/h"),
            ("HERDR_PLUGIN_CONFIG_DIR", "/herdr/c"),
            ("HERDR_PLUGIN_STATE_DIR", "/herdr/s"),
        ];
        let p = Paths::from_sys(&fake(&env));
        assert_eq!(
            (p.config_dir.as_path(), p.state_dir.as_path()),
            (Path::new("/herdr/c"), Path::new("/herdr/s"))
        );
        let mut both = env.to_vec();
        both.push(("GLIFFHERDR_CONFIG_DIR", "/g/c"));
        both.push(("GLIFFHERDR_STATE_DIR", "/g/s"));
        let p = Paths::from_sys(&fake(&both));
        assert_eq!(
            (p.config_dir.as_path(), p.state_dir.as_path()),
            (Path::new("/g/c"), Path::new("/g/s"))
        );
    }

    #[test]
    fn plugin_id_comes_from_herdr_when_set_and_empty_values_are_unset() {
        let p = Paths::from_sys(&fake(&[
            ("HOME", "/h"),
            ("HERDR_PLUGIN_ID", "Other:Id"),
            ("XDG_CONFIG_HOME", ""),
            ("GLIFFHERDR_STATE_DIR", ""),
        ]));
        assert_eq!(
            p.config_dir,
            Path::new("/h/.config/herdr/plugins/config/%4Fther%3A%49d")
        );
        assert_eq!(
            p.state_dir,
            Path::new("/h/.local/state/herdr/plugins/%4Fther%3A%49d")
        );
    }

    #[test]
    fn without_home_a_temp_dir_is_used() {
        let p = Paths::from_sys(&fake(&[]));
        assert!(p.home.ends_with(APP));
    }

    #[test]
    fn file_helpers_use_the_slug() {
        let p = Paths::in_dir(Path::new("/r"));
        let t = Target::parse("j@192.0.2.10").unwrap();
        assert_eq!(p.config_file(), Path::new("/r/config/config.toml"));
        assert_eq!(p.checks_dir(), Path::new("/r/state/checks"));
        assert_eq!(
            p.check_file(&t),
            Path::new("/r/state/checks/j@192.0.2.10-eeb05b1c.json")
        );
        assert_eq!(p.recent_file(), Path::new("/r/state/recent.json"));
        assert_eq!(
            p.window_dir(&t),
            Path::new("/r/state/windows/j@192.0.2.10-eeb05b1c")
        );
        assert_eq!(p.real_config_home, Path::new("/r/home/.config"));
    }

    #[test]
    fn write_atomic_creates_dirs_and_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a/b/file.json");
        write_atomic(&f, b"one").unwrap();
        write_atomic(&f, b"two").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "two");
        let left: Vec<_> = std::fs::read_dir(f.parent().unwrap()).unwrap().collect();
        assert_eq!(left.len(), 1, "temp file left behind");
    }
}
