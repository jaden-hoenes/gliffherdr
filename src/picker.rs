//! The picker TUI: [`run`] owns the terminal and the threads, [`app::App`] owns the state.
//!
//! The event loop polls the terminal for 50 ms at a time, drains what the background threads
//! reported (checks, the open windows, the connect), and redraws only when something changed.
//! Nothing here blocks on the network: checks, the open-windows refresh and the connect each run
//! on their own thread.

pub mod app;
pub mod chooser;
pub mod input;
pub mod view;

use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal, Stdout, stdout};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

use crate::check::{CheckEvent, check_many, check_one, load_cached};
use crate::cli::CliError;
use crate::config::{self, Config};
use crate::connect::{ConnectError, ConnectRequest, Connected, Ctx, Progress, connect};
use crate::demo::{self, Scripted};
use crate::fix::{self, describe, remedy_for};
use crate::herdr::Herdr;
use crate::launch::ScreenChoice;
use crate::paths::Paths;
use crate::roster::{Machine, bare_machine, collect};
use crate::status::sentence;
use crate::sys::{Sys, cmd};
use crate::target::Target;
use app::{App, Effect, Startup};
use chooser::Chooser;
use input::{Input, classify};

/// How often the loop wakes up without input.
const TICK: Duration = Duration::from_millis(50);
/// How often the open gliff windows are looked up.
const OPEN_EVERY: Duration = Duration::from_secs(2);
/// The editors tried after `$VISUAL` and `$EDITOR`.
const FALLBACK_EDITORS: [&str; 2] = ["nvim", "vi"];

type Term = Terminal<CrosstermBackend<Stdout>>;

/// Whether the terminal is in the picker's mode (so the panic hook knows to undo it).
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Put the terminal into the picker's mode: raw, alternate screen, mouse, no cursor.
fn enter() -> std::io::Result<()> {
    enable_raw_mode()?;
    ACTIVE.store(true, Ordering::SeqCst);
    execute!(
        stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        ratatui::crossterm::cursor::Hide
    )
}

/// Undo [`enter`]. Safe to call twice and from a panic hook.
fn leave() {
    if ACTIVE.swap(false, Ordering::SeqCst) {
        let _ = execute!(
            stdout(),
            DisableMouseCapture,
            ratatui::crossterm::cursor::Show,
            LeaveAlternateScreen
        );
        let _ = disable_raw_mode();
    }
}

/// Restores the terminal when dropped: every way out of the picker, an error included.
struct Guard;

impl Drop for Guard {
    fn drop(&mut self) {
        leave();
    }
}

/// Make a panic restore the terminal first, then print the panic as usual.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        leave();
        previous(info);
    }));
}

fn open_terminal() -> anyhow::Result<(Term, Guard)> {
    install_panic_hook();
    enter().context("could not set up the terminal")?;
    let guard = Guard;
    let term = Terminal::new(CrosstermBackend::new(stdout()))?;
    Ok((term, guard))
}

/// What the background threads report besides checks.
enum Msg {
    Open(HashSet<String>),
    Progress(u64, Progress),
    Done(u64, Result<Connected, ConnectError>),
}

/// The configuration, or the defaults and the banner text when the file is invalid.
fn load_config_with_banner(paths: &Paths) -> (Config, Option<String>) {
    match config::load(paths) {
        Ok(c) => (c, None),
        Err(e) => (
            Config::default(),
            Some(format!("{} — ctrl-e to edit", e.summary())),
        ),
    }
}

/// Gather what the picker starts from: the roster, every cache file, the open windows.
pub fn load(ctx: &Ctx, banner: Option<String>) -> Startup {
    let sys = ctx.sys.as_ref();
    let roster = collect(sys, &ctx.paths, &ctx.config);
    let caches: HashMap<String, _> = roster
        .machines
        .iter()
        .filter_map(|m| load_cached(&ctx.paths, &m.target).map(|r| (m.target.to_string(), r)))
        .collect();
    Startup {
        config: ctx.config.clone(),
        machines: roster.machines,
        caches,
        open: crate::hypr::open_targets(sys),
        banner,
        unreadable_sources: roster.warnings.len(),
        in_herdr: Herdr::in_herdr(sys),
    }
}

