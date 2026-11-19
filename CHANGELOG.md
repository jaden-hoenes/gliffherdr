# Changelog

All notable changes are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project follows [Semantic Versioning](https://semver.org/).

## [0.1.0] - 2026-11-18

First release.

### Added

- A picker that opens as a herdr popup (and runs in any terminal with `gliffherdr pick`): fuzzy
  filter, details panel, live status that updates rows in place, a screen chooser, mouse support,
  and a demo mode (`GLIFFHERDR_DEMO=1`). It uses only your terminal's 16 colours, so it follows
  your theme.
- Machines from herdr, `~/.ssh/config`, Tailscale, gliff's own config, recent connections and
  `[[machine]]` entries, merged so each machine is listed once with all its sources. Any source can
  be switched off and any machine hidden.
- Type any `user@host`, ssh alias or `ssh://` address to connect to a machine that is not listed.
- A check for every machine: one ssh call that reports whether Hyprland runs, which screens it has
  and whether `gliff-server` is installed. Nothing is installed on the machine. Results are cached
  and refreshed when herdr starts, when the picker opens, with `Ctrl+R`, and before connecting.
  Unknown host keys are accepted the way gliff accepts them; a changed key is reported and never
  accepted.
- One gliff window per machine, each with its own gliff config. Connecting to a machine whose window
  is open focuses it (Hyprland 0.56 and earlier). Windows start through `uwsm-app` when the session
  uses uwsm.
- Screen choice: mirror the focused screen, mirror a named screen, or a private screen sized to the
  window; per machine with `mode` and `output`, per connect with `Alt+Enter` and `Ctrl+O`.
- Plain-language explanations of what is wrong with a machine, and fixes you can run from the picker
  (`Ctrl+F`) or with `gliffherdr fix`: install gliff, copy your ssh key, accept a host key.
- herdr actions: `connect` (open the picker), `connect-pane` (open the machine the focused pane is
  ssh'd into), `recheck` (check everything and notify); a startup hook that checks machines in the
  background.
- A command line: `list` (table or `--json`), `check`, `connect` (`--mirror`, `--headless`,
  `--output`, `--choose`), `fix`, `doctor`, `config path|init|check`.
- A configuration file with a commented template (`gliffherdr config init`) and errors that name the
  line.
- Install with `herdr plugin install jaden-hoenes/gliffherdr`: a prebuilt static binary for x86_64
  and aarch64 Linux, verified against a SHA-256 checksum, downloaded through `gh` when the
  repository is private, with a source build as the fallback.

[0.1.0]: https://github.com/jaden-hoenes/gliffherdr/releases/tag/v0.1.0
