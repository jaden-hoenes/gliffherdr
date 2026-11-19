# Troubleshooting

Start with the machine's row. Select it in the picker: the panel on the right says what is wrong
and what to do. From a shell, `gliffherdr check <target>` re-checks one machine and
`gliffherdr connect <target>` prints the same sentences and hints.

Then run `gliffherdr doctor` (see [below](#doctor)). If you open an issue, include its output.

## One section per status

### `ready`

Nothing to fix. If a window still does not open, see
[the window never appears](#the-window-never-appears).

### `▲ Hyprland not running`

> `<target>: Hyprland is not running there.`

The machine answered, but no Hyprland session is running on it. gliff mirrors a running session, it
does not start one. Log in at that machine (or start Hyprland there), then press `Ctrl+R`.

### `▲ gliff-server missing`

> `<target>: gliff-server is not installed there.`

gliff needs `gliff-server` on the machine. Select it and press `Ctrl+F`, or run
`gliffherdr fix <target>`. That runs `omarchy pkg add gliff` on the machine over ssh and needs the
`omarchy` command there. On another distribution, install gliff there following
[gliff's instructions](https://github.com/omacom/gliff#install).

If `gliff-server` is installed but not on the `PATH` ssh gives you (it is checked in a
non-interactive `sh`), give its full path in `config.toml`:

```toml
[[machine]]
target = "lab@10.0.0.9"
server_bin = "/opt/gliff/bin/gliff-server"
```

With `server_bin` set, the check trusts that it exists and gliff is started with
`--server-bin`.

### `▲ ssh key refused`

> `<target> refused the ssh key.`

gliff and the check use ssh without prompts (`BatchMode`), so the machine must accept a key. Select
it and press `Ctrl+F` (runs `ssh-copy-id`; type the password once), or run
`gliffherdr fix <target>`.

If you already have a key on that machine and still see this, the key is probably protected by a
passphrase and not loaded: run `ssh-add`, or check that `ssh -o BatchMode=yes <target> true` works in
your terminal. Wrong user is the other usual cause: write the target as `user@host`.

### `▲ unknown host key`

> `<target> has a host key ssh does not know.`

With ssh's default setting, gliffherdr accepts a key it has not seen before on first contact, the
way gliff does, so you only see this when your ssh config says `StrictHostKeyChecking yes` for that
host. Press `Ctrl+F`: it runs `ssh` interactively so you can compare the fingerprint and accept it.

### `▲ host key changed`

> `<target>'s host key has changed.`

The key on record in `~/.ssh/known_hosts` is not the one the machine presented. It can mean the
machine was reinstalled. It can also mean someone is in the middle. gliffherdr never fixes this for
you. If you are sure the machine was reinstalled, remove the old entry and check again:

```sh
ssh-keygen -R <host>
```

### `✕ offline`

> `<target> is not reachable: <why>.`

ssh could not connect. The reason is ssh's own:

| Reason | What to try |
|---|---|
| `cannot resolve the host name` | Typo in the name, or the name is only known to another resolver (Tailscale MagicDNS needs Tailscale running). Try `ping <host>`. |
| `Connection refused` | The machine is up but nothing listens on the ssh port. Start `sshd`, or check the port in the target (`ssh://user@host:2222`). |
| `Connection timed out`, `No route to host`, `Network is unreachable` | The machine is off, asleep, or not on your network. For a Tailscale peer, check it is online. |
| `no answer within 15s` | ssh did not finish in time: a stalled connection, or a login that waits for input. Try `ssh <target> true`. |
| `Connection closed`, `Connection reset` | Something dropped the connection, often a firewall or an sshd limit. |

Raise `check.ssh_timeout_secs` for slow links.

### `– macOS: needs Hyprland`

Not a fault. gliff shows a Hyprland session, so macOS and Windows machines can be listed (they come
from Tailscale) but are never contacted. They cannot be connected to with gliffherdr. Hide them with
`hide` in `config.toml` if they are in the way.

### `✕` followed by ssh's own words (`error`)

> `<target>: <message>`

ssh failed in a way that none of the above covers, and `<message>` is the last line it printed.
Run the same thing by hand to see it all:

```sh
ssh -T -o BatchMode=yes -- <target> true
```

`ssh is not installed` means exactly that. `the machine returned no system report` means ssh
worked but the machine did not run the check script, for example because a `ForceCommand` or a
restricted shell ignores the command it is given.

## The picker does not open

Pressing the key does nothing, or you see a notification titled `Remote desktop`.

- **`ui_busy`**: herdr will not open a popup while Settings, Copy mode or another modal is open.
  Close it and press the key again.
- **`no active workspace`**: a popup needs a workspace. Open one.
- **Nothing at all**: check the binding is loaded and the plugin is installed:

  ```sh
  herdr server reload-config
  herdr plugin list
  herdr plugin action list --plugin jaden-hoenes.gliffherdr
  ```

  The key's `command` must be `jaden-hoenes.gliffherdr.connect`.
- **The log of what herdr ran**:

  ```sh
  herdr plugin log list --plugin jaden-hoenes.gliffherdr
  ```

  Each entry has the command, its status, exit code and stdout and stderr.

You can always run the picker from a shell: `gliffherdr pick`.

## The window never appears

> `gliff did not open a window for <target>.`

gliffherdr started gliff and waited 8 seconds. The message is followed by the last lines gliff
wrote and the path of its log:

```
<state>/windows/<machine>/gliff.log
```

(`<state>` is `~/.local/state/herdr/plugins/jaden-hoenes.gliffherdr`; the picker and `connect` print
the exact path.) The first line is the command that was run. Then:

- **Run the machine's check**: `gliffherdr check <target>` must say `ready`.
- **Run gliff's own diagnostic on the remote machine**: `gliff-probe all`. It checks the compositor,
  the GPU drivers and both video paths, and prints PASS or FAIL for each.
- **Try without video hardware**: add `gliff_args = ["--video", "cpu"]` to the machine.
- **A window that opens black**: the remote GPU may allocate buffers it never fills (a VM's
  virtio-gpu, for example). Set
  `server_bin = "gliff-server --capture shm"` for that machine.
- **gliff itself**: `gliff <target>` in a terminal shows what it says, using your own gliff config.

Other messages from `connect`:

| Message | Meaning |
|---|---|
| `gliff is not installed on this machine.` | The `gliff` command is missing here. `omarchy pkg add gliff`. |
| `No Hyprland session found to open a window in.` | Neither the environment, the systemd user manager nor `hyprctl instances` knows a session. herdr runs plugin commands on its own machine, and that machine needs a desktop. |
| `Could not focus the open window...` | Hyprland refused the focus command; the hint has its answer. |
| `Could not start gliff: ...` | The launch itself failed; the log path follows. |

## Doctor

```sh
gliffherdr doctor
```

```
ok    gliff         gliff 0.4.3 at /usr/bin/gliff
ok    ssh           /usr/bin/ssh
ok    hyprland      Hyprland 0.56.2 reachable
info  launch        uwsm-app
ok    herdr         herdr 0.9.5 (/usr/bin/herdr)
info  config        defaults, no file
ok    state         /home/you/.local/state/herdr/plugins/jaden-hoenes.gliffherdr is writable
info  roster        4 machines (2 herdr, 1 ssh, 1 tailscale)
info  inside herdr  no
```

| Line | Means |
|---|---|
| `gliff` | `FAIL` when `gliff` is not on `PATH`. |
| `ssh` | `FAIL` when `ssh` is not on `PATH`. |
| `hyprland` | `FAIL` when no session is found or `hyprctl` does not answer. |
| `launch` | `uwsm-app` or `setsid`: how windows are started. |
| `herdr` | `warn` when the `herdr` command is missing or does not answer. |
| `config` | The file in use, `defaults, no file`, or `FAIL` with the first error. |
| `state` | `FAIL` when the state directory is not writable. |
| `roster` | How many machines each source names, then one `warn` line per source that could not be read. The picker shows a dim `N sources could not be read — gliffherdr doctor` line under its header while there is one (a configuration error takes that place). |
| `inside herdr` | Whether this shell runs inside herdr. |

The exit code is 1 when any line is `FAIL`. `gliffherdr doctor --json` prints the same as JSON.
