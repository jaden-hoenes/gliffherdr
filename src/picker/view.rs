//! Drawing the picker: a pure function of [`App`]. Terminal colours are the 16 ANSI ones and
//! the modifiers `DIM`, `BOLD`, `REVERSED` and `UNDERLINED`, so the picker takes on the theme of
//! the terminal it runs in.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use super::app::{App, ErrorPanel, Item, ListGeom, MachineItem, Overlay, sources_text};
use crate::fix::describe;
use crate::status::{RowState, Tone, glyph, short_text, tone, without_fix_sentence};
use crate::text::{pad, truncate, width};

/// Terminals narrower than this (or shorter than [`MIN_HEIGHT`]) only get a request to grow.
pub const MIN_WIDTH: u16 = 40;
/// See [`MIN_WIDTH`].
pub const MIN_HEIGHT: u16 = 8;
/// From this width on the details panel is shown.
pub const WIDE_WIDTH: u16 = 96;
/// Width of the details panel including its separator.
pub const PANEL_WIDTH: u16 = 34;

const LABEL_MAX: usize = 14;
const TARGET_MAX: usize = 30;
/// The status column's widest: `macOS: needs Hyprland`, the longest fixed text. The column is as
/// wide as the longest status of the rows, up to this.
const STATUS_WIDTH: usize = 21;
const LABEL_MIN: usize = 4;
const TARGET_MIN: usize = 8;
const STATUS_MIN: usize = 9;
const SOURCES_MIN: usize = 6;
/// Columns between the list's columns.
const GAP: usize = 2;
const PROMPT: &str = "remote desktop › ";
const CURSOR: &str = "█";
/// Width of the key column of the details panel.
const KEY_WIDTH: usize = 8;

/// Dimmed text.
pub const DIM: Style = Style::new().add_modifier(Modifier::DIM);
const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
const REVERSED: Style = Style::new().add_modifier(Modifier::REVERSED);
const MATCH: Style = Style::new()
    .add_modifier(Modifier::BOLD)
    .add_modifier(Modifier::UNDERLINED);
const CYAN: Style = Style::new().fg(Color::Cyan);
const RED_BOLD: Style = Style::new().fg(Color::Red).add_modifier(Modifier::BOLD);

/// The style of a tone: Good green, Accent cyan, Busy and Warn yellow, Bad red, Muted dim.
#[must_use]
pub fn tone_style(t: Tone) -> Style {
    match t {
        Tone::Good => Style::new().fg(Color::Green),
        Tone::Accent => CYAN,
        Tone::Busy | Tone::Warn => Style::new().fg(Color::Yellow),
        Tone::Bad => Style::new().fg(Color::Red),
        Tone::Muted => DIM,
    }
}

/// Write `text` at `(x, y)`, clipped to `clip` (which must lie inside the buffer). Returns the
/// column after the text.
pub fn put(buf: &mut Buffer, clip: Rect, x: u16, y: u16, text: &str, style: Style) -> u16 {
    let clip = clip.intersection(buf.area);
    if y < clip.y || y >= clip.bottom() || x < clip.x || x >= clip.right() {
        return x;
    }
    let room = usize::from(clip.right() - x);
    buf.set_stringn(x, y, text, room, style).0
}

/// A run of text with one style.
type Seg = (String, Style);
/// A line of the details panel.
type Line = Vec<Seg>;

fn put_line(buf: &mut Buffer, clip: Rect, x: u16, y: u16, line: &Line) -> u16 {
    let mut x = x;
    for (text, style) in line {
        x = put(buf, clip, x, y, text, *style);
    }
    x
}

/// `text` with the characters at `hl` in the matched style.
fn put_matches(
    buf: &mut Buffer,
    clip: Rect,
    x: u16,
    y: u16,
    text: &str,
    style: Style,
    hl: &[usize],
) -> u16 {
    if hl.is_empty() {
        return put(buf, clip, x, y, text, style);
    }
    let mut x = x;
    let mut run = String::new();
    let mut run_hl = false;
    for (i, c) in text.chars().enumerate() {
        let h = hl.contains(&i);
        if h != run_hl && !run.is_empty() {
            x = put(
                buf,
                clip,
                x,
                y,
                &run,
                if run_hl { style.patch(MATCH) } else { style },
            );
            run.clear();
        }
        run_hl = h;
        run.push(c);
    }
    put(
        buf,
        clip,
        x,
        y,
        &run,
        if run_hl { style.patch(MATCH) } else { style },
    )
}

