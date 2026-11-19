# 07 — Testing

Six layers. The real-ssh and real-herdr suites are the oracles: they prove the plugin works
against the actual programs it drives, not just against our idea of them.

| Layer | Where | Runs in |
|---|---|---|
| Unit | `#[cfg(test)]` in each module, with `FakeSys` | `cargo test` |
| CLI integration | `tests/cli_*.rs`: the built binary with stub executables on `PATH` | `cargo test` |
| TUI snapshots | `tests/picker_snapshots.rs`: `App` + `draw` on `TestBackend`, `insta::assert_snapshot!` | `cargo test` |
| Contracts | `tests/gliff_contract.rs`, `tests/herdr_contract.rs`, `tests/install_script.rs` | `cargo test` (vendor needed) |
| Real OpenSSH | `tests/e2e_ssh.rs`, gated by `GLIFFHERDR_E2E_SSH=1` | gate, CI |
| Real herdr | `tests/e2e_herdr.rs`, gated by `GLIFFHERDR_E2E_HERDR=1` | gate, CI |

Gated suites print `skipped: set GLIFFHERDR_E2E_…=1` and pass when the variable is unset.

## Coverage requirements

- Every one of the 23 tests of the earlier bash prototype has a Rust counterpart with the same
  intent; renamed concepts (probe → check) follow 00.
- `classify`, `parse_report`, `ssh_destination`, `slug`, `herdr_component`, `Target::parse`,
  roster merging and every `RowState` mapping have table-driven unit tests.
- `config`: defaults, template = defaults, each validation error, unknown key error with line.
- `launch::plan`: argv for mirror/headless/output, uwsm on/off, machine vs global settings.
- `connect`: focus path, not-ready path with remedy, stale re-check, launch + window found,
  launch + no window (error with log tail), no gliff, no graphical env.
- `hypr::focus`: Lua `ok`; Lua error then legacy `ok`; both fail.
- `graphical_env`: from env; from systemd; from `hyprctl instances`; none.

## Isolation from the developer's live herdr (hard rule)

Developers run tests from inside herdr panes, whose environment carries
`HERDR_SOCKET_PATH`, `HERDR_ENV`, `HERDR_PANE_ID`, `HERDR_PLUGIN_*` and so on. A test that lets
those through could drive the developer's real herdr session. Therefore every child process a
test starts (our binary, herdr, sshd, ssh) gets an environment with **every variable starting
with `HERDR_` or `GLIFFHERDR_` removed** before the test sets its own, plus `HOME`,
`XDG_CONFIG_HOME`, `XDG_STATE_HOME`, `XDG_RUNTIME_DIR` pointing into the test's temp dir. The
harness provides `World::command()` that does this; no test builds a `Command` any other way.
A unit test asserts `World::command()` strips a planted `HERDR_SOCKET_PATH`.

## CLI integration harness (`tests/common/mod.rs`)

`World::new()` creates a temp dir with `home/`, `state/`, `config/`, `stubs/` and builds an env:
`HOME`, `XDG_CONFIG_HOME`, `XDG_STATE_HOME`, `GLIFFHERDR_CONFIG_DIR`, `GLIFFHERDR_STATE_DIR`,
`HERDR_BIN_PATH=<stubs>/herdr`, `PATH=<stubs>:<system PATH>`, `WAYLAND_DISPLAY`,
`HYPRLAND_INSTANCE_SIGNATURE`, `GLIFFHERDR_WINDOW_WAIT_MS=300`. Stubs are small `sh` scripts
written by the harness (modelled on the bash prototype's stubs): `ssh` (`-G` answers from fixture
files; checks answer from `hosts/<dest>.{out,err,rc}` and record stdin), `herdr`, `tailscale`,
`hyprctl` (logs calls; `clients -j` from a fixture; `dispatch` prints `ok` or a scripted reply),
`uwsm`, `uwsm-app` (execs everything after `--`), `gliff`, `systemctl`. Launches are real:
`spawn_detached` runs the stub chain (`uwsm-app` → `/usr/bin/env` → stub `gliff`), and the
`gliff` stub writes its argv and `XDG_CONFIG_HOME`/`WAYLAND_DISPLAY` to files, then adds its
window to the `hyprctl clients` fixture so `wait_for_window` finds it. Tests wait for the argv
file to appear (up to 2 s) because the launch is detached. Use
`assert_cmd::Command::cargo_bin("gliffherdr")`.

## TUI snapshots

Build `App` from `demo::ctx()` with a fixed clock, feed it scripted `CheckEvent`s and keys, call
`draw` on `TestBackend::new(100, 22)` and `TestBackend::new(72, 18)`, and snapshot the buffer
(`insta::assert_snapshot!(terminal.backend())`, which prints text without styles) for every row
of 04 §States, plus: filtering with highlights, the chooser, the error panel with a remedy,
selection follows the machine after a check result, Ctrl+R re-sort. Style correctness (tones,
REVERSED selection, DIM) is checked by separate unit tests that inspect `Buffer` cell styles at
known positions. Snapshots are committed; CI runs with `INSTA_UPDATE=no`.

