//! The window: a folder's frames in the loupe and the filmstrip, marked
//! from the keyboard.

use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::SystemTime;

use egui::{Align2, FontId, RichText, Sense, Ui, pos2, vec2};
use omacull_engine::color::Display;
use omacull_engine::cull::{Change, Cull, Filter, Step};
use omacull_engine::disk::{self, Disk, How, Mark, Problem};
use omacull_engine::thumbs;

use crate::commands::Command;
use crate::hotkeys::{self, format_shortcut};
use crate::monitor;
use crate::shoot::Shoot;
use crate::state::{Show, State};
use crate::theme::{self, Theme};

/// Where Omacull keeps what it makes; somewhere else in tests.
pub struct Paths {
    /// Filmstrip thumbnails.
    pub cache: Option<PathBuf>,
    /// The decision log.
    pub log: Option<PathBuf>,
}

/// A folder being read.
struct Opening {
    dir: PathBuf,
    /// The raw to start at, when a raw was opened rather than a folder.
    select: Option<PathBuf>,
    result: Receiver<io::Result<(Cull, Vec<String>)>>,
}

pub struct App {
    theme: Theme,
    theme_rx: Receiver<Theme>,
    /// Problems found at startup, shown in the status bar.
    warnings: Vec<String>,
    /// Commands to run, one a frame, from `OMACULL_SCRIPT` (comma-separated
    /// command names, e.g. `Next,Pick,Quit`). For testing without a
    /// keyboard.
    script: VecDeque<Command>,
    state: State,
    paths: Paths,
    shoot: Option<Shoot>,
    opening: Option<Opening>,
    /// The folder picker, while it's open.
    picking: Option<Receiver<Option<PathBuf>>>,
    disk: Disk,
    /// The last thing to tell the user, and whether it's a problem.
    message: Option<(String, bool)>,
    title: String,
    /// The monitor the window is on; none in tests.
    monitor: Option<monitor::Watch>,
    /// Converts frames to its colours.
    display: Arc<Display>,
}