/// Break `text` into lines of at most `w` columns, at spaces where possible.
fn wrap(text: &str, w: usize) -> Vec<String> {
    let w = w.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let mut word = word.to_string();
        while width(&word) > w {
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
            }
            let mut head = String::new();
            let mut rest = String::new();
            for c in word.chars() {
                if rest.is_empty() && width(&head) + width(&c.to_string()) <= w {
                    head.push(c);
                } else {
                    rest.push(c);
                }
            }
            if head.is_empty() {
                // a character wider than the whole line: give up on it
                head = rest.chars().take(1).collect();
                rest = rest.chars().skip(1).collect();
            }
            lines.push(head);
            word = rest;
        }
        if word.is_empty() {
            continue;
        }
        if cur.is_empty() {
            cur = word;
        } else if width(&cur) + 1 + width(&word) <= w {
            cur.push(' ');
            cur.push_str(&word);
        } else {
            lines.push(std::mem::take(&mut cur));
            cur = word;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// `s` cut from the left to at most `max` columns, keeping its end: `…/windows/j/gliff.log`.
fn tail_truncate(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    let mut tail: Vec<char> = Vec::new();
    let mut used = 1;
    for c in s.chars().rev() {
        let cw = width(&c.to_string());
        if used + cw > max {
            break;
        }
        used += cw;
        tail.push(c);
    }
    format!("…{}", tail.into_iter().rev().collect::<String>())
}

/// `key  text`, the text wrapped with a hanging indent under the first word.
fn hang(key: &str, key_style: Style, text: &str, text_style: Style, w: usize) -> Vec<Line> {
    let indent = width(key) + 2;
    let wrapped = wrap(text, w.saturating_sub(indent).max(1));
    wrapped
        .into_iter()
        .enumerate()
        .map(|(i, l)| {
            let lead: Seg = if i == 0 {
                (format!("{key}  "), key_style)
            } else {
                (" ".repeat(indent), Style::new())
            };
            vec![lead, (l, text_style)]
        })
        .collect()
}

/// `2 min ago`.
fn ago(now: u64, then: u64) -> String {
    let s = now.saturating_sub(then);
    match s {
        0..=4 => "just now".to_string(),
        5..=59 => format!("{s} s ago"),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86_399 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86_400),
    }
}

/// Draw the picker into the frame.
pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    draw_into(frame.buffer_mut(), area, app);
}

/// Draw the picker into `area` of `buf`.
pub fn draw_into(buf: &mut Buffer, area: Rect, app: &App) {
    let area = area.intersection(buf.area);
    app.set_geometry(None);
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        draw_tiny(buf, area);
        return;
    }
    let wide = area.width >= WIDE_WIDTH;
    let mut overlay_area = area;
    let footer_y = area.bottom() - 1;
    draw_header(buf, area, app);
    let mut top = area.y + 1;
    let note = app.sources_note();
    let banner = app
        .banner()
        .map(|b| (b, Style::new().fg(Color::Red)))
        .or_else(|| {
            note.as_deref()
                .map(|n| (n, Style::new().add_modifier(Modifier::DIM)))
        });
    if let Some((text, style)) = banner {
        let row = Rect::new(area.x, top, area.width, 1);
        put(
            buf,
            row,
            area.x + 1,
            top,
            &truncate(text, usize::from(area.width) - 2),
            style,
        );
        top += 1;
    }
    let mid_h = footer_y.saturating_sub(top);
    if wide {
        let list = Rect::new(area.x, top, area.width - PANEL_WIDTH, mid_h);
        let panel = Rect::new(list.right(), top, PANEL_WIDTH, mid_h);
        draw_list(buf, list, app);
        draw_panel(buf, panel, app);
        overlay_area = list;
    } else {
        let info = narrow_info(app, usize::from(area.width) - 2);
        // A blank line keeps the notes apart from the key hints when there is room for it.
        let spacer = u16::from(mid_h >= 6);
        let room = mid_h - spacer;
        let n = u16::try_from(info.len())
            .unwrap_or(u16::MAX)
            .min(room.saturating_sub(3).max(1));
        let list = Rect::new(area.x, top, area.width, room - n);
        draw_list(buf, list, app);
        let below = Rect::new(area.x, list.bottom(), area.width, n);
        for (i, line) in info.iter().take(usize::from(n)).enumerate() {
            put_line(
                buf,
                below,
                area.x + 1,
                below.y + u16::try_from(i).unwrap_or(0),
                line,
            );
        }
    }
    draw_footer(buf, Rect::new(area.x, footer_y, area.width, 1), app);
    if let Overlay::Chooser(c) = app.overlay() {
        c.draw(buf, overlay_area, app.frame());
    }
}

