//! The window: a folder's frames in the loupe and the filmstrip, marked
//! from the keyboard.

use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, SystemTime};

use egui::{Align2, FontId, RichText, Sense, Ui, pos2, vec2};
use omacull_engine::color::Display;
use omacull_engine::cull::{self, Change, Cull, Filter, Formats, Kind, Rating, Step};
use omacull_engine::stacks::Stacking;
use omacull_engine::disk::{self, Disk, How, Mark, Problem};
use omacull_engine::learn::{self, Model};
use omacull_engine::sidecar::{self, REJECT};
use omacull_engine::{signals, thumbs};

use crate::commands::Command;
use crate::config::Config;
use crate::faces::Finder;
use crate::hotkeys::{self, format_shortcut};
use crate::monitor;
use crate::panes::Mode;
use crate::shoot::{Assist, Shoot};
use crate::state::{Place, Show, State};
use crate::tree::{self, Tree};
use crate::theme::{self, Theme};

/// Where Omacull keeps what it makes; somewhere else in tests.
pub struct Paths {
    /// Filmstrip thumbnails.
    pub cache: Option<PathBuf>,
    /// The decision log.
    pub log: Option<PathBuf>,
    /// The program that opens a folder in darktable.
    pub darktable: String,
    /// The faces found in each raw.
    pub faces: Option<PathBuf>,
    /// What was measured of each raw.
    pub signals: Option<PathBuf>,
    /// The model trained on the decision log.
    pub model: Option<PathBuf>,
}

/// A folder being read.
struct Opening {
    dir: PathBuf,
    /// The picture to start at, when one was opened rather than a folder.
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
    /// The folder tree, once it's been shown.
    tree: Option<Tree>,
    /// The cull summary is open.
    summary: bool,
    /// Finds faces; a stand-in in tests.
    finder: Finder,
    config: Config,
    /// What's been learned from the decision log, once there's enough of
    /// it.
    model: Option<Arc<Model>>,
    /// The model being trained, and whether to say how it went.
    learning: Option<(Receiver<Result<Model, String>>, bool)>,
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

