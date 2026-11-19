# Security policy

## What gliffherdr runs

gliffherdr is a herdr plugin that lists machines you can reach over ssh, checks them, and opens a
[gliff](https://github.com/omacom/gliff) window for one. To do that it:

- runs `ssh` to every machine it lists, non-interactively (`BatchMode=yes`), to run one short
  POSIX `sh` script that only reads (`uname`, `hyprctl`, `gliff-server --version`). Nothing is
  installed or written on the remote machine;
- accepts an unknown host key on first contact (`StrictHostKeyChecking=accept-new`), exactly when
  gliff itself would. A changed host key always fails and is never fixed automatically;
- never passes a target, label or config value through a shell. Targets are validated and always
  handed to `ssh` after `--`, so they cannot be read as options;
- starts `gliff` as a detached process, in its own session, with a private config directory per
  machine;
- runs a "fix" (install `gliff-server`, copy your ssh key, accept a host key) only in a herdr pane,
  after an explicit key press, as an ordinary interactive `ssh` or `ssh-copy-id` you can watch.

`config.toml` is trusted input: it can name a custom `server_bin` and extra `gliff` arguments, which
run as you. It is read from herdr's plugin config directory and is never fetched from anywhere.

The release binary is downloaded by `scripts/install.sh` from this repository's GitHub Releases and
checked against the release's `SHA256SUMS` before it is installed.

## Reporting a vulnerability

Please report it privately through GitHub:
**Security tab, then "Report a vulnerability"** on this repository
(GitHub security advisories). Do not open a public issue for something exploitable.

Include what you ran, what you expected, and what happened. I aim to answer within a week and to
ship a fix, with credit if you want it, before details are public.

## Supported versions

Only the latest release receives fixes.
