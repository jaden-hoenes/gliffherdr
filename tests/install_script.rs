//! `scripts/install.sh` and `scripts/dev-link.sh` against a fake release served over `file://`
//! (curl supports it) and a stub `cargo`. Nothing touches the network or the real herdr.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::{Ran, World, find_tool, ran};

const VERSION: &str = "0.1.0";
const FAKE_BINARY: &str = "#!/bin/sh\necho fake gliffherdr\n";
const FAKE_BUILT: &str = "#!/bin/sh\necho built gliffherdr\n";

/// The tools install.sh needs, so `PATH` can be made to lack `cargo`.
const TOOLS: &[&str] = &[
    "sh",
    "uname",
    "sed",
    "head",
    "grep",
    "curl",
    "sha256sum",
    "tar",
    "mktemp",
    "cp",
    "chmod",
    "mv",
    "rm",
    "mkdir",
    "dirname",
    "cat",
    "gzip",
];

fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "aarch64",
        _ => "x86_64",
    }
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// A plugin checkout (the manifest and the scripts, no `.git`) plus a fake release directory.
struct Fixture {
    w: World,
    plugin: PathBuf,
    release: PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let w = World::new();
        let plugin = w.dir.path().join("plugin");
        std::fs::create_dir_all(plugin.join("scripts")).unwrap();
        std::fs::copy(
            repo().join("herdr-plugin.toml"),
            plugin.join("herdr-plugin.toml"),
        )
        .unwrap();
        for s in ["install.sh", "dev-link.sh"] {
            std::fs::copy(
                repo().join("scripts").join(s),
                plugin.join("scripts").join(s),
            )
            .unwrap();
        }
        let release = w.dir.path().join("release");
        let f = Fixture { w, plugin, release };
        f.publish(FAKE_BINARY, true);
        f
    }

    /// Build the fake release: tarball with `gliffherdr`, and a `SHA256SUMS` that is right or
    /// wrong.
    fn publish(&self, binary: &str, good_sum: bool) {
        let dir = self.release.join(format!("v{VERSION}"));
        let src = self.w.dir.path().join("tarsrc");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&src).unwrap();
        let exe = src.join("gliffherdr");
        std::fs::write(&exe, binary).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let name = format!("gliffherdr-{VERSION}-{}-linux-musl.tar.gz", arch());
        let tar = ran(self
            .w
            .command_for(find_tool("tar"))
            .arg("-czf")
            .arg(dir.join(&name))
            .arg("-C")
            .arg(&src)
            .arg("gliffherdr"));
        assert_eq!(tar.code, 0, "{}", tar.all());
        let sums = ran(self
            .w
            .command_for(find_tool("sha256sum"))
            .arg(&name)
            .current_dir(&dir));
        assert_eq!(sums.code, 0, "{}", sums.all());
        let mut line = sums.stdout;
        if !good_sum {
            line = format!("{}{}", "0".repeat(64), &line[64..]);
        }
        std::fs::write(dir.join("SHA256SUMS"), line).unwrap();
    }

    fn base_url(&self) -> String {
        format!("file://{}", self.release.display())
    }

    /// Run `sh scripts/install.sh` in the plugin dir; `path` replaces `PATH` when given.
    fn install(&self, env: &[(&str, &str)], path: Option<&str>) -> Ran {
        let mut cmd = self.w.command_for(find_tool("sh"));
        cmd.arg("scripts/install.sh")
            .current_dir(&self.plugin)
            .env("GLIFFHERDR_RELEASE_BASE_URL", self.base_url());
        if let Some(p) = path {
            cmd.env("PATH", p);
        }
        for (k, v) in env {
            cmd.env(k, v);
        }
        ran(&mut cmd)
    }

    fn installed(&self) -> PathBuf {
        self.plugin.join("bin/gliffherdr")
    }

    /// A stub `cargo` that logs its argv and drops a built binary where cargo would.
    fn stub_cargo(&self) {
        self.w.write_stub(
            "cargo",
            &format!(
                "echo \"$*\" >>\"$STUB/cargo.log\"\nmkdir -p target/release target/debug\n\
                 printf '%s' '{FAKE_BUILT}' >target/release/gliffherdr\n\
                 printf '%s' '{FAKE_BUILT}' >target/debug/gliffherdr\n\
                 chmod 755 target/release/gliffherdr target/debug/gliffherdr\n"
            ),
        );
    }

    /// `PATH` with the stubs and just the tools install.sh needs (no real `cargo`).
    fn path_without_cargo(&self) -> String {
        self.w.minimal_path(TOOLS)
    }
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

