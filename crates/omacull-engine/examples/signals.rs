//! What Omacull measures of each frame of a folder, to check the signals
//! against a real shoot:
//!
//!     cargo run --release -p omacull-engine --example signals -- ~/Pictures/shoot
//!
//! Faces come from Omacull's cache (`~/.cache/omacull/faces`), so open the
//! folder in Omacull first; without them only the focus point and the
//! clipping are measured. Nothing is written.

use std::path::PathBuf;

use omacull_engine::cull::Cull;
use omacull_engine::{faces, image, signals};
use rayon::prelude::*;

fn main() {
    let dir = PathBuf::from(std::env::args_os().nth(1).expect("usage: signals <folder>"));
    let (cull, _) = Cull::open(&dir).expect("open the folder");
    let cache = faces::default_dir();
    let measured: Vec<_> = cull
        .frames()
        .par_iter()
        .map(|frame| {
            let (preview, _, info) = image::preview_with_info(&frame.path).ok()?;
            let found = cache.as_deref().and_then(|dir| faces::load(&frame.path, dir));
            Some((signals::measure(&preview, info.focus, found.as_deref().unwrap_or(&[])), found.is_some()))
        })
        .collect();
    let number = |v: Option<f32>| v.map_or("    -".to_owned(), |v| format!("{v:5.1}"));
    println!("{:<16} {:>5} {:>5} {:>5} {:>6} {:>6} faces", "frame", "focus", "eyes", "open", "blown", "black");
    for (frame, measured) in cull.frames().iter().zip(&measured) {
        let name = frame.path.file_name().unwrap_or_default().to_string_lossy();
        let Some((s, looked)) = measured else {
            println!("{name:<16} no preview");
            continue;
        };
        let open = s.open.map_or("    -".to_owned(), |v| format!("{v:5.2}"));
        let faces = if *looked { s.faces.to_string() } else { "?".into() };
        let shut = if s.shut() { "  shut?" } else { "" };
        let (blown, black) = (s.highlights * 100.0, s.shadows * 100.0);
        println!("{name:<16} {} {} {open} {blown:5.1}% {black:5.1}% {faces}{shut}", number(s.focus), number(s.eyes));
    }
}
