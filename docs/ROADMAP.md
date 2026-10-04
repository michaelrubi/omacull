# Omacull roadmap

What's planned, in the order we plan to do it. [DESIGN.md](DESIGN.md)
covers the scope and architecture.

Status: M0 done on 2026-10-04 (scoped the same day). M1 to M5 built the
same day, waiting to be tried on a real shoot.

Each milestone ends with something Michael can use on a real shoot. M1 is
the point where Omacull replaces XnView MP for a basic cull; everything
after it adds what XnView never had.

## M0. Spikes and scaffold (done)

Answered the questions the design depends on, with a real 787-frame shoot
from the ILCE-7M3, before building UI. The numbers and what follows from
them are in DESIGN.md.

- **Embedded preview size.** 1616×1080 and a 160×120 thumbnail, nothing
  larger. 100% zoom needs a real raw decode.
- **Decode timing.** About 10 ms a preview on one core, 500 in half a
  second on all cores, a folder's camera thumbnails in under 20 ms.
- **Sidecar round-trip.** Ratings, rejects and an intact edit history
  confirmed against darktable 5.6.1 through `darktable-cli`. Left for
  Michael to look at in the darktable window: that the stars and the
  reject show in lighttable, and what an already-imported image does with
  a changed sidecar. `cargo run -p omacull-engine --example rate -- <raw>
  <rating>` writes a rating the way the app will.
- **AF point.** Read from the Sony makernotes for every frame that has
  one.
- **Crate choices.** Our own TIFF reader, `zune-jpeg`, `quick-xml`, and
  `rawler` when M2 needs a raw decode.
- **Scaffold.** Workspace with `omacull-engine` and `omacull`, an empty
  themed window, with theme, hotkeys, `Command`, `OMACULL_SCRIPT` and the
  headless test harness carried over from Omapix. `make install`.

## M1. Loupe cull (built, to be tried on a real shoot)

The minimum that replaces XnView MP. Everything below is in and tested
headless and on a synthetic shoot; still to check by hand on a real
shoot: that stepping never shows a wait, how long the first open of a
big folder takes to fill the thumbnail cache, and the filter and
auto-advance keys in daily use. Decisions made on the way are in
DESIGN.md: thumbnails are shrunk previews in a JPEG cache, the decision
log's schema, the filter keys, and that a frame marked out of the filter
stays until the cursor leaves it.

- Open a folder from the command line, a picker, or the recent list.
- Filmstrip of thumbnails with a disk cache; loupe showing the embedded
  preview, fit to window.
- Step with the arrow keys with neighbours prefetched; Home/End; no
  visible wait.
- Mark: reject, unmark, pick, 1 to 5 stars. Optional auto-advance after a
  mark.
- Marks shown on the filmstrip and the loupe.
- Sidecars written as DESIGN.md specifies; existing ratings read on open.
- Filter the filmstrip: all, undecided, picks and up, rejects, N stars and
  up.
- Undo and redo for marks.
- Decision log written from the first mark, with its schema settled here.
- Status bar: position in folder, counts of picks, rejects and undecided.

## M2. Inspection (built, to be tried on a real shoot)

Everything below is in and tested headless and on a synthetic shoot, but
the raw development has only been tested on files rawler can't decode:
no real ARW could be fetched for testing. Still to check by hand: that
`spike` develops a real shoot at full size and in what time, that the
zoomed frame looks like the preview, the peaking and clipping thresholds,
which frames are taken for manual focus, and the colours on a wide-gamut
monitor. How it's built is in DESIGN.md.

- Instant 100% zoom: hold for a temporary look, tap to toggle, at the
  pointer. Pan while zoomed. Zoom position kept when stepping, so the same
  spot can be checked across a burst. The embedded preview is too small
  for this (see DESIGN.md), so it runs on a decode of the raw, prefetched
  for the neighbours and cached.
- Histogram, highlight and shadow clipping overlays.
- EXIF readout: shutter, aperture, ISO, focal length, lens, time.
- Focus peaking overlay.
- AF point overlay, and "zoom to AF point".
- Colour-managed display through the monitor profile.

## M3. Compare and survey (built, to be tried on a real shoot)

In and tested headless and on a synthetic shoot; the keys and how
compare and survey behave are in DESIGN.md.

- Multi-select in the filmstrip.
- Compare: two frames, zoom and pan locked together, with a key to unlock.
  Mark either; the marked-down side is replaced by the next candidate.
- Survey: three or four frames side by side, laid out by orientation so
  landscape frames stay usefully large; more if selected, with the layout
  degrading gracefully. Knock a frame out with one key until one is left.
- All inspection aids from M2 work in every pane.

## M4. Navigation and handoff (built, to be tried on a real shoot)

In and tested headless; how each part works is in DESIGN.md.

- Folder tree sidebar, toggleable, with counts of raws per folder.
- Cull summary: picks, rejects, undecided, star breakdown, and a jump to
  the first undecided frame.
- "Open in darktable" key for the current folder.
- Remember per-folder position and filter between sessions.

## M5. Faces (built, to be tried on a real shoot)

In and tested headless, and with the real ONNX Runtime and YuNet on a
crowd photo (OpenCV's sample): 50 faces found, every pair of eyes inside
its face. How it works is in DESIGN.md.

- Face and eye detection, run in the background over the folder and
  cached.
- Key to zoom to the nearest face's eyes; cycle faces in group shots.
- Face close-ups strip beside the loupe (optional panel).
- Adds the `omacull-ai` crate, following Omapix's ONNX Runtime approach.

## M6. Stacks

- A stack is a set of frames culled together: enter it in survey, pick a
  winner, reject the rest in one key.
- Stack method setting: off, manual (stack the selection), by capture-time
  gap, by time and visual similarity.
- Stacks collapse in the filmstrip, showing the winner once chosen.

## M7. Assisted and trained auto-cull

In three steps, each useful alone.

1. **Signals.** Per-frame measurements with no training: sharpness at the
   AF point and at the eyes, eyes open or closed, exposure clipping.
   Shown as small indicators, never applied on their own.
2. **Suggestions.** Within a stack, propose a winner from the signals. The
   user confirms or overrides; overrides are logged.
3. **Personal model.** Train on the decision log, locally, to predict this
   user's reject / keep / star choices. Suggestions shown for review, with
   a confidence threshold below which nothing is suggested. No cloud, no
   account.

## M8. Release

- Arch package and AUR. (The desktop entry and icon came early: `make
  install` puts them in.)
- Other raw formats (Canon CR3, Nikon NEF, Fuji RAF), driven by who shows
  up to test them.
- README, CONTRIBUTING, screenshots.
- Config for the pick mapping, for people whose raw developer isn't
  darktable.

## Not planned

Ingest, deleting or moving rejects, colour labels, keywords and IPTC, a
grid view, a catalogue, editing. See "Out" in DESIGN.md. Any of these can
be reopened once the tool is in daily use.