/// Converts to a monitor's colours, or shows sRGB as it is if its profile
/// won't do.
fn display_for(watch: &monitor::Watch) -> Arc<Display> {
    let display = Display::new(watch.profile().cloned()).unwrap_or_else(|e| {
        log::warn!("showing sRGB: can't use the monitor's profile: {e}");
        Display::srgb()
    });
    Arc::new(display)
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, open: Option<PathBuf>) -> Self {
        let ctx = &cc.egui_ctx;
        // Ctrl+= / Ctrl+- are for zooming the image, not the interface.
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        theme::install_font(ctx);
        let theme = Theme::load();
        ctx.set_visuals(theme.visuals());

        let warnings = crate::hotkeys::load();
        for w in &warnings {
            log::warn!("{w}");
        }
        let script = std::env::var("OMACULL_SCRIPT")
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.trim().is_empty())
            .filter_map(|step| {
                let parsed = Command::from_name(step.trim());
                if parsed.is_none() {
                    log::warn!("OMACULL_SCRIPT: can't understand {step:?}");
                }
                parsed
            })
            .collect();
        let paths = Paths { cache: thumbs::default_dir(), log: disk::default_log() };
        let mut app = Self::build(theme, theme::watch(ctx.clone()), warnings, script, State::load(), paths, ctx);
        let watch = monitor::Watch::new();
        app.display = display_for(&watch);
        app.monitor = Some(watch);
        if let Some(path) = open {
            app.open(path, ctx);
        }
        app
    }

    fn build(
        theme: Theme,
        theme_rx: Receiver<Theme>,
        warnings: Vec<String>,
        script: VecDeque<Command>,
        state: State,
        paths: Paths,
        ctx: &egui::Context,
    ) -> Self {
        let session = disk::millis(SystemTime::now());
        let wake = ctx.clone();
        let disk = Disk::new(paths.log.clone(), session, move || wake.request_repaint());
        Self {
            theme,
            theme_rx,
            warnings,
            script,
            state,
            paths,
            shoot: None,
            opening: None,
            picking: None,
            disk,
            message: None,
            title: String::new(),
            monitor: None,
            display: Arc::new(Display::srgb()),
        }
    }

    /// Open a folder of raws in the background, or the folder a raw is in,
    /// starting at that raw.
    fn open(&mut self, path: PathBuf, ctx: &egui::Context) {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        let (dir, select) = match path.parent() {
            Some(parent) if path.is_file() => (parent.to_path_buf(), Some(path)),
            _ => (path, None),
        };
        let (tx, rx) = channel();
        let (ctx, target) = (ctx.clone(), dir.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Cull::open(&target));
            ctx.request_repaint();
        });
        self.message = Some((format!("Opening {}…", dir.display()), false));
        self.opening = Some(Opening { dir, select, result: rx });
    }

    fn finish_opening(&mut self, ctx: &egui::Context) {
        let Some(opening) = &self.opening else { return };
        let result = match opening.result.try_recv() {
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(io::Error::other("reading the folder failed")),
            Ok(result) => result,
        };
        let Opening { dir, select, .. } = self.opening.take().unwrap();
        match result {
            Ok((mut cull, problems)) => {
                if let Some(at) = select.and_then(|s| cull.frames().iter().position(|f| f.path == s)) {
                    cull.go_to(at);
                }
                self.state.add_recent(cull.dir());
                self.message = problems.first().map(|first| {
                    let text = match problems.len() {
                        1 => format!("Couldn't read {first}"),
                        n => format!("Couldn't read {n} sidecars, e.g. {first}"),
                    };
                    (text, true)
                });
                self.shoot = Some(Shoot::new(cull, self.paths.cache.clone(), self.display.clone(), ctx));
            }
            Err(e) => {
                if !dir.is_dir() {
                    self.state.remove_recent(&dir);
                }
                self.message = Some((format!("Couldn't open {}: {e}", dir.display()), true));
            }
        }
    }

    /// Ask for a folder to open, with the system's picker.
    fn pick(&mut self, ctx: &egui::Context) {
        if self.picking.is_some() {
            return;
        }
        let open = self.shoot.as_ref().map(|s| s.cull.dir().to_path_buf());
        let start = open.or_else(|| self.state.recent.first().cloned());
        let mut dialog = rfd::FileDialog::new().set_title("Open Folder");
        if let Some(dir) = start {
            dialog = dialog.set_directory(dir);
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(dialog.pick_folder());
            ctx.request_repaint();
        });
        self.picking = Some(rx);
    }

    fn finish_picking(&mut self, ctx: &egui::Context) {
        let Some(picking) = &self.picking else { return };
        match picking.try_recv() {
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) | Ok(None) => self.picking = None,
            Ok(Some(dir)) => {
                self.picking = None;
                self.open(dir, ctx);
            }
        }
    }

    fn run(&mut self, command: Command, ctx: &egui::Context) {
        match command {
            Command::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Command::Open => self.pick(ctx),
            Command::AutoAdvance => {
                let on = !self.state.auto_advance;
                self.state.set_auto_advance(on);
                self.message = Some((format!("Auto-advance {}", if on { "on" } else { "off" }), false));
            }
            Command::Histogram | Command::Info | Command::Clipping | Command::Peaking | Command::FocusPoint => {
                let mut show = self.state.show;
                let (switch, name) = match command {
                    Command::Histogram => (&mut show.histogram, "Histogram"),
                    Command::Info => (&mut show.info, "Shooting settings"),
                    Command::Clipping => (&mut show.clipping, "Clipping"),
                    Command::Peaking => (&mut show.peaking, "Focus peaking"),
                    _ => (&mut show.focus_point, "Focus point"),
                };
                *switch = !*switch;
                self.message = Some((format!("{name} {}", if *switch { "on" } else { "off" }), false));
                self.state.set_show(show);
            }
            _ => {}
        }
        let Some(shoot) = &mut self.shoot else { return };
        let ppp = ctx.pixels_per_point();
        match command {
            Command::Zoom => {
                let (pointer, time) = ctx.input(|i| (i.pointer.hover_pos(), i.time));
                let (fit, size) = (shoot.fit_rect(), shoot.full_size(ppp));
                shoot.view.key_down(pointer, fit, size, time);
            }
            Command::ZoomToFocus => {
                let focus = shoot.info().and_then(|i| i.focus);
                if !shoot.view.zoom_to_focus(focus, shoot.full_size(ppp)) {
                    self.message = Some(("No focus point recorded for this frame".into(), false));
                }
            }
            _ => {}
        }
        let cull = &mut shoot.cull;
        match command {
            Command::Undo => match cull.undo() {
                Some(change) => record(&self.disk, shoot, change, How::Undo),
                None => self.message = Some(("Nothing to undo".into(), false)),
            },
            Command::Redo => match cull.redo() {
                Some(change) => record(&self.disk, shoot, change, How::Redo),
                None => self.message = Some(("Nothing to redo".into(), false)),
            },
            Command::Previous => _ = cull.step(Step::Previous),
            Command::Next => _ = cull.step(Step::Next),
            Command::First => _ = cull.step(Step::First),
            Command::Last => _ = cull.step(Step::Last),
            _ => {}
        }
        if let Some(rating) = command.rating() {
            if let Some(change) = shoot.cull.mark(rating) {
                record(&self.disk, shoot, change, How::Mark);
            }
            if self.state.auto_advance {
                shoot.cull.step(Step::Next);
            }
        }
        if let Some(filter) = command.filter() {
            shoot.cull.set_filter(filter);
        }
    }

    fn run_script(&mut self, ctx: &egui::Context) {
        // A folder named on the command line is opened first.
        if self.opening.is_some() {
            return;
        }
        let Some(command) = self.script.pop_front() else {
            return;
        };
        log::info!("script: {command:?}");
        self.run(command, ctx);
        ctx.request_repaint();
    }

    /// Marks that didn't reach their sidecar are put back as they are on
    /// disk.
    fn disk_problems(&mut self) {
        let problems: Vec<Problem> = self.disk.problems().collect();
        for problem in problems {
            let text = match problem {
                Problem::Sidecar { path, error, on_disk } => {
                    if let Some(shoot) = &mut self.shoot {
                        shoot.cull.restore(&path, on_disk);
                    }
                    format!("Couldn't mark {}: {error}", file_name(&path))
                }
                Problem::Log(error) => format!("Couldn't write the decision log: {error}"),
            };
            self.message = Some((text, true));
        }
    }

    /// A frame's work before anything is drawn.
    fn step(&mut self, ctx: &egui::Context) {
        if let Some(theme) = self.theme_rx.try_iter().last() {
            ctx.set_visuals(theme.visuals());
            self.theme = theme;
        }
        self.finish_picking(ctx);
        self.finish_opening(ctx);
        self.disk_problems();
        if !ctx.egui_wants_keyboard_input() {
            for command in Command::pressed(ctx) {
                self.run(command, ctx);
            }
        }
        self.run_script(ctx);
        // Held for a look, the zoom ends when its key is let go.
        let zoom_key = hotkeys::current().command(Command::Zoom).map(|s| s.logical_key);
        if let Some(shoot) = &mut self.shoot
            && let Some(key) = zoom_key
            && let (false, time) = ctx.input(|i| (i.key_down(key), i.time))
        {
            shoot.view.key_up(time);
        }
        if self.monitor.as_mut().is_some_and(|m| m.check(ctx)) {
            self.display = display_for(self.monitor.as_ref().unwrap());
            if let Some(shoot) = &mut self.shoot {
                shoot.set_display(self.display.clone());
            }
        }
        if let Some(shoot) = &mut self.shoot {
            shoot.update(ctx, self.state.show);
        }
        let title = match &self.shoot {
            Some(shoot) => format!("{} — Omacull", file_name(shoot.cull.dir())),
            None => "Omacull".into(),
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    fn show(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let bar = egui::Frame::new()
            .fill(self.theme.dark_background)
            .inner_margin(egui::Margin::symmetric(8, 4));
        let command = egui::Panel::bottom("status")
            .frame(bar)
            .show(ui, |ui| self.status_bar(ui))
            .inner;
        if let Some(command) = command {
            self.run(command, &ctx);
        }
        let theme = self.theme.clone();
        if let Some(shoot) = &mut self.shoot {
            let strip = egui::Frame::new().fill(theme.darker_background);
            let clicked = egui::Panel::bottom("filmstrip")
                .frame(strip)
                .resizable(false)
                .show(ui, |ui| shoot.filmstrip(ui, &theme))
                .inner;
            if let Some(index) = clicked {
                shoot.cull.go_to(index);
            }
        }
        let pasteboard = egui::Frame::new().fill(self.theme.pasteboard());
        let mut clicked = None;
        let show = self.state.show;
        egui::CentralPanel::no_frame().frame(pasteboard).show(ui, |ui| match &mut self.shoot {
            Some(shoot) => shoot.loupe(ui, &theme, show),
            None => clicked = self.empty_state(ui),
        });
        match clicked {
            Some(Some(dir)) => self.open(dir, &ctx),
            Some(None) => self.run(Command::Open, &ctx),
            None => {}
        }
    }

    /// Where the cull stands, and the filter and auto-advance switches.
    /// Returns the command for a switch clicked.
    fn status_bar(&self, ui: &mut Ui) -> Option<Command> {
        let mut command = None;
        ui.horizontal(|ui| {
            match &self.shoot {
                Some(shoot) => _ = ui.label(RichText::new(status(&shoot.cull)).color(self.theme.foreground)),
                None => _ = ui.label(RichText::new("No folder open").color(self.theme.dark_foreground)),
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(shoot) = &self.shoot {
                    let filter = shoot.cull.filter();
                    egui::ComboBox::from_id_salt("filter")
                        .selected_text(format!("Show: {}", filter.label()))
                        .show_ui(ui, |ui| {
                            for f in Filter::ALL {
                                let label = ui.selectable_label(f == filter, f.label());
                                if label.on_hover_text(shortcut(Command::show(f))).clicked() {
                                    command = Some(Command::show(f));
                                }
                            }
                        });
                    let auto = ui.selectable_label(self.state.auto_advance, "Auto-advance");
                    if auto.on_hover_text(shortcut(Command::AutoAdvance)).clicked() {
                        command = Some(Command::AutoAdvance);
                    }
                    ui.separator();
                    let Show { histogram, info, clipping, peaking, focus_point } = self.state.show;
                    for (on, label, toggle) in [
                        (histogram, "Histogram", Command::Histogram),
                        (info, "Info", Command::Info),
                        (clipping, "Clipping", Command::Clipping),
                        (peaking, "Peaking", Command::Peaking),
                        (focus_point, "AF", Command::FocusPoint),
                        (shoot.view.zoomed, "100%", Command::Zoom),
                    ] {
                        let tip = format!("{} ({})", toggle.label(), shortcut(toggle));
                        if ui.selectable_label(on, label).on_hover_text(tip).clicked() {
                            command = Some(toggle);
                        }
                    }
                }
                let warnings = (!self.warnings.is_empty()).then(|| (self.warnings.join("; "), true));
                if let Some((text, problem)) = self.message.clone().or(warnings) {
                    let colour = if problem { self.theme.red } else { self.theme.dark_foreground };
                    ui.add(egui::Label::new(RichText::new(text).color(colour)).truncate());
                }
            });
        });
        command
    }

    /// The title, Open… and the recent folders. Returns what was clicked: a
    /// recent folder, or None for Open….
    fn empty_state(&self, ui: &mut Ui) -> Option<Option<PathBuf>> {
        let mut clicked = None;
        ui.vertical_centered(|ui| {
            let space = if self.state.recent.is_empty() { 0.4 } else { 0.18 };
            ui.add_space(ui.available_height() * space);
            ui.label(RichText::new("Omacull").size(28.0).color(self.theme.accent));
            ui.add_space(8.0);
            let open = format!("Open a folder of raws with {}", shortcut(Command::Open));
            ui.label(RichText::new(open).color(self.theme.dark_foreground));
            ui.add_space(12.0);
            if ui.button("Open…").clicked() {
                clicked = Some(None);
            }
            if self.state.recent.is_empty() {
                return;
            }
            ui.add_space(20.0);
            ui.label(RichText::new("Recent folders").strong().color(self.theme.foreground));
            ui.add_space(6.0);
            egui::Frame::new()
                .fill(self.theme.dark_background)
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_max_width(450.0);
                    let (big, small) = (FontId::proportional(13.0), FontId::proportional(11.0));
                    for dir in self.state.recent.iter().take(8) {
                        let size = vec2(ui.available_width(), 26.0);
                        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
                        if response.hovered() {
                            ui.painter().rect_filled(rect, 0.0, self.theme.lighter_background);
                        }
                        let painter = ui.painter();
                        let left = pos2(rect.left() + 6.0, rect.center().y);
                        let (name, foreground) = (file_name(dir), self.theme.foreground);
                        let name = painter.text(left, Align2::LEFT_CENTER, name, big.clone(), foreground);
                        // The folder it's in, shortened from the left to fit.
                        let room = rect.right() - 6.0 - (name.right() + 24.0);
                        let parent = elided(painter, &home_relative(dir.parent().unwrap_or(dir)), &small, room);
                        let right = pos2(rect.right() - 6.0, rect.center().y);
                        painter.text(right, Align2::RIGHT_CENTER, parent, small.clone(), self.theme.dark_foreground);
                        if response.clicked() {
                            clicked = Some(Some(dir.clone()));
                        }
                    }
                });
        });
        clicked
    }
}

