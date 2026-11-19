# 04 — The picker (flagship)

The picker is what users judge the project by. It runs in herdr's popup (72 % × 60 % of the
terminal), and also in any terminal (`gliffherdr pick`). It must look calm and native in
Omarchy's themes, never flicker, and never block on the network.

## Visual design

- **Colours: the terminal's 16-colour palette only** (`Color::Green`, `Yellow`, `Red`, `Cyan`,
  `Blue`, `Reset`) plus `Modifier::DIM`/`BOLD`/`REVERSED`. No RGB, no 256-colour indices: the
  picker inherits the Omarchy/herdr theme automatically. Tone → style: Good = Green, Accent =
  Cyan, Busy = Yellow, Warn = Yellow, Bad = Red, Muted = DIM.
- **No outer border.** herdr draws the popup frame; a second border wastes two columns.
- **Selection:** the selected row is `REVERSED` across its full width; the glyph keeps its tone.
- **Typography:** labels `BOLD`; targets normal, `DIM` when a label exists; sources `DIM`.
- **Matched characters** of the filter are `BOLD | UNDERLINED` in label and target.
- **Spacing:** one blank column left and right of every line; one space between columns.

## Layout

Wide (width ≥ 96 columns):

```
 remote desktop › re█                                                         5 machines
 ▣ studio   studio.lan          window open           herdr tailscale │ studio
 ● j        j@10.0.0.2          ready                 herdr           │ studio.lan
 ⠹ render   render@10.0.0.7     checking…             config          │
 ▲ lab      lab@10.0.0.9        gliff-server missing  ssh             │ Status   window open
 ✕ nas      nas.lan             offline               recent          │ System   Linux
 – air      air.tail.ts.net     macOS: needs Hyprland tailscale       │ Screens  DP-1 DP-2
                                                                      │ Server   gliff-server 0.4.3
                                                                      │ Opens    mirror (focused screen)
                                                                      │ Checked  2 min ago
                                                                      │
 enter connect · alt-enter private screen · ctrl-o screen · ctrl-r re-check · ctrl-e config · esc close
```

- Line 1: prompt `remote desktop › ` + filter text + cursor; right-aligned count
  (`N machines`, or `M of N` while filtering).
- List: columns glyph (1) · label (≤ 14) · target (≤ 30) · status (≤ 22) · sources (rest).
  Widths shrink in that priority (sources first, then status, then target) and truncate with
  `…` using `unicode-width`.
- Details panel: right 34 columns, separated by `│` (DIM). Shows label (BOLD), target, blank,
  then `Status`, `System`, `Screens`, `Server`, `Opens`, `Checked`, `Sources`; rows without data
  are omitted. When the selected machine has a status sentence/hint (not Ready/Open), it is
  shown under the fields, wrapped, with the hint DIM. If a remedy exists, a last line
  `ctrl-f  <describe(remedy)>` (key in Cyan).
- Footer: key hints, DIM, keys not dimmed. `ctrl-f fix` appears only when the selected machine
  has a remedy. Hints that do not fit are dropped from the right.

Narrow (< 96 columns): no details panel; the line above the footer shows the selected machine's
sentence (or `Opens mirror…` when ready), truncated.

Very small (< 40 columns or < 8 rows): a single centred line `make the window larger`.

## States and overlays

| State | What shows |
|---|---|
| Browsing | the layout above |
| Empty roster | list area shows, centred: `No machines found yet.` / `Type user@host and press Enter, or add machines to config.toml (ctrl-e).` |
| Filter matches nothing, filter is a valid target | one synthetic row `↵ connect to <filter>` (glyph `↵`, Accent); Enter connects to it |
| Filter matches nothing, not a valid target | centred `No match.` |
| Connecting | the selected row's glyph spins and its status reads `checking…`, then `opening…`, then `waiting for the window…`; input except Esc is ignored |
| Connect failed | an error panel replaces the details panel (wide) or the sentence line (narrow): message (Red, BOLD), hint, `ctrl-f <describe>` if a remedy exists, `log: <path>` if any; Esc or any movement returns to browsing |
| Screen chooser (ctrl-o) | a centred box (width 44) titled `<label> › screen`, options `Mirror the focused screen`, one `Mirror <output>` per known output, `Private screen sized to the window`; ↑/↓, Enter picks, Esc cancels. Outputs come from the cached check; if there is none, a check runs first (spinner in the box) |
| Config error | a one-line Red banner above the list: `config.toml line 4: unknown field "colour" — ctrl-e to edit`; defaults are used |

On success (`Focused` or `Launched`) the picker exits 0 and herdr closes the popup.

## Keys

