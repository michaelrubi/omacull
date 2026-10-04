# AGENTS.md

Omacull is a fast, keyboard-first photo culler for Omarchy: a folder of raws in, XMP ratings out, then on to darktable. It is the first stage of the workflow Omapix (`~/dev/omapix`) finishes.

**Status:** M8 built, which is the whole roadmap: M1's culling (step, mark, filter, undo, sidecars, decision log), M2's inspection (100% zoom on a full development of the raw, histogram, clipping, focus peaking, focus point, shooting settings, colour management), compare and survey with multi-select, M4's folder tree, cull summary, darktable handoff and per-folder places, M5's face and eye detection (`omacull-ai`, YuNet on the system's ONNX Runtime) with zoom to the eyes and face close-ups, M6's stacks (by hand, by time, or by time and look) with a winner chosen in one key, M7's signals (sharpness, shut eyes, clipping), suggested stack winners and a model trained locally on the decision log, and M8's Arch package, NEF/CR3/RAF reading, `config.toml` and docs. Waiting on a hand test with a real shoot. Read [docs/DESIGN.md](docs/DESIGN.md) for scope and architecture and [docs/ROADMAP.md](docs/ROADMAP.md) for the milestones, what each still wants trying, and the "Later" lists.

## Principles

- **Culling only**: looking at frames and marking them. Everything else has to argue its way in.
- **Never wait for a frame**: embedded camera JPEG as the preview, neighbours prefetched.
- **Keyboard-first**: Bridge/Lightroom conventions (P/X/U, 0-5, arrows).
- **The files are the database**: marks live in darktable-compatible XMP sidecars. Raws are never moved, modified or deleted.
- **Engine/UI separation**: `omacull-engine` has no UI or GPU dependencies and is headlessly testable.

## Conventions

- Same stack and conventions as Omapix: Rust 2024, egui on wgpu, Little CMS 2, GPL-3.0-or-later. When in doubt about theme, hotkeys, the `Command` pattern, the headless `Harness` tests or packaging, look at how Omapix does it and copy that.
- Keep diffs minimal and surgical. No speculative abstractions or unnecessary dependencies.
- Sidecar writes must leave everything except the rating byte-for-byte intact. Test against real darktable-written sidecars.
- Suggestions never mark anything: the user takes one or doesn't, and either way it's logged.
- Only Sony ARW has been read from a real camera. NEF, CR3 and RAF are tested on made-up files (`omacull_engine::testing`); say so wherever they come up.
- Hand testing: Michael tests from the installed binary, not `cargo run`. After a change he'll try by hand, run `make install` and ask him to restart Omacull.
- Whenever non-trivial UI or engine logic is added, write a headless test. In `omacull`, use the `Harness` in `app.rs` (driving egui with synthetic events). Fake raws with real embedded JPEGs come from `omacull_engine::testing` (the `testing` feature).

## Build and test commands

```bash
# Run unit tests (engine, and the app's headless UI tests)
cargo test

# Lint: no warnings
cargo clippy --workspace --all-targets --features omacull-engine/testing

# Build release binary
cargo build --release

# Build and install to ~/.local (the copy Michael actually runs), with
# its launcher entry and icon
make install

# Run the app, with a folder of raws (or a raw, to start at it)
cargo run --release -- path/to/shoot

# Drive the UI without a keyboard: comma-separated Command names
OMACULL_SCRIPT="Next,Pick,Quit" cargo run --release

# What a folder of raws embeds, how fast it decodes, and how fast five
# frames develop at full size for 100% zoom (written to /tmp as .ppm)
cargo run --release -p omacull-engine --example spike -- path/to/shoot

# Fetch the face models (YuNet, MediaPipe's landmarker) into
# ~/.local/share/omacull/models; not needed where Omapix has them
scripts/fetch-models.sh

# What's measured of each frame of a folder: sharpness, shut eyes, clipping
# (faces come from Omacull's cache: open the folder in Omacull first)
cargo run --release -p omacull-engine --example signals -- path/to/shoot

# A made-up shoot, for trying things without a real one
cargo run --release -p omacull-engine --example shoot --features testing -- /tmp/shoot

# The README's screenshots, of the made-up shoot or a folder (Hyprland, grim;
# it takes the pointer for a moment)
scripts/screenshots.sh [path/to/shoot]

# The Arch package
cd packaging/arch && makepkg -si

# Find faces in a photo with the real ONNX Runtime and model (skipped
# without OMACULL_FACES_PHOTO)
OMACULL_FACES_PHOTO=people.jpg cargo test -p omacull-ai finds_faces -- --nocapture

# Write a rating to a raw's sidecar the way the app will (-1 rejects)
cargo run -p omacull-engine --example rate -- path/to/DSC01234.ARW 3
```
