//! The picker's state machine. Pure: it performs no I/O, every reaction to the outside world
//! comes back as an [`Effect`] that [`crate::picker::run`] executes.
//!
//! Rows do not move while the picker is open, except when the filter changes or a `Ctrl+R`
//! round completes. The selection follows the machine (by target), not the index.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config as MatcherConfig, Matcher, Utf32Str};
use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use super::chooser::Chooser;
use super::input::{Input, classify};
use crate::check::{CheckEvent, CheckResult, Status, is_fresh, needs_check};
use crate::config::{Config, Mode};
use crate::connect::{ConnectError, ConnectRequest, Connected, Progress};
use crate::fix::{Remedy, remedy_for};
use crate::launch::ScreenChoice;
use crate::roster::Machine;
use crate::status::{RowState, row_state, sentence};
use crate::target::Target;

/// The longest filter the picker accepts, in characters.
const MAX_FILTER: usize = 200;
/// Two clicks on one row within this many ticks (50 ms each) are a double click.
const DOUBLE_CLICK_TICKS: u64 = 8;

/// What the picker needs to start (or to start over after the configuration changed): the
/// roster, what is cached for it, which windows are open. Built by `picker::load` (or by
/// [`crate::demo::startup`]); the app never reads any of it itself.
#[derive(Debug, Clone)]
pub struct Startup {
    /// The configuration in force (the defaults when the file is invalid).
    pub config: Config,
    /// The machines, in collection order.
    pub machines: Vec<Machine>,
    /// Cached check results by target.
    pub caches: HashMap<String, CheckResult>,
    /// Targets whose gliff window is open.
    pub open: HashSet<String>,
    /// The configuration error banner, complete with its `ctrl-e to edit` tail.
    pub banner: Option<String>,
    /// How many roster sources could not be read (`Roster::warnings`); shown dim in the banner
    /// slot when there is no configuration error.
    pub unreadable_sources: usize,
    /// Whether the picker runs inside herdr (`Ctrl+F` then opens a pane).
    pub in_herdr: bool,
}

/// One machine of the roster and its last check result.
#[derive(Debug, Clone)]
pub struct Row {
    /// The machine.
    pub machine: Machine,
    /// Its last check result, if any.
    pub cached: Option<CheckResult>,
}

/// One line of the visible list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// A machine of the roster.
    Machine(MachineItem),
    /// The `↵ connect to <filter>` row: nothing matched and the filter is a valid target.
    Synthetic(Target),
}

/// A visible machine with the filter's matched characters (indices into the displayed texts).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineItem {
    /// Index into [`App::rows`].
    pub row: usize,
    /// Matched character positions in the label.
    pub label: Vec<usize>,
    /// Matched character positions in the target.
    pub target: Vec<usize>,
    /// Matched character positions in the sources text.
    pub sources: Vec<usize>,
}

/// What is shown on top of the list.
#[derive(Debug, Clone)]
pub enum Overlay {
    /// Nothing.
    None,
    /// The screen chooser.
    Chooser(Chooser),
}

/// A connect in progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connecting {
    /// The machine being opened.
    pub target: String,
    /// The step it is at.
    pub stage: Progress,
}

impl Connecting {
    /// The status text of the step: `checking…`, `opening…`, `waiting for the window…`.
    #[must_use]
    pub fn text(&self) -> &'static str {
        match self.stage {
            Progress::Checking => "checking…",
            Progress::Launching => "opening…",
            Progress::Waiting => "waiting for the window…",
        }
    }
}

/// The panel that replaces the details after a failed connect (or a failed effect).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorPanel {
    /// The machine it is about.
    pub target: String,
    /// What went wrong.
    pub message: String,
    /// What to do about it.
    pub hint: Option<String>,
    /// The fix `Ctrl+F` runs.
    pub remedy: Option<Remedy>,
    /// gliff's log.
    pub log: Option<PathBuf>,
}

/// Where the list was last drawn: what a mouse click needs to find its row. Filled in by `draw`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListGeom {
    /// The rows of the list (one terminal line each).
    pub rect: Rect,
    /// The index of the first visible item.
    pub first: usize,
}

/// What [`crate::picker::run`] must do after a transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Nothing.
    None,
    /// Leave the picker (exit 0).
    Quit,
    /// Connect on a thread, reporting progress.
    Connect(ConnectRequest),
    /// Check every checkable machine again (the machines are in [`App::take_checks`]).
    CheckAll,
    /// Open the `fix` pane in herdr for the machine, then leave.
    OpenFixPane(Target),
    /// Leave the terminal, run the fix in this process, come back.
    FixInline(Target),
    /// Leave the terminal, run the editor on `config.toml`, come back and reload.
    EditConfig,
    /// A check of this machine was queued for the chooser (see [`App::take_checks`]).
    StartChooserCheck(Target),
}

/// The picker's whole state.
pub struct App {
    config: Config,
    rows: Vec<Row>,
    order: Vec<usize>,
    items: Vec<Item>,
    selected: Option<String>,
    checking: HashSet<String>,
    open: HashSet<String>,
    filter: String,
    overlay: Overlay,
    connecting: Option<Connecting>,
    error: Option<ErrorPanel>,
    banner: Option<String>,
    unreadable_sources: usize,
    in_herdr: bool,
    now: u64,
    ticks: u64,
    recheck: Option<HashSet<String>>,
    queue: Vec<Machine>,
    dirty: bool,
    last_click: Option<(usize, u64)>,
    geom: Cell<Option<ListGeom>>,
    scroll: Cell<usize>,
    page: Cell<usize>,
}

/// The key of an item: the target text.
fn item_key(rows: &[Row], item: &Item) -> String {
    match item {
        Item::Machine(m) => rows[m.row].machine.target.to_string(),
        Item::Synthetic(t) => t.to_string(),
    }
}

impl App {
    /// Build the app from what [`Startup`] gathered. Machines with no fresh result (and that can
    /// run gliff at all) start in the checking state and are queued for [`App::take_checks`].
    #[must_use]
    pub fn new(start: Startup, now: u64) -> App {
        let mut app = App {
            config: start.config.clone(),
            rows: Vec::new(),
            order: Vec::new(),
            items: Vec::new(),
            selected: None,
            checking: HashSet::new(),
            open: HashSet::new(),
            filter: String::new(),
            overlay: Overlay::None,
            connecting: None,
            error: None,
            banner: None,
            unreadable_sources: 0,
            in_herdr: false,
            now,
            ticks: 0,
            recheck: None,
            queue: Vec::new(),
            dirty: true,
            last_click: None,
            geom: Cell::new(None),
            scroll: Cell::new(0),
            page: Cell::new(10),
        };
        app.install(start);
        app
    }