fn draw_tiny(buf: &mut Buffer, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let msg = truncate("make the window larger", usize::from(area.width));
    let x = area.x
        + area
            .width
            .saturating_sub(u16::try_from(width(&msg)).unwrap_or(0))
            / 2;
    let y = area.y + area.height / 2;
    put(buf, area, x, y, &msg, Style::new());
}

fn draw_header(buf: &mut Buffer, area: Rect, app: &App) {
    let row = Rect::new(area.x, area.y, area.width, 1);
    let (hits, total) = app.counts();
    let count = if app.filter().is_empty() {
        format!("{total} machine{}", if total == 1 { "" } else { "s" })
    } else {
        format!("{hits} of {total}")
    };
    let inner = usize::from(area.width).saturating_sub(2);
    let avail = inner
        .saturating_sub(width(PROMPT))
        .saturating_sub(width(&count))
        .saturating_sub(2);
    // The tail of a long filter stays visible.
    let filter = app.filter();
    let shown = if width(filter) > avail {
        let mut tail: Vec<char> = Vec::new();
        let mut used = 1;
        for c in filter.chars().rev() {
            let cw = width(&c.to_string());
            if used + cw > avail {
                break;
            }
            used += cw;
            tail.push(c);
        }
        format!("…{}", tail.into_iter().rev().collect::<String>())
    } else {
        filter.to_string()
    };
    let x = put(buf, row, area.x + 1, area.y, PROMPT, DIM);
    let x = put(buf, row, x, area.y, &shown, Style::new());
    put(buf, row, x, area.y, CURSOR, Style::new());
    let cx = area
        .right()
        .saturating_sub(1)
        .saturating_sub(u16::try_from(width(&count)).unwrap_or(0));
    if cx > x + 1 {
        put(buf, row, cx, area.y, &count, DIM);
    }
}

fn draw_footer(buf: &mut Buffer, row: Rect, app: &App) {
    let hints: Vec<(&str, &str)> = if matches!(app.overlay(), Overlay::Chooser(_)) {
        vec![("↑↓", "choose"), ("enter", "select"), ("esc", "cancel")]
    } else if app.connecting().is_some() {
        vec![("esc", "stop waiting")]
    } else {
        let mut v = vec![
            ("enter", "connect"),
            ("alt-enter", "private screen"),
            ("ctrl-o", "screen"),
        ];
        if app.fix_for_selected().is_some() {
            v.push(("ctrl-f", "fix"));
        }
        v.push(("ctrl-r", "re-check"));
        v.push(("ctrl-e", "config"));
        v.push((
            "esc",
            if app.filter().is_empty() {
                "close"
            } else {
                "clear"
            },
        ));
        v
    };
    render_hints(buf, row, &hints);
}

/// Key hints on the last line of `area`.
pub fn draw_hints(buf: &mut Buffer, area: Rect, hints: &[(&str, &str)]) {
    let area = area.intersection(buf.area);
    if area.height == 0 {
        return;
    }
    render_hints(
        buf,
        Rect::new(area.x, area.bottom() - 1, area.width, 1),
        hints,
    );
}

