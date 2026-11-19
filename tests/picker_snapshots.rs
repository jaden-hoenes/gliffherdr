//! The picker's screens: every state of docs/design/04-picker.md at 100x22 (wide) and 72x18 (narrow), the style
//! of known cells, and a render of every size from 1x1 to 200x60 that must not panic.

use std::collections::HashSet;

use gliffherdr::check::{CheckEvent, CheckResult, Status};
use gliffherdr::connect::{ConnectError, Connected, Progress};
use gliffherdr::demo::{self, NOW};
use gliffherdr::fix::Remedy;
use gliffherdr::picker::app::{App, Effect};
use gliffherdr::picker::view::{self, draw_into};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn new_app() -> App {
    App::new(demo::startup(NOW), NOW)
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
}

/// The demo after its scripted checks have all answered.
fn settled() -> App {
    let mut app = new_app();
    for s in demo::script(NOW) {
        app.on_check(s.event);
    }
    app
}

fn select(app: &mut App, label: &str) {
    for _ in 0..app.items().len() {
        if let Some(gliffherdr::picker::app::Item::Machine(m)) = app.selected_item()
            && app.rows()[m.row].machine.display_label() == label
        {
            return;
        }
        app.handle_key(key(KeyCode::Down));
    }
    panic!("no machine {label}");
}

fn render(app: &App, w: u16, h: u16) -> Terminal<TestBackend> {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| view::draw(f, app)).unwrap();
    term
}

fn buffer(app: &App, w: u16, h: u16) -> Buffer {
    render(app, w, h).backend().buffer().clone()
}

/// Snapshot `app` at both sizes.
macro_rules! snap {
    ($name:literal, $app:expr) => {{
        let app = &$app;
        insta::assert_snapshot!(concat!($name, "_100x22"), render(app, 100, 22).backend());
        insta::assert_snapshot!(concat!($name, "_72x18"), render(app, 72, 18).backend());
    }};
}

fn finished(target: &str, status: Status, outputs: &[&str]) -> CheckEvent {
    CheckEvent::Finished(CheckResult {
        target: target.into(),
        status,
        os: Some("Linux".into()),
        outputs: outputs.iter().map(|s| (*s).to_string()).collect(),
        server_version: None,
        checked_at: NOW,
    })
}

// ---- the states of 04 -------------------------------------------------------------------------

#[test]
fn browsing_at_startup_with_checks_running() {
    snap!("browsing_checking", new_app());
}

#[test]
fn browsing_after_the_checks_settled() {
    snap!("browsing_settled", settled());
}

#[test]
fn browsing_a_machine_that_needs_setup_shows_details_and_the_fix_key() {
    let mut app = settled();
    select(&mut app, "lab");
    snap!("browsing_lab", app);
}

#[test]
fn browsing_an_unsupported_and_an_offline_machine() {
    let mut app = settled();
    select(&mut app, "air");
    snap!("browsing_air", app);
    select(&mut app, "nas");
    snap!("browsing_nas", app);
}

#[test]
fn empty_roster() {
    let mut start = demo::startup(NOW);
    start.machines.clear();
    snap!("empty_roster", App::new(start, NOW));
}

#[test]
fn filter_that_is_a_valid_target_offers_a_connect_row() {
    let mut app = settled();
    type_text(&mut app, "ops@10.1.2.3");
    snap!("filter_synthetic", app);
}

#[test]
fn filter_that_matches_nothing_and_is_not_a_target() {
    let mut app = settled();
    type_text(&mut app, "no such !");
    snap!("filter_no_match", app);
}

#[test]
fn filtering_highlights_the_matched_characters() {
    let mut app = settled();
    type_text(&mut app, "re");
    snap!("filter_re", app);
}

#[test]
fn connecting_shows_each_step() {
    let mut app = settled();
    select(&mut app, "render");
    app.handle_key(key(KeyCode::Enter));
    snap!("connecting_checking", app);
    app.on_progress(Progress::Launching);
    snap!("connecting_opening", app);
    app.on_progress(Progress::Waiting);
    for _ in 0..6 {
        app.tick();
    }
    snap!("connecting_waiting", app);
}