/// `gliffherdr pick`: the picker on real data, or on the demo fixtures when
/// `GLIFFHERDR_DEMO=1`.
pub fn pick(sys: &Arc<dyn Sys>) -> Result<(), CliError> {
    if !stdout().is_terminal() || !std::io::stdin().is_terminal() {
        return Err(CliError::failed("pick needs a terminal"));
    }
    let result = if sys.env("GLIFFHERDR_DEMO").as_deref() == Some("1") {
        run_demo()
    } else {
        let paths = Paths::from_sys(sys.as_ref());
        let (config, banner) = load_config_with_banner(&paths);
        run_with(Ctx::new(Arc::clone(sys), paths, config), banner, false)
    };
    result.map_err(|e| CliError::failed(format!("{e:#}")))
}

/// Run the picker on real data until the user leaves or a connect succeeds.
pub fn run(ctx: Ctx) -> anyhow::Result<()> {
    run_with(ctx, None, false)
}

/// Run the picker on the demo fixtures: no network, no real configuration, state or windows.
pub fn run_demo() -> anyhow::Result<()> {
    let ctx = demo::ctx();
    let result = run_with(ctx.clone(), None, true);
    demo::cleanup(&ctx);
    result
}

fn run_with(ctx: Ctx, banner: Option<String>, demo: bool) -> anyhow::Result<()> {
    let (mut term, _guard) = open_terminal()?;
    term.draw(view::draw_loading)?;
    let now = ctx.sys.now_unix();
    let start = if demo {
        demo::startup(now)
    } else {
        load(&ctx, banner)
    };
    let mut app = App::new(start, now);
    let (tx, rx) = channel();
    let (check_tx, check_rx) = channel();
    let mut rt = Runtime {
        ctx,
        demo,
        tx,
        rx,
        check_tx,
        check_rx,
        generation: 0,
        connect_target: None,
        open_inflight: false,
        last_open: Instant::now(),
        scripted: Vec::new(),
        demo_checks_started: false,
    };
    rt.event_loop(&mut term, &mut app)
}

/// Everything the event loop owns besides the terminal and the app.
struct Runtime {
    ctx: Ctx,
    demo: bool,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    check_tx: Sender<CheckEvent>,
    check_rx: Receiver<CheckEvent>,
    generation: u64,
    connect_target: Option<Target>,
    open_inflight: bool,
    last_open: Instant,
    scripted: Vec<(Instant, CheckEvent)>,
    demo_checks_started: bool,
}

enum Flow {
    Continue,
    Quit,
}

impl Runtime {
    fn event_loop(&mut self, term: &mut Term, app: &mut App) -> anyhow::Result<()> {
        let mut last_tick = Instant::now();
        loop {
            app.set_now(self.ctx.sys.now_unix());
            self.start_checks(app);
            if app.take_dirty() {
                term.draw(|f| view::draw(f, app))?;
            }
            let wait = TICK.saturating_sub(last_tick.elapsed());
            let mut effects = Vec::new();
            if event::poll(wait)? {
                // Everything that is already waiting is handled before the next frame.
                loop {
                    match event::read()? {
                        Event::Key(k) => effects.push(app.handle_key(k)),
                        Event::Mouse(m) => effects.push(app.handle_mouse(m)),
                        Event::Resize(..) => app.invalidate(),
                        _ => {}
                    }
                    if !event::poll(Duration::ZERO)? {
                        break;
                    }
                }
            }
            self.drain(app, &mut effects);
            if last_tick.elapsed() >= TICK {
                last_tick = Instant::now();
                app.tick();
                self.refresh_open();
            }
            for effect in effects {
                if let Flow::Quit = self.execute(effect, term, app)? {
                    return Ok(());
                }
            }
        }
    }

