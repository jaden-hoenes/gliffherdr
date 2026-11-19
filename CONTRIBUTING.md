# Contributing to gliffherdr

Thanks for wanting to help. gliffherdr is a herdr plugin written in Rust; this page covers what you
need to build it, test it and send a change.

## Set up

You need Linux, `git`, `ssh`/`sshd` (OpenSSH), and Rust. The toolchain is pinned in
`rust-toolchain.toml`; rustup installs it the first time you run `cargo`. The minimum supported
Rust version is in `Cargo.toml` (`rust-version`) and CI checks it.

```sh
git clone https://github.com/jaden-hoenes/gliffherdr
cd gliffherdr
scripts/vendor.sh        # reference sources of gliff and herdr into .vendor/ (gitignored)
cargo build
```

`scripts/vendor.sh` clones [gliff](https://github.com/omacom/gliff) and
[herdr](https://github.com/herdrdev/herdr) at the commits gliffherdr was written against. They are
only read: the contract tests (`tests/gliff_contract.rs`, `tests/herdr_contract.rs`) check the facts
gliffherdr relies on against them. Without `.vendor/` those tests print why and skip locally; CI
fails instead.

To try your build inside herdr, run `scripts/dev-link.sh`. It builds, symlinks `bin/gliffherdr` to
`target/debug/gliffherdr` and runs `herdr plugin link` for this checkout.
To see the picker without any machines or herdr, run `GLIFFHERDR_DEMO=1 cargo run -- pick`.

## The gate

`scripts/gate.sh` is the definition of done; CI runs the same steps. In order: vendor, `cargo fmt
--check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo doc` with warnings denied,
`cargo test --locked`, the two end-to-end suites, shellcheck, the version check, a scan for leftover
markers, a release build, and a clean working tree.

`scripts/gate.sh --skip-e2e` skips the two end-to-end suites when you have no sshd or herdr at hand.
It is for a quick local run; the pull request must pass the whole gate in CI.

Shellcheck and actionlint run from their container images when they are not installed (`podman`
or `docker`); see the top of `scripts/gate.sh` and the CI workflow for the exact images.
`scripts/deny.sh` runs cargo-deny (licences, advisories, sources): it uses the `cargo-deny` on your
`PATH`, or builds version 0.21.0 once into `/tmp/gliffherdr-deny-cargo`. CI runs it as its own job;
it is not part of `scripts/gate.sh` because advisories are fetched live.

## Tests

- Unit tests live next to the code and use `FakeSys`: every external command goes through the `Sys`
  trait in `src/sys.rs`, so tests script what `ssh`, `hyprctl` or `herdr` would answer. Production
  code never calls `std::process::Command` directly (clippy enforces it).
- CLI tests in `tests/cli_*.rs` run the real binary against stub programs on `PATH`, built by
  `tests/common`. A test never builds a `Command` by hand.
- Tests must never touch your live herdr, real machines or desktop. The harness strips every
  `HERDR_*` and `GLIFFHERDR_*` variable and points `HOME` and `XDG_*` into a temp directory. Keep
  it that way.
- Picker snapshots (`tests/picker_snapshots.rs`) use [insta](https://insta.rs). When a change to the
  picker alters a snapshot, review the diff on purpose:

  ```sh
  cargo test --test picker_snapshots   # fails, writes *.snap.new files next to the snapshots
  cargo insta review                   # accept or reject each change; needs `cargo install cargo-insta`
  ```

  CI runs with `INSTA_UPDATE=no`, so an unreviewed snapshot fails there. Commit the accepted
  `.snap` files (in `tests/snapshots/`) with the change that caused them, and delete any `*.snap.new`
  you rejected.
- Two end-to-end suites exercise the real programs and are off by default:

  ```sh
  GLIFFHERDR_E2E_SSH=1 cargo test --locked --test e2e_ssh
  GLIFFHERDR_E2E_HERDR=1 GLIFFHERDR_HERDR_BIN=$(command -v herdr) cargo test --locked --test e2e_herdr
  ```

  The first starts a private `sshd` as your user on a random local port (needs the `sshd` binary,
  no root). The second starts an isolated herdr server under a temp `HOME`; your running herdr is
  not touched. They print where their temp files are when they fail.

## Decisions and the decisions changelog

The design notes record what the plugin does and why; the decisions file is
[`docs/design/00-decisions.md`](docs/design/00-decisions.md). If your change departs from a decision there, say so
by adding one line, with the reason, under a heading for your change at the end of the file's
"Changelog" section. Decisions are changed only that way, never by silently making the code
disagree with them. Where a design note and the code disagree, the code and that changelog are
right.

The gliff and herdr behaviours gliffherdr depends on are pinned by contract tests. If gliff or
herdr changes one, the test names the row that broke; fix the code and update the note, do not
loosen the test.

## Code style

- `cargo fmt` (see `rustfmt.toml`) and `clippy::pedantic` clean. Every `#[allow]` has a one-line
  reason next to it.
- Every public item has a doc comment saying what it is for.
- The crate has exactly one `unsafe` block (`setsid` before launching a detached `gliff`). Do not
  add another.
- Targets (`user@host`) are validated before use and always passed to `ssh` after `--`; nothing is
  run through a shell except the quoted `server_bin` line fed to the remote check script.
- The picker draws with the 16 ANSI colours and modifiers only, so it follows your terminal theme.
- No new dependencies without a reason in the pull request; `Cargo.lock` is committed.

## Docs

The README, `docs/configuration.md`, `docs/how-it-works.md` and `docs/troubleshooting.md` show real
commands and real output. When a change alters what they show, update them in the same pull
request. `src/config/template.toml` documents every config key; a test keeps it in step with the code.
The picker recording in `docs/assets/` is made with `scripts/demo.sh` (needs `podman` or `docker`
and `rustup`) from `docs/demo.tape`.

## Releasing

Maintainers only. Before tagging: `Cargo.toml`, `herdr-plugin.toml` and the top heading of
`CHANGELOG.md` agree (`scripts/check-versions.sh`), the heading carries the release date, and
`scripts/gate.sh` passes. Push the tag `v<version>`; the release workflow builds the static binaries
for x86_64 and aarch64 (aarch64 is cross-compiled on x86_64 with `rust-lld` and smoke-tested under
qemu, so it also works in a private copy without arm runners), writes `SHA256SUMS` and publishes the release with the changelog section as
its notes. `scripts/install.sh` downloads exactly those files.

`herdr plugin install` takes the default branch, and the installer downloads the prebuilt binary of
the version in `herdr-plugin.toml`. So a released binary must never be paired with a newer manifest
it does not support: any change to the commands in `herdr-plugin.toml`, or to a CLI command the
manifest calls (`connect`, `check`, `open-picker`, `fix`, `pick`), bumps the version in `Cargo.toml`,
`herdr-plugin.toml` and `CHANGELOG.md` to the next unreleased one in the same change.

To rehearse the release before tagging, run the workflow by hand from any branch (Actions, Release,
Run workflow; or `gh workflow run release.yml --ref <branch>`). `dry_run` defaults to true: it
verifies the versions and the changelog section, builds and packs both architectures, writes
`SHA256SUMS` and the notes, and stops before creating a release. The tarballs stay available as
the run's `dist-*` artifacts. `dry_run=false` is refused unless the run is on a tag.

## Commits and pull requests

- One logical change per commit, with an imperative subject of at most about 70 characters, in the
  form `area: what changed`, for example `check: treat a missing server as no_server` or
  `picker: keep the selection on the same machine after a re-check`. Explain why in the body when
  it is not obvious.
- Keep `CHANGELOG.md` up to date for anything a user would notice.
- Open the pull request against `main` and fill in the template. Say which gate steps you ran.

By contributing you agree that your contribution is licensed under the MIT licence of this
repository, and to follow the [Code of Conduct](CODE_OF_CONDUCT.md). To report a security problem,
see [SECURITY.md](SECURITY.md).