#[test]
fn connecting_to_a_typed_target() {
    let mut app = settled();
    type_text(&mut app, "ops@10.1.2.3");
    app.handle_key(key(KeyCode::Enter));
    snap!("connecting_synthetic", app);
}

fn failed_connect() -> App {
    let mut app = settled();
    select(&mut app, "lab");
    app.handle_key(key(KeyCode::Enter));
    app.on_connect(Err(ConnectError {
        message: "lab@10.0.0.9: gliff-server is not installed there.".into(),
        hint: Some("Press ctrl-f to install it, or set server_bin in config.toml.".into()),
        remedy: Some(Remedy::InstallServer),
        log: None,
    }));
    app
}

#[test]
fn connect_failed_with_a_remedy() {
    snap!("error_remedy", failed_connect());
}

#[test]
fn the_fix_is_said_once_in_the_error_panel_and_the_details() {
    for app in [failed_connect(), {
        let mut a = settled();
        select(&mut a, "lab");
        a
    }] {
        for (w, h) in [(100, 22), (72, 18)] {
            let text = format!("{}", render(&app, w, h).backend());
            if w == 100 || app.error().is_some() {
                assert!(text.contains("Or set server_bin"), "{text}");
                assert!(
                    !text.contains("Press ctrl-f to install it"),
                    "the hint repeats the fix line at {w}x{h}:\n{text}"
                );
            }
        }
    }
}

#[test]
fn connect_failed_with_a_log_and_no_remedy() {
    let mut app = settled();
    select(&mut app, "j");
    app.handle_key(key(KeyCode::Enter));
    app.on_connect(Err(ConnectError {
        message: "gliff did not open a window for j@10.0.0.2.".into(),
        hint: Some("error: could not connect: Connection refused (os error 111)".into()),
        remedy: None,
        log: Some("/home/me/.local/state/herdr/plugins/jaden-hoenes.gliffherdr/windows/j-1a2b3c/gliff.log".into()),
    }));
    snap!("error_log", app);
}

#[test]
fn screen_chooser() {
    let mut app = settled();
    select(&mut app, "studio");
    app.handle_key(ctrl('o'));
    snap!("chooser", app);
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Down));
    snap!("chooser_second_output", app);
}

#[test]
fn screen_chooser_while_the_check_runs() {
    let mut app = new_app();
    select(&mut app, "render");
    app.handle_key(ctrl('o'));
    snap!("chooser_loading", app);
    app.on_check(finished(
        "render@10.0.0.7",
        Status::Ready,
        &["DP-1", "DP-2"],
    ));
    snap!("chooser_loaded", app);
}

#[test]
fn config_error_banner() {
    let mut start = demo::startup(NOW);
    start.banner =
        Some("config.toml line 4: unknown field \"colour\" — ctrl-e to edit".to_string());
    snap!("config_banner", App::new(start, NOW));
}

#[test]
fn unreadable_sources_show_a_dim_note_in_the_banner_slot() {
    let mut start = demo::startup(NOW);
    start.unreadable_sources = 2;
    let app = App::new(start, NOW);
    snap!("sources_note", app);
    let buf = buffer(&app, 100, 22);
    let (x, y) = find(&buf, "2 sources could not be read");
    assert_eq!(y, 1);
    assert!(has(&buf, (x, y), Modifier::DIM));
    assert_eq!(fg(&buf, (x, y)), Color::Reset);
    let (x, y) = find(&buf, "gliffherdr doctor");
    assert_eq!(y, 1, "{x}");
}

#[test]
fn a_config_error_hides_the_sources_note() {
    let mut start = demo::startup(NOW);
    start.unreadable_sources = 2;
    start.banner = Some("config.toml line 4: bad — ctrl-e to edit".into());
    let app = App::new(start, NOW);
    assert_eq!(app.sources_note(), None);
    let text = format!("{}", render(&app, 100, 22).backend());
    assert!(!text.contains("could not be read"), "{text}");
}