    /// Check events, scripted demo events and thread messages into the app.
    fn drain(&mut self, app: &mut App, effects: &mut Vec<Effect>) {
        while let Ok(ev) = self.check_rx.try_recv() {
            app.on_check(ev);
        }
        let now = Instant::now();
        let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.scripted)
            .into_iter()
            .partition(|(at, _)| *at <= now);
        self.scripted = later;
        for (_, ev) in due {
            app.on_check(ev);
        }
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Open(open) => {
                    self.open_inflight = false;
                    app.on_open_targets(open);
                }
                Msg::Progress(g, p) if g == self.generation => app.on_progress(p),
                Msg::Done(_, Ok(done)) => effects.push(app.on_connect(Ok(done))),
                Msg::Done(g, Err(e)) if g == self.generation => {
                    // connect may have checked the machine again: show what it found
                    if let Some(c) = self
                        .connect_target
                        .as_ref()
                        .and_then(|t| load_cached(&self.ctx.paths, t))
                    {
                        app.on_check(CheckEvent::Finished(c));
                    }
                    effects.push(app.on_connect(Err(e)));
                }
                Msg::Progress(..) | Msg::Done(..) => {}
            }
        }
    }

    /// Start the checks the app asked for.
    fn start_checks(&mut self, app: &mut App) {
        let machines = app.take_checks();
        if machines.is_empty() {
            return;
        }
        if self.demo {
            let now = self.ctx.sys.now_unix();
            let script: Vec<Scripted> = if self.demo_checks_started {
                demo::recheck_script(now)
            } else {
                demo::script(now)
            };
            self.demo_checks_started = true;
            let wanted: HashSet<String> = machines.iter().map(|m| m.target.to_string()).collect();
            let t0 = Instant::now();
            for s in script {
                let CheckEvent::Finished(r) = &s.event else {
                    continue;
                };
                if wanted.contains(&r.target) {
                    self.scripted
                        .push((t0 + Duration::from_millis(s.at_ms), s.event));
                }
            }
            return;
        }
        // The handle is dropped on purpose: the threads end with their work, and results come
        // back through the channel.
        drop(check_many(
            Arc::clone(&self.ctx.sys),
            self.ctx.paths.clone(),
            self.ctx.config.clone(),
            machines,
            self.check_tx.clone(),
        ));
    }

    /// Look up the open gliff windows every 2 s, on a thread.
    fn refresh_open(&mut self) {
        if self.demo || self.open_inflight || self.last_open.elapsed() < OPEN_EVERY {
            return;
        }
        self.open_inflight = true;
        self.last_open = Instant::now();
        let (sys, tx) = (Arc::clone(&self.ctx.sys), self.tx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Open(crate::hypr::open_targets(sys.as_ref())));
        });
    }

    fn execute(&mut self, effect: Effect, term: &mut Term, app: &mut App) -> anyhow::Result<Flow> {
        match effect {
            Effect::None | Effect::CheckAll | Effect::StartChooserCheck(_) => {}
            Effect::Quit => return Ok(Flow::Quit),
            Effect::Connect(req) => self.start_connect(req, app),
            Effect::OpenFixPane(target) => {
                if self.demo {
                    demo_fix_notice(app, &target);
                } else {
                    let opened = self.ctx.herdr.open_pane(
                        self.ctx.sys.as_ref(),
                        "fix",
                        Some("split"),
                        &[("GLIFFHERDR_FIX_TARGET", target.as_str())],
                        true,
                    );
                    match opened {
                        Ok(()) => return Ok(Flow::Quit),
                        Err(e) => app.on_failure(e.to_string(), None),
                    }
                }
            }
            Effect::FixInline(target) => {
                if self.demo {
                    demo_fix_notice(app, &target);
                } else {
                    self.fix_inline(&target, term, app)?;
                }
            }
            Effect::EditConfig => self.edit_config(term, app)?,
        }
        Ok(Flow::Continue)
    }

    fn start_connect(&mut self, req: ConnectRequest, app: &App) {
        self.generation += 1;
        let generation = self.generation;
        self.connect_target = Some(req.target.clone());
        let tx = self.tx.clone();
        if self.demo {
            let plan = demo_connect(app, &req.target);
            std::thread::spawn(move || {
                for step in plan.steps {
                    let _ = tx.send(Msg::Progress(generation, step));
                    std::thread::sleep(Duration::from_millis(demo::CONNECT_STEP_MS));
                }
                let _ = tx.send(Msg::Done(generation, plan.result));
            });
            return;
        }
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let progress_tx = tx.clone();
            let result = connect(&ctx, &req, &mut |p| {
                let _ = progress_tx.send(Msg::Progress(generation, p));
            });
            let _ = tx.send(Msg::Done(generation, result));
        });
    }

    /// Leave the terminal, run `f`, come back and redraw everything.
    fn outside<T>(
        &mut self,
        term: &mut Term,
        app: &mut App,
        f: impl FnOnce(&mut Self) -> T,
    ) -> anyhow::Result<T> {
        leave();
        let out = f(self);
        enter().context("could not set up the terminal again")?;
        term.clear()?;
        app.invalidate();
        Ok(out)
    }

    fn fix_inline(
        &mut self,
        target: &Target,
        term: &mut Term,
        app: &mut App,
    ) -> anyhow::Result<()> {
        let machine = app.find_row(target.as_str()).map_or_else(
            || bare_machine(self.ctx.sys.as_ref(), &self.ctx.config, target.clone()),
            |r| r.machine.clone(),
        );
        let report = self.outside(term, app, |rt| {
            fix::run(
                rt.ctx.sys.as_ref(),
                &rt.ctx.paths,
                &rt.ctx.config,
                &machine,
                &mut stdout(),
                true,
            )
        })?;
        app.on_check(CheckEvent::Finished(report.result().clone()));
        Ok(())
    }

    fn edit_config(&mut self, term: &mut Term, app: &mut App) -> anyhow::Result<()> {
        if self.demo {
            app.on_failure(
                "Demo mode: config.toml is not opened.",
                Some("The real picker edits it here with $VISUAL, $EDITOR, nvim or vi.".into()),
            );
            return Ok(());
        }
        if let Err(e) = config::write_template_if_missing(&self.ctx.paths) {
            app.on_failure(format!("Could not create config.toml: {e}."), None);
            return Ok(());
        }
        let file = self.ctx.paths.config_file();
        let command = editor_command(self.ctx.sys.as_ref(), &file.to_string_lossy());
        let ran = self.outside(term, app, |rt| rt.ctx.sys.run_interactive(&command))?;
        if let Err(e) = ran {
            app.on_failure(
                format!("Could not start {}: {e}.", command.program),
                Some("Set $VISUAL or $EDITOR to an editor that is installed.".into()),
            );
            return Ok(());
        }
        let (config, banner) = load_config_with_banner(&self.ctx.paths);
        self.ctx.config = config;
        app.reload(load(&self.ctx, banner));
        Ok(())
    }
}