/// `key desc · key desc`: keys plain, the rest dim; what does not fit is dropped from the right.
fn render_hints(buf: &mut Buffer, row: Rect, hints: &[(&str, &str)]) {
    let room = usize::from(row.width).saturating_sub(2);
    let mut x = row.x + 1;
    let mut used = 0;
    for (i, (key, desc)) in hints.iter().enumerate() {
        let sep = if i == 0 { 0 } else { 3 };
        let need = sep + width(key) + 1 + width(desc);
        if used + need > room {
            break;
        }
        if i > 0 {
            x = put(buf, row, x, row.y, " · ", DIM);
        }
        x = put(buf, row, x, row.y, key, Style::new());
        x = put(buf, row, x, row.y, &format!(" {desc}"), DIM);
        used += need;
    }
}

/// The first frame, drawn before the roster is loaded.
pub fn draw_loading(frame: &mut Frame) {
    let area = frame.area();
    let buf = frame.buffer_mut();
    if area.width < 4 || area.height == 0 {
        return;
    }
    let row = Rect::new(area.x, area.y, area.width, 1);
    let x = put(buf, row, area.x + 1, area.y, PROMPT, DIM);
    put(buf, row, x, area.y, CURSOR, Style::new());
    let msg = "loading machines…";
    let y = area.y + area.height / 2;
    let x = area.x
        + area
            .width
            .saturating_sub(u16::try_from(width(msg)).unwrap_or(0))
            / 2;
    put(buf, area, x, y, msg, DIM);
}

// ---- the list ---------------------------------------------------------------------------------

/// The widths of the list's columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cols {
    label: usize,
    target: usize,
    status: usize,
    sources: usize,
}

/// Fit the columns into `avail` columns: sources shrink first (and vanish), then status, then
/// target, then label.
fn plan_cols(app: &App, avail: usize) -> Cols {
    let rows = app.rows();
    let widest = |f: &dyn Fn(&crate::picker::app::Row) -> usize, cap: usize| {
        rows.iter().map(f).max().unwrap_or(0).min(cap)
    };
    let mut label = widest(&|r| r.machine.label.as_deref().map_or(0, width), LABEL_MAX);
    let mut target = widest(&|r| width(r.machine.target.as_str()), TARGET_MAX);
    let mut status = (0..rows.len())
        .map(|i| width(&short_text(&app.state(i))))
        .max()
        .unwrap_or(0)
        .min(STATUS_WIDTH);
    let used = |l: usize, t: usize, s: usize| 2 + if l > 0 { l + GAP } else { 0 } + t + GAP + s;
    for (floor, which) in [(STATUS_MIN, 2), (TARGET_MIN, 1), (LABEL_MIN, 0)] {
        while used(label, target, status) > avail {
            let col = match which {
                2 => &mut status,
                1 => &mut target,
                _ => &mut label,
            };
            if *col <= floor {
                break;
            }
            *col -= 1;
        }
    }
    let spare = avail.saturating_sub(used(label, target, status));
    let sources = if spare >= SOURCES_MIN + GAP {
        spare - GAP
    } else {
        0
    };
    Cols {
        label,
        target,
        status,
        sources,
    }
}

fn draw_list(buf: &mut Buffer, list: Rect, app: &App) {
    if list.width == 0 || list.height == 0 {
        return;
    }
    let h = usize::from(list.height);
    app.set_page(h);
    let items = app.items();
    if items.is_empty() {
        draw_empty(buf, list, app);
        return;
    }
    // Keep the selection visible; the first row only moves when it has to.
    let sel = app.selected_index();
    let mut first = app.scroll_cell().get();
    if let Some(s) = sel {
        if s < first {
            first = s;
        } else if s >= first + h {
            first = s + 1 - h;
        }
    }
    first = first.min(items.len().saturating_sub(h));
    app.scroll_cell().set(first);
    app.set_geometry(Some(ListGeom { rect: list, first }));

    let cols = plan_cols(app, usize::from(list.width).saturating_sub(2));
    let content = Rect::new(list.x, list.y, list.width.saturating_sub(1), list.height);
    for (i, item) in items.iter().enumerate().skip(first).take(h) {
        let y = list.y + u16::try_from(i - first).unwrap_or(0);
        let selected = sel == Some(i);
        draw_item(buf, content, y, item, &cols, selected, app);
        if selected {
            buf.set_style(Rect::new(list.x, y, list.width, 1), REVERSED);
            // The glyph is the one cell that is not reversed: its tone then shows as the
            // glyph's own colour on the terminal background, a small chip in the bar.
            if let Some(cell) = buf.cell_mut((list.x + 1, y)) {
                cell.set_style(Style::new().remove_modifier(Modifier::REVERSED));
            }
        }
    }
}

