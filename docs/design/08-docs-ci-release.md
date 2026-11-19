# 08 — Docs, demo recording, CI, release

## README (the storefront)

Order:

1. `# gliffherdr` + badges (CI, latest release, licence, "herdr plugin").
2. One sentence: *Open a remote desktop to any machine you can ssh to, from a picker inside
   herdr.* Then `docs/assets/picker.gif`.
3. Three-line pitch: lists every machine you already have (herdr machines, ssh config,
   Tailscale, gliff, ones you typed) · checks each over ssh and tells you what's wrong and how
   to fix it · opens each machine in its own gliff window, or focuses it if it's open.
4. **Install** — requirements (Hyprland + gliff here; Hyprland + gliff-server + key login there),
   `herdr plugin install jaden-hoenes/gliffherdr`, the keybinding block from 05, and the
   `connect-pane` binding as an optional extra.
5. **Use** — the key table from 04 (condensed), typing `user@host`, the fix key.
6. **Where machines come from** — the source table from 02 with one line each.
7. **Configuration** — a 15-line `config.toml` example (global mode, one `[[machine]]` with
   headless + output, `hide`), link to `docs/configuration.md`.
8. **How it works** — five bullets (one ssh check running a POSIX script; cache; one window
   per machine with its own gliff config; uwsm launch; host-key policy = gliff's), link to
   `docs/how-it-works.md`.
9. **Command line** — `list`, `check`, `connect`, `doctor` one-liners.
10. **Troubleshooting** — link; the three most common fixes inline.
11. **Development** — `scripts/vendor.sh`, `scripts/dev-link.sh`, `scripts/gate.sh`.
12. **Credits** — gliff (omacom), herdr (herdrdev). Licence.

No mention of how the project was built. No real hostnames or IPs except the demo fixture's.

## `docs/`

- `configuration.md`: every key of `config.toml` (table: key, type, default, meaning), the
  `[[machine]]` matching rules, `hide`, env vars (`GLIFFHERDR_*`, `NO_COLOR`), file locations,
  §Scripting (`list --json` shape, exit codes).
- `how-it-works.md`: the check (the exact ssh command and what the remote script reads), the
  status table with meanings, caching and when checks run, the window model and why (gliff
  reopens its open machines; G4/G5), launch method, env recovery, focus on Hyprland ≥ 0.56 vs
  older, the herdr entry points.
- `troubleshooting.md`: one section per status with the fix; "the picker doesn't open"
  (`ui_busy`, plugin log command); "window never appears" (`gliff.log` location,
  `gliff-probe all`); `doctor`.
- `assets/picker.gif`, `assets/picker.png` (generated, committed).

## Demo recording

`docs/demo.tape` (VHS): `Set Shell bash`, `Set FontSize 16`, `Set Width 1200`,
`Set Height 560`, `Set Theme "TokyoNight"` (Omarchy's default theme), `Set Padding 16`,
`Set TypingSpeed 60ms`; `Env GLIFFHERDR_DEMO 1`; `Hide`, run `gliffherdr pick`, `Show`; let checks
settle (2.5 s), move to `lab` (details + fix hint), type `re` (filter highlights), Backspace
×2, Ctrl+O on `j` (chooser), Esc, Enter on `studio` (focus path exits). `Screenshot
docs/assets/picker.png` on the settled list. Output `docs/assets/picker.gif`.

`scripts/demo.sh` builds release, then runs
`<podman|docker> run --rm -v "$PWD":/vhs:Z -w /vhs ghcr.io/charmbracelet/vhs:v0.12.1 docs/demo.tape` with
the binary on `PATH` inside the container (mount `target/release` and prepend it). The GIF is
not byte-stable, so CI does not regenerate it; maintainers review its frames
(extract with `ffmpeg -i picker.gif -vf "select=not(mod(n\,15))" -vsync vfr /tmp/f%02d.png`
and read them).

## CI (`.github/workflows/ci.yml`)

On push and pull_request. Jobs:

| Job | Runner | Steps |
|---|---|---|
| `lint` | ubuntu-24.04 | checkout; toolchain from `rust-toolchain.toml`; rust-cache; `cargo fmt --check`; `cargo clippy --all-targets --locked -- -D warnings`; `shellcheck scripts/*.sh src/check/remote.sh` (apt `shellcheck`); `scripts/check-versions.sh`; `! grep -rnE 'TODO|FIXME' src` |
| `test` | matrix ubuntu-24.04, ubuntu-24.04-arm | `scripts/vendor.sh`; `cargo test --locked` with `INSTA_UPDATE=no`, `CI=true` |
| `e2e-ssh` | ubuntu-24.04 | apt `openssh-server`; `sudo mkdir -p /run/sshd`; `GLIFFHERDR_E2E_SSH=1 cargo test --locked --test e2e_ssh` |
| `e2e-herdr` | ubuntu-24.04 | download `herdr-linux-x86_64` v0.9.5, verify the sha256 from 00, `chmod +x`; `GLIFFHERDR_E2E_HERDR=1 GLIFFHERDR_HERDR_BIN=… cargo test --locked --test e2e_herdr` |
| `msrv` | ubuntu-24.04 | `dtolnay/rust-toolchain` with `toolchain: 1.88`; `cargo check --locked --all-targets` |
| `deny` | ubuntu-24.04 | `EmbarkStudios/cargo-deny-action@v2` with `deny.toml` (licences: MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2-Clause, BSD-3-Clause, ISC, Unicode-3.0, Zlib, MPL-2.0; advisories deny; bans: multiple-versions warn) |
| `install-script` | ubuntu-24.04 | covered by `cargo test` (`tests/install_script.rs`); no extra job |

Every job that builds ends with `git diff --exit-code && test -z "$(git status --porcelain)"`.
On failure, the e2e jobs upload the temp dirs the tests print (gliff logs, herdr plugin logs,
sshd stderr) with `actions/upload-artifact`.

## Release (`.github/workflows/release.yml`)

On tag `v*`:

1. `verify`: `scripts/check-versions.sh` and the tag equals `v<Cargo version>`.
2. `build`: matrix `{x86_64, ubuntu-24.04}`, `{aarch64, ubuntu-24.04-arm}`; `rustup target add
   <arch>-unknown-linux-musl`; apt `musl-tools`; `cargo build --release --locked --target
   <arch>-unknown-linux-musl`; `cargo about generate about.hbs > THIRD_PARTY_LICENSES.html`
   (installed with `taiki-e/install-action`); tarball
   `gliffherdr-<v>-<arch>-linux-musl.tar.gz` containing `gliffherdr`, `LICENSE`, `README.md`,
   `THIRD_PARTY_LICENSES.html`; upload artifact.
3. `publish`: download artifacts; `sha256sum *.tar.gz > SHA256SUMS`; `softprops/action-gh-release@v3`
   with the tarballs and `SHA256SUMS`, body = the version's CHANGELOG section (extracted by
   `scripts/check-versions.sh --notes <v>`).

## Community and repo files

`LICENSE` (MIT, 00), `CHANGELOG.md` (Keep a Changelog; `## [0.1.0] - unreleased` until the
release is dated), `CONTRIBUTING.md` (setup, the gate, snapshot workflow with
`cargo insta review`, how to run the e2e suites, the decisions-changelog rule, commit style),
`SECURITY.md` (what the plugin runs: ssh to listed machines with BatchMode, accept-new host-key
policy matching gliff, no shell interpretation of targets (validated, passed after `--`),
config is trusted input, the fix pane runs only on an explicit key press; report privately via
GitHub security advisories), `CODE_OF_CONDUCT.md` (Contributor Covenant 2.1, contact via
GitHub), `.github/ISSUE_TEMPLATE/{bug_report.yml,feature_request.yml,config.yml}` (bug form
asks for `gliffherdr doctor` output and versions), `pull_request_template.md`,
`.github/dependabot.yml` (cargo weekly, github-actions weekly).
