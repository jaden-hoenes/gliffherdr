//! The configuration file: `config.toml` in herdr's plugin config directory.
//!
//! A missing file means all defaults. Unknown keys are an error (with line and column) so typos
//! do not silently do nothing.

use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::paths::Paths;
use crate::target::Target;

/// The commented template written by `gliffherdr config init`; loading it yields the defaults.
pub const TEMPLATE: &str = include_str!("config/template.toml");

/// What opening a machine shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Mirror the remote machine's focused screen.
    #[default]
    Mirror,
    /// A private screen sized to the window.
    Headless,
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Mode::Mirror => "mirror",
            Mode::Headless => "headless",
        })
    }
}

/// `[connect]`: defaults for every launch.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ConnectSettings {
    /// What Enter opens.
    pub mode: Mode,
    /// Extra arguments for gliff.
    pub gliff_args: Vec<String>,
    /// The remote gliff-server command (`None` means gliff's default).
    pub server_bin: Option<String>,
}

/// `[sources]`: where machines are listed from.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[allow(clippy::struct_excessive_bools)] // one switch per source; the TOML schema is fixed by docs/design/02-core.md
#[serde(deny_unknown_fields, default)]
pub struct SourceSettings {
    /// herdr's saved machines.
    pub herdr: bool,
    /// Machines connected to before.
    pub recent: bool,
    /// gliff's own machine list.
    pub gliff: bool,
    /// `~/.ssh/config` hosts.
    pub ssh_config: bool,
    /// Online Tailscale peers.
    pub tailscale: bool,
    /// Targets or labels never listed (case-insensitive).
    pub hide: Vec<String>,
}

impl Default for SourceSettings {
    fn default() -> Self {
        SourceSettings {
            herdr: true,
            recent: true,
            gliff: true,
            ssh_config: true,
            tailscale: true,
            hide: Vec::new(),
        }
    }
}

/// `[check]`: when and how machines are checked.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CheckSettings {
    /// Check stale machines when herdr starts.
    pub on_startup: bool,
    /// Seconds a result stays fresh.
    pub fresh_for_secs: u64,
    /// Seconds ssh may take to connect.
    pub ssh_timeout_secs: u64,
}

impl Default for CheckSettings {
    fn default() -> Self {
        CheckSettings {
            on_startup: true,
            fresh_for_secs: 600,
            ssh_timeout_secs: 5,
        }
    }
}

/// `[[machine]]`: settings for one machine.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineConfig {
    /// The ssh destination.
    pub target: String,
    /// The name shown in the picker.
    #[serde(default)]
    pub label: Option<String>,
    /// Overrides `[connect] mode`.
    #[serde(default)]
    pub mode: Option<Mode>,
    /// Mirror this remote screen.
    #[serde(default)]
    pub output: Option<String>,
    /// Overrides `[connect] server_bin`.
    #[serde(default)]
    pub server_bin: Option<String>,
    /// Overrides `[connect] gliff_args`.
    #[serde(default)]
    pub gliff_args: Option<Vec<String>>,
}

/// The whole configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    /// `[connect]`.
    pub connect: ConnectSettings,
    /// `[sources]`.
    pub sources: SourceSettings,
    /// `[check]`.
    pub check: CheckSettings,
    /// `[[machine]]` entries.
    #[serde(rename = "machine")]
    pub machines: Vec<MachineConfig>,
}

/// Why the configuration could not be loaded.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The file exists but cannot be read.
    #[error("cannot read {}: {source}", path.display())]
    Read {
        /// The config file.
        path: PathBuf,
        /// The I/O error.
        source: io::Error,
    },
    /// The file is not valid TOML for the schema (unknown key, wrong type, syntax).
    #[error("{}: {message}", path.display())]
    Parse {
        /// The config file.
        path: PathBuf,
        /// toml's full message, including line, column and a source excerpt.
        message: String,
        /// toml's one-line message without position.
        short: String,
        /// 1-based line of the error.
        line: Option<usize>,
        /// 1-based column of the error.
        column: Option<usize>,
    },
    /// The file parses but a value is not acceptable.
    #[error("{}: {message}", path.display())]
    Invalid {
        /// The config file.
        path: PathBuf,
        /// What is wrong.
        message: String,
    },
}