#[test]
fn the_sources_note_counts_one_source_in_the_singular() {
    let mut start = demo::startup(NOW);
    start.unreadable_sources = 1;
    assert_eq!(
        App::new(start, NOW).sources_note().as_deref(),
        Some("1 source could not be read — gliffherdr doctor")
    );
    assert_eq!(new_app().sources_note(), None);
}

#[test]
fn selection_follows_the_machine_after_a_check_result() {
    let mut app = new_app();
    select(&mut app, "kiosk");
    app.on_check(finished("kiosk.tail.ts.net", Status::NoHyprland, &[]));
    snap!("follows_after_result", app);
}

#[test]
fn ctrl_r_resorts_only_when_the_round_completes() {
    let mut app = settled();
    select(&mut app, "lab");
    assert_eq!(app.handle_key(ctrl('r')), Effect::CheckAll);
    snap!("recheck_running", app);
    for s in demo::recheck_script(NOW) {
        app.on_check(s.event);
    }
    snap!("recheck_sorted", app);
}

#[test]
fn a_window_opening_changes_the_row_in_place() {
    let mut app = settled();
    app.on_open_targets(HashSet::from(["j@10.0.0.2".to_string()]));
    snap!("window_opened_in_place", app);
}

#[test]
fn too_small() {
    insta::assert_snapshot!("tiny_39x12", render(&settled(), 39, 12).backend());
    insta::assert_snapshot!("tiny_100x7", render(&settled(), 100, 7).backend());
}

#[test]
fn a_long_filter_keeps_its_tail_and_long_names_are_truncated() {
    let mut start = demo::startup(NOW);
    start.machines[1].label = Some("a-label-that-is-far-too-long".into());
    start.machines[2].label = None;
    let mut app = App::new(start, NOW);
    app.handle_key(key(KeyCode::Down));
    insta::assert_snapshot!("long_names_100x22", render(&app, 100, 22).backend());
    insta::assert_snapshot!("long_names_72x18", render(&app, 72, 18).backend());
    type_text(&mut app, &"x".repeat(60));
    insta::assert_snapshot!("long_filter_72x18", render(&app, 72, 18).backend());
}

#[test]
fn the_status_column_is_as_wide_as_the_widest_status_shown() {
    // A 14-column label and only short statuses: the sources of every row fit at exactly 100
    // columns (the fixed 21-column status left room for six of their fifteen).
    let mut start = demo::startup(NOW);
    start
        .machines
        .retain(|m| !["air", "lab", "kiosk"].contains(&m.display_label()));
    start.machines[1].label = Some("a-label-that-is-far-too-long".into());
    let app = App::new(start, NOW);
    let text = format!("{}", render(&app, 100, 22).backend());
    assert!(text.contains("herdr tailscale"), "{text}");
    insta::assert_snapshot!("status_fits_100x22", render(&app, 100, 22).backend());
    // A long status keeps its room: the macOS peer's sentence is not cut.
    let mut start = demo::startup(NOW);
    start.machines[1].label = Some("a-label-that-is-far-too-long".into());
    let text = format!("{}", render(&App::new(start, NOW), 100, 22).backend());
    assert!(text.contains("macOS: needs Hyprland"), "{text}");
}

// ---- styles -----------------------------------------------------------------------------------

/// The `(x, y)` of the first cell of `needle` in the buffer.
fn find(buf: &Buffer, needle: &str) -> (u16, u16) {
    let chars: Vec<&str> = needle.split("").filter(|s| !s.is_empty()).collect();
    for y in 0..buf.area.height {
        'x: for x in 0..buf
            .area
            .width
            .saturating_sub(u16::try_from(chars.len()).unwrap())
        {
            for (i, c) in chars.iter().enumerate() {
                if buf[(x + u16::try_from(i).unwrap(), y)].symbol() != *c {
                    continue 'x;
                }
            }
            return (x, y);
        }
    }
    panic!("{needle:?} not on screen:\n{}", dump(buf));
}