fn draw_empty(buf: &mut Buffer, list: Rect, app: &App) {
    let w = usize::from(list.width).saturating_sub(4).clamp(1, 40);
    let lines: Vec<Line> = if app.rows().is_empty() && app.filter().trim().is_empty() {
        let mut v: Vec<Line> = vec![vec![("No machines found yet.".to_string(), Style::new())]];
        v.push(Vec::new());
        v.extend(
            wrap(
                "Type user@host and press Enter, or add machines to config.toml (ctrl-e).",
                w,
            )
            .into_iter()
            .map(|l| vec![(l, DIM)]),
        );
        v
    } else {
        vec![vec![("No match.".to_string(), DIM)]]
    };
    let top = list
        .height
        .saturating_sub(u16::try_from(lines.len()).unwrap_or(u16::MAX))
        / 2;
    for (i, line) in lines.iter().enumerate() {
        let lw: usize = line.iter().map(|(t, _)| width(t)).sum();
        let x = list.x + (list.width.saturating_sub(u16::try_from(lw).unwrap_or(0))) / 2;
        put_line(
            buf,
            list,
            x,
            list.y + top + u16::try_from(i).unwrap_or(0),
            line,
        );
    }
}

fn draw_item(
    buf: &mut Buffer,
    list: Rect,
    y: u16,
    item: &Item,
    cols: &Cols,
    selected: bool,
    app: &App,
) {
    let x0 = list.x + 1;
    let connecting = app.connecting();
    match item {
        Item::Synthetic(t) => {
            let busy = connecting.filter(|c| c.target == t.as_str());
            let (g, gs) = busy.map_or(("↵".to_string(), tone_style(Tone::Accent)), |_| {
                (
                    glyph(&RowState::Checking, app.frame()).to_string(),
                    tone_style(Tone::Busy),
                )
            });
            put(buf, list, x0, y, &g, gs);
            let x = put(
                buf,
                list,
                x0 + 2,
                y,
                "connect to ",
                if selected { Style::new() } else { DIM },
            );
            let room = usize::from(list.right().saturating_sub(x));
            let x = put(buf, list, x, y, &truncate(t.as_str(), room), BOLD);
            if let Some(c) = busy {
                put(
                    buf,
                    list,
                    x + 2,
                    y,
                    c.text(),
                    if selected {
                        Style::new()
                    } else {
                        tone_style(Tone::Busy)
                    },
                );
            }
        }
        Item::Machine(mi) => draw_machine(buf, list, y, mi, cols, selected, app),
    }
}