/// Record a change in the sidecar and the decision log.
fn record(disk: &Disk, shoot: &mut Shoot, change: Change, how: How) {
    let dwell = shoot.dwell();
    let cull = &shoot.cull;
    disk.write(Mark {
        path: cull.frames()[change.index].path.clone(),
        rating: change.now,
        was: change.was,
        how,
        view: "loupe",
        compared: Vec::new(),
        filter: cull.filter(),
        dwell,
        at: SystemTime::now(),
    });
}

fn file_name(path: &Path) -> String {
    path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().into_owned()
}

/// A path with the home folder written as ~.
fn home_relative(path: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home.as_deref().and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) => Path::new("~").join(rest).display().to_string(),
        None => path.display().to_string(),
    }
}

/// `text` without as much of its start as it takes to fit `width`.
fn elided(painter: &egui::Painter, text: &str, font: &FontId, width: f32) -> String {
    let fits = |t: &str| painter.layout_no_wrap(t.to_owned(), font.clone(), egui::Color32::WHITE).size().x <= width;
    if fits(text) {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    (1..chars.len())
        .map(|skip| format!("…{}", chars[skip..].iter().collect::<String>()))
        .find(|t| fits(t))
        .unwrap_or_default()
}

/// A command's shortcut, for a tooltip.
fn shortcut(command: Command) -> String {
    hotkeys::current().command(command).map_or_else(|| "No shortcut".into(), |s| format_shortcut(&s))
}

/// The current frame, its place in the folder, and the cull so far.
fn status(cull: &Cull) -> String {
    let counts = cull.counts();
    let filter = cull.filter();
    let shown = match filter {
        Filter::All => String::new(),
        _ => format!(" ({} shown)", cull.frames().iter().filter(|f| filter.matches(f.rating)).count()),
    };
    format!(
        "{}   {} / {}{shown}   {} picks   {} rejects   {} undecided",
        file_name(&cull.frame().path),
        cull.current() + 1,
        cull.frames().len(),
        counts.picks,
        counts.rejects,
        counts.undecided,
    )
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.step(ctx);
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }

    fn on_exit(&mut self) {
        // Don't lose the last marks.
        self.disk.finish();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::{Duration, Instant};

    use egui::{Event, Key, Modifiers, PointerButton};
    use omacull_engine::cull::PICK;
    use omacull_engine::sidecar::{self, REJECT};
    use omacull_engine::testing::{Arw, Folder};

    const NONE: Modifiers = Modifiers::NONE;
    const CMD_SHIFT: Modifiers = Modifiers { shift: true, ..Modifiers::COMMAND };
    const CMD_ALT: Modifiers = Modifiers { alt: true, ..Modifiers::COMMAND };

    /// The app driven with synthetic events, with no window or GPU.
    struct Harness {
        ctx: egui::Context,
        app: App,
        time: f64,
    }

    impl Harness {
        fn new(script: &[Command]) -> Self {
            Self::with(Paths { cache: None, log: None }, script)
        }

        fn with(paths: Paths, script: &[Command]) -> Self {
            let (_tx, rx) = channel();
            let ctx = egui::Context::default();
            let script = script.iter().copied().collect();
            let app = App::build(Theme::default(), rx, Vec::new(), script, State::default(), paths, &ctx);
            Self { ctx, app, time: 0.0 }
        }

        /// The app with `folder` open and its first preview shown.
        fn open(folder: &Folder, paths: Paths) -> Self {
            let mut h = Self::with(paths, &[]);
            h.app.open(folder.0.clone(), &h.ctx.clone());
            h.wait("the first preview", |app| app.shoot.as_ref().is_some_and(|s| s.preview_ready()));
            h
        }

        fn frame(&mut self, events: Vec<Event>) -> egui::FullOutput {
            self.time += 0.05;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let mut output = self.ctx.run_ui(input, |ui| {
                self.app.step(&ui.ctx().clone());
                self.app.show(ui);
            });
            // There's no renderer to upload textures to.
            output.textures_delta.clear();
            output
        }

        /// A key pressed and let go in one frame.
        fn press(&mut self, modifiers: Modifiers, key: Key) -> egui::FullOutput {
            let mut events = key_events(modifiers, key, true);
            events.extend(key_events(modifiers, key, false));
            self.frame(events)
        }

        fn key_down(&mut self, key: Key) {
            self.frame(key_events(NONE, key, true));
        }

        fn key_up(&mut self, key: Key) {
            self.frame(key_events(NONE, key, false));
        }

        /// Drag with the mouse from `from` to `to`, a frame at each end and
        /// one between.
        fn drag(&mut self, from: egui::Pos2, to: egui::Pos2) {
            let button =
                |at, pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: NONE };
            self.frame(vec![Event::PointerMoved(from), button(from, true)]);
            self.frame(vec![Event::PointerMoved(from + (to - from) / 2.0)]);
            self.frame(vec![Event::PointerMoved(to)]);
            self.frame(vec![button(to, false)]);
        }

        fn shoot(&self) -> &Shoot {
            self.app.shoot.as_ref().unwrap()
        }

        fn click(&mut self, at: egui::Pos2) {
            let button =
                |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: NONE };
            self.frame(vec![Event::PointerMoved(at), button(true)]);
            self.frame(vec![button(false)]);
        }

        /// Run frames until `done`, as the background threads finish.
        fn wait(&mut self, what: &str, done: impl Fn(&App) -> bool) {
            let started = Instant::now();
            while !done(&self.app) {
                assert!(started.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
                self.frame(vec![]);
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        fn cull(&self) -> &Cull {
            &self.app.shoot.as_ref().unwrap().cull
        }

        fn ratings(&self) -> Vec<i32> {
            self.cull().frames().iter().map(|f| f.rating).collect()
        }
    }

    fn key_events(modifiers: Modifiers, key: Key, pressed: bool) -> Vec<Event> {
        vec![
            Event::ModifiersChanged(modifiers),
            Event::Key { key, physical_key: None, pressed, repeat: false, modifiers },
        ]
    }

    fn closes(output: &egui::FullOutput) -> bool {
        output
            .viewport_output
            .values()
            .any(|v| v.commands.iter().any(|c| matches!(c, egui::ViewportCommand::Close)))
    }

    #[test]
    fn ctrl_q_closes_the_window_and_marks_dont() {
        let mut h = Harness::new(&[]);
        assert!(!closes(&h.frame(vec![])));
        assert!(!closes(&h.press(Modifiers::NONE, Key::X)));
        assert!(!closes(&h.press(Modifiers::NONE, Key::Q)));
        assert!(closes(&h.press(Modifiers::COMMAND, Key::Q)));
    }

    #[test]
    fn a_script_runs_one_command_a_frame() {
        let mut h = Harness::new(&[Command::Next, Command::Pick, Command::Quit]);
        assert!(!closes(&h.frame(vec![])));
        assert!(!closes(&h.frame(vec![])));
        assert!(closes(&h.frame(vec![])));
        assert!(h.app.script.is_empty());
    }

    #[test]
    fn marks_step_undo_and_redo_and_reach_the_sidecars_and_the_log() {
        let folder = Folder::with_raws("app-marks", 5, &Arw::default());
        sidecar::write(&folder.raw(3), 4).unwrap();
        let log = folder.0.join("decisions.jsonl");
        let mut h = Harness::open(&folder, Paths { cache: None, log: Some(log.clone()) });
        assert_eq!(h.ratings(), [0, 0, 4, 0, 0], "existing ratings are read on open");
        assert_eq!(h.app.title, format!("omacull-{}-app-marks — Omacull", std::process::id()));

        h.press(NONE, Key::ArrowRight);
        assert_eq!(h.cull().current(), 1);
        h.press(NONE, Key::P);
        h.press(NONE, Key::End);
        assert_eq!(h.cull().current(), 4);
        h.press(NONE, Key::X);
        assert_eq!(h.ratings(), [0, PICK, 4, 0, REJECT]);
        assert_eq!(h.cull().current(), 4, "no auto-advance by default");

        h.press(Modifiers::COMMAND, Key::Z);
        h.press(Modifiers::COMMAND, Key::Z);
        assert_eq!(h.ratings(), [0, 0, 4, 0, 0]);
        assert_eq!(h.cull().current(), 1, "undo shows the frame it changed");
        h.press(CMD_SHIFT, Key::Z);
        assert_eq!(h.ratings(), [0, PICK, 4, 0, 0]);
        h.press(NONE, Key::Home);
        assert_eq!(h.cull().current(), 0);
        h.press(NONE, Key::Num0);
        assert_eq!(status(h.cull()), "DSC00001.ARW   1 / 5   2 picks   0 rejects   3 undecided");

        h.app.disk.finish();
        assert_eq!(sidecar::read(&folder.raw(1)).unwrap(), None, "no sidecar made for no mark");
        assert_eq!(sidecar::read(&folder.raw(2)).unwrap(), Some(PICK));
        assert_eq!(sidecar::read(&folder.raw(5)).unwrap(), Some(0));
        let rows: Vec<serde_json::Value> =
            std::fs::read_to_string(&log).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        let name = |r: &serde_json::Value| r["file"]["path"].as_str().unwrap().rsplit('/').next().unwrap().to_owned();
        let summary: Vec<_> =
            rows.iter().map(|r| (r["how"].as_str().unwrap(), name(r), r["rating"].as_i64().unwrap())).collect();
        assert_eq!(
            summary,
            [
                ("mark", "DSC00002.ARW".into(), 1),
                ("mark", "DSC00005.ARW".into(), -1),
                ("undo", "DSC00005.ARW".into(), 0),
                ("undo", "DSC00002.ARW".into(), 0),
                ("redo", "DSC00002.ARW".into(), 1),
            ]
        );
        assert!(rows.iter().all(|r| r["view"] == "loupe" && r["session"] == rows[0]["session"]));
    }

    #[test]
    fn auto_advance_and_filters() {
        let folder = Folder::with_raws("app-filters", 6, &Arw::default());
        let mut h = Harness::open(&folder, Paths { cache: None, log: None });
        h.press(NONE, Key::A);
        assert!(h.app.state.auto_advance);
        h.press(NONE, Key::P);
        h.press(NONE, Key::X);
        h.press(NONE, Key::Num3);
        assert_eq!(h.ratings(), [PICK, REJECT, 3, 0, 0, 0]);
        assert_eq!(h.cull().current(), 3);

        h.press(CMD_ALT, Key::Num0);
        assert_eq!(h.cull().filter(), Filter::Undecided);
        assert_eq!(h.cull().shown_indices(), [3, 4, 5]);
        h.press(NONE, Key::Num5);
        assert_eq!(h.cull().current(), 4);
        assert_eq!(h.cull().shown_indices(), [4, 5]);

        h.press(CMD_ALT, Key::X);
        assert_eq!(h.cull().current(), 1, "moved onto the only reject");
        h.press(CMD_ALT, Key::Num1);
        assert_eq!(h.cull().current(), 2);
        h.press(NONE, Key::ArrowRight);
        assert_eq!(h.cull().current(), 3);
        h.press(NONE, Key::ArrowRight);
        assert_eq!(h.cull().current(), 3, "nothing after it is a pick");
        assert_eq!(status(h.cull()), "DSC00004.ARW   4 / 6 (3 shown)   3 picks   1 rejects   2 undecided");
        h.press(CMD_ALT, Key::A);
        assert_eq!(h.cull().shown_indices().len(), 6);

        h.press(NONE, Key::A);
        assert!(!h.app.state.auto_advance);
    }

    #[test]
    fn neighbours_are_decoded_ahead_and_thumbnails_cached() {
        let folder = Folder::with_raws("app-prefetch", 12, &Arw::default());
        let cache = folder.0.join("cache");
        let mut h = Harness::open(&folder, Paths { cache: Some(cache.clone()), log: None });
        let previews = |app: &App, range: std::ops::RangeInclusive<usize>| {
            range.into_iter().all(|i| app.shoot.as_ref().unwrap().has_preview(i))
        };
        h.wait("the next four previews", |app| previews(app, 0..=4));
        h.press(NONE, Key::ArrowRight);
        h.press(NONE, Key::ArrowRight);
        h.wait("previews ahead", |app| previews(app, 0..=6));
        h.wait("the loader", |app| app.shoot.as_ref().unwrap().idle());
        h.frame(vec![]);
        let shoot = h.app.shoot.as_ref().unwrap();
        assert!(!shoot.has_preview(7), "only four ahead");
        // Going back, the ring turns round.
        h.press(NONE, Key::ArrowLeft);
        h.wait("previews behind", |app| previews(app, 0..=1));
        let shoot = h.app.shoot.as_ref().unwrap();
        assert!(!shoot.has_preview(5) && !shoot.has_preview(6), "two behind");

        assert!((0..=5).all(|i| shoot.has_thumbnail(i)), "the filmstrip's thumbnails");
        h.wait("the whole folder in the cache", |_| std::fs::read_dir(&cache).unwrap().count() == 12);
    }

    #[test]
    fn clicking_a_thumbnail_goes_to_it() {
        let folder = Folder::with_raws("app-click", 4, &Arw::default());
        let mut h = Harness::open(&folder, Paths { cache: None, log: None });
        let cells = h.app.shoot.as_ref().unwrap().cells.clone();
        let (index, rect) = cells.iter().copied().find(|&(i, _)| i == 2).unwrap();
        assert!(rect.bottom() <= 600.0 && rect.top() > 300.0, "the strip is at the bottom: {rect:?}");
        h.click(rect.center());
        assert_eq!(h.cull().current(), index);
    }

    #[test]
    fn opening_a_raw_starts_at_it() {
        let folder = Folder::with_raws("app-open-raw", 4, &Arw::default());
        let mut h = Harness::with(Paths { cache: None, log: None }, &[]);
        h.app.open(folder.raw(3), &h.ctx.clone());
        h.wait("the folder", |app| app.shoot.is_some());
        assert_eq!(h.cull().current(), 2);
        assert_eq!(h.cull().frames().len(), 4);

        let empty = Folder::new("app-open-empty");
        h.app.open(empty.0.clone(), &h.ctx.clone());
        h.wait("the empty folder", |app| app.opening.is_none());
        assert!(h.app.message.as_ref().is_some_and(|(m, problem)| *problem && m.contains("no raws")));
        assert_eq!(h.cull().frames().len(), 4, "the open folder stays");
    }

    #[test]
    fn a_mark_that_cant_be_written_is_taken_back() {
        let folder = Folder::with_raws("app-unwritable", 2, &Arw::default());
        std::fs::write(sidecar::path_for(&folder.raw(1)), "not a sidecar").unwrap();
        let mut h = Harness::open(&folder, Paths { cache: None, log: None });
        assert!(h.app.message.as_ref().is_some_and(|(m, problem)| *problem && m.contains("DSC00001.ARW.xmp")));
        h.press(NONE, Key::Num4);
        assert_eq!(h.ratings(), [4, 0]);
        h.wait("the write to fail", |app| app.message.as_ref().is_some_and(|(m, _)| m.starts_with("Couldn't mark")));
        assert_eq!(h.ratings(), [0, 0]);
        assert_eq!(std::fs::read_to_string(sidecar::path_for(&folder.raw(1))).unwrap(), "not a sidecar");
    }

    #[test]
    fn the_zoom_key_toggles_when_tapped_and_looks_when_held() {
        let folder = Folder::with_raws("app-zoom-key", 2, &Arw::default());
        let mut h = Harness::open(&folder, Paths { cache: None, log: None });
        h.press(NONE, Key::Z);
        assert!(h.shoot().view.zoomed, "a tap zooms in");
        h.press(NONE, Key::ArrowRight);
        assert!(h.shoot().view.zoomed, "and stays zoomed from frame to frame");
        h.press(NONE, Key::Z);
        assert!(!h.shoot().view.zoomed, "the next tap zooms out");

        h.key_down(Key::Z);
        assert!(h.shoot().view.zoomed);
        for _ in 0..8 {
            h.frame(vec![]);
        }
        assert!(h.shoot().view.zoomed, "held");
        h.key_up(Key::Z);
        assert!(!h.shoot().view.zoomed, "let go after a look");
    }

    #[test]
    fn the_mouse_zooms_at_the_pointer_and_pans() {
        let folder = Folder::with_raws("app-zoom-mouse", 1, &Arw::default());
        let mut h = Harness::open(&folder, Paths { cache: None, log: None });
        let fit = h.shoot().fit_rect();
        h.click(fit.center());
        assert!(h.shoot().view.zoomed, "a click zooms in");
        let before = h.shoot().view.center;
        h.drag(pos2(400.0, 200.0), pos2(300.0, 150.0));
        assert!(h.shoot().view.zoomed, "dragging pans, and stays zoomed");
        let after = h.shoot().view.center;
        assert!(after[0] > before[0] && after[1] > before[1], "dragged left and up shows more to the right and below");
        h.click(fit.center());
        assert!(!h.shoot().view.zoomed, "a click zooms out");
        // Pressing from whole and dragging is a look.
        h.drag(fit.center(), fit.center() + vec2(-60.0, 0.0));
        assert!(!h.shoot().view.zoomed);
    }

    #[test]
    fn full_size_frames_are_developed_ahead_and_shown_in_tiles() {
        let folder = Folder::with_raws("app-full", 3, &Arw::default());
        let mut h = Harness::open(&folder, Paths { cache: None, log: None });
        // The fake raws have no raw data: developing them fails, and the
        // enlarged preview stands in.
        h.wait("a development", |app| app.shoot.as_ref().unwrap().full_state(0).is_some());
        h.wait("the next one", |app| app.shoot.as_ref().unwrap().full_state(1).is_some());
        assert!(h.shoot().full_state(0).unwrap().is_err());
        h.press(NONE, Key::Z);
        assert!(h.shoot().view.zoomed);

        // A real development, as the loader would hand it over.
        let (w, h_) = (1500, 1000);
        let image = omacull_engine::image::Image { width: w, height: h_, rgba: [90, 90, 90, 255].repeat(w * h_) };
        let decoded = omacull_engine::loader::Decoded {
            histogram: omacull_engine::image::Histogram::of(&image),
            marks: vec![0; w * h_],
            image,
            info: Default::default(),
        };
        h.app.shoot.as_mut().unwrap().insert_full(0, decoded);
        h.frame(vec![]);
        let tiles = h.shoot().full_state(0).unwrap().unwrap();
        // The loupe is 800 points wide: two or three tiles across, and as
        // many as fit down, plus a tile's margin.
        assert!((2..=9).contains(&tiles), "{tiles} tiles");
        h.press(NONE, Key::Z);
        h.press(NONE, Key::ArrowRight);
        h.frame(vec![]);
        let left = h.shoot().full_state(0);
        assert!(left.is_none_or(|s| s.is_ok_and(|t| t == 0)), "only the frame on screen keeps tiles");
    }

    #[test]
    fn zooming_to_the_focus_point_follows_it() {
        let folder = Folder::new("app-focus");
        Arw { focus: [6000, 4000, 1000, 1000], ..Arw::default() }.write(&folder.raw(1));
        Arw { focus: [6000, 4000, 5000, 3000], ..Arw::default() }.write(&folder.raw(2));
        Arw { focus_mode: 0, ..Arw::default() }.write(&folder.raw(3));
        let mut h = Harness::open(&folder, Paths { cache: None, log: None });
        h.press(Modifiers::SHIFT, Key::Z);
        let view = &h.shoot().view;
        assert!(view.zoomed && view.follow_focus);
        let first = view.center;
        h.press(NONE, Key::ArrowRight);
        h.wait("the next preview", |app| app.shoot.as_ref().unwrap().preview_ready());
        h.frame(vec![]);
        let second = h.shoot().view.center;
        assert!(second[0] > first[0] && second[1] > first[1], "{first:?} then {second:?}");
        // Manual focus: nothing to go to.
        h.press(NONE, Key::ArrowRight);
        h.wait("the last preview", |app| app.shoot.as_ref().unwrap().preview_ready());
        h.press(NONE, Key::Z);
        h.press(Modifiers::SHIFT, Key::Z);
        assert!(!h.shoot().view.zoomed);
        assert!(h.app.message.as_ref().is_some_and(|(m, _)| m.contains("No focus point")));
    }

    #[test]
    fn overlays_are_switched_remembered_and_baked_in() {
        let folder = Folder::with_raws("app-overlays", 2, &Arw::default());
        let mut h = Harness::open(&folder, Paths { cache: None, log: None });
        assert_eq!(h.app.state.show, Show::default());
        assert!(h.shoot().info().is_some_and(|i| i.exif.iso == Some(400)));
        for key in [Key::H, Key::J, Key::S, Key::F, Key::I] {
            h.press(NONE, key);
        }
        let show = h.app.state.show;
        assert!(show.histogram && show.clipping && show.peaking && show.focus_point && !show.info);
        assert_eq!(h.shoot().preview_bake(), Some(crate::loupe::Bake { clipping: true, peaking: true }));
        h.press(NONE, Key::J);
        assert_eq!(h.shoot().preview_bake(), Some(crate::loupe::Bake { clipping: false, peaking: true }));
        // Stepping on, the next frame has them too.
        h.press(NONE, Key::ArrowRight);
        h.wait("the next preview", |app| app.shoot.as_ref().unwrap().preview_ready());
        h.frame(vec![]);
        assert_eq!(h.shoot().preview_bake(), Some(crate::loupe::Bake { clipping: false, peaking: true }));
    }
}