## Real OpenSSH (`tests/e2e_ssh.rs`)

Spawns a private `sshd` as the current user — no root, no containers:

- `sshd` from `/usr/sbin/sshd` or `/usr/bin/sshd` (else skip with a message unless `CI=true`,
  then fail).
- Temp dir with an ed25519 host key, `sshd_config`: `Port <free port>`, `ListenAddress
  127.0.0.1`, `HostKey`, `PidFile none`, `UsePAM no`, `StrictModes no`,
  `PermitUserEnvironment yes`, `AuthorizedKeysFile <tmp>/authorized_keys`,
  `PasswordAuthentication no`, `KbdInteractiveAuthentication no`, `LogLevel ERROR`. Run
  `sshd -D -e -f <config>`; wait until the port accepts. Kill it on drop (by PID).
- Fake machines are client keys whose `authorized_keys` lines carry
  `environment="PATH=<tmp>/machines/<name>/bin:/usr/bin:/bin"`; each machine's `bin/` holds stub
  `hyprctl`, `gliff-server`, `uname`, `pgrep` that make it look like: `ready` (Linux, Hyprland,
  outputs `DP-1 HDMI-A-1`, server `gliff-server 0.4.3`), `nohypr`, `noserver`, `darwin`.
- Client side: a wrapper `ssh` on the test `PATH` runs the real ssh with `-F <tmp>/ssh_config`;
  `ssh_config` defines Host aliases `ready`, `nohypr`, `noserver`, `darwin` (each with its
  `IdentityFile`, `IdentitiesOnly yes`, `UserKnownHostsFile <tmp>/known_hosts`),
  `nokey` (an identity not in `authorized_keys`), `changed` (a `known_hosts` line with a
  different key for that alias via `HostKeyAlias changed`), `newhost` (empty known_hosts for it,
  default `StrictHostKeyChecking`), `strict` (same but `StrictHostKeyChecking yes`), `closed`
  (a port with nothing listening), `silent` (a local TCP listener that accepts and never
  speaks; `ssh_timeout_secs = 1` in this test's config so the overall timeout is 11 s).
- Asserts `gliffherdr check <alias>` and the JSON from `list --json --check` give: ready →
  Ready with outputs and server version; nohypr → NoHyprland; noserver → NoServer; darwin →
  Unsupported{Darwin}; nokey → Auth; changed → HostKey{changed:true}; newhost → Ready (accepted
  like gliff, and `known_hosts` now has the key); strict → HostKey{changed:false}; closed →
  Offline; silent → Offline "no answer within 11s".
- Also runs `gliffherdr fix nokey` non-interactively with a stub `ssh-copy-id` on PATH that
  appends the key, and asserts it ends Ready (exit 0).

## Real herdr (`tests/e2e_herdr.rs`)

Uses the herdr binary from `GLIFFHERDR_HERDR_BIN` (else `herdr` on PATH; else skip, or fail
when `CI=true`). Everything runs with an isolated `HOME`, `XDG_CONFIG_HOME`, `XDG_STATE_HOME`,
`XDG_RUNTIME_DIR` under a temp dir, so the user's herdr is never touched, and a session named
`gliffherdr-e2e-<pid>`.

1. Build the binary (it is `CARGO_BIN_EXE_gliffherdr`), copy the repo's manifest and a
   `bin/gliffherdr` symlink into a temp plugin dir, and `herdr plugin link <dir>`. Assert the
   link JSON lists 3 actions and 2 panes with the manifest's titles.
2. Write a `config.toml` into `herdr plugin config-dir jaden-hoenes.gliffherdr` with two
   `[[machine]]` entries pointing at closed local ports, then start `herdr --session <s> server`
   in the background and wait for `herdr --session <s> status` to report it running.
3. The startup hook ran: within 20 s, the plugin state dir
   (`$XDG_STATE_HOME/herdr/plugins/jaden-hoenes.gliffherdr/checks/`) holds two JSON files with
   status `offline`. This proves herdr's env and our path rules agree.
4. `herdr --session <s> plugin action invoke recheck --plugin jaden-hoenes.gliffherdr` exits 0
   and `plugin log list --plugin …` shows the command finished with status 0.
5. `plugin action invoke connect` → the popup runs `bin/gliffherdr pick`: assert through
   `plugin log list` that `open-picker` exited 0, and that a process whose argv is
   `<plugin>/bin/gliffherdr pick` exists (scan `/proc/*/cmdline`); then close it with
   `herdr --session <s> api` `popup.close` (or kill that PID).
6. Stop the server with `herdr --session <s> server stop`; on test failure, print the plugin log.

## Definition of done for tests

No `#[ignore]` except the two gated suites' skip path; no `todo!()`/`unimplemented!()`; no
`TODO`/`FIXME` in `src/`; `cargo test` passes twice in a row with a cold `target/`.