/// `$VISUAL`, else `$EDITOR`, else `nvim`, else `vi`, opening `file`. The variables may carry
/// arguments (`code -w`); they are split on whitespace, never run through a shell.
fn editor_command(sys: &dyn Sys, file: &str) -> crate::sys::Cmd {
    let from_env = ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|k| sys.env(k))
        .find(|v| !v.trim().is_empty());
    let spec = from_env.unwrap_or_else(|| {
        FALLBACK_EDITORS
            .iter()
            .find(|e| sys.which(e).is_some())
            .unwrap_or(&"vi")
            .to_string()
    });
    let mut words = spec.split_whitespace();
    let program = words.next().unwrap_or("vi");
    let mut args: Vec<&str> = words.collect();
    args.push(file);
    cmd(program, &args)
}

/// What the demo's connect does: the progress steps, then the result.
struct DemoConnect {
    steps: Vec<Progress>,
    result: Result<Connected, ConnectError>,
}

fn demo_connect(app: &App, target: &Target) -> DemoConnect {
    let row = app.rows().iter().position(|r| r.machine.target == *target);
    let state = row.map(|i| app.state(i));
    let cached = row.and_then(|i| app.rows()[i].cached.as_ref());
    if matches!(state, Some(crate::status::RowState::Open)) {
        return DemoConnect {
            steps: Vec::new(),
            result: Ok(Connected::Focused),
        };
    }
    match cached {
        Some(c) if c.status != crate::check::Status::Ready => {
            let (message, hint) = sentence(&c.status, target.as_str());
            DemoConnect {
                steps: vec![Progress::Checking],
                result: Err(ConnectError {
                    message,
                    hint,
                    remedy: remedy_for(&c.status),
                    log: None,
                }),
            }
        }
        Some(_) => DemoConnect {
            steps: vec![Progress::Launching, Progress::Waiting],
            result: Ok(Connected::Launched),
        },
        None => DemoConnect {
            steps: vec![Progress::Checking, Progress::Launching, Progress::Waiting],
            result: Ok(Connected::Launched),
        },
    }
}

fn demo_fix_notice(app: &mut App, target: &Target) {
    let remedy = app
        .fix_for_selected()
        .map(|(_, r)| describe(r, target.as_str()));
    app.on_failure("Demo mode: no fix is run.", remedy);
}