fn draw_machine(
    buf: &mut Buffer,
    list: Rect,
    row: u16,
    mi: &MachineItem,
    cols: &Cols,
    selected: bool,
    app: &App,
) {
    let machine = &app.rows()[mi.row].machine;
    let mut state = app.state(mi.row);
    let mut status_text = short_text(&state);
    let mut glyph_text = glyph(&state, app.frame()).to_string();
    let mut tone_now = tone(&state);
    if let Some(c) = app
        .connecting()
        .filter(|c| c.target == machine.target.as_str())
    {
        state = RowState::Checking;
        glyph_text = glyph(&state, app.frame()).to_string();
        tone_now = Tone::Busy;
        status_text = c.text().to_string();
    }
    let x0 = list.x + 1;
    put(buf, list, x0, row, &glyph_text, tone_style(tone_now));
    let mut x = x0 + 2;
    let col = |w: usize| u16::try_from(w).unwrap_or(0);

    if cols.label > 0 {
        if let Some(label) = &machine.label {
            put_matches(buf, list, x, row, &pad(label, cols.label), BOLD, &mi.label);
        }
        x += col(cols.label + GAP);
    }
    let target_style = if machine.label.is_some() && !selected {
        DIM
    } else if machine.label.is_some() {
        Style::new()
    } else {
        BOLD
    };
    put_matches(
        buf,
        list,
        x,
        row,
        &pad(machine.target.as_str(), cols.target),
        target_style,
        &mi.target,
    );
    x += col(cols.target + GAP);

    let connecting = app
        .connecting()
        .is_some_and(|c| c.target == machine.target.as_str());
    if cols.status > 0 {
        let style = if selected {
            Style::new()
        } else {
            tone_style(tone_now)
        };
        // The longest step text outgrows the column; the sources make room while it shows.
        let room = if connecting && cols.sources > 0 {
            cols.status + GAP + cols.sources
        } else {
            cols.status
        };
        put(buf, list, x, row, &pad(&status_text, room), style);
        x += col(cols.status + GAP);
    }
    if cols.sources > 0 && !connecting {
        // `herdr …` reads as if a source hid behind a space: close the gap.
        let shown = truncate(&sources_text(machine), cols.sources).replace(" …", "…");
        put_matches(
            buf,
            list,
            x,
            row,
            &shown,
            if selected { Style::new() } else { DIM },
            &mi.sources,
        );
    }
}

// ---- the details panel ------------------------------------------------------------------------

fn draw_panel(buf: &mut Buffer, panel: Rect, app: &App) {
    if panel.width == 0 || panel.height == 0 {
        return;
    }
    for y in panel.y..panel.bottom() {
        put(buf, panel, panel.x, y, "│", DIM);
    }
    let w = usize::from(panel.width).saturating_sub(3);
    let lines = if let Some(e) = app.error() {
        error_lines(e, w, true)
    } else {
        details_lines(app, w)
    };
    for (i, line) in lines.iter().enumerate().take(usize::from(panel.height)) {
        put_line(
            buf,
            panel,
            panel.x + 2,
            panel.y + u16::try_from(i).unwrap_or(0),
            line,
        );
    }
}

fn kv(key: &str, value: &str, style: Style, w: usize) -> Line {
    vec![
        (pad(key, KEY_WIDTH), DIM),
        (truncate(value, w.saturating_sub(KEY_WIDTH)), style),
    ]
}

fn details_lines(app: &App, w: usize) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::new();
    let Some(item) = app.selected_item() else {
        return out;
    };
    let mi = match item {
        Item::Synthetic(t) => {
            out.push(vec![(truncate(t.as_str(), w), BOLD)]);
            out.push(Vec::new());
            out.extend(
                wrap("Not in the list yet. Enter opens it.", w)
                    .into_iter()
                    .map(|l| vec![(l, DIM)]),
            );
            return out;
        }
        Item::Machine(mi) => mi,
    };
    let r = &app.rows()[mi.row];
    let m = &r.machine;
    match &m.label {
        Some(l) => {
            out.push(vec![(truncate(l, w), BOLD)]);
            out.push(vec![(truncate(m.target.as_str(), w), Style::new())]);
        }
        None => out.push(vec![(truncate(m.target.as_str(), w), BOLD)]),
    }
    out.push(Vec::new());

    let state = app.state(mi.row);
    let (status_text, status_style) =
        match app.connecting().filter(|c| c.target == m.target.as_str()) {
            Some(c) => (c.text().to_string(), tone_style(Tone::Busy)),
            None => (short_text(&state), tone_style(tone(&state))),
        };
    out.push(kv("Status", &status_text, status_style, w));
    let cached = r.cached.as_ref();
    let os = cached
        .and_then(|c| c.os.as_deref())
        .map(crate::status::os_name)
        .or_else(|| match &state {
            RowState::Unsupported(os) => Some(crate::status::os_name(os)),
            _ => None,
        });
    if let Some(os) = os {
        out.push(kv("System", &os, Style::new(), w));
    }
    if let Some(c) = cached.filter(|c| !c.outputs.is_empty()) {
        out.push(kv("Screens", &c.outputs.join(" "), Style::new(), w));
    }
    if let Some(v) = cached.and_then(|c| c.server_version.as_deref()) {
        out.push(kv("Server", v, Style::new(), w));
    }
    if matches!(
        state,
        RowState::Open | RowState::Ready | RowState::Checking | RowState::NotChecked
    ) {
        out.push(kv("Opens", &app.opens_text(mi.row), Style::new(), w));
    }
    if let Some(c) = cached {
        out.push(kv(
            "Checked",
            &ago(app.now(), c.checked_at),
            Style::new(),
            w,
        ));
    }
    if !m.sources.is_empty() {
        out.push(kv("Sources", &sources_text(m), Style::new(), w));
    }

    let busy = app
        .connecting()
        .is_some_and(|c| c.target == m.target.as_str());
    let fix = app.fix_for_selected().filter(|_| !busy);
    if let Some((message, hint)) = app.explanation(mi.row).filter(|_| !busy) {
        out.push(Vec::new());
        out.extend(
            wrap(&message, w)
                .into_iter()
                .map(|l| vec![(l, Style::new())]),
        );
        // The fix line below says what ctrl-f does, so the hint does not.
        if let Some(h) = hint_without_fix(hint, fix.is_some()) {
            out.extend(wrap(&h, w).into_iter().map(|l| vec![(l, DIM)]));
        }
    }
    if let Some((target, remedy)) = fix {
        out.push(Vec::new());
        out.extend(hang(
            "ctrl-f",
            CYAN,
            &describe(remedy, target.as_str()),
            Style::new(),
            w,
        ));
    }
    out
}

