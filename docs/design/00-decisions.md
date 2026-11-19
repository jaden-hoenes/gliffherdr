# 00 — Locked decisions

These are the decisions behind gliffherdr 0.1. Changes to a decision are recorded in §Changelog
with the reason. Where another design file disagrees with this one, this file wins; where any of
them disagrees with the code, the code wins.

## Names

| Thing | Value |
|---|---|
| Project / crate / binary | `gliffherdr` |
| Herdr plugin id | `jaden-hoenes.gliffherdr` (constant `PLUGIN_ID` in `src/paths.rs`) |
| Herdr plugin name | `gliffherdr` |
| Repository (at release) | `github.com/jaden-hoenes/gliffherdr` |
| Licence | MIT, `Copyright (c) 2026 Jaden Hoenes` |
| Env var prefix | `GLIFFHERDR_` |
| Warning / error prefix on stderr | `gliffherdr: ` |
| gliff window class it looks for | `gliff` |
| gliff window title separator | `" — "` (space, U+2014 EM DASH, space) |
| Scope description passed to uwsm | `Gliff: <target>` |

**Rename recipe.** Names live in: `Cargo.toml` (`name`, `repository`), `herdr-plugin.toml`
(`id`, `name`), `src/paths.rs` (`PLUGIN_ID`, `APP`), `scripts/install.sh` (`REPO`), README badges,
`docs/`. Search-and-replace `gliffherdr` is safe everywhere except `.vendor/`. Never rename the
gliff contract strings (`gliff`, `— Gliff`, `gliff-server`): they belong to gliff.

## Pinned versions (checked against crates.io and GitHub on 2026-11-18)

### Rust

