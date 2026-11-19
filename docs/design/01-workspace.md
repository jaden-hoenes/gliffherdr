# 01 — Workspace

## Layout

```
gliffherdr/
  Cargo.toml  Cargo.lock  rust-toolchain.toml  rustfmt.toml  clippy.toml  deny.toml  about.toml  about.hbs
  herdr-plugin.toml
  .gitignore                     .vendor/  /target/  /bin/gliffherdr  *.pending-snap
  bin/.gitkeep                   install.sh puts the binary here; only .gitkeep is tracked
  src/
    main.rs                      fn main() -> ExitCode { gliffherdr::cli::main() }
    lib.rs                       pub mod declarations only
    sys.rs  sys/real.rs  sys/fake.rs
    paths.rs
    target.rs
    config.rs  config/template.toml
    roster.rs  roster/sources.rs  roster/ssh_argv.rs
    check.rs  check/remote.sh  check/classify.rs  check/cache.rs
    status.rs                    row states, ranks, glyphs, tones, sentences (shared by list + picker)
    hypr.rs                      Hyprland client + graphical env recovery
    launch.rs                    launch plan, isolated config home, detached spawn, window wait
    connect.rs                   the connect flow
    fix.rs                       remedies
    herdr.rs                     herdr CLI wrapper
    demo.rs                      demo mode (fixture roster, scripted checks)
    picker.rs  picker/app.rs  picker/view.rs  picker/input.rs  picker/chooser.rs
    cli.rs  cli/list.rs  cli/check.rs  cli/connect.rs  cli/fix.rs  cli/doctor.rs  cli/config.rs
  tests/
    common/mod.rs                stub-tool harness (see 07)
    cli_*.rs                     CLI integration tests with stubs on PATH
    picker_snapshots.rs          insta snapshots (TestBackend)
    gliff_contract.rs            reads .vendor/gliff
    herdr_contract.rs            reads .vendor/herdr + herdr-plugin.toml
    e2e_ssh.rs                   real sshd, gated by GLIFFHERDR_E2E_SSH=1
    e2e_herdr.rs                 real herdr, gated by GLIFFHERDR_E2E_HERDR=1
    snapshots/                   insta snapshot files (committed)
  scripts/
    vendor.sh                    clones herdr and gliff at the pinned commits into .vendor/
    gate.sh                      the definition-of-done gate (see 09)
    install.sh                   herdr [[build]] entry: prebuilt download or source build
    dev-link.sh                  cargo build + symlink bin/gliffherdr + herdr plugin link
    demo.sh                      renders docs/assets/picker.gif with VHS in a container
    check-versions.sh            Cargo.toml == herdr-plugin.toml == top CHANGELOG version
  docs/
    demo.tape  assets/  configuration.md  how-it-works.md  troubleshooting.md
  .github/workflows/ci.yml  release.yml   .github/dependabot.yml  ISSUE_TEMPLATE/  pull_request_template.md
  README.md  CHANGELOG.md  CONTRIBUTING.md  SECURITY.md  CODE_OF_CONDUCT.md  LICENSE
  docs/design/                   these design notes
```

## `Cargo.toml`

```toml
[package]
name = "gliffherdr"
version = "0.1.0"
edition = "2024"
rust-version = "1.88"
description = "A herdr plugin that opens gliff remote desktops to any machine you can ssh to"
license = "MIT"
repository = "https://github.com/jaden-hoenes/gliffherdr"
readme = "README.md"
keywords = ["herdr", "gliff", "hyprland", "remote-desktop", "ssh"]
categories = ["command-line-utilities"]
publish = false

[[bin]]
name = "gliffherdr"
path = "src/main.rs"

[dependencies]          # exactly the table in 00 §Runtime dependencies
[dev-dependencies]      # exactly the table in 00 §Dev dependencies

[profile.release]
lto = "thin"
codegen-units = 1
strip = true
panic = "abort"

[lints.rust]
unsafe_code = "deny"          # the single setsid pre_exec is #[allow(unsafe_code)] with a SAFETY comment
missing_docs = "warn"

[lints.clippy]
pedantic = { level = "warn", priority = -1 }
module_name_repetitions = "allow"
must_use_candidate = "allow"
missing_errors_doc = "allow"
```

`rustfmt.toml`: `max_width = 100`, `use_field_init_shorthand = true`. `clippy.toml`:
`disallowed-methods = [{ path = "std::process::Command::new", reason = "use Sys (src/sys.rs)" }]`
with `#[allow(clippy::disallowed_methods)]` only in `src/sys/real.rs` and in `tests/`.

## `.vendor/` (gitignored)

`scripts/vendor.sh` is idempotent: for each of herdr (`https://github.com/herdrdev/herdr.git`,
commit in 00) and gliff (`https://github.com/omacom/gliff.git`, commit in 00) it clones into
`.vendor/<name>` if missing, fetches the commit if `HEAD` differs, and checks it out detached.
It prints the two resolved SHAs. Contract tests skip with a printed reason when `.vendor/` is
missing locally; in CI (`CI=true`) a missing vendor dir is a failure.

## Commands

| Command | What |
|---|---|
| `scripts/vendor.sh` | fetch reference sources |
| `cargo test --locked` | unit, CLI integration, snapshots, contracts |
| `GLIFFHERDR_E2E_SSH=1 cargo test --locked --test e2e_ssh` | real OpenSSH round trips |
| `GLIFFHERDR_E2E_HERDR=1 GLIFFHERDR_HERDR_BIN=<path> cargo test --locked --test e2e_herdr` | real herdr server |
| `scripts/gate.sh` | everything in 09 §Definition of done |
| `scripts/dev-link.sh` | build debug, link into the current herdr |
| `scripts/demo.sh` | regenerate `docs/assets/picker.gif` and `picker.png` |

## Parallel-safety rules

- No servers listen on fixed ports. The sshd e2e binds `127.0.0.1:0`-chosen free ports; the
  herdr e2e uses an isolated `HOME`/`XDG_*` under a temp dir and a session named
  `gliffherdr-e2e-<pid>`.
- Every test writes only under its own `tempfile::TempDir`.
- Worktrees build into their own `target/`.