        let mut warnings = crate::hotkeys::load();
        let (config, problems) = Config::load();
        warnings.extend(problems);
        sidecar::set_naming(config.sidecar);
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
        let log = disk::default_log();
        let paths = Paths {
            cache: thumbs::default_dir(),
            model: log.as_ref().map(|log| log.with_file_name("model.json")),
            log,
            darktable: config.developer.clone(),
            faces: omacull_engine::faces::default_dir(),
            signals: signals::default_dir(),
        };
        let mut app = Self::build(theme, theme::watch(ctx.clone()), warnings, script, State::load(), paths, ctx);
        app.finder = crate::faces::yunet();
        app.config = config;
        app.model = app.paths.model.as_deref().and_then(Model::load).map(Arc::new);
        // Decisions made since it was trained are learned from too.
        let logged = app.paths.log.as_deref().and_then(|log| std::fs::metadata(log).ok()).map_or(0, |m| m.len());
        if logged > 0 && app.model.as_ref().is_none_or(|model| model.log_bytes != logged) {
            app.learn(ctx, false);
        }
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
            tree: None,
            summary: false,
            finder: std::sync::Arc::new(|| Err("Faces aren't looked for".into())),
            config: Config::default(),
            model: None,
            learning: None,
        }
    }

    fn assist(&self) -> Assist {
        Assist { model: self.model.clone(), confidence: self.config.confidence, pick: self.config.pick }
    }

    /// Train the model on the decision log, in the background. `announce`
    /// to say how it went: asked for, not just keeping up.
    fn learn(&mut self, ctx: &egui::Context, announce: bool) {
        if self.learning.is_some() {
            return;
        }
        let Some(log) = self.paths.log.clone() else {
            self.message = announce.then(|| ("There's no decision log to learn from".into(), false));
            return;
        };
        // What's just been marked counts.
        self.disk.flush();
        let (tx, rx) = channel();
        let (ctx, confidence) = (ctx.clone(), self.config.confidence);
        std::thread::spawn(move || {
            let _ = tx.send(learn::learn(&log, confidence, disk::millis(SystemTime::now())));
            ctx.request_repaint();
        });
        self.learning = Some((rx, announce));
        if announce {
            self.message = Some(("Learning from your decisions…".into(), false));
        }
    }

    fn finish_learning(&mut self) {
        let Some((learning, announce)) = &self.learning else { return };
        let result = match learning.try_recv() {
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("Learning failed".to_owned()),
            Ok(result) => result,
        };
        let announce = *announce;
        self.learning = None;
        match result {
            Ok(model) => {
                if let Some(path) = &self.paths.model
                    && let Err(e) = model.save(path)
                {
                    log::warn!("couldn't save {}: {e}", path.display());
                }
                self.model = Some(Arc::new(model));
                let assist = self.assist();
                if let Some(shoot) = &mut self.shoot {
                    shoot.assist = assist;
                }
                if announce {
                    self.message = Some((self.learned().join(". "), false));
                }
            }
            Err(why) if announce => self.message = Some((why, false)),
            Err(why) => log::info!("{why}"),
        }
    }

    /// What's been learned from the decision log, and how it did on the
    /// frames held back to test it.
    fn learned(&self) -> Vec<String> {
        let Some(model) = &self.model else {
            return vec![format!("Nothing learned yet: it takes {} decisions", learn::NEEDED)];
        };
        let [rejected, left, kept] = model.learned;
        let mut lines = vec![format!("Learned from {rejected} rejected, {kept} kept and {left} left unmarked")];
        if model.held > 0 {
            let (held, sure, right) = (model.held, model.sure, model.right);
            lines.push(format!("Of {held} held back, it was sure of {sure} and right about {right}"));
        }
        lines
    }

    /// The frames looked at and left unmarked go in the decision log when
    /// their folder is left: passing a frame over is a choice too.
    fn log_passes(&mut self) {
        let Some(shoot) = &self.shoot else { return };
        for frame in shoot.passed() {
            self.disk.write(mark(shoot, frame, (0, 0), How::Pass, &[], Duration::ZERO));
        }
    }

    /// Remember where the open folder was left, to come back to it.
    fn remember_place(&mut self) {
        if let Some(shoot) = &self.shoot {
            let cull = &shoot.cull;
            let frame = file_name(&cull.frame().path);
            let name = |i: &usize| file_name(&cull.frames()[*i].path);
            let stacks = cull.manual_stacks().iter().map(|s| s.iter().map(name).collect()).collect();
            self.state.remember(Place { dir: cull.dir().to_path_buf(), frame, filter: cull.filter(), stacks });
        }
    }

    /// Hand the open folder to darktable, once every mark is in its sidecar.
    fn darktable(&mut self) {
        let Some(shoot) = &self.shoot else { return };
        self.disk.flush();
        let dir = shoot.cull.dir().to_path_buf();
        let started = std::process::Command::new(&self.paths.darktable)
            .arg(&dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        self.message = Some(match started {
            Ok(mut child) => {
                // Reaped when it's closed.
                std::thread::spawn(move || child.wait());
                (format!("Opened {} in {}", file_name(&dir), self.developer()), false)
            }
            Err(e) => (format!("Couldn't start {}: {e}", self.paths.darktable), true),
        });
    }

    /// The raw developer's name: darktable, unless the config says
    /// otherwise.
    fn developer(&self) -> String {
        file_name(Path::new(&self.paths.darktable))
    }

    /// Open a folder of pictures in the background, or the folder a
    /// picture is in, starting at that picture.
    fn open(&mut self, path: PathBuf, ctx: &egui::Context) {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        let (dir, select) = match path.parent() {
            Some(parent) if path.is_file() => (parent.to_path_buf(), Some(path)),
            _ => (path, None),
        };
        // A picture that was opened is shown, whichever are culled as a
        // rule.
        let formats = match select.as_deref().and_then(cull::kind) {
            Some(kind) if !self.state.formats.takes(kind) => Formats::All,
            _ => self.state.formats,
        };
        let (tx, rx) = channel();
        let (ctx, target) = (ctx.clone(), dir.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Cull::open(&target, formats));
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
                self.log_passes();
                self.remember_place();
                // Where it was left, unless a raw in it was opened.
                let place = self.state.place(cull.dir()).cloned();
                // By name, or failing that the frame named like it: the
                // raw a JPEG came with, when only raws are culled now.
                let at = |name: &str| {
                    let stem = Path::new(name).file_stem();
                    let frames = cull.frames();
                    let named = frames.iter().position(|f| file_name(&f.path) == name);
                    named.or_else(|| frames.iter().position(|f| f.path.file_stem() == stem))
                };
                let start = match select {
                    Some(raw) => cull.frames().iter().position(|f| f.path == raw),
                    None => place.as_ref().and_then(|p| at(&p.frame)),
                };
                let stacks: &[Vec<String>] = place.as_ref().map_or(&[], |p| &p.stacks);
                let manual = stacks.iter().map(|s| s.iter().filter_map(|n| at(n)).collect()).collect();
                cull.set_manual_stacks(manual);
                cull.set_stacking(self.state.stacking);
                if let Some(at) = start {
                    cull.go_to(at);
                }
                if let Some(place) = place {
                    cull.set_filter(place.filter);
                }
                if let Some(tree) = &mut self.tree {
                    tree.show(cull.dir());
                }
                self.state.add_recent(cull.dir());
                self.message = problems.first().map(|first| {
                    let text = match problems.len() {
                        1 => format!("Couldn't read {first}"),
                        n => format!("Couldn't read {n} sidecars, e.g. {first}"),
                    };
                    (text, true)
                });
                let faces = (self.paths.faces.clone(), self.paths.signals.clone(), self.finder.clone());
                let mut shoot = Shoot::new(cull, self.paths.cache.clone(), faces, self.display.clone(), ctx);
                shoot.assist = self.assist();
                self.shoot = Some(shoot);
            }
            Err(e) => {
                if !dir.is_dir() {
                    self.state.remove_recent(&dir);
                }
                self.message = Some((format!("Couldn't open {}: {e}", dir.display()), true));
            }
        }
    }

    /// Cull another of the folder's formats. Where it holds more than
    /// one, it's read again, and opens on the frame it was on or the one
    /// named like it; marks made so far can no longer be undone.
    fn set_formats(&mut self, formats: Formats, ctx: &egui::Context) {
        self.state.set_formats(formats);
        let mut only = String::new();
        if let Some(shoot) = &self.shoot {
            if !shoot.cull.choices().is_empty() {
                let dir = shoot.cull.dir().to_path_buf();
                // The marks on their way to the sidecars are read back from
                // them.
                self.disk.flush();
                return self.open(dir, ctx);
            }
            let held = Kind::ALL.into_iter().find(|&kind| shoot.cull.holds()[kind as usize] > 0);
            only = held.map(|kind| format!(" (this folder has only {})", kind.plural())).unwrap_or_default();
        }
        self.message = Some((format!("Format: {}{only}", formats.label()), false));
    }

    /// Close the open folder, back to the recent ones: its place is
    /// remembered, as when another is opened.
    fn close(&mut self) {
        self.log_passes();
        self.remember_place();
        self.shoot = None;
        // Nor is one on its way opened.
        self.opening = None;
        self.summary = false;
        self.message = None;
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
            Command::Close => self.close(),
            Command::AutoAdvance => {
                let on = !self.state.auto_advance;
                self.state.set_auto_advance(on);
                self.message = Some((format!("Auto-advance {}", if on { "on" } else { "off" }), false));
            }
            Command::Folders => {
                let on = !self.state.folders;
                self.state.set_folders(on);
                // Shown again, it's read again: raws come and go.
                if let Some(tree) = &mut self.tree {
                    tree.refresh();
                }
            }
            Command::Formats => {
                // Round what the folder holds, where there's a choice.
                let choices = self.shoot.as_ref().map(|shoot| (shoot.cull.formats(), shoot.cull.choices()));
                let next = match choices {
                    Some((formats, choices)) if !choices.is_empty() => formats.next(&choices),
                    _ => self.state.formats.next(&Formats::ALL),
                };
                return self.set_formats(next, ctx);
            }
            Command::Summary => self.summary = !self.summary,
            Command::Back if self.summary => {
                self.summary = false;
                return;
            }
            Command::Darktable => self.darktable(),
            Command::Learn => self.learn(ctx, true),
            Command::Histogram
            | Command::Info
            | Command::Clipping
            | Command::Peaking
            | Command::FocusPoint
            | Command::FaceStrip
            | Command::Signals
            | Command::Suggestions => {
                let mut show = self.state.show;
                let (switch, name) = match command {
                    Command::Histogram => (&mut show.histogram, "Histogram"),
                    Command::Info => (&mut show.info, "Shooting settings"),
                    Command::Clipping => (&mut show.clipping, "Clipping"),
                    Command::Peaking => (&mut show.peaking, "Focus peaking"),
                    Command::FaceStrip => (&mut show.faces, "Face close-ups"),
                    Command::Signals => (&mut show.signals, "Signals"),
                    Command::Suggestions => (&mut show.suggestions, "Suggestions"),
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
        let say = |text: &str| Some((text.to_owned(), false));
        // A mark to make on the current frame: a marking key's, or the
        // one suggested.
        let mut rating = if command == Command::Pick { Some(self.config.pick) } else { command.rating() };
        match command {
            Command::Zoom => {
                let (pointer, time) = ctx.input(|i| (i.pointer.hover_pos(), i.time));
                let (fit, size) = (shoot.fit_rect(), shoot.full_size(ppp));
                shoot.view_mut().key_down(pointer, fit, size, time);
                shoot.sync(shoot.active());
            }
            Command::Eyes => {
                let pointer = ctx.input(|i| i.pointer.hover_pos());
                if let Err(why) = shoot.zoom_to_eyes(pointer, ppp) {
                    self.message = Some((why, false));
                }
            }
            Command::ZoomToFocus => {
                let (focus, size) = (shoot.info().and_then(|i| i.focus), shoot.full_size(ppp));
                if shoot.view_mut().zoom_to_focus(focus, size) {
                    shoot.sync(shoot.active());
                } else {
                    self.message = say("No focus point recorded for this frame");
                }
            }
            Command::Compare => {
                if let Err(why) = shoot.compare() {
                    self.message = say(why);
                }
            }
            Command::Survey => {
                if let Err(why) = shoot.survey() {
                    self.message = say(why);
                }
            }
            Command::Back if shoot.mode != Mode::Loupe => shoot.back_to_loupe(),
            Command::Back | Command::SelectNone => shoot.cull.clear_selection(),
            Command::Lock => {
                shoot.locked = !shoot.locked;
                shoot.sync(shoot.active());
                self.message = say(if shoot.locked { "Zoom locked together" } else { "Zoom unlocked" });
            }
            Command::NextPane => shoot.next_pane(),
            Command::KnockOut => shoot.knock_out(),
            Command::SelectPrevious if shoot.mode == Mode::Loupe => _ = shoot.cull.extend(Step::Previous),
            Command::SelectNext if shoot.mode == Mode::Loupe => _ = shoot.cull.extend(Step::Next),
            Command::SelectAll => shoot.cull.select_all(),
            Command::FirstUndecided => match shoot.cull.first_undecided() {
                Some(first) => shoot.cull.go_to(first),
                None => self.message = say("Every frame is decided"),
            },
            Command::Undo | Command::Redo => {
                let (changes, how) = match command {
                    Command::Undo => (shoot.cull.undo(), How::Undo),
                    _ => (shoot.cull.redo(), How::Redo),
                };
                if changes.is_empty() {
                    self.message = say(if how == How::Undo { "Nothing to undo" } else { "Nothing to redo" });
                }
                let compared: Vec<usize> = changes.iter().map(|c| c.index).collect();
                for change in changes {
                    record(&self.disk, shoot, change, how, &compared);
                }
            }
            Command::StackSelection => {
                if shoot.cull.stack_selection() {
                    self.state.set_stacking(Stacking::Manual);
                    self.message = say("Stacked by hand");
                } else {
                    self.message = say("Select two or more frames to stack");
                }
            }
            Command::Unstack => {
                if !shoot.cull.unstack() {
                    self.message = say("Only stacks made by hand come apart");
                }
            }
            Command::ToggleStack => {
                if !shoot.cull.toggle_stack() {
                    self.message = say("Not in a stack");
                }
            }
            Command::Stacking => {
                let stacking = shoot.cull.stacking().next();
                shoot.cull.set_stacking(stacking);
                self.state.set_stacking(stacking);
                let count = shoot.cull.stack_count();
                self.message = Some((format!("Stacks: {} ({count})", stacking.label()), false));
            }
            Command::Accept => match shoot.accept() {
                Ok(marks) if marks.len() == 1 => rating = Some(marks[0].1),
                // A stack's: its best frame wins.
                Ok(marks) => {
                    shoot.cull.go_to(marks[0].0);
                    win(&self.disk, shoot, &marks, self.state.auto_advance);
                }
                Err(why) => self.message = say(why),
            },
            Command::Winner => match shoot.winner() {
                Ok(marks) => win(&self.disk, shoot, &marks, self.state.auto_advance),
                Err(why) => self.message = say(why),
            },
            // Away from the loupe, the arrows work on the panes.
            Command::Previous | Command::Next if shoot.mode != Mode::Loupe => {
                shoot.step_panes(if command == Command::Next { 1 } else { -1 });
            }
            Command::Previous => _ = shoot.cull.step(Step::Previous),
            Command::Next => _ = shoot.cull.step(Step::Next),
            Command::First if shoot.mode == Mode::Loupe => _ = shoot.cull.step(Step::First),
            Command::Last if shoot.mode == Mode::Loupe => _ = shoot.cull.step(Step::Last),
            _ => {}
        }
        if let Some(rating) = rating {
            if let Some(change) = shoot.cull.mark(rating) {
                let compared = shoot.others();
                record(&self.disk, shoot, change, How::Mark, &compared);
            }
            match shoot.mode {
                // Marked down in compare, the next candidate comes in.
                Mode::Compare if rating == REJECT => shoot.knock_out(),
                Mode::Compare => {}
                Mode::Survey if self.state.auto_advance => shoot.step_panes(1),
                Mode::Survey => {}
                Mode::Loupe if self.state.auto_advance => _ = shoot.cull.step(Step::Next),
                Mode::Loupe => {}
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
        self.finish_learning();
        self.disk_problems();
        // Nothing in Omacull takes typing, so no widget keeps the keyboard:
        // egui gives it to the next button on Tab, which is a key here.
        ctx.memory_mut(|m| {
            if let Some(focused) = m.focused() {
                m.surrender_focus(focused);
            }
        });
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
            let before = shoot.view().zoomed;
            for pane in &mut shoot.panes {
                pane.view.key_up(time);
            }
            if shoot.view().zoomed != before {
                shoot.sync(shoot.active());
            }
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
        let (command, stack_to, formats) = egui::Panel::bottom("status")
            .frame(bar)
            .show(ui, |ui| self.status_bar(ui))
            .inner;
        if let Some(command) = command {
            self.run(command, &ctx);
        }
        if let Some(formats) = formats {
            self.set_formats(formats, &ctx);
        }
        if let (Some(stacking), Some(shoot)) = (stack_to, &mut self.shoot) {
            shoot.cull.set_stacking(stacking);
            self.state.set_stacking(stacking);
        }
        let theme = self.theme.clone();
        let show = self.state.show;
        if let Some(shoot) = &mut self.shoot {
            let strip = egui::Frame::new().fill(theme.darker_background);
            let clicked = egui::Panel::bottom("filmstrip")
                .frame(strip)
                .resizable(false)
                .show(ui, |ui| shoot.filmstrip(ui, &theme, show))
                .inner;
            if let Some((index, click)) = clicked {
                shoot.clicked(index, click);
            }
        }
        if self.state.show.faces
            && let Some(shoot) = &mut self.shoot
        {
            let side = egui::Frame::new().fill(theme.dark_background).inner_margin(egui::Margin::same(6));
            egui::Panel::right("faces")
                .frame(side)
                .resizable(false)
                .exact_size(crate::shoot::FACES_WIDTH)
                .show(ui, |ui| shoot.face_strip(ui, &theme));
        }
        if self.state.folders {
            let open = self.shoot.as_ref().map(|s| s.cull.dir().to_path_buf());
            let tree = self.tree.get_or_insert_with(|| Tree::new(open.as_deref(), &ctx));
            let side = egui::Frame::new().fill(theme.dark_background).inner_margin(egui::Margin::same(6));
            let clicked = egui::Panel::left("folders")
                .frame(side)
                .resizable(false)
                .exact_size(tree::WIDTH)
                .show(ui, |ui| tree.ui(ui, &theme, open.as_deref()))
                .inner;
            if let Some(dir) = clicked {
                self.open(dir, &ctx);
            }
        }
        if self.summary
            && let Some(command) = self.summary_window(&ctx)
        {
            self.run(command, &ctx);
        }
        let pasteboard = egui::Frame::new().fill(self.theme.pasteboard());
        let mut clicked = None;
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

    /// Where the cull stands, and the filter, format, stacking and
    /// auto-advance switches. Returns the command for a switch clicked, or
    /// the stacking or the formats chosen.
    fn status_bar(&self, ui: &mut Ui) -> (Option<Command>, Option<Stacking>, Option<Formats>) {
        let (mut command, mut stack_to, mut formats_to) = (None, None, None);
        ui.horizontal(|ui| {
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
                    // Only where there's a choice: more than one kind.
                    let choices = shoot.cull.choices();
                    if !choices.is_empty() {
                        let formats = shoot.cull.formats();
                        let held = Kind::ALL.into_iter().zip(shoot.cull.holds()).filter(|&(_, held)| held > 0);
                        let held: Vec<String> = held.map(|(kind, held)| format!("{held} {}", kind.plural())).collect();
                        let tip = format!("{} ({})", held.join(", "), shortcut(Command::Formats));
                        egui::ComboBox::from_id_salt("formats")
                            .selected_text(format!("Format: {}", formats.label()))
                            .show_ui(ui, |ui| {
                                for f in choices {
                                    if ui.selectable_label(f == formats, f.label()).clicked() {
                                        formats_to = Some(f);
                                    }
                                }
                            })
                            .response
                            .on_hover_text(tip);
                    }
                    let stacking = shoot.cull.stacking();
                    egui::ComboBox::from_id_salt("stacking")
                        .selected_text(format!("Stacks: {}", stacking.label()))
                        .show_ui(ui, |ui| {
                            for s in Stacking::ALL {
                                if ui.selectable_label(s == stacking, s.label()).clicked() {
                                    stack_to = Some(s);
                                }
                            }
                        })
                        .response
                        .on_hover_text(shortcut(Command::Stacking));
                    let auto = ui.selectable_label(self.state.auto_advance, "Auto-advance");
                    if auto.on_hover_text(shortcut(Command::AutoAdvance)).clicked() {
                        command = Some(Command::AutoAdvance);
                    }
                    ui.separator();
                    let Show { histogram, info, clipping, peaking, focus_point, faces, signals, suggestions } =
                        self.state.show;
                    for (on, label, toggle) in [
                        (histogram, "Histogram", Command::Histogram),
                        (info, "Info", Command::Info),
                        (clipping, "Clipping", Command::Clipping),
                        (peaking, "Peaking", Command::Peaking),
                        (focus_point, "AF", Command::FocusPoint),
                        (faces, "Faces", Command::FaceStrip),
                        (signals, "Signals", Command::Signals),
                        (suggestions, "Suggest", Command::Suggestions),
                        (shoot.view().zoomed, "100%", Command::Zoom),
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
                // The room that's left is the status line's, cut short in a
                // narrow window.
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let (text, colour) = match &self.shoot {
                        Some(shoot) => (status_line(shoot), self.theme.foreground),
                        None => ("No folder open".to_owned(), self.theme.dark_foreground),
                    };
                    ui.add(egui::Label::new(RichText::new(text).color(colour)).truncate());
                });
            });
        });
        (command, stack_to, formats_to)
    }

    /// Where the cull stands: picks, rejects, undecided and each star, with
    /// the way to what's left and on to darktable. Returns a command for a
    /// button clicked.
    fn summary_window(&mut self, ctx: &egui::Context) -> Option<Command> {
        let shoot = self.shoot.as_ref()?;
        let counts = shoot.cull.counts();
        let total = shoot.cull.frames().len();
        let mut command = None;
        let mut open = true;
        egui::Window::new("Cull summary")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                let share = |n: usize| format!("{:.0}%", 100.0 * n as f64 / total.max(1) as f64);
                egui::Grid::new("summary").num_columns(3).spacing(vec2(24.0, 6.0)).show(ui, |ui| {
                    let mut row = |label: String, n: usize, colour| {
                        ui.label(RichText::new(label).color(colour));
                        ui.label(n.to_string());
                        ui.label(RichText::new(share(n)).color(self.theme.dark_foreground));
                        ui.end_row();
                    };
                    row("Picks and up".into(), counts.picks, self.theme.accent);
                    for (i, &n) in counts.stars.iter().enumerate().rev() {
                        row(format!("  {}", crate::shoot::stars(i as i32 + 1)), n, self.theme.foreground);
                    }
                    row("Rejects".into(), counts.rejects, self.theme.red);
                    row("Undecided".into(), counts.undecided, self.theme.foreground);
                    row("All".into(), total, self.theme.dark_foreground);
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let first = ui.add_enabled(counts.undecided > 0, egui::Button::new("First undecided"));
                    if first.on_hover_text(shortcut(Command::FirstUndecided)).clicked() {
                        command = Some(Command::FirstUndecided);
                    }
                    let open = format!("Open in {}", self.developer());
                    if ui.button(open).on_hover_text(shortcut(Command::Darktable)).clicked() {
                        command = Some(Command::Darktable);
                    }
                });
                ui.add_space(8.0);
                for line in self.learned() {
                    ui.label(RichText::new(line).color(self.theme.dark_foreground));
                }
                let learn = ui.add_enabled(self.learning.is_none(), egui::Button::new("Learn from my decisions"));
                if learn.on_hover_text(shortcut(Command::Learn)).clicked() {
                    command = Some(Command::Learn);
                }
            });
        if !open {
            self.summary = false;
        }
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
            let open = format!("Open a folder of raws, JPEGs or PNGs with {}", shortcut(Command::Open));
            ui.label(RichText::new(open).color(self.theme.dark_foreground));
            let tree = format!("or find one in the folder tree with {}", shortcut(Command::Folders));
            ui.label(RichText::new(tree).color(self.theme.dark_foreground));
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

/// Record a change in the sidecar and the decision log, with the frames
/// it was weighed against.
fn record(disk: &Disk, shoot: &mut Shoot, change: Change, how: How, compared: &[usize]) {
    let dwell = shoot.dwell();
    disk.write(mark(shoot, change.index, (change.was, change.now), how, compared, dwell));
}

/// A frame's mark as it's logged: with what was measured of it and what
/// was suggested for it, as they stood when it was made.
fn mark(shoot: &Shoot, index: usize, (was, now): (Rating, Rating), how: How, compared: &[usize], dwell: Duration) -> Mark {
    let cull = &shoot.cull;
    let path = |i: usize| cull.frames()[i].path.clone();
    let evidence = shoot.evidence(index);
    Mark {
        path: path(index),
        rating: now,
        was,
        how,
        view: shoot.mode.name(),
        compared: compared.iter().copied().filter(|&f| f != index).map(path).collect(),
        filter: cull.filter(),
        dwell,
        at: SystemTime::now(),
        signals: evidence.map(|(signals, _)| signals),
        standing: evidence.map(|(_, standing)| standing),
        suggested: shoot.suggestion(index),
    }
}

/// A winner's marks: it's kept and the rest go, in one step, and it's
/// back to the loupe.
fn win(disk: &Disk, shoot: &mut Shoot, marks: &[(usize, Rating)], auto_advance: bool) {
    let changes = shoot.cull.mark_many(marks);
    let compared: Vec<usize> = marks.iter().map(|&(f, _)| f).collect();
    for change in changes {
        record(disk, shoot, change, How::Mark, &compared);
    }
    shoot.back_to_loupe();
    if auto_advance {
        shoot.cull.step(Step::Next);
    }
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
/// The status line: the view, the selection, and the cull.
fn status_line(shoot: &Shoot) -> String {
    let view = match shoot.mode {
        Mode::Loupe => String::new(),
        mode => {
            let lock = if shoot.locked { "" } else { ", unlocked" };
            let name = match mode {
                Mode::Compare => "Compare".to_owned(),
                _ => format!("Survey of {}", shoot.panes.len()),
            };
            format!("{name}{lock}   ")
        }
    };
    let selected = match shoot.cull.selection().len() {
        0 => String::new(),
        n => format!("   {n} selected"),
    };
    format!("{view}{}{selected}", status(&shoot.cull))
}

fn status(cull: &Cull) -> String {
    let counts = cull.counts();
    let filter = cull.filter();
    let shown = match filter {
        Filter::All => String::new(),
        _ => format!(" ({} shown)", cull.frames().iter().filter(|f| filter.matches(f)).count()),
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
        self.log_passes();
        self.remember_place();
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
    use omacull_engine::cull::Filter;
    use omacull_engine::cull::Formats;
    use omacull_engine::sidecar;
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
            Self::with(quiet(), script)
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
                |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: NONE };
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
        let mut h = Harness::open(&folder, Paths { log: Some(log.clone()), ..quiet() });
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
        let mut h = Harness::open(&folder, quiet());
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
        let mut h = Harness::open(&folder, Paths { cache: Some(cache.clone()), ..quiet() });
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
        let mut h = Harness::open(&folder, quiet());
        let cells = h.app.shoot.as_ref().unwrap().cells.clone();
        let (index, rect) = cells.iter().copied().find(|&(i, _)| i == 2).unwrap();
        assert!(rect.bottom() <= 600.0 && rect.top() > 300.0, "the strip is at the bottom: {rect:?}");
        h.click(rect.center());
        assert_eq!(h.cull().current(), index);
    }

    #[test]
    fn opening_a_raw_starts_at_it() {
        let folder = Folder::with_raws("app-open-raw", 4, &Arw::default());
        let mut h = Harness::with(quiet(), &[]);
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
        let mut h = Harness::open(&folder, quiet());
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
        let mut h = Harness::open(&folder, quiet());
        h.press(NONE, Key::Z);
        assert!(h.shoot().view().zoomed, "a tap zooms in");
        h.press(NONE, Key::ArrowRight);
        assert!(h.shoot().view().zoomed, "and stays zoomed from frame to frame");
        h.press(NONE, Key::Z);
        assert!(!h.shoot().view().zoomed, "the next tap zooms out");

        h.key_down(Key::Z);
        assert!(h.shoot().view().zoomed);
        for _ in 0..8 {
            h.frame(vec![]);
        }
        assert!(h.shoot().view().zoomed, "held");
        h.key_up(Key::Z);
        assert!(!h.shoot().view().zoomed, "let go after a look");
    }

    #[test]
    fn the_mouse_zooms_at_the_pointer_and_pans() {
        let folder = Folder::with_raws("app-zoom-mouse", 1, &Arw::default());
        let mut h = Harness::open(&folder, quiet());
        let fit = h.shoot().fit_rect();
        h.click(fit.center());
        assert!(h.shoot().view().zoomed, "a click zooms in");
        let before = h.shoot().view().center;
        h.drag(pos2(400.0, 200.0), pos2(300.0, 150.0));
        assert!(h.shoot().view().zoomed, "dragging pans, and stays zoomed");
        let after = h.shoot().view().center;
        assert!(after[0] > before[0] && after[1] > before[1], "dragged left and up shows more to the right and below");
        h.click(fit.center());
        assert!(!h.shoot().view().zoomed, "a click zooms out");
        // Pressing from whole and dragging is a look.
        h.drag(fit.center(), fit.center() + vec2(-60.0, 0.0));
        assert!(!h.shoot().view().zoomed);
    }

    #[test]
    fn full_size_frames_are_developed_ahead_and_shown_in_tiles() {
        let folder = Folder::with_raws("app-full", 3, &Arw::default());
        let mut h = Harness::open(&folder, quiet());
        // The fake raws have no raw data: developing them fails, and the
        // enlarged preview stands in.
        h.wait("a development", |app| app.shoot.as_ref().unwrap().full_state(0).is_some());
        h.wait("the next one", |app| app.shoot.as_ref().unwrap().full_state(1).is_some());
        assert!(h.shoot().full_state(0).unwrap().is_err());
        h.press(NONE, Key::Z);
        assert!(h.shoot().view().zoomed);

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
        let mut h = Harness::open(&folder, quiet());
        h.press(Modifiers::SHIFT, Key::Z);
        let view = h.shoot().view();
        assert!(view.zoomed && view.follow_focus);
        let first = view.center;
        h.press(NONE, Key::ArrowRight);
        h.wait("the next preview", |app| app.shoot.as_ref().unwrap().preview_ready());
        h.frame(vec![]);
        let second = h.shoot().view().center;
        assert!(second[0] > first[0] && second[1] > first[1], "{first:?} then {second:?}");
        // Manual focus: nothing to go to.
        h.press(NONE, Key::ArrowRight);
        h.wait("the last preview", |app| app.shoot.as_ref().unwrap().preview_ready());
        h.press(NONE, Key::Z);
        h.press(Modifiers::SHIFT, Key::Z);
        assert!(!h.shoot().view().zoomed);
        assert!(h.app.message.as_ref().is_some_and(|(m, _)| m.contains("No focus point")));
    }

    #[test]
    fn overlays_are_switched_remembered_and_baked_in() {
        let folder = Folder::with_raws("app-overlays", 2, &Arw::default());
        let mut h = Harness::open(&folder, quiet());
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

    fn panes(h: &Harness) -> Vec<usize> {
        h.shoot().panes.iter().map(|p| p.frame).collect()
    }

    #[test]
    fn compare_brings_in_the_next_candidate_when_a_side_is_rejected() {
        let folder = Folder::with_raws("app-compare", 6, &Arw::default());
        let log = folder.0.join("decisions.jsonl");
        let mut h = Harness::open(&folder, Paths { log: Some(log.clone()), ..quiet() });
        h.press(NONE, Key::C);
        assert_eq!((h.shoot().mode, panes(&h), h.cull().current()), (Mode::Compare, vec![0, 1], 0));
        h.press(NONE, Key::X);
        assert_eq!(h.ratings()[0], REJECT);
        assert_eq!((panes(&h), h.cull().current()), (vec![2, 1], 2), "the rejected side is replaced");
        h.press(NONE, Key::ArrowRight);
        assert_eq!(panes(&h), [3, 1], "the arrows change the active side's frame");
        h.press(NONE, Key::Tab);
        assert_eq!(h.cull().current(), 1);
        h.press(NONE, Key::P);
        assert_eq!(panes(&h), [3, 1], "a pick stays");

        // Zoom is locked together, until unlocked.
        h.press(NONE, Key::Z);
        assert!(h.shoot().panes.iter().all(|p| p.view.zoomed));
        h.press(NONE, Key::L);
        h.press(NONE, Key::Z);
        let zoomed: Vec<bool> = h.shoot().panes.iter().map(|p| p.view.zoomed).collect();
        assert_eq!(zoomed, [true, false]);
        h.press(NONE, Key::Escape);
        assert_eq!((h.shoot().mode, h.cull().current()), (Mode::Loupe, 1));

        h.app.disk.finish();
        let rows: Vec<serde_json::Value> =
            std::fs::read_to_string(&log).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(rows[0]["view"], "compare");
        let compared = rows[0]["compared"][0]["path"].as_str().unwrap();
        assert!(compared.ends_with("DSC00002.ARW"), "{compared}");
    }

    #[test]
    fn survey_knocks_frames_out_until_one_is_left() {
        let folder = Folder::with_raws("app-survey", 6, &Arw::default());
        let mut h = Harness::open(&folder, quiet());
        h.press(NONE, Key::N);
        assert_eq!((h.shoot().mode, panes(&h)), (Mode::Survey, vec![0, 1, 2, 3]));
        h.frame(vec![]);
        // Laid out in two rows, none overlapping.
        let areas: Vec<_> = h.shoot().panes.iter().map(|p| p.view.area).collect();
        for (i, a) in areas.iter().enumerate() {
            assert!(areas[i + 1..].iter().all(|b| !a.intersects(*b)), "{areas:?}");
        }
        h.press(NONE, Key::Slash);
        assert_eq!((panes(&h), h.cull().current()), (vec![1, 2, 3], 1));
        h.press(NONE, Key::ArrowRight);
        assert_eq!(h.cull().current(), 2, "the arrows move between panes");
        h.press(NONE, Key::Num3);
        assert_eq!(h.ratings()[2], 3);
        h.press(NONE, Key::Slash);
        h.press(NONE, Key::Slash);
        assert_eq!((h.shoot().mode, h.cull().current()), (Mode::Loupe, 1), "one left: the loupe");
        assert_eq!(h.ratings(), [0, 0, 3, 0, 0, 0], "knocking out doesn't mark");
    }

    #[test]
    fn the_selection_is_what_gets_surveyed() {
        let folder = Folder::with_raws("app-select", 6, &Arw::default());
        let mut h = Harness::open(&folder, quiet());
        h.press(Modifiers::SHIFT, Key::ArrowRight);
        h.press(Modifiers::SHIFT, Key::ArrowRight);
        assert_eq!(h.cull().selection(), [0, 1, 2]);
        // Ctrl+click adds a frame from the filmstrip.
        let cells = h.shoot().cells.clone();
        let (_, rect) = cells.iter().copied().find(|&(i, _)| i == 4).unwrap();
        let at = rect.center();
        let (primary, modifiers) = (PointerButton::Primary, Modifiers::COMMAND);
        let button = |pressed| Event::PointerButton { pos: at, button: primary, pressed, modifiers };
        h.frame(vec![Event::ModifiersChanged(Modifiers::COMMAND), Event::PointerMoved(at), button(true)]);
        h.frame(vec![button(false)]);
        h.frame(vec![Event::ModifiersChanged(NONE)]);
        assert_eq!(h.cull().selection(), [0, 1, 2, 4]);
        h.press(NONE, Key::N);
        assert_eq!(panes(&h), [0, 1, 2, 4]);
        // A click on another pane makes it the active one.
        h.frame(vec![]);
        let second = h.shoot().panes[1].view.area.center();
        h.click(second);
        assert_eq!(h.cull().current(), 1);
        assert!(!h.shoot().view().zoomed, "and doesn't zoom it");
        h.press(NONE, Key::Escape);
        h.press(NONE, Key::Escape);
        assert!(h.cull().selection().is_empty(), "Escape leaves the survey, then the selection");
    }

    /// The same frames as raws and as the JPEGs a camera writes beside
    /// them, each pair a second after the last.
    fn pairs(name: &str, count: usize) -> Folder {
        let folder = Folder::new(name);
        for i in 1..=count {
            let captured: &'static str = Box::leak(format!("2026:10:04 12:00:{i:02}").into_boxed_str());
            let frame = || Arw { captured: (captured, "0"), ..Arw::default() };
            frame().write(&folder.raw(i));
            std::fs::write(folder.raw(i).with_extension("JPG"), frame().jpeg()).unwrap();
        }
        folder
    }

    fn names(h: &Harness) -> Vec<String> {
        h.cull().frames().iter().map(|f| file_name(&f.path)).collect()
    }

    #[test]
    fn jpegs_are_culled_like_raws_and_zoom_without_developing() {
        let folder = Folder::new("app-jpegs");
        for i in 1..=3 {
            let picture = omacull_engine::testing::jpeg(600, 400, [200, 120, 40]);
            let camera = Arw { preview: picture, orientation: 6, ..Arw::default() };
            std::fs::write(folder.0.join(format!("DSC{i:05}.JPG")), camera.jpeg()).unwrap();
        }
        let cache = folder.0.join("cache");
        let mut h = Harness::open(&folder, Paths { cache: Some(cache.clone()), ..quiet() });
        assert_eq!((h.cull().frames().len(), h.cull().holds()), (3, [0, 3, 0]));
        let settings = "1/250 s   f/2.8   ISO 400   85 mm   FE 85mm F1.8   2026-10-04 12:00:00";
        assert_eq!(h.shoot().info().unwrap().summary(), settings, "from the JPEG's own Exif");
        // 100% is the JPEG itself, decoded ahead like a raw's development.
        h.wait("the full-size frame", |app| app.shoot.as_ref().unwrap().full_state(0).is_some());
        h.press(NONE, Key::Z);
        h.frame(vec![]);
        assert!(h.shoot().view().zoomed);
        assert!(h.shoot().full_state(0).unwrap().is_ok_and(|tiles| tiles > 0), "nothing to develop");
        assert_eq!(h.shoot().full_size(1.0), vec2(400.0, 600.0), "upright");
        h.press(NONE, Key::Z);

        h.press(NONE, Key::Num4);
        h.press(NONE, Key::ArrowRight);
        h.press(NONE, Key::X);
        assert_eq!(status(h.cull()), "DSC00002.JPG   2 / 3   1 picks   1 rejects   1 undecided");
        h.app.disk.finish();
        let sidecar = |i: usize| folder.0.join(format!("DSC{i:05}.JPG.xmp"));
        let rating = |i: usize| sidecar::rating(&std::fs::read_to_string(sidecar(i)).unwrap()).unwrap();
        assert_eq!((rating(1), rating(2), sidecar(3).exists()), (Some(4), Some(REJECT), false));
        h.wait("the thumbnails in the cache", |_| std::fs::read_dir(&cache).is_ok_and(|d| d.count() == 3));

        // Only one format here, so there's nothing to switch between.
        h.press(Modifiers::SHIFT, Key::F);
        assert_eq!(h.app.state.formats, Formats::Raw);
        assert_eq!(h.app.message.as_ref().unwrap().0, "Format: RAW (this folder has only JPEGs)");
        assert_eq!((h.cull().frames().len(), h.ratings()), (3, vec![4, REJECT, 0]), "and nothing is read again");
    }

    #[test]
    fn raws_and_jpegs_are_culled_together_or_one_format_at_a_time() {
        let folder = pairs("app-formats", 3);
        let mut h = Harness::open(&folder, quiet());
        assert_eq!((h.cull().formats(), h.cull().holds()), (Formats::All, [3, 3, 0]));
        assert_eq!(names(&h)[..3], ["DSC00001.ARW", "DSC00001.JPG", "DSC00002.ARW"], "side by side");
        h.press(NONE, Key::ArrowRight);
        h.press(NONE, Key::ArrowRight);
        h.press(NONE, Key::ArrowRight);
        h.press(NONE, Key::Num3);
        assert_eq!(status(h.cull()), "DSC00002.JPG   4 / 6   1 picks   0 rejects   5 undecided");

        // Shift+F: only the raws, at the one that JPEG came with. Its mark
        // is its own.
        h.press(Modifiers::SHIFT, Key::F);
        h.wait("the raws", |app| app.shoot.as_ref().is_some_and(|s| s.cull.formats() == Formats::Raw));
        assert_eq!(names(&h), ["DSC00001.ARW", "DSC00002.ARW", "DSC00003.ARW"]);
        assert_eq!((h.cull().current(), h.ratings(), h.cull().left_out()), (1, vec![0, 0, 0], 3));
        assert_eq!(h.app.state.formats, Formats::Raw);
        assert_eq!(status(h.cull()), "DSC00002.ARW   2 / 3   0 picks   0 rejects   3 undecided");
        // What isn't culled isn't stacked or marked: a burst's winner
        // rejects the other raws, and no JPEG.
        h.press(Modifiers::SHIFT, Key::G);
        h.press(Modifiers::SHIFT, Key::G);
        assert_eq!(h.cull().stacks(), [vec![0, 1, 2]]);
        h.press(NONE, Key::W);
        assert_eq!(h.ratings(), [REJECT, PICK, REJECT]);

        // Again: only the JPEGs, with the mark made before the raws were
        // read, and none of the raws'.
        h.press(Modifiers::SHIFT, Key::F);
        h.wait("the JPEGs", |app| app.shoot.as_ref().is_some_and(|s| s.cull.formats() == Formats::Jpeg));
        assert_eq!(names(&h), ["DSC00001.JPG", "DSC00002.JPG", "DSC00003.JPG"]);
        assert_eq!((h.cull().current(), h.ratings()), (1, vec![0, 3, 0]));
        h.press(Modifiers::SHIFT, Key::F);
        h.wait("everything", |app| app.shoot.as_ref().is_some_and(|s| s.cull.formats() == Formats::All));
        assert_eq!(h.ratings(), [REJECT, 0, PICK, 3, REJECT, 0]);
        assert_eq!(file_name(&h.cull().frame().path), "DSC00002.JPG", "where it was");
    }

    #[test]
    fn a_picture_thats_opened_is_shown_whatever_was_being_culled() {
        let folder = pairs("app-formats-open", 2);
        let mut h = Harness::with(quiet(), &[]);
        h.app.state.set_formats(Formats::Raw);
        h.app.open(folder.0.clone(), &h.ctx.clone());
        h.wait("the raws", |app| app.shoot.is_some());
        assert_eq!(names(&h), ["DSC00001.ARW", "DSC00002.ARW"]);
        h.app.open(folder.raw(2).with_extension("JPG"), &h.ctx.clone());
        h.wait("the JPEG", |app| app.shoot.as_ref().is_some_and(|s| s.cull.formats() == Formats::All));
        assert_eq!((h.cull().frames().len(), file_name(&h.cull().frame().path)), (4, "DSC00002.JPG".to_owned()));
        assert_eq!(h.app.state.formats, Formats::Raw, "for that once");
        // And Shift+F goes on from what's shown.
        h.press(Modifiers::SHIFT, Key::F);
        h.wait("the raws again", |app| app.shoot.as_ref().is_some_and(|s| s.cull.formats() == Formats::Raw));
        assert_eq!(file_name(&h.cull().frame().path), "DSC00002.ARW");
    }

    #[test]
    fn pngs_are_culled_too_with_a_format_of_their_own() {
        use omacull_engine::testing::Png;
        let folder = pairs("app-pngs", 2);
        let export = Png { width: 600, height: 400, deep: true, ..Png::default() };
        std::fs::write(folder.0.join("DSC00001.png"), export.bytes()).unwrap();
        let mut h = Harness::open(&folder, quiet());
        assert_eq!((h.cull().holds(), h.cull().choices()), ([2, 2, 1], Formats::ALL.to_vec()));
        assert_eq!(names(&h)[..4], ["DSC00001.ARW", "DSC00001.JPG", "DSC00001.png", "DSC00002.ARW"]);
        h.press(NONE, Key::ArrowRight);
        h.press(NONE, Key::ArrowRight);
        // 100% is the PNG itself, as a JPEG's is the JPEG.
        h.wait("the full-size frame", |app| app.shoot.as_ref().unwrap().full_state(2).is_some());
        h.press(NONE, Key::Z);
        h.frame(vec![]);
        assert!(h.shoot().full_state(2).unwrap().is_ok_and(|tiles| tiles > 0), "nothing to develop");
        assert_eq!(h.shoot().full_size(1.0), vec2(600.0, 400.0));
        h.press(NONE, Key::Z);
        // Its mark goes in a sidecar of its own.
        h.press(NONE, Key::Num5);
        h.app.disk.finish();
        let sidecar = std::fs::read_to_string(folder.0.join("DSC00001.png.xmp")).unwrap();
        assert_eq!(sidecar::rating(&sidecar).unwrap(), Some(5));
        assert!(!folder.0.join("DSC00001.ARW.xmp").exists() && !folder.0.join("DSC00001.JPG.xmp").exists());

        // Shift+F goes round what the folder holds: the PNGs come last.
        for formats in [Formats::Raw, Formats::Jpeg, Formats::Png] {
            h.press(Modifiers::SHIFT, Key::F);
            h.wait("the next format", |app| app.shoot.as_ref().is_some_and(|s| s.cull.formats() == formats));
        }
        assert_eq!((names(&h), h.ratings(), h.cull().left_out()), (vec!["DSC00001.png".to_owned()], vec![5], 4));
        assert_eq!(h.app.state.formats, Formats::Png);

        // A folder with none is culled whole, and they aren't offered.
        let other = pairs("app-pngs-none", 1);
        h.app.open(other.0.clone(), &h.ctx.clone());
        h.wait("the other folder", |app| app.shoot.as_ref().is_some_and(|s| s.cull.frames().len() == 2));
        assert_eq!(h.cull().formats(), Formats::All);
        assert_eq!(h.cull().choices(), [Formats::All, Formats::Raw, Formats::Jpeg]);
        h.press(Modifiers::SHIFT, Key::F);
        h.wait("its raws", |app| app.shoot.as_ref().is_some_and(|s| s.cull.formats() == Formats::Raw));
        assert_eq!(names(&h), ["DSC00001.ARW"]);
    }

    fn quiet() -> Paths {
        Paths { cache: None, log: None, darktable: "darktable".into(), faces: None, signals: None, model: None }
    }

    #[test]
    fn the_folder_tree_counts_pictures_and_opens_folders() {
        let root = Folder::new("app-tree");
        let (a, b) = (root.0.join("a shoot"), root.0.join("b shoot"));
        for (dir, count) in [(&a, 3), (&b, 2), (&b.join("selects"), 1)] {
            std::fs::create_dir_all(dir).unwrap();
            for i in 1..=count {
                Arw::default().write(&dir.join(format!("DSC{i:05}.ARW")));
            }
        }
        let mut h = Harness::with(quiet(), &[]);
        h.app.open(a.clone(), &h.ctx.clone());
        h.wait("the folder", |app| app.shoot.is_some());
        h.press(NONE, Key::T);
        assert!(h.app.state.folders);
        h.wait("the tree", |app| app.tree.as_ref().is_some_and(|t| !t.reading() && t.rows.len() == 2));
        let tree = h.app.tree.as_ref().unwrap();
        assert_eq!(tree.root, std::fs::canonicalize(&root.0).unwrap());
        let (_, row) = tree.rows.iter().find(|(p, _)| p.ends_with("b shoot")).unwrap().clone();
        h.click(row.center());
        h.wait("the other folder", |app| app.shoot.as_ref().is_some_and(|s| s.cull.dir().ends_with("b shoot")));
        assert_eq!(h.cull().frames().len(), 2);
        // The loupe is beside the tree.
        h.frame(vec![]);
        assert!(h.shoot().view().area.left() >= tree::WIDTH);
    }

    #[test]
    fn folders_are_reopened_where_they_were_left() {
        let root = Folder::new("app-places");
        let (a, b) = (root.0.join("a"), root.0.join("b"));
        for dir in [&a, &b] {
            std::fs::create_dir_all(dir).unwrap();
            for i in 1..=5 {
                Arw::default().write(&dir.join(format!("DSC{i:05}.ARW")));
            }
        }
        let mut h = Harness::with(quiet(), &[]);
        let open = |h: &mut Harness, dir: &Path| {
            h.app.open(dir.to_path_buf(), &h.ctx.clone());
            h.wait("the folder", |app| app.opening.is_none() && app.shoot.is_some());
        };
        open(&mut h, &a);
        h.press(NONE, Key::P);
        h.press(NONE, Key::ArrowRight);
        h.press(NONE, Key::ArrowRight);
        h.press(NONE, Key::ArrowRight);
        h.press(CMD_ALT, Key::Num0);
        open(&mut h, &b);
        assert_eq!(h.cull().current(), 0);
        open(&mut h, &a);
        assert_eq!((h.cull().current(), h.cull().filter()), (3, Filter::Undecided));
    }

    #[test]
    fn ctrl_w_closes_the_folder_and_its_place_is_kept() {
        let folder = Folder::with_raws("app-close", 5, &Arw::default());
        let mut h = Harness::open(&folder, quiet());
        h.press(NONE, Key::ArrowRight);
        h.press(NONE, Key::ArrowRight);
        h.press(NONE, Key::M);
        let output = h.press(Modifiers::COMMAND, Key::W);
        // The folder, not the window.
        assert!(!closes(&output));
        assert!(h.app.shoot.is_none() && !h.app.summary);
        // With nothing open it does nothing, and the tree still browses.
        h.press(Modifiers::COMMAND, Key::W);
        h.press(NONE, Key::T);
        h.wait("the tree", |app| app.tree.as_ref().is_some_and(|t| !t.reading()));
        h.app.open(folder.0.clone(), &h.ctx.clone());
        h.wait("the folder", |app| app.opening.is_none() && app.shoot.is_some());
        assert_eq!(h.cull().current(), 2);
    }

    #[test]
    fn the_summary_counts_and_leads_to_whats_left() {
        let folder = Folder::with_raws("app-summary", 5, &Arw::default());
        let mut h = Harness::open(&folder, quiet());
        h.press(NONE, Key::Num3);
        h.press(NONE, Key::ArrowRight);
        h.press(NONE, Key::X);
        h.press(NONE, Key::End);
        h.press(NONE, Key::M);
        assert!(h.app.summary);
        assert_eq!(h.cull().counts().stars, [0, 0, 1, 0, 0]);
        h.press(Modifiers::SHIFT, Key::U);
        assert_eq!(h.cull().current(), 2, "the first undecided");
        h.press(NONE, Key::Escape);
        assert!(!h.app.summary);
        assert_eq!(h.shoot().mode, Mode::Loupe);
    }

    #[test]
    fn darktable_is_handed_the_folder_once_the_marks_are_written() {
        let folder = Folder::with_raws("app-darktable", 2, &Arw::default());
        // A stand-in for darktable that says what it was given, and whether
        // the sidecar was there yet.
        let script = folder.0.join("darktable.sh");
        let said = folder.0.join("said");
        let sidecar = sidecar::path_for(&folder.raw(1));
        std::fs::write(
            &script,
            format!("#!/bin/sh\ntest -e '{}' && echo \"$1\" > '{}'\n", sidecar.display(), said.display()),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let paths = Paths { darktable: script.display().to_string(), ..quiet() };
        let mut h = Harness::open(&folder, paths);
        h.press(NONE, Key::P);
        h.press(Modifiers::COMMAND, Key::E);
        let started = Instant::now();
        while !said.exists() {
            assert!(started.elapsed() < Duration::from_secs(10), "darktable wasn't started");
            std::thread::sleep(Duration::from_millis(5));
        }
        let given = std::fs::read_to_string(&said).unwrap();
        assert_eq!(given.trim(), std::fs::canonicalize(&folder.0).unwrap().display().to_string());

        let mut h = Harness::open(&folder, Paths { darktable: "/nowhere/darktable".into(), ..quiet() });
        h.press(Modifiers::COMMAND, Key::E);
        assert!(h.app.message.as_ref().is_some_and(|(m, problem)| *problem && m.contains("Couldn't start")));
    }

    #[test]
    fn e_zooms_to_the_eyes_and_cycles_the_faces() {
        let folder = Folder::with_raws("app-eyes", 3, &Arw::default());
        let mut h = Harness::with(quiet(), &[]);
        h.app.finder = crate::faces::tests::two_faces();
        h.app.open(folder.0.clone(), &h.ctx.clone());
        h.wait("the faces", |app| {
            app.shoot.as_ref().is_some_and(|s| s.preview_ready() && s.faces.of(s.cull.current()).is_some())
        });
        h.frame(vec![]);
        h.press(NONE, Key::E);
        let near = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4;
        let faces = h.shoot().faces.of(0).unwrap();
        assert!(h.shoot().view().zoomed);
        let first = h.shoot().view().center;
        assert!(near(first, faces[1].between_eyes()), "the largest face first: {first:?}");
        h.press(NONE, Key::E);
        assert!(near(h.shoot().view().center, faces[0].between_eyes()), "again: the next face, round to the left");
        h.press(NONE, Key::E);
        assert_eq!(h.shoot().view().center, first, "and round again");

        // The close-ups, and a click on one.
        h.press(Modifiers::SHIFT, Key::E);
        assert!(h.app.state.show.faces);
        h.frame(vec![]);
        let cells = h.shoot().close_up_cells.clone();
        assert_eq!(cells.len(), 2);
        assert!(cells[0].left() > h.shoot().view().area.right(), "beside the loupe");
        h.press(NONE, Key::Z);
        assert!(!h.shoot().view().zoomed);
        h.click(cells[1].center());
        assert!(h.shoot().view().zoomed, "a close-up zooms to its face");
        assert_eq!(h.shoot().view().center, first);
    }

    #[test]
    fn without_face_detection_e_says_why() {
        let folder = Folder::with_raws("app-no-faces", 1, &Arw::default());
        let mut h = Harness::with(quiet(), &[]);
        h.app.finder = crate::faces::tests::none_set_up();
        h.app.open(folder.0.clone(), &h.ctx.clone());
        h.wait("the folder", |app| app.shoot.as_ref().is_some_and(|s| s.faces.unavailable.is_some()));
        h.press(NONE, Key::E);
        assert!(h.app.message.as_ref().is_some_and(|(m, _)| m.contains("no detector")));
        assert!(!h.shoot().view().zoomed);
    }

    /// A burst of three, then two frames on their own, ten seconds apart.
    fn bursts(name: &str) -> Folder {
        let folder = Folder::new(name);
        let times =
            [("12:00:00", "100"), ("12:00:00", "300"), ("12:00:00", "500"), ("12:00:10", "0"), ("12:00:20", "0")];
        for (i, (time, fraction)) in times.into_iter().enumerate() {
            let captured: &'static str = Box::leak(format!("2026:10:04 {time}").into_boxed_str());
            Arw { captured: (captured, fraction), ..Arw::default() }.write(&folder.raw(i + 1));
        }
        folder
    }

    #[test]
    fn a_stack_is_surveyed_and_its_winner_picked_in_one_key() {
        let folder = bursts("app-stacks");
        let log = folder.0.join("decisions.jsonl");
        let mut h = Harness::open(&folder, Paths { log: Some(log.clone()), ..quiet() });
        h.press(Modifiers::SHIFT, Key::G);
        h.press(Modifiers::SHIFT, Key::G);
        assert_eq!((h.cull().stacking(), h.app.state.stacking), (Stacking::Time, Stacking::Time));
        assert_eq!(h.cull().shown_indices(), [0, 3, 4], "the burst shows one frame");
        h.press(NONE, Key::ArrowRight);
        assert_eq!(h.cull().current(), 3, "stepping goes past the burst");
        h.press(NONE, Key::ArrowLeft);
        h.press(NONE, Key::N);
        assert_eq!((h.shoot().mode, panes(&h)), (Mode::Survey, vec![0, 1, 2]), "the stack, surveyed");
        h.press(NONE, Key::ArrowRight);
        h.press(NONE, Key::W);
        assert_eq!(h.ratings(), [REJECT, PICK, REJECT, 0, 0]);
        assert_eq!((h.shoot().mode, h.cull().current()), (Mode::Loupe, 1));
        assert_eq!(h.cull().shown_indices(), [1, 3, 4], "the winner stands for the stack");
        h.press(Modifiers::COMMAND, Key::Z);
        assert_eq!(h.ratings(), [0, 0, 0, 0, 0], "one undo takes it all back");

        h.app.disk.finish();
        let rows: Vec<serde_json::Value> =
            std::fs::read_to_string(&log).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(rows.iter().filter(|r| r["how"] == "mark" && r["view"] == "survey").count(), 3);
        assert_eq!(rows[0]["compared"].as_array().unwrap().len(), 2, "weighed against the rest of the stack");
    }

    #[test]
    fn a_stack_opens_out_and_its_winner_can_be_chosen_from_the_loupe() {
        let folder = bursts("app-stacks-loupe");
        let mut h = Harness::open(&folder, quiet());
        h.press(Modifiers::SHIFT, Key::G);
        h.press(Modifiers::SHIFT, Key::G);
        h.press(NONE, Key::G);
        assert_eq!(h.cull().shown_indices().len(), 5, "opened out");
        h.press(NONE, Key::ArrowRight);
        h.press(NONE, Key::Num3);
        h.press(NONE, Key::W);
        assert_eq!(h.ratings(), [REJECT, 3, REJECT, 0, 0], "a starred winner keeps its stars");
        h.press(NONE, Key::ArrowRight);
        h.press(NONE, Key::ArrowRight);
        h.press(NONE, Key::W);
        assert!(h.app.message.as_ref().is_some_and(|(m, _)| m.contains("stack")), "not in a stack");
    }

    #[test]
    fn stacks_made_by_hand_are_remembered_with_the_folder() {
        let folder = bursts("app-stacks-manual");
        let other = Folder::with_raws("app-stacks-other", 2, &Arw::default());
        let mut h = Harness::open(&folder, quiet());
        h.press(NONE, Key::End);
        h.press(Modifiers::SHIFT, Key::ArrowLeft);
        h.press(Modifiers::COMMAND, Key::G);
        assert_eq!(h.cull().stacking(), Stacking::Manual);
        assert_eq!(h.cull().manual_stacks(), [vec![3, 4]]);
        let open = |h: &mut Harness, dir: &Path| {
            h.app.open(dir.to_path_buf(), &h.ctx.clone());
            h.wait("the folder", |app| app.opening.is_none());
        };
        open(&mut h, &other.0);
        open(&mut h, &folder.0);
        assert_eq!(h.cull().manual_stacks(), [vec![3, 4]]);
        h.press(Modifiers::COMMAND | Modifiers::SHIFT, Key::G);
        assert!(h.cull().manual_stacks().is_empty(), "unstacked");
    }

    /// A preview with something to measure where the camera focused: crisp
    /// stripes, or a soft ramp.
    fn preview(crisp: bool) -> Vec<u8> {
        let (w, h) = (480usize, 320);
        let level = |x: usize| if !crisp { (x * 255 / w) as u8 } else if (x / 4).is_multiple_of(2) { 40 } else { 220 };
        let rgba = (0..w * h).flat_map(|i| [level(i % w); 3].into_iter().chain([255])).collect();
        omacull_engine::image::Image { width: w, height: h, rgba }.encode_jpeg(90).unwrap()
    }

    /// [`bursts`], the second frame of the burst the only sharp one.
    fn a_burst_with_one_sharp_frame(name: &str) -> Folder {
        let folder = bursts(name);
        for i in 1..=3 {
            let captured = ("2026:10:04 12:00:00", ["100", "300", "500"][i - 1]);
            Arw { captured, preview: preview(i == 2), ..Arw::default() }.write(&folder.raw(i));
        }
        folder
    }

    fn rows(log: &Path) -> Vec<serde_json::Value> {
        std::fs::read_to_string(log).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    fn named(row: &serde_json::Value) -> &str {
        row["file"]["path"].as_str().unwrap().rsplit('/').next().unwrap()
    }

    /// A row's frame, the mark made and the mark that was suggested.
    fn made(row: &serde_json::Value) -> (&str, Option<i64>, Option<i64>) {
        (named(row), row["rating"].as_i64(), row["suggested"]["rating"].as_i64())
    }

    #[test]
    fn the_best_of_a_stack_is_suggested_and_taken_or_overridden() {
        let folder = a_burst_with_one_sharp_frame("app-suggest");
        let log = folder.0.join("decisions.jsonl");
        let mut h = Harness::open(&folder, Paths { log: Some(log.clone()), ..quiet() });
        h.press(Modifiers::SHIFT, Key::G);
        h.press(Modifiers::SHIFT, Key::G);
        h.wait("the suggestion", |app| app.shoot.as_ref().unwrap().suggestion(1).is_some());
        let suggested: Vec<_> = (0..5).map(|f| h.shoot().suggestion(f).map(|s| (s.rating, s.by))).collect();
        let by = omacull_engine::signals::By::Signals;
        assert_eq!(suggested, [Some((REJECT, by)), Some((PICK, by)), Some((REJECT, by)), None, None]);
        assert_eq!(h.cull().shown_indices(), [0, 1, 3, 4], "the sharp frame stands for the stack, beside the cursor's");

        // Y takes it, from any frame of the stack.
        h.press(NONE, Key::Y);
        assert_eq!(h.ratings(), [REJECT, PICK, REJECT, 0, 0]);
        assert_eq!((h.cull().current(), h.shoot().suggestion(1)), (1, None));
        h.press(Modifiers::COMMAND, Key::Z);
        assert_eq!(h.ratings(), [0; 5], "one undo takes it all back");
        // Another frame chosen instead is an override, and logged as one.
        h.press(NONE, Key::G);
        h.press(NONE, Key::ArrowRight);
        h.press(NONE, Key::W);
        assert_eq!(h.ratings(), [REJECT, REJECT, PICK, 0, 0]);
        h.press(NONE, Key::ArrowRight);
        h.press(NONE, Key::Y);
        assert!(h.app.message.as_ref().is_some_and(|(m, _)| m.contains("Nothing is suggested")));

        h.app.disk.finish();
        let rows = rows(&log);
        let taken: Vec<_> = rows[..3].iter().map(made).collect();
        let (pick, reject) = (Some(1), Some(-1));
        assert_eq!(taken, [("DSC00002.ARW", pick, pick), ("DSC00001.ARW", reject, reject), ("DSC00003.ARW", reject, reject)]);
        assert!(rows[..3].iter().all(|r| r["suggested"]["by"] == "signals" && r["standing"]["of"] == 3));
        assert!(rows[0]["signals"]["focus"].as_f64().unwrap() > 100.0 && rows[0]["standing"]["sharp"] == 1.0);
        assert!(rows[1]["standing"]["sharp"].as_f64().unwrap() < 0.1, "soft beside it");
        let overridden = rows.iter().rfind(|r| named(r) == "DSC00002.ARW").unwrap();
        assert_eq!((overridden["how"].as_str(), made(overridden)), (Some("mark"), ("DSC00002.ARW", reject, pick)));
        assert!(rows.iter().filter(|r| r["how"] != "mark").all(|r| r["suggested"].is_null()), "undo has none");
    }

    #[test]
    fn signals_are_shown_as_words_for_whats_wrong() {
        use omacull_engine::signals::{Signals, Standing};
        let folder = Folder::with_raws("app-signals", 2, &Arw { preview: preview(true), ..Arw::default() });
        let mut h = Harness::open(&folder, quiet());
        assert!(!h.app.state.show.signals && h.app.state.show.suggestions);
        h.press(NONE, Key::Q);
        assert!(h.app.state.show.signals);
        // Taken in the same second, the two frames are measured against
        // each other.
        h.wait("the signals", |app| app.shoot.as_ref().unwrap().evidence(0).is_some_and(|(_, s)| s.of == 2));
        let (signals, standing) = h.shoot().evidence(0).unwrap();
        assert!(signals.focus.unwrap() > 100.0 && signals.eyes.is_none());
        assert_eq!(standing.sharp, Some(1.0));
        h.frame(vec![]);
        let theme = Theme::default();
        let words = |signals: &Signals, standing: &Standing| -> Vec<(String, bool)> {
            let chips = crate::shoot::chips(signals, standing, &theme);
            chips.into_iter().map(|(text, colour)| (text, colour == theme.red)).collect()
        };
        assert_eq!(words(&signals, &Standing::default()), [(format!("Focus {:.0}", signals.focus.unwrap()), false)], "alone");
        let soft = Standing { of: 4, sharp: Some(0.62), open: Some(0.1) };
        let bad = Signals { eyes: Some(31.4), open: Some(0.1), highlights: 0.08, shadows: 0.2, ..signals };
        assert_eq!(
            words(&bad, &soft),
            [
                ("Eyes 31, 62% of the sharpest".into(), true),
                ("Eyes shut?".into(), true),
                ("8% blown".into(), true),
                ("20% black".into(), true),
            ]
        );
        let best = Standing { sharp: Some(1.0), ..soft };
        assert_eq!(words(&Signals { open: Some(0.9), ..signals }, &best)[0].0, format!("Focus {:.0}, sharpest of 4", signals.focus.unwrap()));
        h.press(Modifiers::SHIFT, Key::Q);
        assert!(!h.app.state.show.suggestions);
    }

    #[test]
    fn frames_passed_over_are_logged_when_the_folder_is_left() {
        let folder = Folder::with_raws("app-passes", 5, &Arw::default());
        let other = Folder::with_raws("app-passes-other", 3, &Arw::default());
        let log = folder.0.join("decisions.jsonl");
        let mut h = Harness::open(&folder, Paths { log: Some(log.clone()), ..quiet() });
        h.press(NONE, Key::P);
        h.press(NONE, Key::ArrowRight);
        h.press(NONE, Key::ArrowRight);
        h.app.open(other.0.clone(), &h.ctx.clone());
        h.wait("the other folder", |app| app.opening.is_none());
        // Only looked through, the other folder says nothing.
        h.press(NONE, Key::ArrowRight);
        eframe::App::on_exit(&mut h.app);
        let passed: Vec<_> = rows(&log).iter().filter(|r| r["how"] == "pass").map(|r| named(r).to_owned()).collect();
        assert_eq!(passed, ["DSC00002.ARW", "DSC00003.ARW"], "what was seen and left, not what was marked or never reached");
        assert!(!sidecar::path_for(&folder.raw(2)).exists(), "nothing is written for a pass");
    }

    #[test]
    fn the_model_learns_from_the_log_and_its_suggestions_are_reviewed() {
        let folder = Folder::new("app-learn");
        for (i, crisp) in [false, true, false].into_iter().enumerate() {
            Arw { preview: preview(crisp), ..Arw::default() }.write(&folder.raw(i + 1));
        }
        // A cull so far: the soft rejected, the sharp kept.
        let (log, model) = (folder.0.join("decisions.jsonl"), folder.0.join("model.json"));
        let earlier: String = (0..120)
            .map(|i| {
                let (focus, rating) = if i % 2 == 0 { (2.0, -1) } else { (150.0, 1) };
                let signals = serde_json::json!({"focus": focus, "eyes": null, "open": null, "highlights": 0.0, "shadows": 0.0, "faces": 0, "face": 0.0});
                let file = serde_json::json!({"path": format!("/earlier/DSC{i:05}.ARW"), "captured": null});
                format!("{}\n", serde_json::json!({"file": file, "rating": rating, "signals": signals}))
            })
            .collect();
        std::fs::write(&log, earlier).unwrap();
        let mut h = Harness::open(&folder, Paths { log: Some(log.clone()), model: Some(model.clone()), ..quiet() });
        h.press(NONE, Key::M);
        h.press(NONE, Key::Y);
        assert!(h.app.message.as_ref().is_some_and(|(m, _)| m.contains("Nothing is suggested")));
        h.press(Modifiers::COMMAND, Key::L);
        h.wait("the model", |app| app.model.is_some());
        assert!(model.exists(), "kept for next time");
        let said = h.app.message.clone().unwrap().0;
        assert!(said.starts_with("Learned from 60 rejected, 60 kept and 0 left unmarked. Of 2"), "{said}");
        h.press(NONE, Key::Escape);

        h.wait("its suggestions", |app| app.shoot.as_ref().unwrap().suggestion(2).is_some());
        let suggested: Vec<_> = (0..3).map(|f| h.shoot().suggestion(f).map(|s| s.rating)).collect();
        assert_eq!(suggested, [Some(REJECT), Some(PICK), Some(REJECT)]);
        assert!(h.shoot().suggestion(0).unwrap().confidence >= 0.8);
        h.press(NONE, Key::Num2);
        h.press(CMD_ALT, Key::Y);
        assert_eq!((h.cull().filter(), h.cull().current()), (Filter::Suggested, 1));
        assert_eq!(h.cull().shown_indices(), [1, 2], "what there is to review");
        h.press(NONE, Key::Y);
        h.press(NONE, Key::ArrowRight);
        h.press(NONE, Key::Y);
        assert_eq!(h.ratings(), [2, PICK, REJECT]);
        // With suggestions off there are none, to see or to take.
        h.press(Modifiers::COMMAND, Key::Z);
        h.press(Modifiers::SHIFT, Key::Q);
        assert_eq!(h.shoot().suggestion(2), None);

        h.app.disk.finish();
        let rows = rows(&log);
        let marks: Vec<_> = rows[120..123].iter().map(made).collect();
        let (pick, reject) = (Some(1), Some(-1));
        assert_eq!(
            marks,
            [("DSC00001.ARW", Some(2), reject), ("DSC00002.ARW", pick, pick), ("DSC00003.ARW", reject, reject)],
            "an override, and two taken"
        );
        assert!(rows[120..123].iter().all(|r| r["suggested"]["by"] == "model"));
        assert_eq!(rows[122]["filter"], "suggested");

        // Too little to learn from says so, and the model stays.
        std::fs::write(&log, "").unwrap();
        h.press(Modifiers::COMMAND, Key::L);
        h.wait("learning", |app| app.learning.is_none());
        assert!(h.app.message.as_ref().is_some_and(|(m, _)| m.contains("0 of the 100")));
        assert!(h.app.model.is_some());
    }

    #[test]
    fn a_pick_is_as_many_stars_as_the_config_says() {
        let folder = bursts("app-pick");
        let mut h = Harness::with(quiet(), &[]);
        h.app.config.pick = 3;
        h.app.open(folder.0.clone(), &h.ctx.clone());
        h.wait("the folder", |app| app.shoot.as_ref().is_some_and(|s| s.preview_ready()));
        h.press(Modifiers::SHIFT, Key::G);
        h.press(Modifiers::SHIFT, Key::G);
        h.press(NONE, Key::W);
        assert_eq!(h.ratings(), [3, REJECT, REJECT, 0, 0], "a winner is a pick");
        h.press(NONE, Key::End);
        h.press(NONE, Key::P);
        h.press(NONE, Key::ArrowLeft);
        h.press(NONE, Key::Num1);
        assert_eq!(h.ratings()[3..], [1, 3]);
    }
}