    /// Replace the roster, caches, open set and configuration (after the configuration was
    /// edited). The filter and the selection stay; rows are sorted afresh; stale machines are
    /// checked.
    pub fn reload(&mut self, start: Startup) {
        self.dirty = true;
        self.install(start);
    }

    fn install(&mut self, start: Startup) {
        let Startup {
            config,
            machines,
            mut caches,
            open,
            banner,
            unreadable_sources,
            in_herdr,
        } = start;
        self.config = config;
        self.banner = banner;
        self.unreadable_sources = unreadable_sources;
        self.in_herdr = in_herdr;
        self.open = open;
        self.rows = machines
            .into_iter()
            .map(|machine| {
                let cached = caches.remove(machine.target.as_str());
                Row { machine, cached }
            })
            .collect();
        let known: HashSet<&str> = self
            .rows
            .iter()
            .map(|r| r.machine.target.as_str())
            .collect();
        self.checking.retain(|t| known.contains(t.as_str()));
        self.queue.clear();
        let (now, fresh) = (self.now, self.config.check.fresh_for_secs);
        for row in &self.rows {
            let t = row.machine.target.as_str();
            let stale = row.cached.as_ref().is_none_or(|r| !is_fresh(r, now, fresh));
            if row.machine.checkable() && stale && self.checking.insert(t.to_string()) {
                self.queue.push(row.machine.clone());
            }
        }
        self.recheck = None;
        self.resort(false);
    }

    // ---- read access for the view and the runner ------------------------------------------

    /// Every machine of the roster, in collection order.
    #[must_use]
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The visible list.
    #[must_use]
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// The filter text.
    #[must_use]
    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// The overlay on top of the list.
    #[must_use]
    pub fn overlay(&self) -> &Overlay {
        &self.overlay
    }

    /// The connect in progress.
    #[must_use]
    pub fn connecting(&self) -> Option<&Connecting> {
        self.connecting.as_ref()
    }

    /// The error panel.
    #[must_use]
    pub fn error(&self) -> Option<&ErrorPanel> {
        self.error.as_ref()
    }

    /// The configuration error banner.
    #[must_use]
    pub fn banner(&self) -> Option<&str> {
        self.banner.as_deref()
    }

    /// The dim note about roster sources that could not be read, shown where the banner goes when
    /// there is no configuration error.
    #[must_use]
    pub fn sources_note(&self) -> Option<String> {
        let n = self.unreadable_sources;
        (self.banner.is_none() && n > 0).then(|| {
            let (count, noun) = if n == 1 {
                ("1".to_string(), "source")
            } else {
                (n.to_string(), "sources")
            };
            format!("{count} {noun} could not be read — gliffherdr doctor")
        })
    }

    /// The configuration in force.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The clock the app was last given, in Unix seconds.
    #[must_use]
    pub fn now(&self) -> u64 {
        self.now
    }

    /// The spinner frame (advances every 100 ms of ticks).
    #[must_use]
    pub fn frame(&self) -> usize {
        usize::try_from(self.ticks / 2).unwrap_or(0)
    }

    /// Whether the picker runs inside herdr.
    #[must_use]
    pub fn in_herdr(&self) -> bool {
        self.in_herdr
    }

    /// The shown count: `(matching, all)`.
    #[must_use]
    pub fn counts(&self) -> (usize, usize) {
        let total = self.rows.len();
        if self.filter.is_empty() {
            (total, total)
        } else {
            let hits = self
                .items
                .iter()
                .filter(|i| matches!(i, Item::Machine(_)))
                .count();
            (hits, total)
        }
    }

    /// The state of a row, with the checking and open sets applied.
    #[must_use]
    pub fn state(&self, row: usize) -> RowState {
        let r = &self.rows[row];
        let t = r.machine.target.as_str();
        row_state(
            &r.machine,
            r.cached.as_ref(),
            self.checking.contains(t),
            r.machine.is_open(&self.open),
        )
    }

    /// The index in [`App::items`] of the selected item.
    #[must_use]
    pub fn selected_index(&self) -> Option<usize> {
        let key = self.selected.as_ref()?;
        self.items
            .iter()
            .position(|i| &item_key(&self.rows, i) == key)
    }

    /// The selected item.
    #[must_use]
    pub fn selected_item(&self) -> Option<&Item> {
        self.selected_index().map(|i| &self.items[i])
    }

    /// The selected machine's row index (`None` on the synthetic row or an empty list).
    #[must_use]
    pub fn selected_row(&self) -> Option<usize> {
        match self.selected_item()? {
            Item::Machine(m) => Some(m.row),
            Item::Synthetic(_) => None,
        }
    }

    /// The row of a machine by target.
    #[must_use]
    pub fn find_row(&self, target: &str) -> Option<&Row> {
        self.rows
            .iter()
            .find(|r| r.machine.target.as_str() == target)
    }

    /// What Enter opens for a machine: its configured screen, else `[connect] mode`.
    #[must_use]
    pub fn opens_text(&self, row: usize) -> String {
        let s = &self.rows[row].machine.settings;
        if let Some(o) = &s.output {
            return format!("mirror {o}");
        }
        match s.mode.unwrap_or(self.config.connect.mode) {
            Mode::Mirror => "mirror (focused screen)".to_string(),
            Mode::Headless => "private screen".to_string(),
        }
    }

    /// The fix `Ctrl+F` would run: the selected machine's remedy (only while it is not busy and
    /// has no open window), else the remedy of the error panel about it.
    #[must_use]
    pub fn fix_for_selected(&self) -> Option<(Target, Remedy)> {
        let row = self.selected_row()?;
        let r = &self.rows[row];
        if matches!(self.state(row), RowState::NeedsSetup(_))
            && let Some(rem) = r.cached.as_ref().and_then(|c| remedy_for(&c.status))
        {
            return Some((r.machine.target.clone(), rem));
        }
        let e = self.error.as_ref()?;
        (e.target == r.machine.target.as_str())
            .then_some(e.remedy)
            .flatten()
            .map(|rem| (r.machine.target.clone(), rem))
    }

    /// The sentence and hint explaining a failing row, `None` for rows that need no explanation.
    #[must_use]
    pub fn explanation(&self, row: usize) -> Option<(String, Option<String>)> {
        let target = self.rows[row].machine.target.as_str();
        match self.state(row) {
            RowState::NeedsSetup(s) | RowState::Unreachable(s) => Some(sentence(&s, target)),
            RowState::Unsupported(os) => Some(sentence(&Status::Unsupported { os }, target)),
            _ => None,
        }
    }