/// A full-screen screen chooser for `connect --choose`. Returns the choice, or `None` when the
/// user cancelled. When nothing is cached for the machine it is checked first (a spinner shows).
pub fn choose_screen(ctx: &Ctx, target: &Target) -> anyhow::Result<Option<ScreenChoice>> {
    let machine: Machine = bare_machine(ctx.sys.as_ref(), &ctx.config, target.clone());
    let label = machine.display_label().to_string();
    let Some(cached) = load_cached(&ctx.paths, target) else {
        let (tx, rx) = channel();
        let (check_ctx, check_machine) = (ctx.clone(), machine);
        std::thread::spawn(move || {
            let r = check_one(
                check_ctx.sys.as_ref(),
                &check_ctx.paths,
                &check_ctx.config,
                &check_machine,
            );
            let _ = tx.send(r);
        });
        return choose_loop(Chooser::loading(label, target.clone()), Some(&rx));
    };
    choose_loop(Chooser::ready(label, target.clone(), cached.outputs), None)
}

fn choose_loop(
    mut chooser: Chooser,
    checked: Option<&Receiver<crate::check::CheckResult>>,
) -> anyhow::Result<Option<ScreenChoice>> {
    let (mut term, _guard) = open_terminal()?;
    let mut ticks: usize = 0;
    let mut dirty = true;
    loop {
        if let Some(rx) = checked
            && chooser.is_loading()
            && let Ok(r) = rx.try_recv()
        {
            chooser.set_outputs(r.outputs);
            dirty = true;
        }
        if dirty || chooser.is_loading() {
            term.draw(|f| {
                let area = f.area();
                chooser.draw(f.buffer_mut(), area, ticks / 2);
                view::draw_hints(
                    f.buffer_mut(),
                    area,
                    &[("↑↓", "choose"), ("enter", "select"), ("esc", "cancel")],
                );
            })?;
            dirty = false;
        }
        if !event::poll(TICK)? {
            ticks += 1;
            continue;
        }
        let Event::Key(k) = event::read()? else {
            dirty = true; // a resize
            continue;
        };
        dirty = true;
        match classify(k) {
            Input::Quit | Input::Esc => return Ok(None),
            Input::Up => chooser.move_by(-1),
            Input::Down => chooser.move_by(1),
            Input::Home => chooser.first(),
            Input::End => chooser.last(),
            Input::Enter => {
                if let Some(c) = chooser.pick() {
                    return Ok(Some(c));
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::fake::FakeSys;

    #[test]
    fn the_editor_is_visual_then_editor_then_nvim_then_vi() {
        let fake = FakeSys::new();
        let line = |f: &FakeSys| editor_command(f, "/c/config.toml").line();
        assert_eq!(line(&fake), "vi /c/config.toml");
        fake.add_which("nvim");
        assert_eq!(line(&fake), "nvim /c/config.toml");
        fake.set_env("EDITOR", "nano");
        assert_eq!(line(&fake), "nano /c/config.toml");
        fake.set_env("VISUAL", "code -w");
        assert_eq!(line(&fake), "code -w /c/config.toml");
        fake.set_env("VISUAL", "  ");
        assert_eq!(line(&fake), "nano /c/config.toml");
    }

    #[test]
    fn load_counts_the_sources_that_could_not_be_read() {
        let fake = Arc::new(FakeSys::new());
        fake.add_which("herdr");
        fake.on("herdr", &["machine", "list"])
            .code(1)
            .stderr("boom");
        let sys: Arc<dyn Sys> = fake;
        let root = std::env::temp_dir().join(format!("gliffherdr-load-{}", std::process::id()));
        let ctx = Ctx::new(sys, Paths::in_dir(&root), Config::default());
        let start = load(&ctx, None);
        assert_eq!(start.unreadable_sources, 1);
        assert_eq!(
            App::new(start, 0).sources_note().as_deref(),
            Some("1 source could not be read — gliffherdr doctor")
        );
    }

    #[test]
    fn the_demo_connect_plan_matches_the_machines_state() {
        let app = App::new(demo::startup(demo::NOW), demo::NOW);
        let plan = |t: &str| demo_connect(&app, &Target::parse(t).unwrap());
        let studio = plan("studio.lan");
        assert!(studio.steps.is_empty() && studio.result == Ok(Connected::Focused));
        let j = plan("j@10.0.0.2");
        assert_eq!(j.steps, [Progress::Launching, Progress::Waiting]);
        let render = plan("render@10.0.0.7");
        assert_eq!(
            render.steps,
            [Progress::Checking, Progress::Launching, Progress::Waiting]
        );
        let lab = plan("lab@10.0.0.9");
        let err = lab.result.unwrap_err();
        assert!(err.message.contains("gliff-server is not installed"));
        assert!(err.remedy.is_some());
    }
}