/// A hint as shown next to a fix line (`fix_shown`): without its `Press ctrl-f to …` sentence.
fn hint_without_fix(hint: Option<String>, fix_shown: bool) -> Option<String> {
    hint.and_then(|h| {
        if fix_shown {
            without_fix_sentence(&h)
        } else {
            Some(h)
        }
    })
}

fn error_lines(e: &ErrorPanel, w: usize, gap: bool) -> Vec<Line> {
    let mut out: Vec<Line> = wrap(&e.message, w)
        .into_iter()
        .map(|l| vec![(l, RED_BOLD)])
        .collect();
    if gap {
        out.push(Vec::new());
    }
    if let Some(h) = hint_without_fix(e.hint.clone(), e.remedy.is_some()) {
        out.extend(wrap(&h, w).into_iter().map(|l| vec![(l, Style::new())]));
    }
    if let Some(rem) = e.remedy {
        if gap {
            out.push(Vec::new());
        }
        out.extend(hang(
            "ctrl-f",
            CYAN,
            &describe(rem, &e.target),
            Style::new(),
            w,
        ));
    }
    if let Some(log) = &e.log {
        if gap {
            out.push(Vec::new());
        }
        out.push(vec![(
            format!(
                "log: {}",
                tail_truncate(&log.display().to_string(), w.saturating_sub(5))
            ),
            DIM,
        )]);
    }
    out
}

/// What goes under the list in the narrow layout: the error panel, or one line about the
/// selected machine.
fn narrow_info(app: &App, w: usize) -> Vec<Line> {
    if let Some(e) = app.error() {
        return error_lines(e, w, false);
    }
    let Some(Item::Machine(mi)) = app.selected_item() else {
        return vec![Vec::new()];
    };
    let line: Line = match app.state(mi.row) {
        RowState::Open | RowState::Ready => vec![
            ("Opens ".to_string(), DIM),
            (
                truncate(&app.opens_text(mi.row), w.saturating_sub(6)),
                Style::new(),
            ),
        ],
        _ => match app.explanation(mi.row) {
            Some((message, hint)) => {
                let message = truncate(&message, w);
                let used = width(&message);
                let mut l: Line = vec![(message, Style::new())];
                if let Some(h) = hint
                    && used + 3 < w
                {
                    l.push((" ".to_string(), Style::new()));
                    l.push((truncate(&h, w - used - 1), DIM));
                }
                l
            }
            None => Vec::new(),
        },
    };
    vec![line]
}
