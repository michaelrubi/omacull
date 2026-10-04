//! Custom hotkeys configuration loaded from `~/.config/omacull/hotkeys.toml`.
//!
//! Overrides command shortcuts. Keyed by command names
//! (`Command::from_name` / `format!("{c:?}")`).
//!
//! Format:
//! ```toml
//! [commands]
//! Reject = "Delete"
//! Next = "Space"
//! Pick = ""          # empty string: no shortcut
//! ```

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::OnceLock;

use egui::{Key, KeyboardShortcut, Modifiers};
use serde::Deserialize;

use crate::commands::Command;

/// Loaded once at startup (see [`load`]); the defaults until then, and in
/// tests.
static HOTKEYS: OnceLock<Hotkeys> = OnceLock::new();

/// The shortcuts in use.
pub(crate) fn current() -> &'static Hotkeys {
    HOTKEYS.get_or_init(Hotkeys::default)
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Hotkeys {
    commands: HashMap<Command, Option<KeyboardShortcut>>,
    keyboard_order: Vec<Command>,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self::from_overrides(HashMap::new())
    }
}

impl Hotkeys {
    pub(crate) fn from_overrides(command_overrides: HashMap<Command, Option<KeyboardShortcut>>) -> Self {
        let mut commands = HashMap::new();
        for &cmd in Command::ALL {
            let sc = match command_overrides.get(&cmd) {
                Some(override_sc) => *override_sc,
                None => cmd.default_shortcut(),
            };
            commands.insert(cmd, sc);
        }

        let mut keyboard_order: Vec<Command> = Command::KEYBOARD_ORDER
            .iter()
            .copied()
            .filter(|c| commands.get(c).copied().flatten().is_some())
            .collect();
        for &cmd in Command::ALL {
            if !keyboard_order.contains(&cmd) && commands.get(&cmd).copied().flatten().is_some() {
                keyboard_order.push(cmd);
            }
        }

        // Sort by modifier count descending (most modifiers first).
        // Stable sort preserves a deterministic relative order.
        keyboard_order.sort_by(|a, b| {
            let count_a = commands.get(a).copied().flatten().map_or(0, |s| modifier_count(&s));
            let count_b = commands.get(b).copied().flatten().map_or(0, |s| modifier_count(&s));
            count_b.cmp(&count_a)
        });

        Self {
            commands,
            keyboard_order,
        }
    }

    pub(crate) fn command(&self, cmd: Command) -> Option<KeyboardShortcut> {
        self.commands.get(&cmd).copied().flatten()
    }

    pub(crate) fn keyboard_order(&self) -> &[Command] {
        &self.keyboard_order
    }

    pub(crate) fn check_clashes(&self, warnings: &mut Vec<String>) {
        let mut by_shortcut: BTreeMap<String, Vec<Command>> = BTreeMap::new();
        for &cmd in Command::ALL {
            if let Some(sc) = self.command(cmd) {
                by_shortcut.entry(format_shortcut(&sc)).or_default().push(cmd);
            }
        }

        for (sc_str, cmds) in &by_shortcut {
            if cmds.len() > 1 {
                let names: Vec<_> = cmds.iter().map(|c| format!("{c:?}")).collect();
                warnings.push(format!(
                    "Hotkeys: clash between commands {} on shortcut {sc_str}",
                    names.join(" and ")
                ));
            }
        }
    }
}

pub(crate) fn modifier_count(s: &KeyboardShortcut) -> usize {
    (s.modifiers.alt as usize)
        + (s.modifiers.shift as usize)
        + ((s.modifiers.command || s.modifiers.ctrl || s.modifiers.mac_cmd) as usize)
}

pub(crate) fn format_shortcut(s: &KeyboardShortcut) -> String {
    let mut parts = Vec::new();
    if s.modifiers.command || s.modifiers.ctrl || s.modifiers.mac_cmd {
        parts.push("Ctrl");
    }
    if s.modifiers.alt {
        parts.push("Alt");
    }
    if s.modifiers.shift {
        parts.push("Shift");
    }
    // The arrows' symbols are hard to type back into hotkeys.toml.
    let symbol = s.logical_key.symbol_or_name();
    parts.push(if symbol.is_ascii() { symbol } else { s.logical_key.name() });
    parts.join("+")
}