fn dump(buf: &Buffer) -> String {
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn has(buf: &Buffer, at: (u16, u16), m: Modifier) -> bool {
    buf[at].modifier.contains(m)
}

fn fg(buf: &Buffer, at: (u16, u16)) -> Color {
    buf[at].fg
}

#[test]
fn glyphs_carry_the_tone_of_their_state() {
    let mut app = settled();
    select(&mut app, "studio"); // keep the selection off the glyphs we inspect
    app.handle_key(key(KeyCode::Down)); // j: selected; look at others
    let buf = buffer(&app, 100, 22);
    let glyph = |g: &str| {
        let (_, y) = find(&buf, g);
        (1, y) // the glyph column
    };
    assert_eq!(fg(&buf, glyph("▣")), Color::Cyan, "open = accent");
    assert_eq!(fg(&buf, glyph("▲")), Color::Yellow, "needs setup = warn");
    assert_eq!(fg(&buf, glyph("✕")), Color::Red, "offline = bad");
    assert!(has(&buf, glyph("–"), Modifier::DIM), "unsupported = muted");
    assert_eq!(buf[glyph("▲")].symbol(), "▲");
}

#[test]
fn ready_glyph_is_green_and_the_spinner_yellow() {
    let mut app = new_app();
    app.handle_key(key(KeyCode::End)); // select air, away from j and render
    let buf = buffer(&app, 100, 22);
    let (_, y) = find(&buf, "●");
    assert_eq!(fg(&buf, (1, y)), Color::Green);
    let (_, y) = find(&buf, "⠋");
    assert_eq!(fg(&buf, (1, y)), Color::Yellow);
    // and the status text of a ready row is green too
    let (x, y) = find(&buf, "ready");
    assert_eq!(fg(&buf, (x, y)), Color::Green);
}

#[test]
fn the_selected_row_is_reversed_across_the_list_width_except_its_glyph() {
    let mut app = settled();
    app.handle_key(key(KeyCode::Down)); // j
    let buf = buffer(&app, 100, 22);
    let (_, y) = find(&buf, "j@10.0.0.2");
    let list_width = 100 - 34;
    for x in (0..list_width).filter(|&x| x != 1) {
        assert!(
            has(&buf, (x, y), Modifier::REVERSED),
            "x={x}\n{}",
            dump(&buf)
        );
    }
    assert!(
        !has(&buf, (1, y), Modifier::REVERSED),
        "the glyph is the one cell left alone: its tone shows as its own colour"
    );
    assert!(
        !has(&buf, (list_width, y), Modifier::REVERSED),
        "the panel is not reversed"
    );
    for other in [y - 1, y + 1] {
        assert!(!has(&buf, (5, other), Modifier::REVERSED));
    }
    // the glyph keeps its tone on the selected row
    assert_eq!(fg(&buf, (1, y)), Color::Green);
}

#[test]
fn labels_are_bold_targets_and_sources_dim() {
    let mut app = settled();
    app.handle_key(key(KeyCode::End)); // air selected; inspect j
    let buf = buffer(&app, 100, 22);
    let (x, y) = find(&buf, "j  ");
    assert!(has(&buf, (x, y), Modifier::BOLD), "label");
    let (x, y) = find(&buf, "j@10.0.0.2");
    assert!(
        has(&buf, (x, y), Modifier::DIM),
        "target dims when a label exists"
    );
    let (x, _) = find(&buf, "herdr");
    let (_, y) = find(&buf, "j@10.0.0.2");
    let _ = x;
    // sources column: the cell after the status text
    let sx = (0..66u16)
        .find(|&sx| buf[(sx, y)].symbol() == "h" && buf[(sx + 1, y)].symbol() == "e")
        .expect("sources");
    assert!(has(&buf, (sx, y), Modifier::DIM), "sources");
}

#[test]
fn matched_characters_are_bold_and_underlined() {
    let mut app = settled();
    type_text(&mut app, "ren");
    let buf = buffer(&app, 100, 22);
    let (_, y) = find(&buf, "render");
    for x in 3..6 {
        assert!(has(&buf, (x, y), Modifier::UNDERLINED), "x={x}");
        assert!(has(&buf, (x, y), Modifier::BOLD));
    }
    // "der" is not part of the match
    assert!(!has(&buf, (6, y), Modifier::UNDERLINED));
}

#[test]
fn footer_keys_are_plain_and_their_words_dim() {
    let buf = buffer(&settled(), 100, 22);
    let (x, y) = find(&buf, "enter");
    assert!(!has(&buf, (x, y), Modifier::DIM));
    assert!(has(&buf, (x + 6, y), Modifier::DIM), "connect");
    assert_eq!(y, 21, "the footer is the last line");
}

#[test]
fn the_fix_hint_appears_only_with_a_remedy() {
    let mut app = settled();
    let buf = buffer(&app, 100, 22);
    assert!(!dump(&buf).contains("ctrl-f"), "studio has no remedy");
    select(&mut app, "lab");
    let buf = buffer(&app, 100, 22);
    let (x, y) = find(&buf, "ctrl-f  Install");
    assert_eq!(
        fg(&buf, (x, y)),
        Color::Cyan,
        "the key in the panel is cyan"
    );
    assert!(dump(&buf).lines().last().unwrap().contains("ctrl-f fix"));
}

#[test]
fn the_error_message_is_red_and_bold() {
    let buf = buffer(&failed_connect(), 100, 22);
    let (x, y) = find(&buf, "lab@10.0.0.9: gliff-server");
    assert_eq!(fg(&buf, (x, y)), Color::Red);
    assert!(has(&buf, (x, y), Modifier::BOLD));
    let (x, y) = find(&buf, "ctrl-f  Install gliff");
    assert_eq!(fg(&buf, (x, y)), Color::Cyan);
}

#[test]
fn the_banner_is_red_and_the_separator_dim() {
    let mut start = demo::startup(NOW);
    start.banner = Some("config.toml line 4: bad — ctrl-e to edit".into());
    let buf = buffer(&App::new(start, NOW), 100, 22);
    let (x, y) = find(&buf, "config.toml line 4");
    assert_eq!((y, fg(&buf, (x, y))), (1, Color::Red));
    let (x, y) = find(&buf, "│");
    assert!(has(&buf, (x, y), Modifier::DIM));
    assert_eq!(x, 100 - 34);
}

#[test]
fn only_the_16_ansi_colours_and_basic_modifiers_are_used() {
    let allowed = [
        Color::Reset,
        Color::Black,
        Color::Red,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Magenta,
        Color::Cyan,
        Color::Gray,
        Color::DarkGray,
        Color::LightRed,
        Color::LightGreen,
        Color::LightYellow,
        Color::LightBlue,
        Color::LightMagenta,
        Color::LightCyan,
        Color::White,
    ];
    let ok = Modifier::DIM | Modifier::BOLD | Modifier::REVERSED | Modifier::UNDERLINED;
    for app in states() {
        for (w, h) in [(100, 22), (72, 18), (96, 30), (40, 8)] {
            let buf = buffer(&app, w, h);
            for cell in &buf.content {
                for c in [cell.fg, cell.bg, cell.underline_color] {
                    assert!(allowed.contains(&c), "{c:?}");
                }
                assert!(ok.contains(cell.modifier), "{:?}", cell.modifier);
            }
        }
    }
}

#[test]
fn the_list_has_no_outer_border() {
    let buf = buffer(&settled(), 100, 22);
    let text = dump(&buf);
    for c in ["┌", "└", "─", "┐", "┘"] {
        assert!(!text.contains(c), "{c}");
    }
    assert!(
        text.lines()
            .next()
            .unwrap()
            .starts_with(" remote desktop › ")
    );
}

#[test]
fn every_line_keeps_a_blank_column_on_both_sides() {
    let buf = buffer(&settled(), 100, 22);
    for y in 0..22 {
        assert_eq!(buf[(0, y)].symbol(), " ", "left margin of line {y}");
        assert_eq!(buf[(99, y)].symbol(), " ", "right margin of line {y}");
    }
}

// ---- no panics at any size --------------------------------------------------------------------

fn states() -> Vec<App> {
    let mut v = vec![new_app(), settled()];
    let mut a = settled();
    type_text(&mut a, "re");
    v.push(a);
    let mut a = settled();
    type_text(&mut a, "zz@h");
    v.push(a);
    let mut a = settled();
    type_text(&mut a, "no match !");
    v.push(a);
    v.push(failed_connect());
    let mut a = settled();
    a.handle_key(ctrl('o'));
    v.push(a);
    let mut a = new_app();
    select(&mut a, "render");
    a.handle_key(ctrl('o'));
    v.push(a);
    let mut a = settled();
    select(&mut a, "lab");
    v.push(a);
    let mut a = settled();
    a.handle_key(key(KeyCode::Enter));
    v.push(a);
    let mut s = demo::startup(NOW);
    s.machines.clear();
    s.banner = Some(
        "config.toml line 1: this banner is long enough to be cut at narrow sizes — ctrl-e to edit"
            .into(),
    );
    v.push(App::new(s, NOW));
    let mut a = settled();
    a.handle_key(key(KeyCode::Enter));
    a.on_connect(Err(ConnectError {
        message: "x".repeat(300),
        hint: Some("y ".repeat(200)),
        remedy: Some(Remedy::CopyKey),
        log: Some("/".repeat(300).into()),
    }));
    v.push(a);
    let mut a = settled();
    a.handle_key(key(KeyCode::Enter));
    let _ = a.on_connect(Ok(Connected::Focused));
    v.push(a);
    v
}

fn draw_at(app: &App, w: u16, h: u16) {
    let area = Rect::new(0, 0, w, h);
    let mut buf = Buffer::empty(area);
    draw_into(&mut buf, area, app);
}

#[test]
fn every_size_from_1x1_to_200x60_renders_without_panicking() {
    // Every size for the screens that have the most going on ...
    let states = states();
    for app in [&states[1], &states[5], &states[6], &states[11]] {
        for w in 1..=200u16 {
            for h in 1..=60u16 {
                draw_at(app, w, h);
            }
        }
    }
}

#[test]
fn every_state_renders_at_the_sizes_around_the_layout_thresholds() {
    // ... and every screen at the sizes where the layout changes, plus a coarse sweep.
    let widths: Vec<u16> = (1..=200)
        .filter(|w| w % 7 == 0 || (38..=42).contains(w) || (94..=98).contains(w))
        .collect();
    let heights: Vec<u16> = (1..=60)
        .filter(|h| h % 5 == 0 || (6..=9).contains(h))
        .collect();
    for app in states() {
        for &w in &widths {
            for &h in &heights {
                draw_at(&app, w, h);
            }
        }
    }
}

#[test]
fn a_terminal_backend_survives_resizing_between_frames() {
    let app = settled();
    let mut term = Terminal::new(TestBackend::new(100, 22)).unwrap();
    for (w, h) in [
        (100, 22),
        (30, 5),
        (1, 1),
        (72, 18),
        (200, 60),
        (96, 8),
        (95, 9),
    ] {
        term.backend_mut().resize(w, h);
        term.draw(|f| view::draw(f, &app)).unwrap();
    }
}

#[test]
fn drawing_is_deterministic() {
    let app = settled();
    let a = buffer(&app, 100, 22);
    let b = buffer(&app, 100, 22);
    assert_eq!(a, b);
}
