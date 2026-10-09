//! Every user action, with its label and default shortcut (Bridge and
//! Lightroom's, where they have one). The keyboard, and headless scripts,
//! both go through this list, so they can't disagree.

use egui::{Key, KeyboardShortcut, Modifiers};
use omacull_engine::cull::{Filter, PICK, Rating};
use omacull_engine::sidecar::REJECT;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    Quit,
    Open,
    Undo,
    Redo,
    Previous,
    Next,
    First,
    Last,
    Reject,
    Unmark,
    Pick,
    Star1,
    Star2,
    Star3,
    Star4,
    Star5,
    AutoAdvance,
    ShowAll,
    ShowUndecided,
    ShowPicks,
    ShowStars2,
    ShowStars3,
    ShowStars4,
    ShowStars5,
    ShowRejects,
    Zoom,
    ZoomToFocus,
    Histogram,
    Info,
    Clipping,
    Peaking,
    FocusPoint,
    Compare,
    Survey,
    Back,
    Lock,
    NextPane,
    KnockOut,
    SelectPrevious,
    SelectNext,
    SelectAll,
    SelectNone,
    Folders,
    Summary,
    FirstUndecided,
    Darktable,
    Export,
    Eyes,
    FaceStrip,
    StackSelection,
    Unstack,
    ToggleStack,
    Stacking,
    Formats,
    Winner,
    Signals,
    Suggestions,
    Accept,
    ShowSuggested,
    Learn,
}

const CMD: Modifiers = Modifiers::COMMAND;
const CMD_SHIFT: Modifiers = Modifiers {
    shift: true,
    ..Modifiers::COMMAND
};
const CMD_ALT: Modifiers = Modifiers {
    alt: true,
    ..Modifiers::COMMAND
};

impl Command {
    pub const ALL: &[Command] = &[
        Command::Quit,
        Command::Open,
        Command::Undo,
        Command::Redo,
        Command::Previous,
        Command::Next,
        Command::First,
        Command::Last,
        Command::Reject,
        Command::Unmark,
        Command::Pick,
        Command::Star1,
        Command::Star2,
        Command::Star3,
        Command::Star4,
        Command::Star5,
        Command::AutoAdvance,
        Command::ShowAll,
        Command::ShowUndecided,
        Command::ShowPicks,
        Command::ShowStars2,
        Command::ShowStars3,
        Command::ShowStars4,
        Command::ShowStars5,
        Command::ShowRejects,
        Command::Zoom,
        Command::ZoomToFocus,
        Command::Histogram,
        Command::Info,
        Command::Clipping,
        Command::Peaking,
        Command::FocusPoint,
        Command::Compare,
        Command::Survey,
        Command::Back,
        Command::Lock,
        Command::NextPane,
        Command::KnockOut,
        Command::SelectPrevious,
        Command::SelectNext,
        Command::SelectAll,
        Command::SelectNone,
        Command::Folders,
        Command::Summary,
        Command::FirstUndecided,
        Command::Darktable,
        Command::Export,
        Command::Eyes,
        Command::FaceStrip,
        Command::StackSelection,
        Command::Unstack,
        Command::ToggleStack,
        Command::Stacking,
        Command::Formats,
        Command::Winner,
        Command::Signals,
        Command::Suggestions,
        Command::Accept,
        Command::ShowSuggested,
        Command::Learn,
    ];

    /// Look a command up by its name in code, e.g. "Reject".
    pub fn from_name(name: &str) -> Option<Command> {
        Self::ALL.iter().copied().find(|c| format!("{c:?}") == name)
    }

