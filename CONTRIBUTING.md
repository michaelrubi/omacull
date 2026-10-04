# Contributing to Omacull

Omacull is a small, opinionated project, built around one photographer's
culling and tested on one machine with one camera. That makes outside
help useful in ways that aren't all code.

Issues and pull requests go to
[github.com/michaelrubi/omacull](https://github.com/michaelrubi/omacull).

## Ways to help

- **Try it with your camera.** It's built on Sony ARW. Nikon NEF, Canon
  CR3 and Fuji RAF are read from descriptions of the formats and have
  never met a real file. Run the spike on a folder of yours and send what
  it prints, whether it works or not:

  ```bash
  cargo run --release -p omacull-engine --example spike -- path/to/shoot
  ```

  A raw that fails, if you can share one, is worth more than any report.
- **Try it on your machine.** It has only run on Omarchy (Arch, Hyprland).
  A report from another Wayland compositor, X11, another GPU or another
  distribution is worth having, even if all it says is "it works".
- **Report bugs.** See below for what to put in one.
- **Say where it differs from Bridge and Lightroom.** A marking or
  filtering key that doesn't behave as theirs does is a bug here, not a
  matter of taste.
- **Say how the signals do on your shoots.** Sharpness, shut eyes and
  clipping are measured with thresholds tried on one shoot. `signals`
  prints them for a folder:

  ```bash
  cargo run --release -p omacull-engine --example signals -- path/to/shoot
  ```
- **Package it.** There's an Arch package in
  [packaging/arch](packaging/arch), not yet on the AUR.
- **Write code.** The [roadmap](docs/ROADMAP.md) lists what's still to be
  tried, and its "Later:" lines are mostly small and self-contained.

## Reporting a bug

Open an issue with:

- what you did, what you expected, and what happened
- the commit you built (`git rev-parse --short HEAD`)
- your distribution, compositor and GPU
- your camera, and for a problem with one raw, the raw itself if you can
  share it
- for a crash or anything odd: the output of running Omacull from a
  terminal with `RUST_LOG=info omacull path/to/shoot`
- for a mark that went wrong: the sidecar before and after, if you have
  them. A sidecar changed anywhere but its rating is the worst bug Omacull
  can have.

## Before you write code

Open an issue first for anything bigger than a fix. Omacull says no to a
lot, and it's better to find that out before the work than after:

- **Culling only.** Looking at frames and marking them. No ingest, no
  renaming, no deleting, no keywords, no editing, no catalogue.
- **Never wait for a frame.** Nothing may slow stepping down.
- **Bridge and Lightroom's keys.** Where they have a key for something,
  Omacull uses it.
- **The files are the database.** Marks live in XMP sidecars. Raws are
  never moved, changed or deleted.
- **Opinionated.** Good defaults instead of settings.
- **Nothing online.** Omacull never opens a network connection. Models
  are downloaded by a script the user runs, and what it learns from the
  user stays on their machine.

[docs/DESIGN.md](docs/DESIGN.md) has these in full.

## Building and testing

You need a recent stable Rust, Little CMS 2 (`lcms2`) and Vulkan. See the
[README](README.md#install).

```bash
cargo test
```

```bash
cargo clippy --workspace --all-targets --features omacull-engine/testing
```

Both should pass with no warnings before you open a pull request. There's
no CI yet, so nothing else will check. The tests need no display, GPU,
models or raws: they make their own, small files shaped like each
camera's (`omacull_engine::testing`).

To try a change by hand, `cargo run --release -- path/to/shoot`, or
`make install` to replace the copy in `~/.local/bin`. Without a shoot to
hand, this makes one up:

```bash
cargo run --release -p omacull-engine --example shoot --features testing -- /tmp/shoot
```

`OMACULL_SCRIPT` drives the app through the same code as the keyboard,
which is useful for reproducing a bug:

```bash
OMACULL_SCRIPT="Next,Pick,Stacking,Survey" cargo run --release -- /tmp/shoot
```

`scripts/screenshots.sh` takes the README's screenshots that way.

One test needs something the repository doesn't have, a photo of people
and the models, and is skipped without them; it says what it needs in the
comment above it.

## How the code is laid out

```
crates/
  omacull-engine   raws, previews, sidecars, the cull itself, stacks,
                   signals, the decision log and what's learned from it.
                   No UI and no GPU, so all of it is tested headless.
  omacull-ai       ONNX Runtime and the face models. No UI.
  omacull          the app: egui on wgpu, views, commands, theme.
```

- **Keep the engine free of UI and GPU code.** Anything about a folder of
  raws that needs no window goes in `omacull-engine`, with unit tests.
- **Everything the user can do is a `Command`**
  (`crates/omacull/src/commands.rs`), so shortcuts and `OMACULL_SCRIPT`
  can't drift apart.
- **Test what you add.** UI behaviour is tested by driving egui with
  made-up events; the `Harness` in `crates/omacull/src/app.rs` is the
  pattern to copy.
- **Sidecar writes change the rating and nothing else.** Test against
  sidecars darktable wrote (`crates/omacull-engine/testdata`).
- **Keep changes small.** No new abstraction until something needs it, and
  no new dependency without a reason that's worth its build time.
- **Don't run `cargo fmt`.** The code isn't formatted with rustfmt's
  defaults, and reformatting a file buries the change in it. Match the
  code around yours, and turn off format-on-save.

## Pull requests

- One feature or fix in each, on a branch from `main`.
- Commit messages say what changed for the user.
- If the change finishes something on the roadmap, say so there, with
  anything left over as a "Later:" line. If it adds a shortcut, add it to
  DESIGN.md.
- Say how you tested it, and with which camera's raws.

[AGENTS.md](AGENTS.md) has the same rules in short, for coding agents.

## Licences

Omacull is GPL-3.0-or-later, and contributions are taken under the same
licence.

- **Code from elsewhere** is fine from GPL-compatible projects (darktable,
  digiKam and Geeqie among them). Say where it came from in the file it
  lands in.
- **New dependencies** need GPL-compatible licences.
- **Models** need weights and training data that both allow commercial
  use, since Omacull is for client work. A model goes in
  `scripts/fetch-models.sh` with its licence, its author, its SHA-256 and
  a download URL fixed to one revision.
