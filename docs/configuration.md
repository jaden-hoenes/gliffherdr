# Configuration

gliffherdr reads one file, `config.toml`. It is optional: without it every default below applies.

```sh
gliffherdr config path     # where the file is (or would be)
gliffherdr config init     # write a commented template there; never overwrites
gliffherdr config check    # validate it: prints "ok", or the error and exits 2
```

In the picker, `Ctrl+E` opens the file in `$VISUAL`, else `$EDITOR`, else `nvim`, else `vi`
(writing the template first when there is no file), then reloads it.

A mistake does not break anything: unknown keys and bad values are errors, the picker shows the
first one as a red line above the list (`config.toml line 4: unknown field ...`) and uses the
defaults until you fix it. `list`, `check`, `connect` and the other commands refuse to run and exit
2 until the file is valid.

## Keys

### `[connect]`

Defaults for every window gliffherdr opens.

| Key | Type | Default | Meaning |
|---|---|---|---|
| `mode` | `"mirror"` or `"headless"` | `"mirror"` | What Enter opens. `mirror` shows the remote machine's focused screen. `headless` opens a private screen that exists only for your session and follows the window size. |
| `gliff_args` | array of strings | `[]` | Extra arguments for every `gliff` launch, for example `["--no-clipboard"]`. |
| `server_bin` | string | unset | The `gliff-server` command to run on the remote machine (gliff's `--server-bin`), for example `"gliff-server --low-bandwidth"`. Empty means unset, which means plain `gliff-server`. When it is set, checks do not look for `gliff-server` on the machine: its presence is trusted. |

### `[sources]`

Where machines are listed from. See [Where machines come from](../README.md#where-machines-come-from).

| Key | Type | Default | Meaning |
|---|---|---|---|
| `herdr` | bool | `true` | Machines saved in herdr. |
| `recent` | bool | `true` | Machines you connected to before (the last 30). |
| `gliff` | bool | `true` | Machines in gliff's own config (`~/.config/gliff/config.toml`). |
| `ssh_config` | bool | `true` | `Host` entries of `~/.ssh/config` and the files it includes. |
| `tailscale` | bool | `true` | Online Tailscale peers. |
| `hide` | array of strings | `[]` | Machines never listed, matched by target or label, case-insensitive. |

`[[machine]]` entries are a source too and cannot be switched off.

### `[check]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `on_startup` | bool | `true` | Check machines in the background when herdr starts. Machines that only Tailscale knows are left for the picker. |
| `fresh_for_secs` | integer, at least 1 | `600` | How long a result counts as fresh. Fresh results are not checked again. |
| `ssh_timeout_secs` | integer, 1 to 60 | `5` | ssh's `ConnectTimeout` for a check. A check as a whole is given this plus 10 seconds. |

### `[[machine]]`

Settings for one machine. Repeat the table for more machines. Only `target` is required.

| Key | Type | Default | Meaning |
|---|---|---|---|
| `target` | string | required | Anything ssh accepts: `user@host`, an ssh alias, `ssh://user@host:2222`. |
| `label` | string | none | The name shown in the picker. |
| `mode` | `"mirror"` or `"headless"` | `[connect] mode` | Overrides `[connect] mode` for this machine. |
| `output` | string | none | Always mirror this remote screen (for example `"HDMI-A-1"`). Overrides `mode`. Letters, digits, `.`, `_` and `-` only. |
| `server_bin` | string | `[connect] server_bin` | Overrides `[connect] server_bin`. |
| `gliff_args` | array of strings | `[connect] gliff_args` | Replaces `[connect] gliff_args` (it does not add to it). |

A machine entry also lists the machine when no other source does.

What Enter opens, in order: the choice you made in the picker (`Alt+Enter`, `Ctrl+O`) or on the
command line (`--mirror`, `--headless`, `--output`, `--choose`); else the machine's `output`; else
the machine's `mode`; else `[connect] mode`.

### How a `[[machine]]` entry finds its machine

An entry applies to a listed machine when its `target` is written exactly like the machine's
target, or when the entry spells out `user@host` (or `ssh://user@host:port`) and that user, host and
port equal the machine's resolved ones (`ssh://j@10.0.0.2:22` and `j@10.0.0.2` are the same
machine). The entry's text is not looked up in your ssh config: an entry for the alias `devbox`
matches a machine listed as `devbox`, not one listed as `j@10.0.0.2`. The first matching entry in
the file wins, and its `label` is shown for the machine.

A machine is listed once however many sources name it. Two sources that resolve (through
`ssh -G`) to the same user, host and port are one row, and Tailscale peers are matched to existing
machines by host name or address.

### `hide`

`hide = ["backup-nas", "10.0.0.5"]` removes every machine that has any of those as its target or
label, however many sources named it. Matching is case-insensitive and on the whole text, not part
of it. It applies after the sources are merged, so hiding the ssh alias also hides the herdr entry
of the same machine. It also hides machines that have a `[[machine]]` entry.

## Environment variables

| Variable | Effect |
|---|---|
| `GLIFFHERDR_CONFIG_DIR` | Directory holding `config.toml`. Beats everything below. |
| `GLIFFHERDR_STATE_DIR` | Directory for the cache, `recent.json` and window state. Beats everything below. |
| `GLIFFHERDR_DEMO=1` | `gliffherdr pick` shows made-up machines and touches nothing. |
| `GLIFFHERDR_FIX_TARGET` | The machine `gliffherdr fix` works on when no argument is given. The picker sets it for the fix pane. |
| `GLIFFHERDR_WINDOW_WAIT_MS` | How long `connect` waits for gliff's window to appear, in milliseconds. Default `8000`. |
| `GLIFFHERDR_BUILD` | For `scripts/install.sh`: `source` or `prebuilt` forces how the binary is installed. |
| `GLIFFHERDR_RELEASE_BASE_URL` | For `scripts/install.sh`: replaces `https://github.com/jaden-hoenes/gliffherdr/releases/download`. |
| `GLIFFHERDR_GH` | For `scripts/install.sh`: `1` or `0` forces or forbids the `gh release download` fallback (used by default only for the real GitHub URL, when the anonymous download fails). |
| `NO_COLOR` | Set to anything non-empty: `list` and `doctor` print no colour. The picker only uses your terminal's own 16 colours. |
| `VISUAL`, `EDITOR` | The editor for `Ctrl+E`. They may carry arguments (`code -w`). |

gliffherdr also reads variables that herdr and your session set:

| Variable | Used for |
|---|---|
| `HERDR_PLUGIN_CONFIG_DIR`, `HERDR_PLUGIN_STATE_DIR` | The plugin's directories when herdr runs it. |
| `HERDR_PLUGIN_ID` | The plugin id for `herdr plugin pane open` (default `jaden-hoenes.gliffherdr`). |
| `HERDR_BIN_PATH` | The herdr binary to call (default `herdr` on `PATH`). |
| `HERDR_ENV=1` | Tells the picker it runs inside herdr, so `Ctrl+F` opens a pane instead of fixing in place. |
| `HERDR_PANE_ID`, `HERDR_PLUGIN_CONTEXT_JSON` | The focused pane, for `connect --from-pane`. |
| `XDG_CONFIG_HOME`, `XDG_STATE_HOME`, `HOME` | Where the directories below are when nothing overrides them. |
| `WAYLAND_DISPLAY`, `HYPRLAND_INSTANCE_SIGNATURE`, `XDG_RUNTIME_DIR` | Finding your Hyprland session. When they are missing (herdr started over ssh) they are recovered, see [how it works](how-it-works.md#finding-your-desktop). |

## Where things live

Inside herdr the directories arrive in `HERDR_PLUGIN_CONFIG_DIR` and `HERDR_PLUGIN_STATE_DIR`.
Run from a shell, gliffherdr computes the same directories with herdr's own rules, so a shell and
the plugin share one config and one cache. The order is `GLIFFHERDR_*_DIR`, then `HERDR_PLUGIN_*_DIR`,
then the default:

| What | Default |
|---|---|
| Config | `~/.config/herdr/plugins/config/jaden-hoenes.gliffherdr/config.toml` |
| Check results | `~/.local/state/herdr/plugins/jaden-hoenes.gliffherdr/checks/<machine>.json` |
| Recent machines | `~/.local/state/herdr/plugins/jaden-hoenes.gliffherdr/recent.json` |
| One window's gliff config and log | `~/.local/state/herdr/plugins/jaden-hoenes.gliffherdr/windows/<machine>/` (`config/`, `gliff.log`) |

`~/.config` and `~/.local/state` follow `XDG_CONFIG_HOME` and `XDG_STATE_HOME`. `<machine>` is the
target in lowercase with unusual characters replaced by `_`, a dash and eight hex digits.
`herdr plugin config-dir jaden-hoenes.gliffherdr` prints the config directory.

Deleting `checks/` only means machines are checked again.

## Scripting

### `gliffherdr list --json`

Prints an array, best machines first (the order of the picker). It makes no ssh connection; it
reports the last cached check. Add `--check` to check every machine first, or `--check-stale` to
check only those without a fresh result.

```json
[
  {
    "checked_at": 1795058137,
    "detail": null,
    "label": "j",
    "os": "Linux",
    "outputs": ["HDMI-A-1"],
    "server_version": "gliff-server 0.4.3",
    "sources": ["herdr"],
    "state": "ready",
    "status": { "kind": "ready" },
    "target": "j@10.0.0.2",
    "window_open": false
  }
]
```

| Field | Type | Meaning |
|---|---|---|
| `target` | string | What is passed to ssh and gliff. |
| `label` | string or null | The name from herdr, Tailscale or your config. |
| `sources` | array of strings | `config`, `herdr`, `recent`, `gliff`, `ssh`, `tailscale`. |
| `state` | string | What the picker shows, see below. |
| `status` | object or null | The last check's result, `null` if there is none. `kind` is `ready`, `no_hyprland`, `no_server`, `unsupported` (with `os`), `offline` (with `detail`), `auth`, `host_key` (with `changed`, true when a different key is on record) or `error` (with `detail`). |
| `detail` | string or null | A short explanation of `status`, `null` when ready or unchecked. |
| `os` | string or null | `uname -s` of the machine; for Tailscale peers never checked, the peer's OS. |
| `outputs` | array of strings | The machine's Hyprland screens, for `--output`. Empty when unknown. |
| `server_version` | string or null | First line of `gliff-server --version`. |
| `checked_at` | integer or null | Unix time of the last check. |
| `window_open` | bool | A gliff window for this exact target is open now. |

`state` is one of `open`, `ready`, `not_checked`, `needs_setup` (`no_hyprland`, `no_server`, `auth`,
`host_key`), `unreachable` (`offline`, `error`) or `unsupported`. Open windows come first, then ready
machines, and so on in that order.

The shape is stable from 0.1.0: fields are only added, never renamed or removed within 0.x.

Examples:

```sh
gliffherdr list --json | jq -r '.[] | select(.state == "ready") | .target'
gliffherdr list --json --check-stale | jq -r '.[] | select(.state == "needs_setup") | .target'
```

### Other machine-readable output

- `gliffherdr doctor --json` prints `[{"check":"gliff","level":"ok","message":"..."}]`. `level` is
  `ok`, `warn`, `fail` or `info`.
- `gliffherdr check [TARGET...]` prints one line per machine, `<target>  <status>  <detail>`, with
  the status names `ready`, `no-hyprland`, `no-server`, `unsupported`, `offline`, `auth`, `hostkey`
  and `error`. `--quiet` prints nothing.

### Exit codes

| Code | Meaning |
|---|---|
| `0` | Success. For `check`: it ran, whatever it found; read the output or `list --json`. |
| `1` | The operation failed: `connect` could not open or focus the window, `fix` did not leave the machine ready, `doctor` found a `FAIL`, `pick` or `open-picker` could not run. |
| `2` | Usage error (clap's own, or a bad target or screen name), or an invalid `config.toml`. |

Errors go to stderr as `gliffherdr: <message>`, followed by the hint on the next line, indented by
two spaces. `connect` prints `Opened <target>.` or `Focused the open window for <target>.` on
stdout, and progress lines on stderr only when stderr is a terminal.
