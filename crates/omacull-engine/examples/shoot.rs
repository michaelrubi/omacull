//! A made-up shoot, for trying Omacull without a real one, and for its
//! screenshots: bursts of a few scenes, each with one frame sharper than
//! the rest, some of them marked already.
//!
//!     cargo run --release -p omacull-engine --example shoot --features testing -- /tmp/shoot
//!
//! The raws hold a preview and a thumbnail and no raw data, so 100% zoom
//! shows the preview enlarged.

use std::path::PathBuf;

use omacull_engine::image::Image;
use omacull_engine::sidecar;
use omacull_engine::testing::Arw;

const WIDTH: usize = 1616;
const HEIGHT: usize = 1080;

/// Each scene's sky at the top and at the horizon, and its nearest hills.
const SCENES: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
    ([40.0, 90.0, 170.0], [250.0, 190.0, 120.0], [30.0, 60.0, 40.0]),
    ([20.0, 30.0, 70.0], [240.0, 120.0, 90.0], [25.0, 30.0, 45.0]),
    ([110.0, 160.0, 220.0], [235.0, 235.0, 225.0], [50.0, 80.0, 45.0]),
    ([60.0, 50.0, 110.0], [250.0, 160.0, 150.0], [45.0, 35.0, 50.0]),
    ([30.0, 110.0, 150.0], [200.0, 230.0, 210.0], [20.0, 70.0, 60.0]),
    ([90.0, 70.0, 60.0], [250.0, 210.0, 140.0], [60.0, 45.0, 30.0]),
];

/// The same every time, and different for every pixel.
fn noise(x: usize, y: usize) -> f32 {
    let n = (x as u32).wrapping_mul(374_761_393) ^ (y as u32).wrapping_mul(668_265_263);
    let n = (n ^ (n >> 13)).wrapping_mul(1_274_126_177);
    ((n ^ (n >> 16)) & 0xff) as f32 / 255.0
}

/// A landscape of sorts: a sky, a sun, three ranges of hills and grass in
/// front. `shift` moves the camera a little; `soft` blurs it, as a missed
/// focus would.
fn picture(scene: usize, shift: f32, soft: usize) -> Image {
    let (top, horizon, near) = SCENES[scene % SCENES.len()];
    let s = scene as f32;
    let sun = (WIDTH as f32 * (0.25 + 0.1 * s) + shift, HEIGHT as f32 * (0.3 + 0.03 * s));
    let mut rgba = vec![255u8; WIDTH * HEIGHT * 4];
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let (fx, fy) = (x as f32 + shift, y as f32);
            let down = fy / (HEIGHT as f32 * 0.7);
            let mut colour: [f32; 3] = std::array::from_fn(|c| top[c] + (horizon[c] - top[c]) * down.min(1.0));
            let from_sun = (fx - sun.0).hypot(fy - sun.1);
            let glow = if from_sun < 46.0 { 1.0 } else { (120.0 / from_sun).powi(2).min(0.6) };
            colour = colour.map(|c| c + (255.0 - c) * glow);
            // Further ranges are paler; the nearest is grass.
            for range in 0..3 {
                let r = range as f32;
                let line = HEIGHT as f32 * (0.52 + 0.13 * r)
                    + 40.0 * (fx * (0.004 + 0.002 * r) + s * 1.7 + r * 2.1).sin()
                    + 14.0 * (fx * (0.013 + 0.004 * r) + s + r).sin();
                if fy > line {
                    let haze = 0.55 - 0.25 * r;
                    colour = std::array::from_fn(|c| near[c] + (horizon[c] - near[c]) * haze);
                    if range == 2 {
                        let blade = noise(x + shift as usize, y / 3) * 70.0 - 20.0;
                        colour = colour.map(|c| c + blade);
                    }
                }
            }
            let at = (y * WIDTH + x) * 4;
            for c in 0..3 {
                rgba[at + c] = colour[c].clamp(0.0, 255.0) as u8;
            }
        }
    }
    let mut image = Image { width: WIDTH, height: HEIGHT, rgba };
    // Shrunk and enlarged again, it's out of focus.
    if soft > 1 {
        let small = image.shrunk(WIDTH / soft);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let from = small.pixel((x / soft).min(small.width - 1), (y / soft).min(small.height - 1));
                image.rgba[(y * WIDTH + x) * 4..][..4].copy_from_slice(&from);
            }
        }
    }
    image
}

fn main() {
    let dir = PathBuf::from(std::env::args_os().nth(1).expect("usage: shoot <folder>"));
    std::fs::create_dir_all(&dir).expect("make the folder");
    // How soft each frame of each scene's burst is, and the marks a few
    // have already.
    let bursts: [&[usize]; 6] = [&[2, 1, 3, 2], &[1], &[3, 2, 1], &[1, 4], &[2, 2, 1, 3, 2], &[1]];
    let marked = [(5, 3), (6, -1), (7, -1), (8, 1), (10, -1), (16, 4)];
    let mut number = 0;
    for (scene, burst) in bursts.iter().enumerate() {
        for (k, &soft) in burst.iter().enumerate() {
            number += 1;
            let preview = picture(scene, k as f32 * 6.0, soft);
            let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
            let arw = Arw {
                preview: preview.encode_jpeg(88).expect("preview"),
                thumbnail: preview.shrunk(160).encode_jpeg(80).expect("thumbnail"),
                // Focused on the grass in front.
                focus: [6000, 4000, 3000, 3500],
                captured: (leak(format!("2026:10:04 12:{scene:02}:00")), leak(format!("{}", k * 250))),
                ..Arw::default()
            };
            let path = dir.join(format!("DSC{number:05}.ARW"));
            arw.write(&path);
            if let Some(&(_, rating)) = marked.iter().find(|&&(n, _)| n == number) {
                sidecar::write(&path, rating).expect("mark");
            }
        }
    }
    println!("{number} raws in {}", dir.display());
}
