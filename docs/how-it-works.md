# How it works

gliffherdr is a herdr plugin: one static binary, `gliffherdr`, that herdr runs for a key press, a
popup and a startup hook. It never talks to herdr's socket; it only calls the `herdr` command.

## The check

A check asks one question about one machine: could gliff open a window on it? It is one ssh call.
First, locally, `ssh -G -- <target>` (3 second limit) tells which host-key policy ssh would use.
Then:

```sh
ssh -T -o BatchMode=yes -o ConnectTimeout=5 -o LogLevel=ERROR \
    [-o StrictHostKeyChecking=accept-new] -- <target> sh -s
```

`ConnectTimeout` is `check.ssh_timeout_secs`. `StrictHostKeyChecking=accept-new` is added exactly
when `ssh -G` says `stricthostkeychecking ask`, which is what gliff does too, so a machine gliff
would connect to is a machine the check can reach. A host key that has changed is never accepted.
The whole call is killed after `ssh_timeout_secs` plus 10 seconds.

The script is sent on stdin and runs in `sh` on the machine, so nothing is installed there and the
login shell does not matter. When you set `server_bin`, a first line `server_bin='...'` is sent
before it. The script prints `key=value` lines:

| Line | Comes from |
|---|---|
| `os` | `uname -s` |
| `hyprland` | `1` when `hyprctl version` works (with `-i 0`, the first running instance, because an ssh login has no `HYPRLAND_INSTANCE_SIGNATURE`), else `1` when `pgrep -x Hyprland` finds a process |
| `outputs` | the screen names from `hyprctl monitors` |
| `gliff_server` | `command -v gliff-server`; `custom` when `server_bin` is set |
| `server_version` | the first line of `gliff-server --version` |

It sets `XDG_RUNTIME_DIR` to `/run/user/<uid>` when the login did not, as gliff-server does. Each
command reads `/dev/null` so none can swallow the rest of the script. Read it:
[`src/check/remote.sh`](../src/check/remote.sh).

### What the result means

| Status | Shown as | When |
|---|---|---|
| `ready` | `● ready` | `os` is `Linux`, Hyprland runs, `gliff-server` is found |
| `no_hyprland` | `▲ Hyprland not running` | Linux, but no Hyprland session |
| `no_server` | `▲ gliff-server missing` | Hyprland runs, `gliff-server` is not found |
| `unsupported` | `– macOS: needs Hyprland` | `os` is not `Linux`. Tailscale peers whose OS is macOS or Windows are marked this way without any ssh call; iOS and Android peers are not listed |
| `offline` | `✕ offline` | ssh could not connect: timeout, refused, no route, unreachable, closed, reset, or the name does not resolve |
| `auth` | `▲ ssh key refused` | ssh printed `Permission denied` |
| `host_key` | `▲ unknown host key` or `▲ host key changed` | `Host key verification failed`, or `REMOTE HOST IDENTIFICATION HAS CHANGED` |
| `error` | `✕` and ssh's last line | anything else, including ssh missing, or a machine that ran no script |

A check that runs out of time is `offline`. Otherwise ssh's stderr is matched in this order: changed
host key, unknown host key, permission denied, unresolvable name, connection problems, anything else.
[Troubleshooting](troubleshooting.md) has the fix for each.

### Caching, and when checks run

