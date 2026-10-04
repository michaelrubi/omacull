//! The folder tree beside the loupe: the folders round the open one, with
//! how many raws each holds, read in the background as they're opened out.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::{Align2, FontId, Rect, RichText, Sense, Ui, pos2, vec2};
use omacull_engine::folders::{self, Folder};

use crate::theme::Theme;

pub const WIDTH: f32 = 260.0;
const ROW: f32 = 22.0;
const INDENT: f32 = 14.0;

enum Listing {
    Reading,
    Read(Vec<Folder>),
    Failed(String),
}

pub struct Tree {
    /// The folder at the top; ↑ goes to the one above.
    pub root: PathBuf,
    listings: HashMap<PathBuf, Listing>,
    expanded: HashSet<PathBuf>,
    tx: Sender<(PathBuf, io::Result<Vec<Folder>>)>,
    rx: Receiver<(PathBuf, io::Result<Vec<Folder>>)>,
    ctx: egui::Context,
    /// The rows where they were last drawn, for tests.
    pub rows: Vec<(PathBuf, Rect)>,
}

/// Where the tree starts with nothing open: `~/Pictures`, else home.
fn default_root() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
    let pictures = home.join("Pictures");
    if pictures.is_dir() { pictures } else { home }
}

impl Tree {
    pub fn new(open: Option<&Path>, ctx: &egui::Context) -> Self {
        let (tx, rx) = channel();
        let mut tree = Self {
            root: default_root(),
            listings: HashMap::new(),
            expanded: HashSet::new(),
            tx,
            rx,
            ctx: ctx.clone(),
            rows: Vec::new(),
        };
        if let Some(open) = open {
            tree.show(open);
        }
        tree
    }

    /// Make sure `open` is in the tree, opened out to: rooted at the folder
    /// above it unless it's already under the root.
    pub fn show(&mut self, open: &Path) {
        if !open.starts_with(&self.root) || open == self.root {
            self.root = open.parent().unwrap_or(open).to_path_buf();
        }
        let mut dir = open.parent();
        while let Some(d) = dir.filter(|d| d.starts_with(&self.root) && *d != self.root) {
            self.expanded.insert(d.to_path_buf());
            dir = d.parent();
        }
    }

    /// Read again: what's in the folders may have changed.
    pub fn refresh(&mut self) {
        self.listings.clear();
    }

    fn listing(&mut self, dir: &Path) -> &Listing {
        if !self.listings.contains_key(dir) {
            self.listings.insert(dir.to_path_buf(), Listing::Reading);
            let (tx, ctx, dir) = (self.tx.clone(), self.ctx.clone(), dir.to_path_buf());
            std::thread::spawn(move || {
                let listed = folders::list(&dir);
                let _ = tx.send((dir, listed));
                ctx.request_repaint();
            });
        }
        &self.listings[dir]
    }

    #[cfg(test)]
    /// Whether a folder is still being read.
    pub fn reading(&self) -> bool {
        self.listings.values().any(|l| matches!(l, Listing::Reading))
    }

    /// The tree, with `open` marked. Returns the folder clicked, to open.
    pub fn ui(&mut self, ui: &mut Ui, theme: &Theme, open: Option<&Path>) -> Option<PathBuf> {
        for (dir, listed) in self.rx.try_iter() {
            let listing = match listed {
                Ok(folders) => Listing::Read(folders),
                Err(e) => Listing::Failed(e.to_string()),
            };
            self.listings.insert(dir, listing);
        }
        let mut clicked = None;
        ui.horizontal(|ui| {
            if ui.small_button("↑").on_hover_text("The folder above").clicked()
                && let Some(parent) = self.root.parent()
            {
                self.root = parent.to_path_buf();
            }
            let whole = self.root.display().to_string();
            let name = self.root.file_name().map_or_else(|| whole.clone(), |n| n.to_string_lossy().into_owned());
            ui.label(RichText::new(name).strong().color(theme.foreground)).on_hover_text(whole);
        });
        ui.separator();
        self.rows.clear();
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let root = self.root.clone();
            self.folder(ui, &root, 0, theme, open, &mut clicked);
        });
        clicked
    }

    fn folder(
        &mut self,
        ui: &mut Ui,
        dir: &Path,
        depth: usize,
        theme: &Theme,
        open: Option<&Path>,
        clicked: &mut Option<PathBuf>,
    ) {
        let folders = match self.listing(dir) {
            Listing::Reading => {
                ui.label(RichText::new("…").color(theme.dark_foreground));
                return;
            }
            Listing::Failed(e) => {
                ui.label(RichText::new(e.clone()).color(theme.red));
                return;
            }
            Listing::Read(folders) => folders.clone(),
        };
        for folder in folders {
            let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
            self.rows.push((folder.path.clone(), rect));
            let painter = ui.painter();
            let here = open == Some(folder.path.as_path());
            if here {
                painter.rect_filled(rect, 0.0, theme.selection);
            } else if response.hovered() {
                painter.rect_filled(rect, 0.0, theme.lighter_background);
            }
            let left = rect.left() + 4.0 + depth as f32 * INDENT;
            let expanded = self.expanded.contains(&folder.path);
            let arrow = Rect::from_min_size(pos2(left, rect.top()), vec2(INDENT, ROW));
            if folder.nested {
                let glyph = if expanded { "▾" } else { "▸" };
                let font = FontId::proportional(12.0);
                painter.text(arrow.center(), Align2::CENTER_CENTER, glyph, font, theme.dark_foreground);
            }
            let colour = match () {
                () if here => theme.accent,
                () if folder.raws > 0 => theme.foreground,
                () => theme.dark_foreground,
            };
            let name_at = pos2(arrow.right() + 2.0, rect.center().y);
            painter.text(name_at, Align2::LEFT_CENTER, &folder.name, FontId::proportional(13.0), colour);
            if folder.raws > 0 {
                let count_at = pos2(rect.right() - 6.0, rect.center().y);
                let count = folder.raws.to_string();
                painter.text(count_at, Align2::RIGHT_CENTER, count, FontId::proportional(11.0), theme.dark_foreground);
            }
            if response.clicked() {
                let on_arrow = response.interact_pointer_pos().is_some_and(|p| p.x < arrow.right());
                if folder.nested && (on_arrow || folder.raws == 0) {
                    if !self.expanded.remove(&folder.path) {
                        self.expanded.insert(folder.path.clone());
                    }
                } else {
                    *clicked = Some(folder.path.clone());
                }
            }
            if folder.nested && self.expanded.contains(&folder.path) {
                self.folder(ui, &folder.path, depth + 1, theme, open, clicked);
            }
        }
    }
}