    /// Machines whose check was requested since the last call (startup, `Ctrl+R`, the chooser,
    /// a configuration reload). The runner starts one `check_many` for them.
    pub fn take_checks(&mut self) -> Vec<Machine> {
        std::mem::take(&mut self.queue)
    }

    /// Whether anything changed since the last call (the runner redraws only then).
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// Set by `draw`: where the list is on screen.
    pub fn set_geometry(&self, g: Option<ListGeom>) {
        self.geom.set(g);
    }

    /// Used and updated by `draw`: the first visible item.
    #[must_use]
    pub fn scroll_cell(&self) -> &Cell<usize> {
        &self.scroll
    }

    /// Set by `draw`: how many rows the list shows (the size of a page).
    pub fn set_page(&self, rows: usize) {
        self.page.set(rows.max(1));
    }

    // ---- inputs ---------------------------------------------------------------------------

    /// Ask for a redraw (the terminal was resized or cleared).
    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    /// Give the app the clock (Unix seconds).
    pub fn set_now(&mut self, now: u64) {
        if now != self.now {
            self.now = now;
            self.dirty = true;
        }
    }

    /// 50 ms passed: advance the spinner and the double-click clock.
    pub fn tick(&mut self) {
        self.ticks += 1;
        let busy = !self.checking.is_empty() || self.connecting.is_some();
        if busy && self.ticks.is_multiple_of(2) {
            self.dirty = true;
        }
    }

    /// A key press.
    pub fn handle_key(&mut self, key: ratatui::crossterm::event::KeyEvent) -> Effect {
        let input = classify(key);
        if input == Input::Other {
            return Effect::None;
        }
        self.dirty = true;
        if input == Input::Quit {
            return Effect::Quit;
        }
        if self.connecting.is_some() {
            if input == Input::Esc {
                self.connecting = None;
            }
            return Effect::None;
        }
        if matches!(self.overlay, Overlay::Chooser(_)) {
            return self.chooser_key(input);
        }
        if self.error.is_some() && input != Input::Fix {
            self.error = None;
            if input == Input::Esc {
                return Effect::None;
            }
        }
        match input {
            Input::Char(c) => {
                if self.filter.chars().count() < MAX_FILTER {
                    self.filter.push(c);
                    self.filter_changed();
                }
                Effect::None
            }
            Input::Backspace => {
                if self.filter.pop().is_some() {
                    self.filter_changed();
                }
                Effect::None
            }
            Input::ClearFilter => {
                if !self.filter.is_empty() {
                    self.filter.clear();
                    self.filter_changed();
                }
                Effect::None
            }
            Input::DeleteWord => {
                let trimmed = self.filter.trim_end().len();
                self.filter.truncate(trimmed);
                let keep = self.filter.rfind(char::is_whitespace).map_or(0, |i| {
                    i + self.filter[i..].chars().next().map_or(1, char::len_utf8)
                });
                self.filter.truncate(keep);
                self.filter_changed();
                Effect::None
            }
            Input::Up => {
                self.move_by(-1, true);
                Effect::None
            }
            Input::Down => {
                self.move_by(1, true);
                Effect::None
            }
            Input::PageUp => {
                self.move_by(-self.page_step(), false);
                Effect::None
            }
            Input::PageDown => {
                self.move_by(self.page_step(), false);
                Effect::None
            }
            Input::Home => {
                self.select_index(0);
                Effect::None
            }
            Input::End => {
                self.select_index(self.items.len().saturating_sub(1));
                Effect::None
            }
            Input::Enter => self.connect_selected(None),
            Input::AltEnter => self.connect_selected(Some(ScreenChoice::Headless)),
            Input::Chooser => self.open_chooser(),
            Input::Fix => match self.fix_for_selected() {
                Some((t, _)) if self.in_herdr => Effect::OpenFixPane(t),
                Some((t, _)) => Effect::FixInline(t),
                None => Effect::None,
            },
            Input::Recheck => self.recheck_all(),
            Input::Edit => Effect::EditConfig,
            Input::Esc => {
                if self.filter.is_empty() {
                    Effect::Quit
                } else {
                    self.filter.clear();
                    self.filter_changed();
                    Effect::None
                }
            }
            Input::Quit | Input::Other => Effect::None,
        }
    }