Every result is saved as one small JSON file per machine (see
[where things live](configuration.md#where-things-live)). A result is fresh for
`check.fresh_for_secs` (600 seconds by default); fresh results are not repeated. `list` and the
picker show cached results immediately and never wait for the network. At most 12 checks run at
once.

| When | Which machines |
|---|---|
| herdr starts (startup hook, `check.on_startup`) | those with a missing or stale result, except machines that only Tailscale knows. Silent. |
| the picker opens | every machine with a missing or stale result. Rows update in place as answers arrive. |
| `Ctrl+R` in the picker, or the `recheck` action | all checkable machines. `recheck` ends with a herdr notification such as `3 ready · 1 need setup · 1 unreachable`. |
| before connecting | the machine, when its result is missing, stale, or a failure other than `unsupported` (so a machine you just fixed does not need a manual re-check) |
| `gliffherdr check ...`, `list --check`, `list --check-stale` | what you name |

Machines whose OS is known not to be Linux are never contacted.

## Connecting and the window model

gliff is not a single-instance application. Every launch is a new process and a new window, and on
start gliff reopens every machine its config lists as open, then the machine you named. Started
twice with one config, the second window would also reopen the first machine's session.

So gliffherdr gives each machine its own gliff config directory,
`<state>/windows/<machine>/config`, and starts gliff with `XDG_CONFIG_HOME` pointing there. A
window opens exactly one machine and cannot drag another one's session along. The directory gets
symlinks to your `fontconfig`, `gtk-4.0`, `gtk-3.0` and `dconf` settings; gliff takes its theme from
`$HOME`, so windows still follow your Omarchy theme.

To connect to a machine:

1. gliff must be installed, and a Hyprland session must be found.
2. If a gliff window is open for the target, focus it and stop. A window is recognised by its class,
   `gliff`, and its title, `<target> — Gliff`, from `hyprctl clients -j`. A window opened under
   another spelling of the same machine (`j` and `j@10.0.0.2`) is found too: when no title matches
   exactly, the title targets of the other gliff windows are resolved with `ssh -G` and compared by
   `user@host:port`.
3. Check the machine if the cached result is missing, stale or a failure. Not `ready` means
   stop with the sentence and fix described in [troubleshooting](troubleshooting.md).
4. Pick the screen: `--mirror`, `--headless` or `--output` (or the picker's choice), else the
   machine's `output`, else its `mode`, else `[connect] mode`.
5. Start gliff, then wait up to 8 seconds (`GLIFFHERDR_WINDOW_WAIT_MS`) for its window to appear.
   The machine is added to `recent`.

### The launch

```sh
uwsm-app -a gliff -d "Gliff: <target>" -- \
    env XDG_CONFIG_HOME=<state>/windows/<machine>/config \
    gliff [gliff_args] [--server-bin <cmd>] [--headless | --output <name>] -- <target>
```

`uwsm-app` is used when it is installed and `uwsm check is-active` succeeds (Omarchy's default).
Each window then gets its own systemd scope named `Gliff: <target>`, the way Omarchy starts apps.
Otherwise `env ... gliff ...` runs directly. Either way the process starts in a new session
(`setsid`), reads `/dev/null`, writes to `<state>/windows/<machine>/gliff.log`, and is never waited
for, so it outlives the picker, the herdr pane and herdr itself. The log's first line is the time
and the command.

Hyprland's `exec` dispatcher is not used: its syntax changed in Hyprland 0.56.

### Finding your desktop

herdr runs plugin commands in its own environment. A herdr server that was started over ssh, or
from a unit, has no `WAYLAND_DISPLAY`. gliffherdr then reads it from the systemd user manager
(`systemctl --user show-environment`: `WAYLAND_DISPLAY`, `HYPRLAND_INSTANCE_SIGNATURE`, `DISPLAY`,
`XDG_CURRENT_DESKTOP`, `XDG_SESSION_TYPE`), and failing that from `hyprctl instances`, and passes
those variables to every `hyprctl` call and to gliff. `XDG_RUNTIME_DIR` defaults to
`/run/user/<uid>`. With no session found, `connect` says
`No Hyprland session found to open a window in.` Run `gliffherdr doctor` to see what it found.

### Focus

Hyprland 0.56 replaced its dispatcher syntax with Lua. To focus a window, gliffherdr first runs
`hyprctl dispatch 'hl.dsp.focus({ window = "address:0x..." })'`. If Hyprland does not answer `ok`,
it runs the older `hyprctl dispatch focuswindow address:0x...`. So both Hyprland 0.56 and earlier
releases work. Window addresses are validated before they are used.

## The herdr entry points

Declared in [`herdr-plugin.toml`](../herdr-plugin.toml). herdr runs each as a command with the plugin
directory as its working directory.

| Entry point | Kind | Runs | What it does |
|---|---|---|---|
| `connect` | action, global | `gliffherdr open-picker` | Asks herdr to open the `picker` popup. If herdr refuses (for example `ui_busy`), it shows herdr's message as a notification. |
| `picker` | popup pane, 72% by 60% | `gliffherdr pick` | The picker. |
| `connect-pane` | action, pane | `gliffherdr connect --from-pane` | Asks herdr what runs in the focused pane; if it is `ssh`, connects to that destination (`-p` and `-l` are understood) and reports failures as notifications. Otherwise opens the picker. |
| `recheck` | action, global | `gliffherdr check --all --notify` | Checks every machine and reports the outcome as a notification. |
| `fix` | split pane | `gliffherdr fix` | Runs the remedy for the machine in `GLIFFHERDR_FIX_TARGET`, which the picker sets when you press `Ctrl+F`. |
| startup | hook | `gliffherdr check --startup --quiet` | Checks stale machines when herdr starts. |

Run an action from the command line with
`herdr plugin action invoke jaden-hoenes.gliffherdr.<id>`.

Everything that opens a window or runs a remedy happens on an explicit key press. The startup hook
and `recheck` only check: they change nothing on a remote machine (a first-contact host key is
recorded in `known_hosts`, as gliff would).

## Fixes

`gliffherdr fix <target>` (and `Ctrl+F` in the picker) checks the machine again and, if there is a
remedy, runs it with your terminal attached so passwords and prompts work:

| Status | Remedy |
|---|---|
| `no_server` | `ssh -t -- <target> '...omarchy pkg add gliff...'`, if the machine has the `omarchy` command |
| `auth` | `ssh-copy-id <target>` |
| `host_key`, unknown | `ssh -o StrictHostKeyChecking=ask -- <target> true`, which shows the key and asks |

Then it checks again and says what it found. A changed host key has no remedy: it is explained,
never fixed. Inside herdr, `Ctrl+F` opens the `fix` pane as a split; outside herdr, the picker steps
out of its screen, runs the fix in your terminal and comes back.
