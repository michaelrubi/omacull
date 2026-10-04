//! What Omacull remembers between sessions (`~/.config/omacull/state.toml`):
//! the folders opened recently, whether marks auto-advance, and what the
//! loupe shows over a frame.

use std::path::{Path, PathBuf};

const MAX_RECENT: usize = 10;

/// What the loupe shows over a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Show {
    pub histogram: bool,
    /// The shooting settings.
    pub info: bool,
    pub clipping: bool,
    pub peaking: bool,
    pub focus_point: bool,
}

impl Default for Show {
    /// Only the shooting settings, a line under the frame.
    fn default() -> Self {
        Self { histogram: false, info: true, clipping: false, peaking: false, focus_point: false }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct State {
    #[serde(default)]
    pub auto_advance: bool,
    #[serde(default)]
    pub show: Show,
    /// Most recent first.
    #[serde(default)]
    pub recent: Vec<PathBuf>,
    /// Where it's saved; nowhere in tests.
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl State {
    pub fn load() -> Self {
        crate::hotkeys::config_dir().map_or_else(Self::default, |dir| Self::load_from(dir.join("state.toml")))
    }

    /// Read from `path`, and save there from now on. Folders that have gone
    /// are forgotten.
    pub fn load_from(path: PathBuf) -> Self {
        let mut state: Self = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| toml::from_str(&text).map_err(|e| log::warn!("ignoring {}: {e}", path.display())).ok())
            .unwrap_or_default();
        state.recent.retain(|p| p.is_dir());
        state.recent.truncate(MAX_RECENT);
        state.path = Some(path);
        state
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(path, toml::to_string(self).unwrap_or_default()));
        if let Err(e) = written {
            log::warn!("couldn't save {}: {e}", path.display());
        }
    }

    pub fn add_recent(&mut self, dir: &Path) {
        let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        self.recent.retain(|p| *p != dir);
        self.recent.insert(0, dir);
        self.recent.truncate(MAX_RECENT);
        self.save();
    }

    pub fn remove_recent(&mut self, dir: &Path) {
        self.recent.retain(|p| p != dir);
        self.save();
    }

    pub fn set_auto_advance(&mut self, on: bool) {
        self.auto_advance = on;
        self.save();
    }

    pub fn set_show(&mut self, show: Show) {
        self.show = show;
        self.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omacull_engine::testing::Folder;

    #[test]
    fn recent_folders_and_auto_advance_are_remembered() {
        let folder = Folder::new("state");
        let (a, b) = (folder.0.join("a"), folder.0.join("b"));
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        let path = folder.0.join("config").join("state.toml");

        let mut state = State::load_from(path.clone());
        assert_eq!(state, State { path: Some(path.clone()), ..State::default() });
        state.add_recent(&a);
        state.add_recent(&b);
        state.add_recent(&a);
        state.set_auto_advance(true);
        state.set_show(Show { histogram: true, ..Show::default() });

        let (a, b) = (std::fs::canonicalize(a).unwrap(), std::fs::canonicalize(b).unwrap());
        let loaded = State::load_from(path.clone());
        assert_eq!(loaded.recent, [a.clone(), b.clone()], "most recent first, once each");
        assert!(loaded.auto_advance);
        assert!(loaded.show.histogram && loaded.show.info);

        std::fs::remove_dir(&b).unwrap();
        assert_eq!(State::load_from(path).recent, [a], "gone folders are forgotten");
    }
}
