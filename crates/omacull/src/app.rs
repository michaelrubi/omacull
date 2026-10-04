//! The window: for now an empty one, themed, that takes commands.

use std::collections::VecDeque;
use std::sync::mpsc::Receiver;

use egui::{RichText, Ui};

use crate::commands::Command;
use crate::theme::{self, Theme};

pub struct App {
    theme: Theme,
    theme_rx: Receiver<Theme>,
    /// Problems found at startup, shown in the status bar.
    warnings: Vec<String>,
    /// Commands to run, one a frame, from `OMACULL_SCRIPT` (comma-separated
    /// command names, e.g. `Next,Pick,Quit`). For testing without a
    /// keyboard.
    script: VecDeque<Command>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
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
        Self {
            theme,
            theme_rx: theme::watch(ctx.clone()),
            warnings,
            script: std::env::var("OMACULL_SCRIPT")
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
                .collect(),
        }
    }

    fn run(&mut self, command: Command, ctx: &egui::Context) {
        match command {
            Command::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            // Nothing is open to step through or mark until M1.
            Command::Undo | Command::Redo => {}
            Command::Previous | Command::Next | Command::First | Command::Last => {}
            Command::Reject | Command::Unmark | Command::Pick => {}
            Command::Star1 | Command::Star2 | Command::Star3 | Command::Star4 | Command::Star5 => {}
        }
    }

    fn run_script(&mut self, ctx: &egui::Context) {
        let Some(command) = self.script.pop_front() else {
            return;
        };
        log::info!("script: {command:?}");
        self.run(command, ctx);
        ctx.request_repaint();
    }

    /// A frame's work before anything is drawn.
    fn step(&mut self, ctx: &egui::Context) {
        if let Some(theme) = self.theme_rx.try_iter().last() {
            ctx.set_visuals(theme.visuals());
            self.theme = theme;
        }
        if !ctx.egui_wants_keyboard_input() {
            for command in Command::pressed(ctx) {
                self.run(command, ctx);
            }
        }
        self.run_script(ctx);
    }

    fn show(&mut self, ui: &mut Ui) {
        let bar = egui::Frame::new()
            .fill(self.theme.dark_background)
            .inner_margin(egui::Margin::symmetric(8, 4));
        egui::Panel::bottom("status")
            .frame(bar)
            .show(ui, |ui| self.status_bar(ui));
        let pasteboard = egui::Frame::new().fill(self.theme.pasteboard());
        egui::CentralPanel::no_frame()
            .frame(pasteboard)
            .show(ui, |ui| self.empty_state(ui));
    }

    fn status_bar(&self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if self.warnings.is_empty() {
                ui.label(RichText::new("No folder open").color(self.theme.dark_foreground));
            } else {
                ui.label(RichText::new(self.warnings.join("; ")).color(self.theme.red));
            }
        });
    }

    fn empty_state(&self, ui: &mut Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.4);
            ui.label(RichText::new("Omacull").size(28.0).color(self.theme.accent));
        });
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.step(ctx);
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    use egui::{Event, Key, Modifiers};

    /// The app driven with synthetic events, with no window or GPU.
    struct Harness {
        ctx: egui::Context,
        app: App,
        time: f64,
    }

    impl Harness {
        fn new(script: &[Command]) -> Self {
            let (_tx, rx) = channel();
            Self {
                ctx: egui::Context::default(),
                app: App {
                    theme: Theme::default(),
                    theme_rx: rx,
                    warnings: Vec::new(),
                    script: script.iter().copied().collect(),
                },
                time: 0.0,
            }
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

        fn press(&mut self, modifiers: Modifiers, key: Key) -> egui::FullOutput {
            self.frame(vec![
                Event::ModifiersChanged(modifiers),
                Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
            ])
        }
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
}
