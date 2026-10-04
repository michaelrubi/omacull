# Omacull roadmap

What's planned, in the order we plan to do it. [DESIGN.md](DESIGN.md)
covers the scope and architecture.

Status: nothing built yet. Scoped on 2026-10-04. M0 is next.

Each milestone ends with something Michael can use on a real shoot. M1 is
the point where Omacull replaces XnView MP for a basic cull; everything
after it adds what XnView never had.

## M0. Spikes and scaffold (next)

Answer the questions the design depends on, with real files, before
building UI.

- **Embedded preview size.** Check what the ARWs from Michael's camera
  actually embed (dimensions of every embedded JPEG). Decides whether 100%
  zoom runs on the embedded preview or needs a real raw decode. See
  DESIGN.md.
- **Decode timing.** Time embedded-JPEG extraction and decode for a
  500-frame folder, cold and warm. Target: a frame ready in well under a
  display refresh once prefetched, and a whole folder's thumbnails in a
  few seconds.
- **Sidecar round-trip.** Write a rating into a fresh sidecar and into one
  darktable already wrote, then confirm in darktable: the rating shows,
  reject shows as rejected, and the edit history is intact. Confirm how
  darktable treats a changed sidecar for an already-imported image.
- **AF point.** Confirm the focus location can be read from Sony
  makernotes.
- **Crate choices.** Settle raw container, JPEG decode and XMP crates from
  the results above.
- **Scaffold.** Workspace with `omacull-engine` and `omacull`, an empty
  themed window, with theme, hotkeys, `Command` and the headless test
  harness carried over from Omapix. `make install`, AGENTS.md updated with
  real commands.

## M1. Loupe cull

The minimum that replaces XnView MP.

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

## M2. Inspection

- Instant 100% zoom: hold for a temporary look, tap to toggle, at the
  pointer. Pan while zoomed. Zoom position kept when stepping, so the same
  spot can be checked across a burst.
- Histogram, highlight and shadow clipping overlays.
- EXIF readout: shutter, aperture, ISO, focal length, lens, time.
- Focus peaking overlay.
- AF point overlay, and "zoom to AF point".
- Colour-managed display through the monitor profile.

## M3. Compare and survey

- Multi-select in the filmstrip.
- Compare: two frames, zoom and pan locked together, with a key to unlock.
  Mark either; the marked-down side is replaced by the next candidate.
- Survey: three or four frames side by side, laid out by orientation so
  landscape frames stay usefully large; more if selected, with the layout
  degrading gracefully. Knock a frame out with one key until one is left.
- All inspection aids from M2 work in every pane.

## M4. Navigation and handoff

- Folder tree sidebar, toggleable, with counts of raws per folder.
- Cull summary: picks, rejects, undecided, star breakdown, and a jump to
  the first undecided frame.
- "Open in darktable" key for the current folder.
- Remember per-folder position and filter between sessions.

## M5. Faces

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

- Arch package and AUR, desktop entry, icon.
- Other raw formats (Canon CR3, Nikon NEF, Fuji RAF), driven by who shows
  up to test them.
- README, CONTRIBUTING, screenshots.
- Config for the pick mapping, for people whose raw developer isn't
  darktable.

## Not planned

Ingest, deleting or moving rejects, colour labels, keywords and IPTC, a
grid view, a catalogue, editing. See "Out" in DESIGN.md. Any of these can
be reopened once the tool is in daily use.