impl ConfigError {
    /// The config file the error is about.
    pub fn path(&self) -> &Path {
        match self {
            ConfigError::Read { path, .. }
            | ConfigError::Parse { path, .. }
            | ConfigError::Invalid { path, .. } => path,
        }
    }

    /// The error's line, when it has one.
    pub fn line(&self) -> Option<usize> {
        match self {
            ConfigError::Parse { line, .. } => *line,
            _ => None,
        }
    }

    /// One line for banners: `config.toml line 4: unknown field `colour`…`.
    pub fn summary(&self) -> String {
        let file = self.path().file_name().map_or_else(
            || self.path().display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        match self {
            ConfigError::Read { source, .. } => format!("{file}: cannot read: {source}"),
            ConfigError::Parse {
                short,
                line: Some(line),
                ..
            } => {
                format!("{file} line {line}: {}", first_line(short))
            }
            ConfigError::Parse { short, .. } => format!("{file}: {}", first_line(short)),
            ConfigError::Invalid { message, .. } => format!("{file}: {message}"),
        }
    }
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s)
}

/// 1-based line and column of byte `offset` in `text`.
fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, column)
}

/// Load `config.toml` from `paths`. A missing file yields the defaults.
pub fn load(paths: &Paths) -> Result<Config, ConfigError> {
    let path = paths.config_file();
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text, &path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
        Err(source) => Err(ConfigError::Read { path, source }),
    }
}

/// Parse and validate configuration text; `path` is only used in errors.
pub fn parse(text: &str, path: &Path) -> Result<Config, ConfigError> {
    let mut config: Config = toml::from_str(text).map_err(|e| {
        let pos = e.span().map(|s| line_col(text, s.start));
        ConfigError::Parse {
            path: path.to_path_buf(),
            message: e.to_string().trim_end().to_string(),
            short: e.message().to_string(),
            line: pos.map(|p| p.0),
            column: pos.map(|p| p.1),
        }
    })?;
    config.normalize();
    config.validate().map_err(|message| ConfigError::Invalid {
        path: path.to_path_buf(),
        message,
    })?;
    Ok(config)
}

impl Config {
    /// An empty `server_bin` means "not set".
    fn normalize(&mut self) {
        let clean = |v: &mut Option<String>| {
            if v.as_deref().is_some_and(|s| s.trim().is_empty()) {
                *v = None;
            }
        };
        clean(&mut self.connect.server_bin);
        for m in &mut self.machines {
            clean(&mut m.server_bin);
            clean(&mut m.label);
            clean(&mut m.output);
        }
    }

    fn validate(&self) -> Result<(), String> {
        for (i, m) in self.machines.iter().enumerate() {
            if let Err(e) = Target::parse(&m.target) {
                return Err(format!(
                    "machine #{} has an invalid target {:?}: {e}",
                    i + 1,
                    m.target
                ));
            }
            if let Some(o) = &m.output {
                let ok = o
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
                if !ok {
                    return Err(format!(
                        "machine {:?}: output {o:?} is not a screen name (letters, digits, . _ -)",
                        m.target
                    ));
                }
            }
        }
        if self.check.fresh_for_secs < 1 {
            return Err("check.fresh_for_secs must be at least 1".to_string());
        }
        if !(1..=60).contains(&self.check.ssh_timeout_secs) {
            return Err("check.ssh_timeout_secs must be between 1 and 60".to_string());
        }
        Ok(())
    }
}