| Key | Action |
|---|---|
| printable chars | append to filter |
| Backspace / Ctrl+H | delete last char |
| Ctrl+U | clear filter |
| Ctrl+W | delete last word |
| ↑ ↓, Ctrl+P Ctrl+N, Ctrl+K Ctrl+J | move selection (wraps) |
| PgUp PgDn, Home End | page / first / last |
| Enter | connect with the machine's default screen choice |
| Alt+Enter | connect with `Headless` |
| Ctrl+O | screen chooser |
| Ctrl+F | fix (only when a remedy exists): in herdr, `open_pane("fix", Some("split"), [("GLIFFHERDR_FIX_TARGET", t)], focus=true)` then exit 0; outside herdr, leave the alternate screen, run `gliffherdr fix <t>` inline (same process, `fix::run`), re-enter, reload that machine's cache |
| Ctrl+R | check every checkable machine again; when all finish, re-sort |
| Ctrl+E | edit config: leave the alternate screen, run `$VISUAL`, else `$EDITOR`, else `nvim`, else `vi` on `config.toml` (written from the template first if missing), re-enter, reload config and roster, re-sort |
| Esc | in an overlay: close it; else clear the filter if not empty; else exit 0 |
| Ctrl+C, Ctrl+Q | exit 0 |
| mouse | left click selects a row; double click connects; wheel moves selection |

## Behaviour

- Startup: load config (errors → banner), collect the roster, read every cache file, read
  `open_targets` (if Hyprland is reachable), sort by `(rank, collection index)`, render. The first
  frame must appear before any network call. Then start `check_many` for machines where
  the cached result is missing or not fresh (`!is_fresh`; Tailscale peers included).
- Rows never move except when the filter changes or a Ctrl+R round completes (00 ⚑7). The
  selection follows the machine (by target), not the index.
- Every 2 s, refresh `open_targets` in a background thread.
- Tick: 50 ms terminal-event poll; drain check events and timers; redraw only when something
  changed (spinner frames count as changes while anything is checking).
- Filter: `nucleo-matcher` with `Pattern::parse(filter, CaseMatching::Smart, Normalization::Smart)`
  over the haystack `"<label> <target> <sources>"`; order matches by score, ties by rank then
  index.
- Terminal hygiene: alternate screen, raw mode, mouse capture, hidden cursor; restored on every
  exit path including panics (a panic hook that restores the terminal, then prints the panic).

## Module structure

```rust
pub struct App { /* roster, caches, row states, filter, selection, overlay, checking set, open set, banner, pending connect */ }
impl App {
    pub fn new(ctx: Ctx, now: u64) -> App;
    pub fn handle_key(&mut self, key: KeyEvent) -> Effect;      // pure state transition
    pub fn handle_mouse(&mut self, ev: MouseEvent) -> Effect;
    pub fn on_check(&mut self, ev: CheckEvent);
    pub fn on_open_targets(&mut self, open: HashSet<String>);
    pub fn on_connect(&mut self, result: Result<Connected, ConnectError>) -> Effect;
    pub fn tick(&mut self);                                     // advances the spinner
}
pub enum Effect { None, Quit, Connect(ConnectRequest), CheckAll, OpenFixPane(Target), FixInline(Target), EditConfig, StartChooserCheck(Target) }
pub fn draw(frame: &mut Frame, app: &App);                    // view.rs; pure function of App
pub fn run(ctx: Ctx) -> anyhow::Result<()>;                   // picker.rs: terminal setup, event loop, effect execution
```

`App` never performs I/O; `run` executes effects (connect runs on a thread and reports back
through a channel so the spinner keeps turning). This split is what makes snapshot tests
possible (07).

## Demo mode

`GLIFFHERDR_DEMO=1` replaces the real `Ctx` with `demo::ctx()`: a `FakeSys`-backed context whose
roster is the seven machines below, with no network, and whose clock and checks are scripted.
It exists for the README recording and for snapshot tests; it must never touch the user's real
config, state or windows (paths point into a temp dir created at start).

| Label | Target | Sources | Initial | After |
|---|---|---|---|---|
| studio | `studio.lan` | herdr tailscale | Open (window open) | — |
| j | `j@10.0.0.2` | herdr | Ready (outputs `HDMI-A-1`) | — |
| render | `render@10.0.0.7` | config | Checking | Ready (outputs `DP-1 DP-2`) after 1.2 s |
| kiosk | `kiosk.tail.ts.net` | tailscale | Checking | NoHyprland after 0.8 s |
| lab | `lab@10.0.0.9` | ssh | NoServer | — (ctrl-f shows the fix description) |
| nas | `nas.lan` | recent | Checking | Offline (`no answer within 15s`) after 2.0 s |
| air | `air.tail.ts.net` | tailscale | Unsupported (macOS) | — |

Connecting in demo mode shows the real progress states with 400 ms between them, then exits 0
printing nothing. Focusing `studio` exits immediately. No real gliff is launched.

## Quality bar

- Snapshot every state in the table above at 100×22 (wide) and 72×18 (narrow) — see 07.
- Record the demo with VHS (08) and look at the frames: alignment of columns, truncation,
  spinner, the details panel, the chooser box. Fix anything that looks off and record again.
  At least three review-and-fix passes.
- No panics on any terminal size from 1×1 up (a property test over sizes 1..=200 × 1..=60 on
  the demo `App` renders without panicking).
