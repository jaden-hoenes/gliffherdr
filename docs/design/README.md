# Design notes

These files describe the design of gliffherdr 0.1: the decisions, the module surfaces, the
picker, the herdr integration, the command line, the tests and the release setup. They were
written before the code and kept up to date through `00-decisions.md` §Changelog, which records
every place the implementation departs from the original design and why.

The code wins where they disagree. Treat these notes as the reasoning behind the code, not as a
specification it is checked against.

| File | Contents |
|---|---|
| [00-decisions.md](00-decisions.md) | names, pinned versions, architecture and behaviour decisions, scope, §Changelog |
| [01-workspace.md](01-workspace.md) | repository layout, `Cargo.toml`, scripts, the vendored reference sources |
| [02-core.md](02-core.md) | the command seam, paths, targets, config, machine sources, checks, row status |
| [03-desktop.md](03-desktop.md) | the gliff contract, Hyprland, launching, connect, fix, the herdr wrapper |
| [04-picker.md](04-picker.md) | the picker and demo mode |
| [05-herdr.md](05-herdr.md) | the herdr manifest, entry points, install script |
| [06-cli.md](06-cli.md) | commands, flags, outputs, exit codes |
| [07-testing.md](07-testing.md) | test layers, isolation from a live herdr, the real-ssh and real-herdr suites |
| [08-docs-ci-release.md](08-docs-ci-release.md) | README, docs, demo recording, CI, release |

Behaviour references, read by the contract tests from `.vendor/` (`scripts/vendor.sh`): herdr
v0.9.5 (`2ecf19a`, Apache-2.0) and gliff 0.4.3 (`9d93732`, MIT). No code from either is copied;
herdr's plugin path-component rule is re-implemented from its description and test vectors.