    /// A mouse event: a left click selects, a second click on the same row connects, the wheel
    /// moves the selection.
    pub fn handle_mouse(&mut self, ev: MouseEvent) -> Effect {
        if self.connecting.is_some() || !matches!(self.overlay, Overlay::None) {
            return Effect::None;
        }
        match ev.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                self.dirty = true;
                self.error = None;
                let d = if ev.kind == MouseEventKind::ScrollUp {
                    -1
                } else {
                    1
                };
                self.move_by(d, false);
                Effect::None
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(g) = self.geom.get() else {
                    return Effect::None;
                };
                if !g.rect.contains(Position::new(ev.column, ev.row)) {
                    return Effect::None;
                }
                let idx = g.first + usize::from(ev.row - g.rect.y);
                if idx >= self.items.len() {
                    return Effect::None;
                }
                self.dirty = true;
                self.error = None;
                let double = self.last_click.is_some_and(|(i, t)| {
                    i == idx && self.ticks.saturating_sub(t) <= DOUBLE_CLICK_TICKS
                });
                self.select_index(idx);
                if double {
                    self.last_click = None;
                    self.connect_selected(None)
                } else {
                    self.last_click = Some((idx, self.ticks));
                    Effect::None
                }
            }
            _ => Effect::None,
        }
    }

    /// A check started or finished. Results replace the row's state in place; the order only
    /// changes when the `Ctrl+R` round they belong to is complete.
    pub fn on_check(&mut self, ev: CheckEvent) {
        self.dirty = true;
        match ev {
            CheckEvent::Started(t) => {
                if self.rows.iter().any(|r| r.machine.target.as_str() == t) {
                    self.checking.insert(t);
                }
            }
            CheckEvent::Finished(result) => {
                let t = result.target.clone();
                self.checking.remove(&t);
                if let Some(row) = self
                    .rows
                    .iter_mut()
                    .find(|r| r.machine.target.as_str() == t)
                {
                    row.cached = Some(result.clone());
                }
                if let Overlay::Chooser(c) = &mut self.overlay
                    && c.is_loading()
                    && c.target().as_str() == t
                {
                    c.set_outputs(result.outputs);
                }
                if let Some(pending) = &mut self.recheck {
                    pending.remove(&t);
                    if pending.is_empty() {
                        self.recheck = None;
                        self.resort(false);
                    }
                }
            }
        }
    }

    /// The set of machines with an open gliff window changed.
    pub fn on_open_targets(&mut self, open: HashSet<String>) {
        if open != self.open {
            self.open = open;
            self.dirty = true;
        }
    }

    /// The connect moved on to its next step.
    pub fn on_progress(&mut self, p: Progress) {
        if let Some(c) = &mut self.connecting {
            c.stage = p;
            self.dirty = true;
        }
    }

    /// The connect finished. Success leaves the picker even when the user stopped waiting (the
    /// window is open); a failure is shown only if the user is still waiting for it.
    pub fn on_connect(&mut self, result: Result<Connected, ConnectError>) -> Effect {
        self.dirty = true;
        match result {
            Ok(_) => Effect::Quit,
            Err(e) => {
                if let Some(c) = self.connecting.take() {
                    self.error = Some(ErrorPanel {
                        target: c.target,
                        message: e.message,
                        hint: e.hint,
                        remedy: e.remedy,
                        log: e.log,
                    });
                }
                Effect::None
            }
        }
    }

    /// Show a failure that is not a connect's (a fix pane that would not open, an editor that
    /// would not start) in the error panel, about the selected machine.
    pub fn on_failure(&mut self, message: impl Into<String>, hint: Option<String>) {
        self.dirty = true;
        let target = self
            .selected_item()
            .map(|i| item_key(&self.rows, i))
            .unwrap_or_default();
        self.error = Some(ErrorPanel {
            target,
            message: message.into(),
            hint,
            remedy: None,
            log: None,
        });
    }

    // ---- internals ------------------------------------------------------------------------

    fn page_step(&self) -> isize {
        isize::try_from(self.page.get().saturating_sub(1).max(1)).unwrap_or(1)
    }

    fn chooser_key(&mut self, input: Input) -> Effect {
        let Overlay::Chooser(c) = &mut self.overlay else {
            return Effect::None;
        };
        match input {
            Input::Esc => {
                self.overlay = Overlay::None;
                Effect::None
            }
            Input::Up => {
                c.move_by(-1);
                Effect::None
            }
            Input::Down => {
                c.move_by(1);
                Effect::None
            }
            Input::Home => {
                c.first();
                Effect::None
            }
            Input::End => {
                c.last();
                Effect::None
            }
            Input::Enter => {
                let (target, choice) = (c.target().clone(), c.pick());
                match choice {
                    Some(choice) => {
                        self.overlay = Overlay::None;
                        self.begin_connect(target, Some(choice))
                    }
                    None => Effect::None,
                }
            }
            _ => Effect::None,
        }
    }

    fn open_chooser(&mut self) -> Effect {
        let Some(item) = self.selected_item().cloned() else {
            return Effect::None;
        };
        match item {
            Item::Synthetic(t) => {
                self.overlay = Overlay::Chooser(Chooser::ready(t.to_string(), t, Vec::new()));
                Effect::None
            }
            Item::Machine(mi) => {
                let r = &self.rows[mi.row];
                if !r.machine.checkable() {
                    return Effect::None;
                }
                let (label, target) = (
                    r.machine.display_label().to_string(),
                    r.machine.target.clone(),
                );
                if let Some(c) = &r.cached {
                    self.overlay =
                        Overlay::Chooser(Chooser::ready(label, target, c.outputs.clone()));
                    return Effect::None;
                }
                let machine = r.machine.clone();
                self.overlay = Overlay::Chooser(Chooser::loading(label, target.clone()));
                if self.checking.insert(target.to_string()) {
                    self.queue.push(machine);
                }
                Effect::StartChooserCheck(target)
            }
        }
    }

    fn recheck_all(&mut self) -> Effect {
        if self.recheck.is_some() {
            return Effect::None;
        }
        let machines: Vec<Machine> = self
            .rows
            .iter()
            .filter(|r| r.machine.checkable())
            .map(|r| r.machine.clone())
            .collect();
        if machines.is_empty() {
            return Effect::None;
        }
        let mut pending = HashSet::new();
        for m in machines {
            pending.insert(m.target.to_string());
            if self.checking.insert(m.target.to_string()) {
                self.queue.push(m);
            }
        }
        self.recheck = Some(pending);
        Effect::CheckAll
    }

    fn connect_selected(&mut self, choice: Option<ScreenChoice>) -> Effect {
        let Some(item) = self.selected_item().cloned() else {
            return Effect::None;
        };
        match item {
            Item::Synthetic(t) => self.begin_connect(t, choice),
            Item::Machine(mi) => {
                let m = self.rows[mi.row].machine.clone();
                if !m.checkable() {
                    // No ssh attempt for a machine known not to run Hyprland: say why.
                    if let Some((message, hint)) = self.explanation(mi.row) {
                        self.error = Some(ErrorPanel {
                            target: m.target.to_string(),
                            message,
                            hint,
                            remedy: None,
                            log: None,
                        });
                    }
                    return Effect::None;
                }
                self.begin_connect(m.target, choice)
            }
        }
    }

    fn begin_connect(&mut self, target: Target, choice: Option<ScreenChoice>) -> Effect {
        let row = self.find_row(target.as_str());
        let cached = row.and_then(|r| r.cached.as_ref());
        let open = self.open.contains(target.as_str())
            || row.is_some_and(|r| r.machine.is_open(&self.open));
        let stage = if needs_check(cached, self.now, self.config.check.fresh_for_secs) && !open {
            Progress::Checking
        } else {
            Progress::Launching
        };
        self.error = None;
        self.connecting = Some(Connecting {
            target: target.to_string(),
            stage,
        });
        Effect::Connect(ConnectRequest { target, choice })
    }

    /// Move the selection by `delta` rows: wrapping (arrow keys) or stopping at the ends.
    fn move_by(&mut self, delta: isize, wrap: bool) {
        let n = self.items.len();
        if n == 0 {
            return;
        }
        let cur = self.selected_index().unwrap_or(0);
        let next = if wrap {
            let n_i = isize::try_from(n).unwrap_or(isize::MAX);
            let cur_i = isize::try_from(cur).unwrap_or(0);
            usize::try_from((cur_i + delta).rem_euclid(n_i)).unwrap_or(0)
        } else {
            let cur_i = isize::try_from(cur).unwrap_or(0);
            usize::try_from((cur_i + delta).max(0))
                .unwrap_or(0)
                .min(n - 1)
        };
        self.select_index(next);
    }

    fn select_index(&mut self, i: usize) {
        if let Some(item) = self.items.get(i) {
            self.selected = Some(item_key(&self.rows, item));
        }
    }

    fn filter_changed(&mut self) {
        // Typing selects the best match; clearing the filter keeps the machine you are on.
        self.resort(!self.filter.is_empty());
    }

    /// Sort the rows by `(rank, collection index)` and rebuild the visible list. The selection
    /// stays on its machine unless `reset` or it is no longer listed.
    fn resort(&mut self, reset: bool) {
        let mut order: Vec<usize> = (0..self.rows.len()).collect();
        let ranks: Vec<u8> = (0..self.rows.len())
            .map(|i| crate::status::rank(&self.state(i)))
            .collect();
        order.sort_by_key(|&i| (ranks[i], i));
        self.order = order;
        self.items = self.build_items();
        let keep = !reset && self.selected_index().is_some();
        if !keep {
            self.selected = self.items.first().map(|i| item_key(&self.rows, i));
        }
        self.error = self.error.take().filter(|_| keep);
    }

    fn build_items(&self) -> Vec<Item> {
        let filter = self.filter.trim();
        if filter.is_empty() {
            return self
                .order
                .iter()
                .map(|&row| {
                    Item::Machine(MachineItem {
                        row,
                        label: Vec::new(),
                        target: Vec::new(),
                        sources: Vec::new(),
                    })
                })
                .collect();
        }
        let pattern = Pattern::parse(filter, CaseMatching::Smart, Normalization::Smart);
        let mut matcher = Matcher::new(MatcherConfig::DEFAULT);
        let mut buf = Vec::new();
        let mut hits: Vec<(u32, usize, MachineItem)> = Vec::new();
        for (pos, &row) in self.order.iter().enumerate() {
            let m = &self.rows[row].machine;
            let label = m.label.clone().unwrap_or_default();
            let target = m.target.to_string();
            let sources = sources_text(m);
            let hay = format!("{label} {target} {sources}");
            let mut idx = Vec::new();
            let Some(score) =
                pattern.indices(Utf32Str::new(&hay, &mut buf), &mut matcher, &mut idx)
            else {
                continue;
            };
            idx.sort_unstable();
            idx.dedup();
            let (lc, tc) = (label.chars().count(), target.chars().count());
            let mut item = MachineItem {
                row,
                label: Vec::new(),
                target: Vec::new(),
                sources: Vec::new(),
            };
            // Highlight each field on its own where the filter matches it there; a filter
            // whose words match different fields (`foo bar`) falls back to the joint match.
            let mut field = |text: &str| -> Option<Vec<usize>> {
                if text.is_empty() {
                    return None;
                }
                let mut v = Vec::new();
                pattern.indices(Utf32Str::new(text, &mut buf), &mut matcher, &mut v)?;
                v.sort_unstable();
                v.dedup();
                Some(v.into_iter().map(|i| i as usize).collect())
            };
            let (l, t, s) = (field(&label), field(&target), field(&sources));
            if l.is_some() || t.is_some() || s.is_some() {
                item.label = l.unwrap_or_default();
                item.target = t.unwrap_or_default();
                item.sources = s.unwrap_or_default();
            } else {
                for i in idx.into_iter().map(|i| i as usize) {
                    if i < lc {
                        item.label.push(i);
                    } else if i > lc && i <= lc + tc {
                        item.target.push(i - lc - 1);
                    } else if i > lc + tc + 1 {
                        item.sources.push(i - lc - tc - 2);
                    }
                }
            }
            hits.push((score, pos, item));
        }
        if hits.is_empty() {
            return Target::parse(filter)
                .map(|t| vec![Item::Synthetic(t)])
                .unwrap_or_default();
        }
        hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        hits.into_iter().map(|(_, _, i)| Item::Machine(i)).collect()
    }
}