    /// Every command with a shortcut, most modifiers first. egui matches
    /// Ctrl+Z even when Shift is also held, so Ctrl+Shift+Z has to be
    /// checked before it.
    pub const KEYBOARD_ORDER: &[Command] = &[
        Command::Unstack,
        Command::ShowAll,
        Command::ShowUndecided,
        Command::ShowPicks,
        Command::ShowStars2,
        Command::ShowStars3,
        Command::ShowStars4,
        Command::ShowStars5,
        Command::ShowRejects,
        Command::ShowSuggested,
        Command::Redo,
        Command::ZoomToFocus,
        Command::SelectPrevious,
        Command::SelectNext,
        Command::SelectAll,
        Command::SelectNone,
        Command::FirstUndecided,
        Command::Export,
        Command::Darktable,
        Command::FaceStrip,
        Command::StackSelection,
        Command::Stacking,
        Command::Formats,
        Command::Suggestions,
        Command::Learn,
        Command::Quit,
        Command::Open,
        Command::Undo,
        Command::Previous,
        Command::Next,
        Command::First,
        Command::Last,
        Command::Reject,
        Command::Unmark,
        Command::Pick,
        Command::Star1,
        Command::Star2,
        Command::Star3,
        Command::Star4,
        Command::Star5,
        Command::AutoAdvance,
        Command::Zoom,
        Command::Histogram,
        Command::Info,
        Command::Clipping,
        Command::Peaking,
        Command::FocusPoint,
        Command::Compare,
        Command::Survey,
        Command::Back,
        Command::Lock,
        Command::NextPane,
        Command::KnockOut,
        Command::Folders,
        Command::Summary,
        Command::Eyes,
        Command::ToggleStack,
        Command::Winner,
        Command::Signals,
        Command::Accept,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Command::Quit => "Quit",
            Command::Open => "Open Folder…",
            Command::Undo => "Undo",
            Command::Redo => "Redo",
            Command::Previous => "Previous Frame",
            Command::Next => "Next Frame",
            Command::First => "First Frame",
            Command::Last => "Last Frame",
            Command::Reject => "Reject",
            Command::Unmark => "Unmark",
            Command::Pick => "Pick",
            Command::Star1 => "1 Star",
            Command::Star2 => "2 Stars",
            Command::Star3 => "3 Stars",
            Command::Star4 => "4 Stars",
            Command::Star5 => "5 Stars",
            Command::AutoAdvance => "Auto-Advance After a Mark",
            Command::ShowAll => "Show All",
            Command::ShowUndecided => "Show Undecided",
            Command::ShowPicks => "Show Picks and Up",
            Command::ShowStars2 => "Show 2 Stars and Up",
            Command::ShowStars3 => "Show 3 Stars and Up",
            Command::ShowStars4 => "Show 4 Stars and Up",
            Command::ShowStars5 => "Show 5 Stars",
            Command::ShowRejects => "Show Rejects",
            Command::Zoom => "Zoom to 100% (Hold for a Look)",
            Command::ZoomToFocus => "Zoom to the Focus Point",
            Command::Histogram => "Histogram",
            Command::Info => "Shooting Settings",
            Command::Clipping => "Highlight and Shadow Clipping",
            Command::Peaking => "Focus Peaking",
            Command::FocusPoint => "Focus Point",
            Command::Compare => "Compare",
            Command::Survey => "Survey",
            Command::Back => "Back to the Loupe",
            Command::Lock => "Lock Zoom Together",
            Command::NextPane => "Next Pane",
            Command::KnockOut => "Knock Out of Survey",
            Command::SelectPrevious => "Select the Previous Frame Too",
            Command::SelectNext => "Select the Next Frame Too",
            Command::SelectAll => "Select All",
            Command::SelectNone => "Select None",
            Command::Folders => "Folder Tree",
            Command::Summary => "Cull Summary",
            Command::FirstUndecided => "First Undecided Frame",
            Command::Darktable => "Open in the Raw Developer",
            Command::Export => "Copy What's Shown to a Folder…",
            Command::Eyes => "Zoom to the Eyes (Again: the Next Face)",
            Command::FaceStrip => "Face Close-Ups",
            Command::StackSelection => "Stack the Selection",
            Command::Unstack => "Unstack",
            Command::ToggleStack => "Open Out or Close Up the Stack",
            Command::Stacking => "How Frames Are Stacked",
            Command::Formats => "Which Formats Are Culled: All, RAW, JPEG or PNG",
            Command::Winner => "Winner: Pick It, Reject the Rest",
            Command::Signals => "Signals: Sharpness, Shut Eyes, Clipping",
            Command::Suggestions => "Suggestions",
            Command::Accept => "Take the Suggestion",
            Command::ShowSuggested => "Show Suggested",
            Command::Learn => "Learn from My Decisions",
        }
    }

    pub fn default_shortcut(self) -> Option<KeyboardShortcut> {
        let s = |m, k| Some(KeyboardShortcut::new(m, k));
        match self {
            Command::Quit => s(CMD, Key::Q),
            Command::Open => s(CMD, Key::O),
            Command::Undo => s(CMD, Key::Z),
            Command::Redo => s(CMD_SHIFT, Key::Z),
            Command::Previous => s(Modifiers::NONE, Key::ArrowLeft),
            Command::Next => s(Modifiers::NONE, Key::ArrowRight),
            Command::First => s(Modifiers::NONE, Key::Home),
            Command::Last => s(Modifiers::NONE, Key::End),
            Command::Reject => s(Modifiers::NONE, Key::X),
            Command::Unmark => s(Modifiers::NONE, Key::U),
            Command::Pick => s(Modifiers::NONE, Key::P),
            Command::Star1 => s(Modifiers::NONE, Key::Num1),
            Command::Star2 => s(Modifiers::NONE, Key::Num2),
            Command::Star3 => s(Modifiers::NONE, Key::Num3),
            Command::Star4 => s(Modifiers::NONE, Key::Num4),
            Command::Star5 => s(Modifiers::NONE, Key::Num5),
            Command::AutoAdvance => s(Modifiers::NONE, Key::A),
            // Bridge's filter keys, with X for rejects as in marking.
            Command::ShowAll => s(CMD_ALT, Key::A),
            Command::ShowUndecided => s(CMD_ALT, Key::Num0),
            Command::ShowPicks => s(CMD_ALT, Key::Num1),
            Command::ShowStars2 => s(CMD_ALT, Key::Num2),
            Command::ShowStars3 => s(CMD_ALT, Key::Num3),
            Command::ShowStars4 => s(CMD_ALT, Key::Num4),
            Command::ShowStars5 => s(CMD_ALT, Key::Num5),
            Command::ShowRejects => s(CMD_ALT, Key::X),
            // Lightroom's Z, I and J; the rest by their initials.
            Command::Zoom => s(Modifiers::NONE, Key::Z),
            Command::ZoomToFocus => s(Modifiers::SHIFT, Key::Z),
            Command::Histogram => s(Modifiers::NONE, Key::H),
            Command::Info => s(Modifiers::NONE, Key::I),
            Command::Clipping => s(Modifiers::NONE, Key::J),
            Command::Peaking => s(Modifiers::NONE, Key::S),
            Command::FocusPoint => s(Modifiers::NONE, Key::F),
            // Lightroom's C, N and / (out of the survey); L for the lock.
            Command::Compare => s(Modifiers::NONE, Key::C),
            Command::Survey => s(Modifiers::NONE, Key::N),
            Command::Back => s(Modifiers::NONE, Key::Escape),
            Command::Lock => s(Modifiers::NONE, Key::L),
            Command::NextPane => s(Modifiers::NONE, Key::Tab),
            Command::KnockOut => s(Modifiers::NONE, Key::Slash),
            Command::SelectPrevious => s(Modifiers::SHIFT, Key::ArrowLeft),
            Command::SelectNext => s(Modifiers::SHIFT, Key::ArrowRight),
            Command::SelectAll => s(CMD, Key::A),
            Command::SelectNone => s(CMD, Key::D),
            Command::Folders => s(Modifiers::NONE, Key::T),
            Command::Summary => s(Modifiers::NONE, Key::M),
            Command::FirstUndecided => s(Modifiers::SHIFT, Key::U),
            // Lightroom's Edit In, and its Export.
            Command::Darktable => s(CMD, Key::E),
            Command::Export => s(CMD_SHIFT, Key::E),
            Command::Eyes => s(Modifiers::NONE, Key::E),
            Command::FaceStrip => s(Modifiers::SHIFT, Key::E),
            // Lightroom's Ctrl+G groups into a stack, and G-ish opens it.
            Command::StackSelection => s(CMD, Key::G),
            Command::Unstack => s(CMD_SHIFT, Key::G),
            Command::ToggleStack => s(Modifiers::NONE, Key::G),
            Command::Stacking => s(Modifiers::SHIFT, Key::G),
            Command::Formats => s(Modifiers::SHIFT, Key::F),
            Command::Winner => s(Modifiers::NONE, Key::W),
            // Q for the quality of a frame; Y to say yes to a suggestion.
            Command::Signals => s(Modifiers::NONE, Key::Q),
            Command::Suggestions => s(Modifiers::SHIFT, Key::Q),
            Command::Accept => s(Modifiers::NONE, Key::Y),
            Command::ShowSuggested => s(CMD_ALT, Key::Y),
            Command::Learn => s(CMD, Key::L),
        }
    }

    /// The mark a marking command makes.
    pub fn rating(self) -> Option<Rating> {
        Some(match self {
            Command::Reject => REJECT,
            Command::Unmark => 0,
            Command::Pick => PICK,
            Command::Star1 => 1,
            Command::Star2 => 2,
            Command::Star3 => 3,
            Command::Star4 => 4,
            Command::Star5 => 5,
            _ => return None,
        })
    }

    /// The filter a filtering command shows.
    pub fn filter(self) -> Option<Filter> {
        Some(match self {
            Command::ShowAll => Filter::All,
            Command::ShowUndecided => Filter::Undecided,
            Command::ShowPicks => Filter::AtLeast(PICK),
            Command::ShowStars2 => Filter::AtLeast(2),
            Command::ShowStars3 => Filter::AtLeast(3),
            Command::ShowStars4 => Filter::AtLeast(4),
            Command::ShowStars5 => Filter::AtLeast(5),
            Command::ShowRejects => Filter::Rejects,
            Command::ShowSuggested => Filter::Suggested,
            _ => return None,
        })
    }

    /// The command that shows a filter.
    pub fn show(filter: Filter) -> Command {
        Self::ALL.iter().copied().find(|c| c.filter() == Some(filter)).unwrap_or(Command::ShowAll)
    }

    /// Commands whose shortcut was pressed this frame, consuming the keys.
    pub fn pressed(ctx: &egui::Context) -> Vec<Command> {
        Self::pressed_with(ctx, crate::hotkeys::current())
    }

    /// [`Self::pressed`] with these shortcuts.
    pub fn pressed_with(ctx: &egui::Context, hotkeys: &crate::hotkeys::Hotkeys) -> Vec<Command> {
        ctx.input_mut(|i| {
            let mut pressed: Vec<Command> = hotkeys
                .keyboard_order()
                .iter()
                .copied()
                .filter(|&c| hotkeys.command(c).is_some_and(|s| i.consume_shortcut(&s)))
                .collect();
            // 0 is no stars, as in Bridge and Lightroom, unless it was
            // given to another command (which has consumed it by now).
            if hotkeys.command(Command::Unmark).is_some()
                && i.consume_key(Modifiers::NONE, Key::Num0)
                && !pressed.contains(&Command::Unmark)
            {
                pressed.push(Command::Unmark);
            }
            pressed
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shortcut_is_in_keyboard_order() {
        for &c in Command::ALL {
            if c.default_shortcut().is_some() {
                assert!(Command::KEYBOARD_ORDER.contains(&c), "{c:?} missing from KEYBOARD_ORDER");
            }
        }
    }

    #[test]
    fn names_round_trip() {
        for &c in Command::ALL {
            assert_eq!(Command::from_name(&format!("{c:?}")), Some(c));
        }
    }

    /// Commands pressed in a frame with this key and these modifiers held.
    fn press(ctx: &egui::Context, modifiers: Modifiers, key: Key) -> Vec<Command> {
        let events = vec![
            egui::Event::ModifiersChanged(modifiers),
            egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            },
        ];
        let input = egui::RawInput {
            events,
            ..Default::default()
        };
        let mut pressed = Vec::new();
        let mut output = ctx.run_ui(input, |ui| pressed = Command::pressed(ui.ctx()));
        // There's no renderer to upload textures to.
        output.textures_delta.clear();
        pressed
    }

    #[test]
    fn marks_and_steps_are_single_keys() {
        let ctx = egui::Context::default();
        let none = Modifiers::NONE;
        assert_eq!(press(&ctx, none, Key::X), [Command::Reject]);
        assert_eq!(press(&ctx, none, Key::U), [Command::Unmark]);
        assert_eq!(press(&ctx, none, Key::Num0), [Command::Unmark]);
        assert_eq!(press(&ctx, none, Key::P), [Command::Pick]);
        assert_eq!(press(&ctx, none, Key::Num3), [Command::Star3]);
        assert_eq!(press(&ctx, none, Key::ArrowRight), [Command::Next]);
        assert_eq!(press(&ctx, none, Key::Home), [Command::First]);
        assert_eq!(press(&ctx, none, Key::A), [Command::AutoAdvance]);
        assert_eq!(press(&ctx, none, Key::B), []);
        assert_eq!(press(&ctx, none, Key::Z), [Command::Zoom]);
        assert_eq!(press(&ctx, Modifiers::SHIFT, Key::Z), [Command::ZoomToFocus]);
        assert_eq!(press(&ctx, none, Key::J), [Command::Clipping]);
    }

    #[test]
    fn filters_are_ctrl_alt_and_dont_mark() {
        let ctx = egui::Context::default();
        assert_eq!(press(&ctx, CMD_ALT, Key::Num0), [Command::ShowUndecided]);
        assert_eq!(press(&ctx, CMD_ALT, Key::Num3), [Command::ShowStars3]);
        assert_eq!(press(&ctx, CMD_ALT, Key::X), [Command::ShowRejects]);
        assert_eq!(press(&ctx, CMD_ALT, Key::A), [Command::ShowAll]);
        assert_eq!(press(&ctx, CMD_ALT, Key::Y), [Command::ShowSuggested]);
        for filter in Filter::ALL {
            assert_eq!(Command::show(filter).filter(), Some(filter));
        }
    }

    #[test]
    fn redo_isnt_mistaken_for_undo() {
        let ctx = egui::Context::default();
        assert_eq!(press(&ctx, CMD, Key::Z), [Command::Undo]);
        assert_eq!(press(&ctx, CMD_SHIFT, Key::Z), [Command::Redo]);
        // Nor Ctrl+X for a reject.
        assert_eq!(press(&ctx, CMD, Key::X), []);
        // Nor learning for locking the zoom, nor suggestions for signals.
        assert_eq!(press(&ctx, CMD, Key::L), [Command::Learn]);
        assert_eq!(press(&ctx, Modifiers::NONE, Key::L), [Command::Lock]);
        assert_eq!(press(&ctx, Modifiers::SHIFT, Key::Q), [Command::Suggestions]);
        // Nor copying for the raw developer, the eyes or their close-ups.
        assert_eq!(press(&ctx, CMD_SHIFT, Key::E), [Command::Export]);
        assert_eq!(press(&ctx, CMD, Key::E), [Command::Darktable]);
        assert_eq!(press(&ctx, Modifiers::SHIFT, Key::E), [Command::FaceStrip]);
        assert_eq!(press(&ctx, Modifiers::NONE, Key::E), [Command::Eyes]);
        // Nor the formats for the focus point.
        assert_eq!(press(&ctx, Modifiers::SHIFT, Key::F), [Command::Formats]);
        assert_eq!(press(&ctx, Modifiers::NONE, Key::F), [Command::FocusPoint]);
        assert_eq!(press(&ctx, Modifiers::NONE, Key::Q), [Command::Signals]);
        assert_eq!(press(&ctx, Modifiers::NONE, Key::Y), [Command::Accept]);
    }
}