#[test]
fn good_checksum_installs_the_prebuilt_binary() {
    let f = Fixture::new();
    let r = f.install(&[], Some(&f.path_without_cargo()));
    assert_eq!(r.code, 0, "{}", r.all());
    assert_eq!(
        r.stdout,
        format!("gliffherdr {VERSION}: installed bin/gliffherdr (prebuilt)\n")
    );
    assert_eq!(read(&f.installed()), FAKE_BINARY);
    let mode = std::fs::metadata(f.installed())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755);
}

#[test]
fn bad_checksum_without_cargo_fails_with_the_message() {
    let f = Fixture::new();
    f.publish(FAKE_BINARY, false);
    let r = f.install(&[], Some(&f.path_without_cargo()));
    assert_eq!(r.code, 1, "{}", r.all());
    assert!(
        r.stderr.contains(&format!(
            "gliffherdr: could not download the v{VERSION} binary and cargo is not installed. \
             Install Rust (https://rustup.rs) and run herdr plugin install again."
        )),
        "{}",
        r.stderr
    );
    assert!(r.stderr.contains("checksum mismatch"), "{}", r.stderr);
    assert!(!f.installed().exists(), "nothing may be installed");
}

#[test]
fn a_missing_checksum_line_is_a_failure_too() {
    let f = Fixture::new();
    std::fs::write(
        f.release.join(format!("v{VERSION}/SHA256SUMS")),
        format!("{}  something-else.tar.gz\n", "a".repeat(64)),
    )
    .unwrap();
    let r = f.install(&[], Some(&f.path_without_cargo()));
    assert_eq!(r.code, 1, "{}", r.all());
    assert!(r.stderr.contains("has no entry for"), "{}", r.stderr);
    assert!(!f.installed().exists());
}

#[test]
fn a_missing_release_is_a_failure_without_cargo() {
    let f = Fixture::new();
    std::fs::remove_dir_all(f.release.join(format!("v{VERSION}"))).unwrap();
    let r = f.install(&[], Some(&f.path_without_cargo()));
    assert_eq!(r.code, 1, "{}", r.all());
    assert!(r.stderr.contains("could not download"), "{}", r.stderr);
}

#[test]
fn source_mode_runs_cargo_and_copies_the_binary() {
    let f = Fixture::new();
    f.stub_cargo();
    let r = f.install(&[("GLIFFHERDR_BUILD", "source")], None);
    assert_eq!(r.code, 0, "{}", r.all());
    assert_eq!(
        r.stdout,
        format!("gliffherdr {VERSION}: installed bin/gliffherdr (built from source)\n")
    );
    assert_eq!(f.w.read_data("cargo.log"), "build --release --locked\n");
    assert_eq!(read(&f.installed()), FAKE_BUILT);
}

#[test]
fn source_mode_without_cargo_says_so() {
    let f = Fixture::new();
    let r = f.install(
        &[("GLIFFHERDR_BUILD", "source")],
        Some(&f.path_without_cargo()),
    );
    assert_eq!(r.code, 1, "{}", r.all());
    assert!(r.stderr.contains("cargo is not installed"), "{}", r.stderr);
}