/// Write [`TEMPLATE`] to the config file unless it exists. Returns whether it was written.
pub fn write_template_if_missing(paths: &Paths) -> io::Result<bool> {
    let path = paths.config_file();
    std::fs::create_dir_all(&paths.config_dir)?;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut f) => {
            f.write_all(TEMPLATE.as_bytes())?;
            Ok(true)
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(text: &str) -> Result<Config, ConfigError> {
        parse(text, Path::new("/c/config.toml"))
    }

    fn invalid(text: &str) -> String {
        match p(text).unwrap_err() {
            ConfigError::Invalid { message, .. } => message,
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn defaults_are_the_documented_ones() {
        let c = Config::default();
        assert_eq!(c.connect.mode, Mode::Mirror);
        assert!(c.connect.gliff_args.is_empty() && c.connect.server_bin.is_none());
        assert!(c.sources.herdr && c.sources.recent && c.sources.gliff);
        assert!(c.sources.ssh_config && c.sources.tailscale && c.sources.hide.is_empty());
        assert!(c.check.on_startup);
        assert_eq!((c.check.fresh_for_secs, c.check.ssh_timeout_secs), (600, 5));
        assert_eq!(c.machines.len(), 0);
    }

    #[test]
    fn empty_text_and_missing_file_are_defaults() {
        assert_eq!(p("").unwrap(), Config::default());
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(&Paths::in_dir(dir.path())).unwrap(), Config::default());
    }

    #[test]
    fn the_template_loads_as_the_defaults() {
        assert_eq!(p(TEMPLATE).unwrap(), Config::default());
    }

    #[test]
    fn every_template_example_line_is_valid_when_uncommented() {
        // Example lines are written `#key = value`; prose is `# text`.
        let text: String = TEMPLATE
            .lines()
            .map(|l| match l.strip_prefix('#') {
                Some(rest) if !rest.starts_with(' ') && !rest.is_empty() => rest.to_string(),
                _ => l.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        let c = p(&text).unwrap();
        assert_eq!(c.machines.len(), 1);
        assert_eq!(c.machines[0].target, "j@10.0.0.2");
        assert_eq!(c.machines[0].mode, Some(Mode::Headless));
        assert_eq!(c.connect.server_bin, None, "empty string means unset");
    }

    #[test]
    fn the_template_mentions_every_key() {
        for key in [
            "mode",
            "gliff_args",
            "server_bin",
            "herdr",
            "recent",
            "gliff",
            "ssh_config",
            "tailscale",
            "hide",
            "on_startup",
            "fresh_for_secs",
            "ssh_timeout_secs",
            "target",
            "label",
            "output",
        ] {
            assert!(
                TEMPLATE.contains(&format!("#{key} =")),
                "template lacks {key}"
            );
        }
    }

    #[test]
    fn a_full_file_parses() {
        let c = p(r#"
[connect]
mode = "headless"
gliff_args = ["--no-clipboard", "--video", "cpu"]
server_bin = "gliff-server --low-bandwidth"
[sources]
tailscale = false
ssh_config = false
hide = ["nas"]
[check]
on_startup = false
fresh_for_secs = 30
ssh_timeout_secs = 2
[[machine]]
target = "j@10.0.0.2"
label = "desk"
mode = "mirror"
output = "HDMI-A-1"
server_bin = "gs"
gliff_args = []
[[machine]]
target = "other"
"#)
        .unwrap();
        assert_eq!(c.connect.mode, Mode::Headless);
        assert_eq!(c.connect.gliff_args, ["--no-clipboard", "--video", "cpu"]);
        assert_eq!(
            c.connect.server_bin.as_deref(),
            Some("gliff-server --low-bandwidth")
        );
        assert!(!c.sources.tailscale && !c.sources.ssh_config && c.sources.herdr);
        assert_eq!(c.sources.hide, ["nas"]);
        assert!(!c.check.on_startup);
        assert_eq!((c.check.fresh_for_secs, c.check.ssh_timeout_secs), (30, 2));
        assert_eq!(c.machines.len(), 2);
        let m = &c.machines[0];
        assert_eq!(m.label.as_deref(), Some("desk"));
        assert_eq!(m.mode, Some(Mode::Mirror));
        assert_eq!(m.output.as_deref(), Some("HDMI-A-1"));
        assert_eq!(m.gliff_args, Some(vec![]));
        assert_eq!(c.machines[1].label, None);
    }

    #[test]
    fn empty_server_bin_means_unset() {
        let c =
            p("[connect]\nserver_bin = \"\"\n[[machine]]\ntarget = \"h\"\nserver_bin = \"  \"\n")
                .unwrap();
        assert_eq!(c.connect.server_bin, None);
        assert_eq!(c.machines[0].server_bin, None);
    }

    #[test]
    fn unknown_keys_are_errors_with_a_line() {
        let err = p("[connect]\nmode = \"mirror\"\ncolour = \"blue\"\n").unwrap_err();
        assert_eq!(err.line(), Some(3));
        assert!(err.to_string().contains("unknown field `colour`"), "{err}");
        assert!(err.to_string().starts_with("/c/config.toml"), "{err}");
        assert_eq!(
            err.summary(),
            "config.toml line 3: unknown field `colour`, expected one of `mode`, `gliff_args`, `server_bin`"
        );
        for bad in [
            "colour = 1",
            "[machine]\ntarget = \"h\"",
            "[[machine]]\ntarget = \"h\"\nspeed = 1",
            "[check]\nspeed = 1",
        ] {
            assert!(matches!(p(bad), Err(ConfigError::Parse { .. })), "{bad}");
        }
    }

    #[test]
    fn syntax_and_type_errors_are_parse_errors() {
        assert_eq!(p("[connect\n").unwrap_err().line(), Some(1));
        for bad in [
            "[connect]\nmode = \"sideways\"",
            "[check]\nfresh_for_secs = \"ten\"",
            "[sources]\nherdr = 1",
            "[[machine]]\nlabel = \"x\"",
        ] {
            assert!(matches!(p(bad), Err(ConfigError::Parse { .. })), "{bad}");
        }
    }

    #[test]
    fn validation_messages() {
        assert!(invalid("[[machine]]\ntarget = \"-oProxyCommand=x\"").contains("invalid target"));
        assert!(invalid("[[machine]]\ntarget = \"\"").contains("empty"));
        assert!(
            invalid("[[machine]]\ntarget = \"h\"\noutput = \"a b\"").contains("not a screen name")
        );
        assert!(invalid("[check]\nfresh_for_secs = 0").contains("fresh_for_secs"));
        assert!(invalid("[check]\nssh_timeout_secs = 0").contains("ssh_timeout_secs"));
        assert!(invalid("[check]\nssh_timeout_secs = 61").contains("ssh_timeout_secs"));
        assert!(p("[check]\nssh_timeout_secs = 60\nfresh_for_secs = 1").is_ok());
        assert!(p("[[machine]]\ntarget = \"h\"\noutput = \"DP-1.a_b\"").is_ok());
    }

    #[test]
    fn invalid_summary_has_no_line() {
        let err = p("[check]\nfresh_for_secs = 0").unwrap_err();
        assert_eq!(err.line(), None);
        assert!(
            err.summary()
                .starts_with("config.toml: check.fresh_for_secs")
        );
    }

    #[test]
    fn load_reads_the_file_and_reports_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        std::fs::write(paths.config_file(), "[check]\nfresh_for_secs = 9\n").unwrap();
        assert_eq!(load(&paths).unwrap().check.fresh_for_secs, 9);
        std::fs::write(paths.config_file(), "x = 1\n").unwrap();
        assert_eq!(load(&paths).unwrap_err().path(), paths.config_file());
    }

    #[test]
    fn load_reports_unreadable_files() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        std::fs::create_dir_all(paths.config_file()).unwrap(); // a directory, not a file
        let err = load(&paths).unwrap_err();
        assert!(matches!(err, ConfigError::Read { .. }), "{err}");
        assert!(err.summary().contains("cannot read"));
    }

    #[test]
    fn template_is_written_once() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        assert!(write_template_if_missing(&paths).unwrap());
        assert_eq!(
            std::fs::read_to_string(paths.config_file()).unwrap(),
            TEMPLATE
        );
        std::fs::write(paths.config_file(), "# mine\n").unwrap();
        assert!(!write_template_if_missing(&paths).unwrap());
        assert_eq!(
            std::fs::read_to_string(paths.config_file()).unwrap(),
            "# mine\n"
        );
    }

    #[test]
    fn mode_displays_lowercase() {
        assert_eq!(Mode::Mirror.to_string(), "mirror");
        assert_eq!(Mode::Headless.to_string(), "headless");
    }
}
