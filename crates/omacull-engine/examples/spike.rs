//! The M0 questions, asked of a real folder of raws:
//!
//!     cargo run --release -p omacull-engine --example spike -- ~/Pictures/shoot [frames]
//!
//! What the raws embed, whether the focus location is there, and how long a
//! frame takes to get from disk to pixels. Run it twice for cold and warm
//! numbers; to make it cold again without root, evict the files first:
//!
//!     for f in ~/Pictures/shoot/*.ARW; do dd if="$f" iflag=nocache count=0 status=none; done

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use omacull_engine::raw::RawFile;
use rayon::prelude::*;
use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

/// Decode to RGBA, as it would be uploaded to the GPU.
fn decode(jpeg: &[u8]) -> (usize, usize, Vec<u8>) {
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(jpeg), options);
    let pixels = decoder.decode().expect("decode");
    let info = decoder.info().expect("info");
    (usize::from(info.width), usize::from(info.height), pixels)
}

fn size(jpeg: &[u8]) -> (usize, usize) {
    let mut decoder = JpegDecoder::new(ZCursor::new(jpeg));
    decoder.decode_headers().expect("headers");
    let info = decoder.info().expect("info");
    (usize::from(info.width), usize::from(info.height))
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Median and worst of some timings.
fn summary(mut times: Vec<Duration>) -> String {
    times.sort();
    format!("median {:.2} ms, worst {:.2} ms", ms(times[times.len() / 2]), ms(times[times.len() - 1]))
}

fn main() {
    let mut args = std::env::args_os().skip(1);
    let dir = PathBuf::from(args.next().expect("usage: spike <folder> [frames]"));
    let frames: usize = args.next().map_or(500, |n| n.to_string_lossy().parse().expect("frames"));
    let mut raws: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("read folder")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("arw")))
        .collect();
    raws.sort();
    raws.truncate(frames);
    println!("{} raws in {}", raws.len(), dir.display());

    // Thumbnails first, while the files are as cold as they'll be: every
    // frame's camera thumbnail, on all cores, as the filmstrip would ask.
    let start = Instant::now();
    let thumbs: usize = raws
        .par_iter()
        .map(|path| {
            let raw = RawFile::open(path).expect("open");
            decode(&raw.read(raw.thumbnail().expect("thumbnail")).expect("read")).2.len()
        })
        .sum();
    println!("thumbnails, all cores: {:.0} ms for the folder ({} MB of pixels)", ms(start.elapsed()), thumbs >> 20);

    // Then each frame's preview, one at a time, as stepping would.
    let (mut opens, mut reads, mut decodes) = (Vec::new(), Vec::new(), Vec::new());
    let mut sizes: BTreeMap<Vec<(usize, usize)>, usize> = BTreeMap::new();
    let (mut focused, mut bytes) = (0, 0);
    let start = Instant::now();
    for path in &raws {
        let t = Instant::now();
        let raw = RawFile::open(path).expect("open");
        opens.push(t.elapsed());
        let t = Instant::now();
        let jpeg = raw.read(raw.preview().expect("preview")).expect("read");
        reads.push(t.elapsed());
        let t = Instant::now();
        let (w, h, _) = decode(&jpeg);
        decodes.push(t.elapsed());
        assert!(w > 0 && h > 0);
        bytes += jpeg.len();
        focused += usize::from(raw.focus.is_some());
        let all = raw.jpegs.iter().map(|&j| size(&raw.read(j).expect("read"))).collect();
        *sizes.entry(all).or_default() += 1;
    }
    let total = start.elapsed();
    println!("previews, one core: {:.0} ms for the folder, {:.2} ms a frame", ms(total), ms(total) / raws.len() as f64);
    println!("  open and find:  {}", summary(opens));
    println!("  read preview:   {} ({} KB average)", summary(reads), bytes / raws.len() / 1024);
    println!("  decode to RGBA: {}", summary(decodes));

    let start = Instant::now();
    raws.par_iter().for_each(|path| {
        let raw = RawFile::open(path).expect("open");
        decode(&raw.read(raw.preview().expect("preview")).expect("read"));
    });
    println!("previews, all cores: {:.0} ms for the folder", ms(start.elapsed()));

    println!("embedded JPEGs:");
    for (sizes, count) in &sizes {
        let sizes: Vec<String> = sizes.iter().map(|(w, h)| format!("{w}×{h}")).collect();
        println!("  {count} raws: {}", sizes.join(", "));
    }
    println!("focus location: {focused} of {} raws", raws.len());
    if let Some(raw) = raws.iter().filter_map(|p| RawFile::open(p).ok()).find(|r| r.focus.is_some()) {
        println!("  e.g. {:?}, orientation {}", raw.focus.unwrap(), raw.orientation);
    }
}