#[test]
fn prebuilt_mode_falls_back_to_source_when_the_download_is_bad() {
    let f = Fixture::new();
    f.stub_cargo();
    f.publish(FAKE_BINARY, false);
    let r = f.install(&[("GLIFFHERDR_BUILD", "prebuilt")], None);
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(r.stdout.ends_with("(built from source)\n"), "{}", r.stdout);
    assert_eq!(f.w.read_data("cargo.log"), "build --release --locked\n");
}

#[test]
fn with_cargo_installed_the_prebuilt_binary_is_still_the_default() {
    let f = Fixture::new();
    f.stub_cargo();
    // A git checkout that is not a tag, which is what `herdr plugin install owner/repo` makes.
    std::fs::create_dir(f.plugin.join(".git")).unwrap();
    let r = f.install(&[], None);
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(r.stdout.ends_with("(prebuilt)\n"), "{}", r.stdout);
    assert!(!r.stderr.contains("checkout"), "no warning: {}", r.stderr);
    assert_eq!(read(&f.installed()), FAKE_BINARY);
    assert_eq!(f.w.read_data("cargo.log"), "", "cargo must not run");
}

#[test]
fn a_missing_release_asset_falls_back_to_source_when_cargo_exists() {
    let f = Fixture::new();
    f.stub_cargo();
    std::fs::remove_dir_all(f.release.join(format!("v{VERSION}"))).unwrap();
    let r = f.install(&[], None);
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(r.stderr.contains("could not download"), "{}", r.stderr);
    assert!(
        r.stderr.contains("falling back to a source build"),
        "{}",
        r.stderr
    );
    assert!(r.stdout.ends_with("(built from source)\n"), "{}", r.stdout);
    assert_eq!(f.w.read_data("cargo.log"), "build --release --locked\n");
    assert_eq!(read(&f.installed()), FAKE_BUILT);
}

#[test]
fn a_failed_verification_falls_back_to_source_when_cargo_exists() {
    let f = Fixture::new();
    f.stub_cargo();
    f.publish(FAKE_BINARY, false);
    let r = f.install(&[], None);
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(r.stderr.contains("checksum mismatch"), "{}", r.stderr);
    assert!(r.stdout.ends_with("(built from source)\n"), "{}", r.stdout);
    assert_eq!(
        read(&f.installed()),
        FAKE_BUILT,
        "the bad binary is never installed"
    );
}

#[test]
fn an_explicit_source_build_wins_over_the_prebuilt_default() {
    let f = Fixture::new(); // a good release is published
    f.stub_cargo();
    let r = f.install(&[("GLIFFHERDR_BUILD", "source")], None);
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(r.stdout.ends_with("(built from source)\n"), "{}", r.stdout);
}

#[test]
fn an_unknown_build_mode_is_refused() {
    let f = Fixture::new();
    let r = f.install(&[("GLIFFHERDR_BUILD", "wishful")], None);
    assert_eq!(r.code, 1, "{}", r.all());
    assert!(r.stderr.contains("source or prebuilt"), "{}", r.stderr);
}

#[test]
fn dev_link_no_link_builds_and_symlinks_only() {
    let f = Fixture::new();
    f.stub_cargo();
    let r = ran(f
        .w
        .command_for(find_tool("sh"))
        .args(["scripts/dev-link.sh", "--no-link"])
        .current_dir(&f.plugin));
    assert_eq!(r.code, 0, "{}", r.all());
    assert_eq!(f.w.read_data("cargo.log"), "build\n");
    let target = std::fs::read_link(f.installed()).unwrap();
    assert_eq!(target, Path::new("../target/debug/gliffherdr"));
    assert_eq!(read(&f.installed()), FAKE_BUILT, "the symlink resolves");
    assert!(r.stdout.contains("not linking"), "{}", r.stdout);
    assert_eq!(f.w.read_data("herdr.log"), "", "herdr must not be called");
}

#[test]
fn dev_link_links_through_herdr_bin_path() {
    let f = Fixture::new();
    f.stub_cargo();
    let r = ran(f
        .w
        .command_for(find_tool("sh"))
        .arg("scripts/dev-link.sh")
        .current_dir(&f.plugin));
    assert_eq!(r.code, 0, "{}", r.all());
    // The world's HERDR_BIN_PATH is the logging stub; a real herdr is never involved.
    let log = f.w.read_data("herdr.log");
    assert_eq!(log, format!("plugin link {}\n", f.plugin.display()));
}

