//! Set a raw's rating in its sidecar, as the app will, to check by hand
//! what darktable makes of it (-1 rejects, 0 clears, 1 to 5 are stars):
//!
//!     cargo run -p omacull-engine --example rate -- DSC01234.ARW 3

use std::path::PathBuf;

use omacull_engine::sidecar;

fn main() -> std::io::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let usage = "usage: rate <raw> <rating>";
    let raw = PathBuf::from(args.next().expect(usage));
    let rating: i32 = args.next().expect(usage).to_string_lossy().parse().expect(usage);
    let before = sidecar::read(&raw)?;
    sidecar::write(&raw, rating)?;
    println!("{}: {before:?} -> {:?}", sidecar::path_for(&raw).display(), sidecar::read(&raw)?);
    Ok(())
}
