# 06 — Command line

clap derive, `#[command(name = "gliffherdr", version, about)]`. `pub struct Cli` and
`pub enum Command` live in `src/cli.rs` and are public (the herdr contract test parses manifest
argv with them).

Exit codes: `0` success; `1` the operation failed (connect failed, a fix did not end ready,
doctor found a failure); `2` usage error or invalid config (clap's own usage errors already
exit 2). Errors print `gliffherdr: <message>` to stderr, then the hint on the next line indented
two spaces.

| Command | Flags | Behaviour |
|---|---|---|
| `pick` | — | the picker (04). With `GLIFFHERDR_DEMO=1`, demo mode |
| `open-picker` | — | 05 |
| `connect <TARGET>` | `--mirror` \| `--headless` \| `--output <NAME>` \| `--choose` (mutually exclusive); `--from-pane [<PANE_ID>]` (instead of TARGET) | 03 §connect. Progress lines on stderr when it is a terminal (`Checking j@10.0.0.2…`). `--choose` shows the picker's screen chooser full-screen. Prints `Focused the open window for <t>.` or `Opened <t>.` |
| `list` | `--json`, `--check` (check all first), `--check-stale` | table or JSON, sorted by `(rank, index)` |
| `check [TARGET…]` | `--all`, `--stale`, `--startup`, `--quiet`, `--notify` | with targets: check exactly those (they need not be in the roster). `--all`: every checkable. `--stale`: every checkable whose cached result is missing or not fresh. `--startup`: 05. Output: one line per target `<target>  <status-short>  <detail>` unless `--quiet` |
| `fix [TARGET]` | — | 03 §fix |
| `doctor` | `--json` | 06 §doctor |
| `config` | subcommands `path`, `init`, `check` | `path` prints the config file path; `init` writes the template if missing and prints the path (`already exists` otherwise, exit 0); `check` loads it and prints `ok` or the error (exit 2) |

## `list` output

Table (stdout; colours only when stdout is a terminal and `NO_COLOR` is unset):

```
  ● j        j@10.0.0.2         ready                 herdr
  – air      air.tail.ts.net    macOS: needs Hyprland tailscale
```

Same column rules as the picker list. `--json`: an array of

```json
{"target":"j@10.0.0.2","label":"j","sources":["herdr"],"state":"ready","status":{"kind":"ready"},
 "detail":null,"os":"Linux","outputs":["HDMI-A-1"],"server_version":"gliff-server 0.4.3",
 "checked_at":1795058137,"window_open":false}
```

`state` is the `RowState` in snake case (`open`, `ready`, `checking`, `not_checked`,
`needs_setup`, `unreachable`, `unsupported`); `status` is the cached `Status` or `null`. The
JSON shape is documented in `docs/configuration.md` §Scripting and is stable from 0.1.0.

## `doctor`

Checks, each `ok` / `warn` / `FAIL` with one line:

| Check | FAIL / warn when |
|---|---|
| `gliff` on PATH, and its version (`gliff --version`) | FAIL missing |
| `ssh` on PATH | FAIL missing |
| Hyprland reachable (`graphical_env` + `hyprctl version`) | FAIL |
| launch method | info: `uwsm-app` or `setsid` |
| herdr CLI (`HERDR_BIN_PATH` or PATH) and `herdr --version` | warn missing |
| config file | FAIL invalid; info path (or "defaults, no file") |
| state dir writable | FAIL |
| roster | info `N machines (a herdr, b config, …)` and each source warning as warn |
| running inside herdr | info yes/no |

Exit 1 if any FAIL. `--json`: `[{"check":…,"level":"ok|warn|fail|info","message":…}]`.