#[test]
fn dev_link_rejects_unknown_arguments() {
    let f = Fixture::new();
    let r = ran(f
        .w
        .command_for(find_tool("sh"))
        .args(["scripts/dev-link.sh", "--bogus"])
        .current_dir(&f.plugin));
    assert_eq!(r.code, 2, "{}", r.all());
}

impl Fixture {
    /// Move the published release out of the anonymous URL's reach (as a private repository's
    /// assets are) and add a stub `gh` that serves it from there for `release download`.
    fn make_release_private(&self, gh_works: bool) {
        let public = self.release.join(format!("v{VERSION}"));
        let private = self.w.dir.path().join("private");
        std::fs::rename(&public, &private).unwrap();
        let body = if gh_works {
            format!(
                "echo \"$*\" >>\"$STUB/gh.log\"\n\
                 dir=; while [ $# -gt 0 ]; do [ \"$1\" = --dir ] && dir=$2; shift; done\n\
                 cp {p}/* \"$dir\"/\n",
                p = private.display()
            )
        } else {
            "echo \"$*\" >>\"$STUB/gh.log\"\necho 'HTTP 404: Not Found' >&2\nexit 1\n".to_string()
        };
        self.w.write_stub("gh", &body);
    }
}

#[test]
fn a_private_release_is_fetched_with_gh() {
    let f = Fixture::new();
    f.make_release_private(true);
    let r = f.install(&[("GLIFFHERDR_GH", "1")], Some(&f.path_without_cargo()));
    assert_eq!(r.code, 0, "{}", r.all());
    assert_eq!(read(&f.installed()), FAKE_BINARY);
    assert!(
        r.stderr.contains("trying gh release download"),
        "{}",
        r.stderr
    );
    let log = f.w.read_data("gh.log");
    assert!(
        log.starts_with(&format!(
            "release download v{VERSION} --repo jaden-hoenes/gliffherdr --pattern gliffherdr-{VERSION}-{}-linux-musl.tar.gz --pattern SHA256SUMS --dir ",
            arch()
        )),
        "{log}"
    );
    assert!(r.stdout.contains("(prebuilt)"), "{}", r.stdout);
}

#[test]
fn a_failed_gh_download_without_cargo_fails_with_the_message() {
    let f = Fixture::new();
    f.make_release_private(false);
    let r = f.install(&[("GLIFFHERDR_GH", "1")], Some(&f.path_without_cargo()));
    assert_eq!(r.code, 1, "{}", r.all());
    assert!(
        r.stderr.contains("gh release download failed"),
        "{}",
        r.stderr
    );
    assert!(r.stderr.contains("cargo is not installed"), "{}", r.stderr);
    assert!(!f.installed().exists());
}

#[test]
fn gh_is_not_used_for_a_custom_release_url() {
    let f = Fixture::new();
    f.make_release_private(true);
    let r = f.install(&[], Some(&f.path_without_cargo()));
    assert_eq!(r.code, 1, "{}", r.all());
    assert!(!r.stderr.contains("trying gh"), "{}", r.stderr);
    assert_eq!(f.w.read_data("gh.log"), "");
}

#[test]
fn a_checksum_mismatch_is_not_retried_with_gh() {
    let f = Fixture::new();
    f.publish(FAKE_BINARY, false);
    f.w.write_stub("gh", "echo \"$*\" >>\"$STUB/gh.log\"\nexit 0\n");
    let r = f.install(&[("GLIFFHERDR_GH", "1")], Some(&f.path_without_cargo()));
    assert_eq!(r.code, 1, "{}", r.all());
    assert!(r.stderr.contains("checksum mismatch"), "{}", r.stderr);
    assert_eq!(f.w.read_data("gh.log"), "");
}
