# AGENTS.md

Omacull is a fast, keyboard-first photo culler for Omarchy: a folder of raws in, XMP ratings out, then on to darktable. It is the first stage of the workflow Omapix (`~/dev/omapix`) finishes.

**Status:** scoped, nothing built. Read [docs/DESIGN.md](docs/DESIGN.md) for scope and architecture and [docs/ROADMAP.md](docs/ROADMAP.md) for the milestones. M0 is next.

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
- Build and test commands get added here once the M0 scaffold exists.
