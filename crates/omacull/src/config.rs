//! The few settings Omacull has (`~/.config/omacull/config.toml`): how
//! marks are written for raw developers other than darktable, and how sure
//! the model has to be to suggest one. Shortcuts are in `hotkeys.toml`.
//!
//! ```toml
//! pick = 3
//! sidecar = "adobe"
//! developer = "rawtherapee"
//! confidence = 0.9
//! ```

use omacull_engine::cull::{PICK, Rating};
use omacull_engine::sidecar::Naming;
use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// The stars a pick is written as, 1 to 5.
    pub pick: Rating,
    /// How sidecars are named.
    pub sidecar: Naming,
    /// The program a folder is handed to.
    pub developer: String,
    /// How sure the model has to be to suggest a mark, 0.5 to 0.99.
    pub confidence: f32,
}

impl Default for Config {
    /// darktable's ways.
    fn default() -> Self {
        Self { pick: PICK, sidecar: Naming::Darktable, developer: "darktable".into(), confidence: 0.8 }
    }
}

impl Config {
    /// What `text` sets, with the defaults for the rest and for anything
    /// that can't be used, and what was wrong with it.
    pub fn parse(text: &str) -> (Self, Vec<String>) {
        let mut config: Self = match toml::from_str(text) {
            Ok(config) => config,
            Err(e) => return (Self::default(), vec![format!("Config: failed to parse config.toml: {e}")]),
        };
        let mut warnings = Vec::new();
        if !(1..=5).contains(&config.pick) {
            warnings.push(format!("Config: pick = {} isn't 1 to 5", config.pick));
            config.pick = PICK;
        }
        if config.developer.trim().is_empty() {
            warnings.push("Config: developer is empty".into());
            config.developer = Self::default().developer;
        }
        if !(0.5..=0.99).contains(&config.confidence) {
            warnings.push(format!("Config: confidence = {} isn't 0.5 to 0.99", config.confidence));
            config.confidence = Self::default().confidence;
        }
        (config, warnings)
    }

    /// Read `config.toml` from the config folder, writing one that lists
    /// every setting, commented out, if there's none. Returns warnings
    /// about problems in it.
    pub fn load() -> (Self, Vec<String>) {
        let Some(path) = crate::hotkeys::config_dir().map(|dir| dir.join("config.toml")) else {
            return (Self::default(), Vec::new());
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let written = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&path, TEMPLATE));
                if let Err(e) = written {
                    log::warn!("Config: couldn't write {}: {e}", path.display());
                }
                (Self::default(), Vec::new())
            }
            Err(e) => (Self::default(), vec![format!("Config: failed to read config.toml: {e}")]),
        }
    }
}

const TEMPLATE: &str = "\
# Omacull's settings. Each is listed with its default, which Omacull uses
# unless you change it here: remove the # at the start of its line and edit
# the value. Restart Omacull to apply. Problems are shown in the status bar
# at startup. Shortcuts are in hotkeys.toml.

# For raw developers other than darktable.
#
# The stars a pick (P, and a winner) is written as, 1 to 5. darktable has no
# pick flag, so a pick is one star; if one star means something else where
# your raws go next, use another.
# pick = 1
#
# How sidecars are named: \"darktable\" for DSC01234.ARW.xmp, or \"adobe\" for
# DSC01234.xmp, which Lightroom, Bridge, Capture One and most others read,
# and LightCraft writes.
# sidecar = \"darktable\"
#
# The program the folder is handed to (Ctrl+E), as `program folder`.
# \"lightcraft\" is handed the pictures shown instead, not the folder.
# developer = \"darktable\"

# How sure the model trained on your decisions has to be before it suggests
# a mark, from 0.5 to 0.99. Higher: fewer suggestions, more of them right.
# confidence = 0.8
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_template_lists_every_default_and_reads_back_as_them() {
        assert_eq!(Config::parse(TEMPLATE), (Config::default(), Vec::new()));
        // Uncommented, every setting reads back as the default it shows.
        let setting = |l: &'static str| {
            l.strip_prefix("# ").filter(|l| l.split_once(" = ").is_some_and(|(name, _)| !name.contains(' ')))
        };
        let uncommented: String = TEMPLATE.lines().map(|l| format!("{}\n", setting(l).unwrap_or(l))).collect();
        assert_eq!(Config::parse(&uncommented), (Config::default(), Vec::new()), "{uncommented}");
        for setting in ["pick", "sidecar", "developer", "confidence"] {
            assert!(uncommented.contains(&format!("\n{setting} = ")), "{setting} missing");
        }
    }

    #[test]
    fn settings_are_read_and_bad_ones_reported() {
        let text = "pick = 3\nsidecar = \"adobe\"\ndeveloper = \"rawtherapee\"\nconfidence = 0.9\n";
        let config = Config { pick: 3, sidecar: Naming::Adobe, developer: "rawtherapee".into(), confidence: 0.9 };
        assert_eq!(Config::parse(text), (config, Vec::new()));

        let (config, warnings) = Config::parse("pick = 0\nconfidence = 2.0\ndeveloper = \" \"\n");
        assert_eq!(config, Config::default());
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        let (config, warnings) = Config::parse("picks = 3\n");
        assert_eq!(config, Config::default());
        assert!(warnings[0].contains("failed to parse"), "{warnings:?}");
    }
}