pub(crate) fn parse_shortcut(s: &str) -> Result<Option<KeyboardShortcut>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }

    let (mods_part, key_part) = if s == "+" {
        ("", "+")
    } else if let Some(stripped) = s.strip_suffix("++") {
        (stripped, "+")
    } else if let Some((m, k)) = s.rsplit_once('+') {
        (m, k)
    } else {
        ("", s)
    };

    let mut modifiers = Modifiers::NONE;
    if !mods_part.is_empty() {
        for mod_str in mods_part.split('+') {
            let m = mod_str.trim().to_ascii_lowercase();
            match m.as_str() {
                "ctrl" | "control" | "cmd" | "command" | "super" | "meta" => {
                    modifiers.command = true;
                }
                "alt" | "opt" | "option" => {
                    modifiers.alt = true;
                }
                "shift" => {
                    modifiers.shift = true;
                }
                _ => return Err(format!("unknown modifier {mod_str:?} in shortcut {s:?}")),
            }
        }
    }

    let key_str = key_part.trim();
    if key_str.is_empty() {
        return Err(format!("missing key in shortcut {s:?}"));
    }

    let key = Key::from_name(key_str)
        .ok_or_else(|| format!("unknown key {key_str:?} in shortcut {s:?}"))?;

    Ok(Some(KeyboardShortcut::new(modifiers, key)))
}

#[derive(Debug, Deserialize, Default)]
struct HotkeysFile {
    #[serde(default)]
    commands: BTreeMap<String, String>,
}

pub(crate) fn parse_file(s: &str) -> (Hotkeys, Vec<String>) {
    let file: HotkeysFile = match toml::from_str(s) {
        Ok(f) => f,
        Err(e) => {
            return (
                Hotkeys::default(),
                vec![format!("Hotkeys: failed to parse hotkeys.toml: {e}")],
            );
        }
    };

    let mut command_overrides = HashMap::new();
    let mut warnings = Vec::new();

    for (key, val) in &file.commands {
        match Command::from_name(key.trim()) {
            Some(cmd) => match parse_shortcut(val) {
                Ok(sc) => {
                    command_overrides.insert(cmd, sc);
                }
                Err(_) => {
                    warnings.push(format!(
                        "Hotkeys: invalid shortcut '{val}' for command '{key}'"
                    ));
                }
            },
            None => {
                warnings.push(format!("Hotkeys: unknown command '{key}'"));
            }
        }
    }

    let hotkeys = Hotkeys::from_overrides(command_overrides);
    hotkeys.check_clashes(&mut warnings);

    (hotkeys, warnings)
}

fn config_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(dir.join("omacull"))
}

pub(crate) fn default_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("hotkeys.toml"))
}

/// Load `hotkeys.toml` from the config folder, if there is one. Returns
/// warnings about problems in it, which leave the defaults in place.
pub(crate) fn load() -> Vec<String> {
    let Some(path) = default_path() else {
        return Vec::new();
    };
    let text = match std::fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return vec![format!("Hotkeys: failed to read hotkeys.toml: {e}")],
        Ok(text) => text,
    };
    // With no file yet (or an empty one), write one listing everything that
    // can be changed, all commented out, so there's something to edit.
    if text.trim().is_empty() {
        let written = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&path, template()));
        if let Err(e) = written {
            log::warn!("Hotkeys: couldn't write {}: {e}", path.display());
        }
        return Vec::new();
    }
    let (hotkeys, warnings) = parse_file(&text);
    let _ = HOTKEYS.set(hotkeys);
    warnings
}