/// The sources of a machine as the list shows them: `herdr tailscale`.
#[must_use]
pub fn sources_text(m: &Machine) -> String {
    m.sources
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::{self, NOW};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn app() -> App {
        App::new(demo::startup(NOW), NOW)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn type_text(a: &mut App, s: &str) {
        for c in s.chars() {
            assert_eq!(a.handle_key(key(KeyCode::Char(c))), Effect::None);
        }
    }

    fn selected_label(a: &App) -> String {
        match a.selected_item() {
            Some(Item::Machine(m)) => a.rows()[m.row].machine.display_label().to_string(),
            Some(Item::Synthetic(t)) => format!("↵{t}"),
            None => String::new(),
        }
    }

    fn labels(a: &App) -> Vec<String> {
        a.items()
            .iter()
            .map(|i| match i {
                Item::Machine(m) => a.rows()[m.row].machine.display_label().to_string(),
                Item::Synthetic(t) => format!("↵{t}"),
            })
            .collect()
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

    #[test]
    fn startup_sorts_by_rank_then_index_and_queues_the_unchecked() {
        let mut a = app();
        assert_eq!(
            labels(&a),
            ["studio", "j", "render", "kiosk", "nas", "lab", "air"]
        );
        assert_eq!(selected_label(&a), "studio");
        let queued: Vec<String> = a
            .take_checks()
            .iter()
            .map(|m| m.target.to_string())
            .collect();
        assert_eq!(queued, ["render@10.0.0.7", "kiosk.tail.ts.net", "nas.lan"]);
        assert_eq!(a.take_checks().len(), 0);
        assert_eq!(a.counts(), (7, 7));
    }

    #[test]
    fn stale_results_are_rechecked_and_fresh_ones_are_not() {
        let mut s = demo::startup(NOW);
        let c = s.caches.get_mut("j@10.0.0.2").unwrap();
        c.checked_at = NOW - 601;
        let mut a = App::new(s, NOW);
        let queued: Vec<String> = a
            .take_checks()
            .iter()
            .map(|m| m.target.to_string())
            .collect();
        assert!(queued.contains(&"j@10.0.0.2".to_string()), "{queued:?}");
        assert!(!queued.contains(&"studio.lan".to_string()));
    }

    #[test]
    fn printable_keys_filter_and_backspace_removes() {
        let mut a = app();
        type_text(&mut a, "re");
        assert_eq!(a.filter(), "re");
        assert_eq!(selected_label(&a), "render");
        assert_eq!(a.handle_key(key(KeyCode::Backspace)), Effect::None);
        assert_eq!(a.filter(), "r");
        a.handle_key(ctrl('h'));
        assert_eq!(a.filter(), "");
        a.handle_key(key(KeyCode::Backspace));
        assert_eq!(a.filter(), "");
    }

    #[test]
    fn ctrl_u_clears_and_ctrl_w_deletes_a_word() {
        let mut a = app();
        type_text(&mut a, "abc def ghi");
        a.handle_key(ctrl('w'));
        assert_eq!(a.filter(), "abc def ");
        a.handle_key(ctrl('w'));
        assert_eq!(a.filter(), "abc ");
        a.handle_key(ctrl('w'));
        assert_eq!(a.filter(), "");
        type_text(&mut a, "xyz");
        a.handle_key(ctrl('u'));
        assert_eq!(a.filter(), "");
        a.handle_key(ctrl('w'));
        assert_eq!(a.filter(), "");
    }

    #[test]
    fn movement_keys_wrap_and_page_home_end_clamp() {
        let mut a = app();
        assert_eq!(selected_label(&a), "studio");
        a.handle_key(key(KeyCode::Up));
        assert_eq!(selected_label(&a), "air");
        a.handle_key(key(KeyCode::Down));
        assert_eq!(selected_label(&a), "studio");
        a.handle_key(ctrl('n'));
        a.handle_key(ctrl('j'));
        assert_eq!(selected_label(&a), "render");
        a.handle_key(ctrl('p'));
        a.handle_key(ctrl('k'));
        assert_eq!(selected_label(&a), "studio");
        a.handle_key(key(KeyCode::End));
        assert_eq!(selected_label(&a), "air");
        a.handle_key(key(KeyCode::PageDown));
        assert_eq!(selected_label(&a), "air");
        a.handle_key(key(KeyCode::Home));
        assert_eq!(selected_label(&a), "studio");
        a.handle_key(key(KeyCode::PageUp));
        assert_eq!(selected_label(&a), "studio");
        a.set_page(4);
        a.handle_key(key(KeyCode::PageDown));
        assert_eq!(selected_label(&a), "kiosk");
    }

    #[test]
    fn enter_connects_with_the_machines_default_and_alt_enter_with_headless() {
        let mut a = app();
        a.handle_key(key(KeyCode::Down));
        let eff = a.handle_key(key(KeyCode::Enter));
        assert_eq!(
            eff,
            Effect::Connect(ConnectRequest {
                target: Target::parse("j@10.0.0.2").unwrap(),
                choice: None
            })
        );
        assert_eq!(a.connecting().unwrap().target, "j@10.0.0.2");
        // fresh and ready: no check first
        assert_eq!(a.connecting().unwrap().text(), "opening…");

        let mut a = app();
        a.handle_key(key(KeyCode::Down));
        let eff = a.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        assert_eq!(
            eff,
            Effect::Connect(ConnectRequest {
                target: Target::parse("j@10.0.0.2").unwrap(),
                choice: Some(ScreenChoice::Headless)
            })
        );
    }

    #[test]
    fn connecting_a_machine_that_needs_a_check_starts_at_checking() {
        let mut a = app();
        a.handle_key(key(KeyCode::Down));
        a.handle_key(key(KeyCode::Down)); // render: no result yet
        a.handle_key(key(KeyCode::Enter));
        assert_eq!(a.connecting().unwrap().text(), "checking…");
        a.on_progress(Progress::Launching);
        assert_eq!(a.connecting().unwrap().text(), "opening…");
        a.on_progress(Progress::Waiting);
        assert_eq!(a.connecting().unwrap().text(), "waiting for the window…");
    }

    #[test]
    fn while_connecting_only_esc_and_quit_do_anything() {
        let mut a = app();
        a.handle_key(key(KeyCode::Enter));
        for k in [
            key(KeyCode::Down),
            key(KeyCode::Char('x')),
            key(KeyCode::Enter),
            ctrl('o'),
            ctrl('r'),
            ctrl('e'),
            ctrl('f'),
        ] {
            assert_eq!(a.handle_key(k), Effect::None);
        }
        assert_eq!(a.filter(), "");
        assert!(a.connecting().is_some());
        assert_eq!(a.handle_key(key(KeyCode::Esc)), Effect::None);
        assert!(a.connecting().is_none());

        a.handle_key(key(KeyCode::Enter));
        assert_eq!(a.handle_key(ctrl('c')), Effect::Quit);
    }

    #[test]
    fn a_successful_connect_quits_even_after_esc() {
        let mut a = app();
        a.handle_key(key(KeyCode::Enter));
        assert_eq!(a.on_connect(Ok(Connected::Focused)), Effect::Quit);
        let mut a = app();
        a.handle_key(key(KeyCode::Enter));
        a.handle_key(key(KeyCode::Esc));
        assert_eq!(a.on_connect(Ok(Connected::Launched)), Effect::Quit);
    }

    #[test]
    fn a_failed_connect_shows_the_panel_and_esc_closes_only_the_panel() {
        let mut a = app();
        type_text(&mut a, "lab");
        a.handle_key(key(KeyCode::Enter));
        let err = ConnectError {
            message: "lab@10.0.0.9: gliff-server is not installed there.".into(),
            hint: Some("Press ctrl-f to install it.".into()),
            remedy: Some(Remedy::InstallServer),
            log: None,
        };
        assert_eq!(a.on_connect(Err(err)), Effect::None);
        assert!(a.connecting().is_none());
        assert_eq!(
            a.error().unwrap().message,
            "lab@10.0.0.9: gliff-server is not installed there."
        );
        assert_eq!(a.handle_key(key(KeyCode::Esc)), Effect::None);
        assert!(a.error().is_none());
        assert_eq!(a.filter(), "lab", "esc only closed the panel");
    }

    #[test]
    fn a_failed_connect_after_stopping_to_wait_is_dropped() {
        let mut a = app();
        a.handle_key(key(KeyCode::Enter));
        a.handle_key(key(KeyCode::Esc));
        let err = ConnectError {
            message: "x".into(),
            hint: None,
            remedy: None,
            log: None,
        };
        a.on_connect(Err(err));
        assert!(a.error().is_none());
    }

    #[test]
    fn movement_dismisses_the_error_panel() {
        let mut a = app();
        a.handle_key(key(KeyCode::Enter));
        a.on_connect(Err(ConnectError {
            message: "x".into(),
            hint: None,
            remedy: None,
            log: None,
        }));
        a.handle_key(key(KeyCode::Down));
        assert!(a.error().is_none());
        assert_eq!(selected_label(&a), "j");
    }

    #[test]
    fn esc_clears_the_filter_then_quits() {
        let mut a = app();
        type_text(&mut a, "re");
        assert_eq!(a.handle_key(key(KeyCode::Esc)), Effect::None);
        assert_eq!(a.filter(), "");
        assert_eq!(a.handle_key(key(KeyCode::Esc)), Effect::Quit);
    }

    #[test]
    fn ctrl_c_and_ctrl_q_quit() {
        assert_eq!(app().handle_key(ctrl('c')), Effect::Quit);
        assert_eq!(app().handle_key(ctrl('q')), Effect::Quit);
    }

    #[test]
    fn nothing_matching_a_valid_target_offers_a_synthetic_row() {
        let mut a = app();
        type_text(&mut a, "zz@10.9.9.9");
        assert_eq!(labels(&a), ["↵zz@10.9.9.9"]);
        assert_eq!(a.counts(), (0, 7));
        let eff = a.handle_key(key(KeyCode::Enter));
        assert_eq!(
            eff,
            Effect::Connect(ConnectRequest {
                target: Target::parse("zz@10.9.9.9").unwrap(),
                choice: None
            })
        );
        assert_eq!(a.connecting().unwrap().text(), "checking…");
    }

    #[test]
    fn a_typed_target_is_trimmed_before_it_is_connected_to() {
        // A typed target is trimmed: ' j@10.0.0.2 ' connects to j@10.0.0.2.
        let mut a = app();
        type_text(&mut a, " zz@10.9.9.9 ");
        assert_eq!(
            a.handle_key(key(KeyCode::Enter)),
            Effect::Connect(ConnectRequest {
                target: Target::parse("zz@10.9.9.9").unwrap(),
                choice: None
            })
        );
    }

    #[test]
    fn nothing_matching_an_invalid_target_shows_no_rows() {
        let mut a = app();
        type_text(&mut a, "zz zz!");
        assert_eq!(a.items().len(), 0);
        assert_eq!(a.handle_key(key(KeyCode::Enter)), Effect::None);
        assert!(a.connecting().is_none());
    }

    #[test]
    fn filter_orders_by_score_and_records_matches() {
        let mut a = app();
        type_text(&mut a, "re");
        let Some(Item::Machine(m)) = a.items().first() else {
            panic!("no match")
        };
        assert_eq!(a.rows()[m.row].machine.display_label(), "render");
        assert_eq!(m.label, [0, 1]);
        assert_eq!(a.counts().0, a.items().len());
        // every match really contains r and e somewhere in label/target/sources
        assert!(a.items().len() >= 2);
    }

    #[test]
    fn typing_selects_the_best_match_and_clearing_keeps_the_machine() {
        let mut a = app();
        a.handle_key(key(KeyCode::End));
        assert_eq!(selected_label(&a), "air");
        a.handle_key(key(KeyCode::Char('l')));
        a.handle_key(key(KeyCode::Char('a')));
        a.handle_key(key(KeyCode::Char('b')));
        assert_eq!(selected_label(&a), "lab");
        a.handle_key(key(KeyCode::Esc));
        assert_eq!(selected_label(&a), "lab");
        assert_eq!(a.items().len(), 7);
    }

    #[test]
    fn results_update_rows_in_place_and_the_selection_stays() {
        let mut a = app();
        // select kiosk (a checking row)
        a.handle_key(key(KeyCode::Down));
        a.handle_key(key(KeyCode::Down));
        a.handle_key(key(KeyCode::Down));
        assert_eq!(selected_label(&a), "kiosk");
        let before = labels(&a);
        a.on_check(finished("kiosk.tail.ts.net", Status::NoHyprland, &[]));
        a.on_check(finished("render@10.0.0.7", Status::Ready, &["DP-1"]));
        a.on_check(finished(
            "nas.lan",
            Status::Offline {
                detail: "no answer within 15s".into(),
            },
            &[],
        ));
        assert_eq!(labels(&a), before, "rows do not move");
        assert_eq!(selected_label(&a), "kiosk");
        assert!(matches!(a.state(3), RowState::NeedsSetup(_)));
    }

    #[test]
    fn ctrl_r_rechecks_everything_checkable_and_resorts_when_all_finish() {
        let mut a = app();
        a.take_checks();
        // move to lab
        type_text(&mut a, "lab");
        a.handle_key(key(KeyCode::Esc));
        assert_eq!(selected_label(&a), "lab");
        assert_eq!(a.handle_key(ctrl('r')), Effect::CheckAll);
        let queued = a.take_checks();
        // render, kiosk and nas are already being checked from startup
        assert_eq!(queued.len(), 3, "studio j lab; not air");
        assert_eq!(a.handle_key(ctrl('r')), Effect::None, "a round is running");
        let order_before = labels(&a);
        for ev in demo::recheck_script(NOW) {
            a.on_check(ev.event);
            if labels(&a) != order_before {
                break;
            }
        }
        assert_ne!(
            labels(&a),
            order_before,
            "re-sorted when the round completed"
        );
        assert_eq!(
            labels(&a),
            ["studio", "j", "render", "kiosk", "lab", "nas", "air"]
        );
        assert_eq!(
            selected_label(&a),
            "lab",
            "the selection followed the machine"
        );
    }

    #[test]
    fn ctrl_o_opens_the_chooser_from_the_cache() {
        let mut a = app();
        a.handle_key(key(KeyCode::Down)); // j
        assert_eq!(a.handle_key(ctrl('o')), Effect::None);
        let Overlay::Chooser(c) = a.overlay() else {
            panic!("no chooser")
        };
        let opts: Vec<_> = c.options().into_iter().map(|o| o.0).collect();
        assert_eq!(
            opts,
            [
                "Mirror the focused screen",
                "Mirror HDMI-A-1",
                "Private screen sized to the window"
            ]
        );
        // down, enter: picks Mirror HDMI-A-1
        a.handle_key(key(KeyCode::Down));
        let eff = a.handle_key(key(KeyCode::Enter));
        assert_eq!(
            eff,
            Effect::Connect(ConnectRequest {
                target: Target::parse("j@10.0.0.2").unwrap(),
                choice: Some(ScreenChoice::Output("HDMI-A-1".into()))
            })
        );
        assert!(matches!(a.overlay(), Overlay::None));
    }

    #[test]
    fn chooser_wraps_and_esc_cancels_without_leaving() {
        let mut a = app();
        a.handle_key(key(KeyCode::Down));
        a.handle_key(ctrl('o'));
        a.handle_key(key(KeyCode::Up)); // wraps to the last option
        let eff = a.handle_key(key(KeyCode::Enter));
        assert_eq!(
            eff,
            Effect::Connect(ConnectRequest {
                target: Target::parse("j@10.0.0.2").unwrap(),
                choice: Some(ScreenChoice::Headless)
            })
        );
        let mut a = app();
        a.handle_key(ctrl('o'));
        assert_eq!(a.handle_key(key(KeyCode::Esc)), Effect::None);
        assert!(matches!(a.overlay(), Overlay::None));
        assert_eq!(a.handle_key(ctrl('c')), Effect::Quit);
    }

    #[test]
    fn ctrl_o_without_a_cached_result_checks_first() {
        let mut a = app();
        a.take_checks();
        a.handle_key(key(KeyCode::Down));
        a.handle_key(key(KeyCode::Down)); // render: no result, already checking
        let eff = a.handle_key(ctrl('o'));
        assert!(matches!(eff, Effect::StartChooserCheck(_)));
        let Overlay::Chooser(c) = a.overlay() else {
            panic!()
        };
        assert!(c.is_loading());
        // keys do nothing but Esc while loading
        assert_eq!(a.handle_key(key(KeyCode::Enter)), Effect::None);
        a.on_check(finished(
            "render@10.0.0.7",
            Status::Ready,
            &["DP-1", "DP-2"],
        ));
        let Overlay::Chooser(c) = a.overlay() else {
            panic!()
        };
        assert!(!c.is_loading());
        assert_eq!(c.options().len(), 4);
    }

    #[test]
    fn ctrl_o_on_an_unsupported_machine_does_nothing() {
        let mut a = app();
        a.handle_key(key(KeyCode::End)); // air
        assert_eq!(a.handle_key(ctrl('o')), Effect::None);
        assert!(matches!(a.overlay(), Overlay::None));
    }

    #[test]
    fn enter_on_an_unsupported_machine_explains_without_connecting() {
        let mut a = app();
        a.handle_key(key(KeyCode::End));
        assert_eq!(a.handle_key(key(KeyCode::Enter)), Effect::None);
        assert!(a.connecting().is_none());
        assert!(a.error().unwrap().message.contains("runs macOS"));
    }

    #[test]
    fn ctrl_f_needs_a_remedy_and_depends_on_herdr() {
        let mut a = app();
        assert_eq!(
            a.handle_key(ctrl('f')),
            Effect::None,
            "studio has no remedy"
        );
        type_text(&mut a, "lab");
        let t = Target::parse("lab@10.0.0.9").unwrap();
        assert_eq!(a.handle_key(ctrl('f')), Effect::FixInline(t.clone()));
        let mut s = demo::startup(NOW);
        s.in_herdr = true;
        let mut a = App::new(s, NOW);
        type_text(&mut a, "lab");
        assert_eq!(a.handle_key(ctrl('f')), Effect::OpenFixPane(t));
        assert!(a.fix_for_selected().is_some());
    }

    #[test]
    fn ctrl_e_asks_for_the_editor() {
        assert_eq!(app().handle_key(ctrl('e')), Effect::EditConfig);
    }

    #[test]
    fn reload_keeps_the_selection_and_the_filter_and_checks_new_stale_machines() {
        let mut a = app();
        type_text(&mut a, "lab");
        a.take_checks();
        let mut s = demo::startup(NOW);
        s.banner = Some("config.toml line 4: x — ctrl-e to edit".into());
        s.caches.remove("lab@10.0.0.9");
        a.reload(s);
        assert_eq!(a.filter(), "lab");
        assert_eq!(selected_label(&a), "lab");
        assert_eq!(a.banner(), Some("config.toml line 4: x — ctrl-e to edit"));
        assert!(
            a.take_checks()
                .iter()
                .any(|m| m.target.as_str() == "lab@10.0.0.9")
        );
    }

    #[test]
    fn mouse_click_selects_and_double_click_connects_and_wheel_moves() {
        use ratatui::crossterm::event::MouseEventKind as K;
        let mut a = app();
        a.set_geometry(Some(ListGeom {
            rect: Rect::new(0, 2, 60, 5),
            first: 0,
        }));
        let ev = |kind, row| MouseEvent {
            kind,
            column: 4,
            row,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            a.handle_mouse(ev(K::Down(MouseButton::Left), 4)),
            Effect::None
        );
        assert_eq!(selected_label(&a), "render");
        a.tick();
        let eff = a.handle_mouse(ev(K::Down(MouseButton::Left), 4));
        assert!(matches!(eff, Effect::Connect(_)));
        a.on_connect(Err(ConnectError {
            message: "x".into(),
            hint: None,
            remedy: None,
            log: None,
        }));
        // a click on another row, then a late second click: no double click
        a.handle_mouse(ev(K::Down(MouseButton::Left), 3));
        for _ in 0..20 {
            a.tick();
        }
        assert_eq!(
            a.handle_mouse(ev(K::Down(MouseButton::Left), 3)),
            Effect::None
        );
        // outside the list
        assert_eq!(
            a.handle_mouse(ev(K::Down(MouseButton::Left), 40)),
            Effect::None
        );
        a.handle_mouse(ev(K::ScrollDown, 3));
        assert_eq!(selected_label(&a), "render");
        a.handle_mouse(ev(K::ScrollUp, 3));
        a.handle_mouse(ev(K::ScrollUp, 3));
        a.handle_mouse(ev(K::ScrollUp, 3));
        a.handle_mouse(ev(K::ScrollUp, 3));
        assert_eq!(selected_label(&a), "studio", "the wheel stops at the end");
    }

    #[test]
    fn tick_marks_dirty_only_while_something_spins() {
        let mut a = app();
        assert!(a.take_dirty());
        for _ in 0..4 {
            a.tick();
        }
        assert!(a.take_dirty(), "render, kiosk and nas are checking");
        a.on_check(finished("render@10.0.0.7", Status::Ready, &[]));
        a.on_check(finished("kiosk.tail.ts.net", Status::NoHyprland, &[]));
        a.on_check(finished("nas.lan", Status::Auth, &[]));
        a.take_dirty();
        for _ in 0..4 {
            a.tick();
        }
        assert!(!a.take_dirty(), "nothing is animating");
    }

    #[test]
    fn open_targets_update_in_place() {
        let mut a = app();
        let before = labels(&a);
        a.take_dirty();
        a.on_open_targets(HashSet::new());
        assert!(a.take_dirty());
        assert_eq!(a.state(0), RowState::Ready);
        a.on_open_targets(HashSet::new());
        assert!(!a.take_dirty(), "no change, no redraw");
        assert_eq!(labels(&a), before);
    }

    #[test]
    fn an_empty_roster_has_no_rows_and_keys_are_harmless() {
        let mut s = demo::startup(NOW);
        s.machines.clear();
        let mut a = App::new(s, NOW);
        for k in [
            key(KeyCode::Down),
            key(KeyCode::Up),
            key(KeyCode::Enter),
            key(KeyCode::End),
            ctrl('o'),
            ctrl('f'),
            ctrl('r'),
        ] {
            assert_eq!(a.handle_key(k), Effect::None);
        }
        assert!(a.selected_item().is_none());
        assert_eq!(a.counts(), (0, 0));
    }

    #[test]
    fn failures_of_effects_show_in_the_panel() {
        let mut a = app();
        a.on_failure("a popup pane is already open", None);
        assert_eq!(a.error().unwrap().message, "a popup pane is already open");
    }
}