| Item | Value |
|---|---|
| Edition | 2024 |
| `rust-version` (MSRV) | `1.88` (ratatui 0.30's MSRV) |
| Toolchain pin (`rust-toolchain.toml`) | `1.99.1`, components `rustfmt`, `clippy` |

### Runtime dependencies (exact `=` pins are not used; caret ranges below, `Cargo.lock` committed)

| Crate | Version | Features | Why |
|---|---|---|---|
| `clap` | `4.6.8` | `derive` | CLI |
| `ratatui` | `0.30.2` | default (`crossterm` backend) | picker TUI; use the re-export `ratatui::crossterm`, never a separate `crossterm` dependency |
| `serde` | `1.0.229` | `derive` | config, cache, JSON |
| `serde_json` | `1.0.154` | | herdr/hyprctl/tailscale JSON, cache files, `list --json` |
| `toml` | `1.2.1` | | config file |
| `anyhow` | `1.0.104` | | CLI-level errors only |
| `thiserror` | `2.0.21` | | library error enums |
| `nucleo-matcher` | `0.3.1` | | fuzzy filter in the picker |
| `libc` | `0.2.191` | | `setsid()` in `pre_exec` for detached launches |
| `unicode-width` | `0.2.3` | | column truncation in the picker and `list` |

### Dev dependencies

| Crate | Version | Why |
|---|---|---|
| `tempfile` | `3.27.0` | temp dirs in tests |
| `assert_cmd` | `2.2.2` | CLI integration tests |
| `predicates` | `3.1.4` | assertions with `assert_cmd` |
| `insta` | `1.49.0` | TUI buffer snapshots |

No other dependency may be added without a §Changelog entry. Everything above is added to
`Cargo.toml` up front; `Cargo.toml` and `Cargo.lock` change only with a §Changelog line.

### External tools and pinned references

| Item | Pin | Used for |
|---|---|---|
| herdr | `v0.9.5` = `2ecf19a92fc3a7fff75b6ce3655ef04f4625ec01` | behaviour reference (`.vendor/herdr`), real-herdr e2e |
| herdr release asset `herdr-linux-x86_64` | `sha256:14d9e8bc8b4a9e0118d77c32266f76d461f09b40342eb23752960bd89f59929d` | CI e2e |
| herdr release asset `herdr-linux-aarch64` | `sha256:17d7536f2da50859523680ef279dffcac8e4a235f4e46fe8da6c2ec44141897c` | (local arm64 runs) |
| gliff | `0.4.3` = `9d93732ae3fd4319e071feeecf38ab2cfc19edfc` | behaviour reference (`.vendor/gliff`), contract test |
| VHS | `v0.12.1` (container image `ghcr.io/charmbracelet/vhs:v0.12.1`) | README demo recording |
| shellcheck | `v0.11.0` (container image `docker.io/koalaman/shellcheck:v0.11.0` when not on PATH) | `scripts/*.sh`, `src/check/remote.sh` |
| `actions/checkout` | `v7` | CI |
| `actions/upload-artifact` / `download-artifact` | `v7` / `v8` | CI |
| `Swatinem/rust-cache` | `v2` | CI |
| `dtolnay/rust-toolchain` | `master` with `toolchain:` input | CI (MSRV job) |
| `softprops/action-gh-release` | `v3` | release |
| `taiki-e/install-action` | `v2` | installs `cargo-about` (0.9.3) in CI |
| cargo-deny (via `EmbarkStudios/cargo-deny-action@v2`) | 0.21.0 | licence and advisory check |
| actionlint (container `docker.io/rhysd/actionlint:1.7.12`) | 1.7.12 | workflow lint |
| `EmbarkStudios/cargo-deny-action` | `v2` | licence and advisory check |
| herdr `min_herdr_version` | `0.9.0` | first release with saved machines (`herdr machine list`); popup panes (0.7.4), `plugin pane open --env` (≤ 0.7.5) are older |
| Hyprland tested | `0.56.2` (Lua dispatch) and the legacy dispatcher syntax of earlier releases | see 03 §Focus |

**Containers:** wherever this spec says to run a tool image, use `podman` when it works (rootless podman is what the dev machine has; its Docker daemon is not running), else `docker`. Scripts pick the first of `podman`, `docker` that answers `<rt> info`. Use fully qualified image names (`docker.io/...`, `ghcr.io/...`).

## Architecture decisions

1. **One Rust package, a library and a binary.** `src/lib.rs` holds everything; `src/main.rs` is
   ten lines that call `gliffherdr::cli::main()`. Tests use the library directly.
2. **Every external command goes through the `Sys` trait** (`src/sys.rs`). Production code never
   calls `std::process::Command` outside `src/sys/real.rs`. Unit tests use `FakeSys`; CLI
   integration tests put stub executables on `PATH`. This is the main testability seam.
3. **No async runtime.** Checks run on plain threads (at most 12) and report through
   `std::sync::mpsc`. The picker polls terminal events with a 50 ms timeout and drains the
   channel each tick.
4. **The remote check is one ssh call running a POSIX `sh` script** fed on stdin
   (`src/check/remote.sh`, embedded with `include_str!`). Nothing is installed on the remote.
5. **One gliff window per machine, each with its own `XDG_CONFIG_HOME`.** gliff is not
   single-instance and reopens every machine its config lists as open (gliff
   `crates/gliff/src/main.rs` lines 225–245 and 505–516 at the pinned commit). Sharing one
   config between windows duplicates sessions. Font, GTK and dconf settings are symlinked in.
6. **An open window is found by its title**, `<target> — Gliff`, with class `gliff`, through
   `hyprctl clients -j`. Connecting to a machine whose window is open focuses it instead.
7. **Launch like Omarchy does**: `uwsm-app -a gliff -d "Gliff: <target>" -- …` when
   `uwsm check is-active` succeeds, otherwise directly; in both cases the child is started in a
   new session (`setsid`) with stdin from `/dev/null` and output to a per-window log, and never
   waited for. Hyprland's `exec` dispatcher is not used (its syntax changed in 0.56).
8. **The picker is our own ratatui TUI**, not fzf: rows update in place as checks finish.
9. **Configuration is one TOML file** in herdr's plugin config dir; state is JSON files in
   herdr's plugin state dir. When herdr's env vars are absent the same dirs are computed with
   herdr's own path rules, so `gliffherdr` run from a shell shares the cache with the plugin.
10. **The herdr CLI is the only herdr API used** (through `HERDR_BIN_PATH`), never the raw socket.
11. **Distribution:** herdr's `[[build]]` runs `scripts/install.sh`, which downloads the
    prebuilt static binary for the manifest version from GitHub Releases and checks its SHA-256,
    or builds from source with cargo when the checkout is not exactly the release tag or no
    prebuilt exists. The binary lands in `bin/gliffherdr` (gitignored).

## Behavioural decisions (⚑ = differs from the earlier bash prototype)

1. ⚑ **Host keys follow gliff.** The check adds `-o StrictHostKeyChecking=accept-new` exactly
   when `ssh -G -- <target>` prints `stricthostkeychecking ask`, as gliff does
   (`crates/gliff-transport/src/ssh.rs` `host_key_checking_is_default`). A changed key still
   fails. The bash prototype refused unknown keys although gliff would accept them.
2. ⚑ **`check` replaces `probe`** in every user-facing name (subcommand, actions, docs).
3. ⚑ **One config file, `config.toml`,** replaces the bash prototype's `config` + `hosts`. No migration: the prototype
   was never released.
4. ⚑ **Per-machine settings** (`[[machine]]`: label, mode, output, server_bin, gliff_args).
5. ⚑ **Fix-it remedies** run in a herdr split pane (`fix` entrypoint): install gliff-server,
   copy the ssh key, accept a new host key. Changed host keys are explained, never fixed
   automatically.
6. ⚑ **"This pane's machine" action** (`connect-pane`, context `pane`): connects to the
   destination of an `ssh` process running in the focused pane, else opens the picker.
7. ⚑ **Picker rows do not move while it is open** except when the filter changes or after a
   full re-check (Ctrl+R) completes; results update rows in place.
8. ⚑ **Connect errors stay inside the picker** as a panel with the hint and, when there is one,
   the fix key. The bash prototype printed and waited for a key.
9. A machine is listed once however many sources name it; its row shows every source.
10. Tailscale peers that are offline, or whose OS is iOS or Android, are not listed; macOS and
    Windows peers are listed as unsupported without an ssh attempt.
11. `~/.ssh/config` hosts with user `git`, or a host name matching the git-host list in 02, are
    not listed.
12. Checks happen: on herdr start for all non-Tailscale machines with a stale result
    (`check.on_startup`), on picker open for every stale machine, on Ctrl+R for all, and before
    connecting when the cached result is stale or a failure other than "unsupported".
13. A result is fresh for `check.fresh_for_secs` (default 600).
14. Desktop notifications are herdr notifications (`herdr notification show`), used only for the
    `recheck` action's summary and for failures of actions that have no terminal.

## In scope (0.1.0)

Everything in specs 02–08: the picker, `list`, `check`, `connect`, `fix`, `doctor`, `config`,
`open-picker`, demo mode, the herdr manifest and its five entrypoints, install script,
real-ssh and real-herdr end-to-end suites, CI, release workflow, README, docs, community files.

## Out of scope (0.1.0)

- Non-Hyprland targets (VNC, RDP, macOS Screen Sharing). They are listed as unsupported.
- Opening a machine as a tab in an already open gliff window (gliff has no IPC for it).
- Wake-on-LAN, hiding machines from the picker by key, shell completions, Windows/macOS hosts
  for the plugin itself, a docs website, npm/crates.io publishing (GitHub Releases only).
- Installing anything on remote machines except through the explicit, interactive fix pane.

## §Changelog

Append `### <milestone>` headings with one line per deviation and its reason. Parallel branches
append separate headings; resolve conflicts by keeping both.

### Foundation

- `RealSys::spawn_detached` appends to the log (creates it when missing) instead of truncating: 03 §launch writes a header line before spawning. A thread reaps the child.
- `config/template.toml` has three uncommented section headers, not four: an uncommented `[[machine]]` would be an invalid entry without `target`.
- `check::classify(out, ssh_timeout_secs)` takes `check.ssh_timeout_secs` (the `t` of the table) and adds the 10 s itself, so the message reads `no answer within <t+10>s`.
- `target::resolve` fallback keeps the user written in the target (else `-`): with `-` for everyone, `a@h` and `b@h` merged whenever `ssh -G` failed.
- `sources.hide` is applied after merging against every target and label merged into a machine, so hiding an ssh alias also hides the herdr entry for the same machine.
- A merged Tailscale peer supplies `os_hint` to a machine that has none; a peer with empty `OS` has no hint.
- A report without an `os=` line is `Status::Error("the machine returned no system report")`, not `Unsupported` with an empty OS.
- Missing herdr or tailscale binaries are silent, other source failures are warnings; the gliff source reads `machines` only; Tailscale candidates are sorted by target.
- `herdr_component` ports herdr's complete function (empty id, reserved stems, 120-character cap with SHA-256 suffix) using a small in-file SHA-256, no new dependency.
- `config init` prints `already exists: <path>` when the file exists; `check` with no selection prints `nothing to check` on stderr (as the bash prototype did); `list --json` `os` falls back to the OS hint.
- Added beyond 02: `src/text.rs` (column helpers shared by `list` and the picker), `Paths::in_dir`, `paths::write_atomic`, `roster::bare_machine`, `Roster::find`, `check::{status_from_report, ssh_command, overall_timeout_secs}`, `status::{os_name, status_name, status_detail}`, `cli::{Io, CliError, run}`, `tests/harness.rs`.
- Test harness strips `XDG_*`, `HYPRLAND_*`, `SSH_*`, `WAYLAND_DISPLAY` and `DISPLAY` from the parent environment too (beyond `HERDR_*`/`GLIFFHERDR_*`), and offers `World::minimal_path` because the developer's real `PATH` contains `gliff-server`.

### Desktop

- Tailscale source: peers whose `Tags` contain `tag:ingress` (Funnel ingress nodes, `ShareeNode`, empty `DNSName` and `OS`, host name `funnel-ingress-node`) and peers with both an empty `DNSName` and an empty `OS` are not machines and are dropped; a peer with an empty `OS` but a DNS name is still listed as checkable.
- `fix::describe(remedy, target: &str) -> String` (03 had `describe(remedy) -> &'static str`): the description names the target, so it cannot be static. `fix::run(sys, paths, config, &Machine, out, wait) -> FixReport` is the shared runner; it also waits for Enter when there is no remedy, so the `fix` pane does not close over its message. `fix` exits 1 without a `gliffherdr:` line when the machine does not end ready (`CliError::quiet`).
- `LaunchPlan` has a fifth field `real_config_home`: `launch` needs it to link fonts and GTK settings and 03's struct had no place for it. `launch` truncates the log (header line) before `spawn_detached` appends.
- `Herdr` also holds the plugin id (`HERDR_PLUGIN_ID`, else `PLUGIN_ID`) for `--plugin`, as the bash prototype's `open-picker` did; added `Herdr::{bin, plugin_id, version}`, `HerdrError::{NotInstalled, Spawn, TimedOut, Failed, Unreadable}`, `herdr::{parse_machines, pane_from_env, pane_from_context}`. The context JSON pane id is `focused_pane_id` (herdr's `PluginInvocationContext`), then `pane.pane_id`, `pane.id`, `pane_id`.
- Added beyond 03: `hypr::open_targets(sys)` (recovers the session itself; used by `list` and meant for the picker's refresh), `Hypr::version`, `Graphical::var`, `Ctx::new` and `Ctx: Clone`, `launch::{valid_output_name, rfc3339, ScreenChoice::from_mode}`, `connect::ConnectError: Display + Error`.
- `connect --from-pane`: an explicit `PANE_ID` beats `HERDR_PANE_ID`, which beats the context JSON; no pane id or no ssh process opens the picker; a `process-info` failure is a notification and exit 1, not a picker.
- `connect` CLI: a bad target, bad `--output` name or `--choose` is exit 2 (usage); the messages are lowercase `not an ssh destination: <t>: <why>` (as `check`), `not a screen name: <n>`. The bash prototype's `--from-picker`/`--query` flags are gone (the picker passes the target).
- The CLI rewrites the picker's `Press ctrl-f to …` hint to ``Run `gliffherdr fix <t>` to …`` (`cli::connect::cli_hint`); `ConnectError.hint` keeps the picker wording.
- `check --notify` adds a fourth summary part `<d> unsupported` (05 lists three) so a roster of only macOS peers does not read `nothing to check`; a failing notification exits 1.
- `wait_for_window` stops at the sum of its 250 ms sleeps or, if `hyprctl` itself hangs, `within` + 1 s of wall clock (`Sys::now_unix`); with `FakeSys` only the first applies.
- `from_herdr` warnings keep their earlier wording through `Herdr::machines` (`machine list failed: <message>` now carries herdr's JSON `error.message` when there is one); a timed-out herdr reads `herdr did not answer within 5s`.
- Harness stubs extended (`tests/common/mod.rs`): `hyprctl version -j` prints JSON, `herdr --version` prints a version, `herdr pane process-info` prints `$STUB/process-info.json`. The obsolete `cli_check::check_notify_is_not_built_yet` was removed.

### CI and release

- `.gitignore` lists `.vendor` instead of `.vendor/`: the worktrees symlink `.vendor`, and a trailing-slash pattern matches directories only, so `git status --porcelain` was never empty there.
- CI `lint` also runs `cargo doc --no-deps --locked` with `RUSTDOCFLAGS=-D warnings` and the full marker scan of the gate's step 10 (`TODO|FIXME|todo!(|unimplemented!(|dbg!(|not built yet`), not only `TODO|FIXME` as 08 lists: CI should run the whole definition of done that does not need a display. The `test` job runs `cargo test --locked` as 08 says; `INSTA_UPDATE=no` and `CI=true` are set for the whole workflow.
- The MSRV job runs `cargo +1.88 check`, not a bare `cargo check`: `rust-toolchain.toml` (1.99.1) overrides the toolchain `dtolnay/rust-toolchain` installs as default. Verified locally that the tree checks on 1.88.0.
- e2e jobs set `TMPDIR` to `$RUNNER_TEMP/gliffherdr-e2e` and upload it on failure; this only captures what the suites leave behind.
- `deny.toml`: licence allowlist exactly as 08 (no additions were needed: the tree is MIT, Apache-2.0, MPL-2.0, Unicode-3.0, Zlib); `yanked = "deny"`, `wildcards = "deny"`, only crates.io and no git sources; checked against the four Linux targets. `cargo deny check` is clean (duplicate `syn` is a warning).
- `about.toml` has no `no-clearly-defined` key (cargo-about 0.9.3 rejects it; it is off unless configured); `cargo about` must be built with `--features cli`; the workflow uses `taiki-e/install-action` for it, and runs `generate --locked --fail`.
- `scripts/gate.sh` gained `--skip-e2e` (steps 6 and 7 are printed as skipped) for machines without sshd or herdr; CI never uses it.
- actionlint in the container needs `--user 0` (rootless podman cannot read 0600 files otherwise) and explicit workflow paths (a worktree's `.git` file points outside the mount).
- `release.yml` has the `publish` job also check that exactly two tarballs were downloaded, and the `verify` job check that the CHANGELOG section is non-empty; the build job asserts the binary is statically linked and runs `--version` before packing.

### herdr integration and end-to-end tests

- No product bug was exposed by the real-OpenSSH and real-herdr suites (check classification, accept-new, the startup hook paths, the `recheck`/`connect`/`connect-pane` actions, `Herdr::{version, machines, notify, open_pane, pane_processes}` and the `fix` pane all behaved as specified), so `src/` is unchanged.
- `scripts/install.sh` goes slightly beyond 05: an unknown `GLIFFHERDR_BUILD` value exits 1 (`GLIFFHERDR_BUILD must be source or prebuilt, not <v>`), an architecture other than x86_64/aarch64 builds from source even when `GLIFFHERDR_BUILD=prebuilt`, the binary is copied to `bin/gliffherdr.new` and renamed (an upgrade never leaves a half-written executable), and each failed download/checksum step prints its own `gliffherdr: …` line before the fallback or the final error. Checksum verification requires the tarball's line to be present in `SHA256SUMS` (grep) as well as `sha256sum -c --ignore-missing`, because `--ignore-missing` alone can pass when nothing was verified.
- `tests/e2e_ssh.rs` gives each alias its own loopback address (`127.0.0.<n>`, sshd listens on all of them): `roster` lists a machine once per `user@host:port`, so ten aliases of one address collapse into one row in `list --json --check`. Per-alias `check` is unaffected.
- `tests/e2e_ssh.rs`: `silent` (accepts, never speaks) ends as `Offline "Connection timed out during banner exchange"` after about 1 s, not `no answer within 11s`: ssh's own `ConnectTimeout` covers the banner exchange and wins. The 11 s overall timeout of 02 is exercised by an extra alias `stall` (sends an SSH banner, then goes silent), which ends `Offline "no answer within 11s"`. Each fake machine's `PATH` is only its own `bin/` (symlinks to `sh id sed tr head cat` plus stubs), because this machine's `/usr/bin` has a real `gliff-server` and `hyprctl`. Extra test: `fix noserver` over real ssh ends exit 1 with the install remedy's own message.
- `tests/e2e_herdr.rs`: the plugin dir's `bin/gliffherdr` is a recording `sh` wrapper that runs `bin/gliffherdr.real` (a symlink to `CARGO_BIN_EXE_gliffherdr`) instead of a bare symlink. It writes argv, `HERDR_PLUGIN_*`, `HERDR_PANE_ID` and the context JSON of every invocation to a file, which is the only way to observe a popup command that exits faster than a test can poll `/proc`, and it proves 05's claims (popups get no `HERDR_PANE_ID`, the pane is in the context JSON, startup gets `HERDR_PLUGIN_EVENT=startup` and herdr's own state dir).
- `tests/e2e_herdr.rs`: a headless server has no workspace, and then `open-picker` fails with `no active workspace` (asserted as its own test, exit 1 with that herdr message); the popup tests create a workspace first. `popup.close` has no CLI form, so the test sends it over the session's API socket. The popup test asserts a running `gliffherdr pick` process, then `popup.close`, then that the process is gone.
- `tests/e2e_herdr.rs` machines are `ssh://e2e@127.0.0.1:<closed port>` with every roster source switched off in its `config.toml` (a first experiment with only `herdr = false` let the developer's real Tailscale peers into `check --all`). Pane shells run with `SHELL=/bin/sh` (a user's zsh starts its new-user wizard in the empty `HOME` and swallows typed commands). Extra tests: `connect-pane` against a real `ssh` in a real pane (real `pane process-info`, check, failure on stderr), and the library's `Herdr` wrapper plus the `fix` split pane driven against the real server.
- The user's `herdr` on `PATH` may be a launcher script that resets `HERDR_SESSION`/`HERDR_SOCKET_PATH` and execs `$HOME/.local/share/herdr/bin/herdr`: the suite symlinks that directory from the real `HOME` into the isolated one when the binary is not an ELF file, and drives plugin-side calls with the `HERDR_BIN_PATH` herdr itself reports. herdr's sockets live in `$XDG_CONFIG_HOME/herdr/sessions/<session>/` and must fit `sun_path`; with a long `TMPDIR` the suite points `XDG_CONFIG_HOME` at a short `/tmp/ge-*` symlink (removed on drop).
- Both e2e suites keep their temp dir and print its path when a test fails (CI uploads `$TMPDIR`); passing runs clean up. The harness (`tests/common/mod.rs`) gained `background_command` and `World::env_pairs` for long-running children (sshd, the herdr server): the clippy ban on `Command::new` has one more allowed place, in the harness.
- `tests/herdr_contract.rs` also checks the entry-point ids against what the code opens, placements and popup sizes, `PLUGIN_ID`, the exact build/startup commands, and that the manifest uses only keys herdr documents. herdr's action-context enum also has `selection` (05 lists four contexts); the test requires only that ours are a subset.

### Picker

- `App::new(start: Startup, now)` replaces 04's `App::new(ctx, now)`: the app performs no I/O, so `picker::load` (roster, caches, open windows, `in_herdr`) builds a `Startup` and the app only reads it. `App::reload(Startup)` serves `Ctrl+E`. Added to the app beyond 04: `on_progress`, `on_failure`, `set_now`, `invalidate`, `take_checks` (the machines whose check the app asked for: startup, `Ctrl+R`, the chooser, reload), `take_dirty`, and read accessors for the view. `Effect::{CheckAll, StartChooserCheck}` carry no machines; the runner takes them from `take_checks`.
- `demo::ctx()` is only the `FakeSys` context and temp paths; `demo::{startup, script, recheck_script, machines}` hold the fixture and its timings (the snapshot tests use them with a fixed clock, `demo::NOW`). The demo clock is frozen at `demo::NOW`, so `Checked 2 min ago` stays put.
- Demo connect: a not-ready machine fails with the real sentence and remedy (shows the error panel) instead of 04's "then exits 0"; `studio` (window open) exits at once, ready and unchecked machines show the progress steps 400 ms apart and exit 0. `Ctrl+F` and `Ctrl+E` in demo mode show a notice in the error panel instead of running anything.
- List columns are separated by two spaces (04 says one): with content-sized columns one space made `kiosk.tail.ts.net Hyprland not running` read as one phrase in the recording. The status column is a fixed 21 columns (the longest fixed text, `macOS: needs Hyprland`) so columns do not shift when results arrive. A machine without a label shows its target (bold) in the target column and leaves the label column blank.
- Selected row: `REVERSED` across the whole list width except the glyph cell, which is not reversed so its tone shows as its own colour (reversed it was a black glyph in a coloured square). Text on the selected row is not dimmed or tone-coloured (tone text inside the bar was loud).
- Narrow layout: the error panel is up to several lines above the footer (04: it replaces the one sentence line, which cannot hold message, hint, fix line and log); a blank line separates the notes from the footer when there is room. Wide layout: the chooser is centred over the list, not the whole terminal, so it never covers the details panel.
- While connecting, `Esc` stops waiting (the connect result is dropped unless it succeeded, which still closes the picker) and `Ctrl+C`/`Ctrl+Q` still leave; 04 says all input except Esc is ignored.
- `Enter` on a machine known not to run Hyprland (macOS, Windows) shows its sentence in the error panel and makes no `connect` call (which would ssh to it). `Ctrl+O` on the synthetic `↵ connect to` row opens the chooser without outputs.
- The footer says `esc clear` while a filter is typed, `esc stop waiting` while connecting, and `↑↓ choose · enter select · esc cancel` in the chooser. A first frame (`loading machines…`) is drawn before the roster is collected.
- `connect --choose`: cancelling is `gliffherdr: cancelled`, exit 1; without a terminal it is exit 2 `--choose needs a terminal`. `pick` without a terminal is exit 1 `pick needs a terminal`. `CliError::not_built` is gone (nothing is unbuilt).
- `docs/demo.tape` has one more scene than 08 lists (connecting to `lab` shows the error panel). `scripts/demo.sh` builds for `x86_64-unknown-linux-musl` (`rustup target add` first) because the VHS image's glibc is older than the host's.

### Hardening

- Window lookup is by resolved `user@host:port` as well as by title: `Hypr::find_window` matches an exact `<target> — ` title first, then resolves (`ssh -G` through `target::resolve`) only the remaining gliff windows and compares their title targets with the requested target. `Hypr::open_targets` (and so `hypr::open_targets`) returns each title target plus its resolved `user@host:port`; `Machine::is_open` tests both the machine's target and its resolved key. This changes 03's one-line `find_window`/`open_targets` comments only; the picker's `window open` marker and `list` use it. The troubleshooting tip about a second window is removed.
- `release.yml` also runs on `workflow_dispatch` with a `dry_run` boolean input (default true): `verify` and `build` run as for a tag, the `publish` job still downloads, checksums and extracts the notes, and only the `softprops/action-gh-release` step is skipped. `verify` takes the version from `Cargo.toml` (`v<version>`) for the notes check, so it works off a tag; `dry_run=false` off a tag fails in `verify`.
- cargo-deny has no container image (`docker.io/embarkstudios/cargo-deny:0.21.0` does not exist): `scripts/deny.sh` runs `cargo-deny` from `PATH`, else installs 0.21.0 with `cargo install --locked` into `/tmp/gliffherdr-deny-cargo` (its own `CARGO_HOME`) and runs `check`. It is not a gate step: advisories are fetched live and CI runs the check as its own job.
- The picker shows `Roster.warnings`: `Startup` has `unreadable_sources: usize`, and when there is no configuration banner the banner slot shows one DIM line `<N> sources could not be read — gliffherdr doctor` (`1 source …` in the singular; 04 had no such line).
- The picker does not repeat the fix: where the `ctrl-f  <remedy>` line is shown (the details panel, the error panel) the hint loses its `Press ctrl-f to …` sentence (`status::without_fix_sentence`: `Or set server_bin in config.toml.`, `gliff needs a key login without a prompt.`, nothing for the host-key hint). The one-line narrow note and the CLI (`cli_hint`) keep the full hint.
- The picker's status column is as wide as the longest status among the rows (at most 21, `macOS: needs Hyprland`), not a fixed 21, so the sources column fits when no long status is listed. It therefore changes width when a result changes the longest status; rows still do not move.
- `list` leaves the label column blank for a machine without a label instead of repeating its target; `--json` already had `label: null`. The picker already did this.
- `scripts/install.sh` mode rule (replaces 05 §install.sh step 2 and the source-build clause of Architecture decision 11): `GLIFFHERDR_BUILD=source|prebuilt` still wins; otherwise the prebuilt binary for the manifest version is downloaded and verified (source only for an architecture without one); when that release asset is missing or fails verification and `cargo` exists the script builds from source, else it fails with the existing message. The exact-tag test (`git describe --exact-match`) and the `this checkout is not the v$VERSION release` warning are gone: `herdr plugin install owner/repo` installs the default branch, which is rarely a tag, so the old rule compiled on almost every install. CONTRIBUTING now requires any change to the manifest's commands or to a CLI the manifest calls to bump the version, so a released binary never meets a manifest it does not support.

### Private repository

- `scripts/install.sh`: when the anonymous download of the release assets fails and the `gh` CLI is installed, it tries `gh release download v<version> --repo jaden-hoenes/gliffherdr` (a private repository's assets need a login), before the source fallback. Only for the default GitHub URL unless `GLIFFHERDR_GH=1`; a checksum mismatch is never retried. The repository is private at first.
- `release.yml` builds both architectures on `ubuntu-24.04`: aarch64 is cross-compiled for `aarch64-unknown-linux-musl` with `rust-lld` as the linker (pure Rust, no C) and smoke-tested with `qemu-aarch64-static`, because GitHub's arm64 runners are not available to private repositories. `ci.yml` tests on `ubuntu-24.04-arm` only when the repository is public.