/// A `hotkeys.toml` listing every command with its default shortcut, all
/// commented out.
fn template() -> String {
    let quoted = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let mut out = String::from(
        "# Omacull keyboard shortcuts.\n\
         #\n\
         # Every command is listed with its default shortcut, which Omacull uses\n\
         # unless you change it here. To change one, remove the # at the start of\n\
         # its line and edit the shortcut; \"\" removes it. Restart Omacull to\n\
         # apply. Problems are shown in the status bar at startup.\n\
         #\n\
         # A shortcut is Ctrl, Alt and Shift joined to a key with +, such as\n\
         # \"Ctrl+Shift+Z\", \"X\", \"Space\", \"F7\" or \"Right\".\n\
         #\n\
         # 0 also unmarks, unless you give 0 to another command.\n\n[commands]\n",
    );
    for &cmd in Command::ALL {
        let shortcut = cmd.default_shortcut().map_or(String::new(), |s| format_shortcut(&s));
        out.push_str(&format!("# {cmd:?} = {}  # {}\n", quoted(&shortcut), cmd.label()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_template_lists_every_default_and_reads_back_as_them() {
        let text = template();
        let (hotkeys, warnings) = parse_file(&text);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(hotkeys, Hotkeys::default());
        // Uncommented, every line reads back as the default it shows.
        let uncommented: String = text
            .lines()
            .map(|l| l.strip_prefix("# ").filter(|l| l.contains(" = ")).unwrap_or(l))
            .map(|l| format!("{l}\n"))
            .collect();
        let (hotkeys, warnings) = parse_file(&uncommented);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(hotkeys, Hotkeys::default());
        for &cmd in Command::ALL {
            assert!(text.contains(&format!("# {cmd:?} = ")), "{cmd:?} missing");
        }
    }

    #[test]
    fn parse_shortcuts_modifiers_functions_symbols_and_empty() {
        let s = parse_shortcut("Ctrl+Shift+Z").unwrap().unwrap();
        assert_eq!(
            s.modifiers,
            Modifiers {
                shift: true,
                ..Modifiers::COMMAND
            }
        );
        assert_eq!(s.logical_key, Key::Z);

        let s = parse_shortcut("x").unwrap().unwrap();
        assert_eq!(s.modifiers, Modifiers::NONE);
        assert_eq!(s.logical_key, Key::X);

        let s = parse_shortcut("Shift+F6").unwrap().unwrap();
        assert_eq!(s.modifiers, Modifiers::SHIFT);
        assert_eq!(s.logical_key, Key::F6);

        let s = parse_shortcut("Ctrl++").unwrap().unwrap();
        assert_eq!(s.modifiers, Modifiers::COMMAND);
        assert_eq!(s.logical_key, Key::Plus);

        // Empty string
        assert_eq!(parse_shortcut("").unwrap(), None);
        assert_eq!(parse_shortcut("   ").unwrap(), None);

        // Errors
        assert!(parse_shortcut("Ctrl+").is_err());
        assert!(parse_shortcut("InvalidMod+K").is_err());
        assert!(parse_shortcut("Ctrl+NonExistentKey").is_err());
    }

    #[test]
    fn clash_detection_reported() {
        let toml = r#"
[commands]
Pick = "X"
"#;
        let (_, warnings) = parse_file(toml);
        assert!(
            warnings.iter().any(|w| w.contains("clash between commands Reject and Pick") && w.contains("shortcut X")),
            "expected clash on X, got: {warnings:?}"
        );
    }

    #[test]
    fn unknown_names_reported() {
        let toml = r#"
[commands]
MadeUpCommand = "Ctrl+K"
"#;
        let (_, warnings) = parse_file(toml);
        assert!(
            warnings.iter().any(|w| w.contains("unknown command 'MadeUpCommand'")),
            "got: {warnings:?}"
        );
    }

    #[test]
    fn overrides_applied_to_matching() {
        let toml = r#"
[commands]
Reject = "Ctrl+Shift+X"
Next = "Space"
Pick = ""
"#;
        let (hotkeys, warnings) = parse_file(toml);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(hotkeys.command(Command::Pick), None);
        assert_eq!(hotkeys.command(Command::Undo), Command::Undo.default_shortcut());
        // With more modifiers than Ctrl+Z now, Reject is checked before Undo.
        let order = hotkeys.keyboard_order();
        let pos = |c| order.iter().position(|&x| x == c).unwrap();
        assert!(pos(Command::Reject) < pos(Command::Undo));

        let ctx = egui::Context::default();
        let press = |modifiers: Modifiers, key: Key| -> Vec<Command> {
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
            let mut out = ctx.run_ui(input, |ui| {
                pressed = Command::pressed_with(ui.ctx(), &hotkeys);
            });
            out.textures_delta.clear();
            pressed
        };
        // 0 still unmarks, until it's given to something else.
        assert_eq!(press(Modifiers::NONE, Key::Num0), [Command::Unmark]);
        let (zero_is_next, _) = parse_file("[commands]\nNext = \"0\"\n");
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: Key::Num0,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
            ..Default::default()
        };
        let mut pressed = Vec::new();
        let mut out = ctx.run_ui(input, |ui| pressed = Command::pressed_with(ui.ctx(), &zero_is_next));
        out.textures_delta.clear();
        assert_eq!(pressed, [Command::Next]);
        // The old shortcuts do nothing; the new ones work.
        assert_eq!(press(Modifiers::NONE, Key::X), []);
        assert_eq!(press(Modifiers::NONE, Key::P), []);
        assert_eq!(press(Modifiers::COMMAND | Modifiers::SHIFT, Key::X), [Command::Reject]);
        assert_eq!(press(Modifiers::NONE, Key::Space), [Command::Next]);
    }
}
