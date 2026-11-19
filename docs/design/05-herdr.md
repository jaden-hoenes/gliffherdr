# 05 — herdr integration and distribution

Reference: `.vendor/herdr/docs/next/website/src/content/docs/plugins.mdx` (herdr v0.9.5). Facts
used here: commands are argv arrays run with the plugin root as cwd and no shell; build commands
get no runtime env; startup hooks get `HERDR_PLUGIN_EVENT=startup`; popups get no
`HERDR_PANE_ID` but the underlying pane is in `HERDR_PLUGIN_CONTEXT_JSON`; action contexts are
`global`, `workspace`, `tab`, `pane` (`.vendor/herdr/src/api/schema/plugins.rs`
`PluginActionContext`); `plugin pane open` without `--placement` uses the manifest placement.

## `herdr-plugin.toml` (exact)

```toml
id = "jaden-hoenes.gliffherdr"
name = "gliffherdr"
version = "0.1.0"
min_herdr_version = "0.9.0"
description = "Remote desktop to any machine you can ssh to: a picker that opens gliff windows for your herdr machines, ssh hosts and Tailscale peers."
platforms = ["linux"]

[[build]]
command = ["sh", "scripts/install.sh"]

[[startup]]
command = ["bin/gliffherdr", "check", "--startup", "--quiet"]

[[panes]]
id = "picker"
title = "Remote desktop"
placement = "popup"
width = "72%"
height = "60%"
command = ["bin/gliffherdr", "pick"]

[[panes]]
id = "fix"
title = "Remote desktop: fix"
placement = "split"
command = ["bin/gliffherdr", "fix"]

[[actions]]
id = "connect"
title = "Remote desktop: connect to a machine"
contexts = ["global"]
command = ["bin/gliffherdr", "open-picker"]

[[actions]]
id = "connect-pane"
title = "Remote desktop: this pane's machine"
contexts = ["pane"]
command = ["bin/gliffherdr", "connect", "--from-pane"]

[[actions]]
id = "recheck"
title = "Remote desktop: re-check machines"
contexts = ["global"]
command = ["bin/gliffherdr", "check", "--all", "--notify"]
```

`tests/herdr_contract.rs` asserts: the manifest parses; ids match herdr's rules (plugin id chars
`[A-Za-z0-9.:_-]`, local ids `[A-Za-z0-9:_-]`, unique per kind); every `command[0]` is
`bin/gliffherdr` or `sh`; every subcommand used exists in the clap definition (parse each argv
with `cli::Cli::try_parse_from`); `version` equals `CARGO_PKG_VERSION`; contexts are in herdr's
enum (read from `.vendor/herdr/src/api/schema/plugins.rs`); `min_herdr_version` is `0.9.0`.

## Entry points

| Entry | Runs | Behaviour |
|---|---|---|
| action `connect` | `open-picker` | `herdr plugin pane open --plugin <id> --entrypoint picker`. On herdr error (e.g. `ui_busy`), `notify("Remote desktop", <herdr's message>)` and exit 1 |
| pane `picker` | `pick` | the picker (04) |
| action `connect-pane` | `connect --from-pane` | pane id = `HERDR_PANE_ID`, else `.pane.pane_id` / focused pane id from `HERDR_PLUGIN_CONTEXT_JSON`; `pane_processes` → first argv with an `ssh_destination` → connect to it with no terminal: on failure `notify("Remote desktop", message + " " + hint)`, exit 1. No ssh process → behave like `open-picker` with the filter preset to nothing |
| action `recheck` | `check --all --notify` | checks every checkable machine, then `notify("Remote desktop", "<a> ready · <b> need setup · <c> unreachable")` (omit zero parts; `nothing to check` when empty) |
| pane `fix` | `fix` | 03 §fix, target from `GLIFFHERDR_FIX_TARGET` (passed with `--env` by the picker) |
| startup | `check --startup --quiet` | when `check.on_startup`, checks non-Tailscale checkable machines whose cached result is missing or not fresh; prints nothing; exit 0 even when machines are unreachable |

Context JSON parsing reads only the fields it needs with `serde_json::Value` and tolerates
anything missing.

## `scripts/install.sh` (POSIX sh, shellcheck-clean)

Run by herdr from the plugin root on `herdr plugin install`. Steps:

1. `VERSION` = the `version = "…"` line of `herdr-plugin.toml`. `ARCH` = `uname -m` mapped
   `x86_64`→`x86_64`, `aarch64|arm64`→`aarch64`; anything else → source build.
2. Mode: `GLIFFHERDR_BUILD=source` → source; `GLIFFHERDR_BUILD=prebuilt` → prebuilt; otherwise
   prebuilt when the checkout is exactly tag `v$VERSION` (`git describe --exact-match --tags
   HEAD` succeeds and prints it) or when there is no `.git`; else source when `cargo` exists;
   else prebuilt with the warning `gliffherdr: this checkout is not the v$VERSION release; installing the v$VERSION binary`.
3. Prebuilt: download into a `mktemp -d` dir with `curl -fsSL` (fallback `wget -qO`):
   `https://github.com/jaden-hoenes/gliffherdr/releases/download/v$VERSION/gliffherdr-$VERSION-$ARCH-linux-musl.tar.gz`
   and `…/SHA256SUMS`; verify with `sha256sum -c --ignore-missing` (the line for that tarball
   must be present); extract `gliffherdr` to `bin/gliffherdr`, `chmod 755`. On download or
   checksum failure: fall back to source if `cargo` exists, else exit 1 with
   `gliffherdr: could not download the v$VERSION binary and cargo is not installed. Install Rust (https://rustup.rs) and run herdr plugin install again.`
4. Source: `cargo build --release --locked`; copy `target/release/gliffherdr` to `bin/gliffherdr`.
5. Print `gliffherdr <version>: installed bin/gliffherdr (<prebuilt|built from source>)`.

Environment overrides for tests: `GLIFFHERDR_RELEASE_BASE_URL` replaces
`https://github.com/jaden-hoenes/gliffherdr/releases/download`. `tests/install_script.rs`
serves a fake release from a temp dir via `file://` URLs (curl supports them) and checks: good
checksum installs; bad checksum fails (no cargo on PATH) with the message; source mode runs a
stub `cargo` from PATH.

## `scripts/dev-link.sh`

`cargo build`, `ln -sf ../target/debug/gliffherdr bin/gliffherdr`, then
`"${HERDR_BIN_PATH:-herdr}" plugin link "$PWD"` unless `--no-link`. Prints what it did.

## Keybinding (documented, never written by the plugin)

```toml
[[keys.command]]
key = "prefix+alt+r"
type = "plugin_action"
command = "jaden-hoenes.gliffherdr.connect"
description = "Remote desktop"
```

Optional second binding suggested in the README: `prefix+alt+shift+r` →
`jaden-hoenes.gliffherdr.connect-pane`.
