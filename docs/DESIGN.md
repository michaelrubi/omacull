# Omacull design

Omacull is a fast, keyboard-first photo culler for Omarchy. It does one
job: get from a folder of raws to a rated folder of raws, quickly, and
hand that to darktable. It is the first stage of the workflow that
[Omapix](https://github.com/michaelrubi/omapix) finishes:

    cull (Omacull) → base edit (darktable) → retouch (Omapix) → export

It is an independent project, not part of Omarchy.

## Principles

1. **Culling only.** The complaint about XnView MP is that there is too
   much going on. Anything that isn't looking at frames and marking them
   has to argue its way in.
2. **Never wait for a frame.** Stepping to the next image shows it
   immediately. The embedded camera JPEG is the preview; neighbours are
   decoded ahead of the cursor.
3. **Keyboard-first, Bridge/Lightroom muscle memory.** Marking, stepping,
   zooming and switching views are single keys. Rebindable in
   `~/.config/omacull/hotkeys.toml`.
4. **The files are the database.** Marks live in XMP sidecars beside the
   raws. No catalogue, no import step. Raws are never moved, renamed,
   modified or deleted.
5. **Omarchy-native.** Wayland, colours from the active Omarchy theme and
   following theme switches live, behaves well as a Hyprland tile.
6. **Opinionated, not configurable.** Same as Omapix: good defaults over
   preference dialogs.

## Scope

Decided in the 2026-10-04 scoping interview.

### In

- **Input:** raw-only shoots of 200 to 1000 frames, Sony ARW first.
- **Marks:** reject, pick and 0 to 5 stars. Nothing else.
- **Views:** loupe with filmstrip, compare (2-up, synced zoom and pan), and
  survey (3 or 4 side by side, more if they still fit usefully; knock out
  losers until one is left).
- **Inspection:** instant 100% zoom, jump to faces and eyes, focus peaking,
  the camera's AF point, histogram, highlight and shadow clipping, EXIF.
- **Stacks:** frames grouped for culling as a set. The method is a
  setting: off, manual, by capture time, or by capture time and visual
  similarity.
- **Navigation:** `omacull <dir>`, a folder picker, recent folders, and a
  folder tree sidebar.
- **Handoff:** sidecars darktable reads, a cull summary (picks, rejects,
  undecided, with a jump to what's undecided), and a key that opens the
  folder in darktable.
- **Auto-cull, later:** a model each user trains on their own decisions.
  Every decision is logged from the first usable build so the data exists
  when that work starts.

### Out

- Ingest from cards, renaming, backup copies.
- Deleting or moving rejects.
- Colour labels, keywords, captions, IPTC.
- A thumbnail grid as a primary view (the filmstrip takes multi-select).
- A catalogue or search across shoots.
- Rendering raws with darktable's edit applied.
- Any editing.

## Marks and the darktable sidecar

darktable has stars and a reject flag but no pick flag, so:

| Mark      | `xmp:Rating` | Key (default) |
|-----------|--------------|---------------|
| Reject    | -1           | X             |
| Unmarked  | 0            | U or 0        |
| Pick      | 1            | P             |
| Stars     | 1 to 5       | 1 to 5        |

A pick is one star. In darktable, "rated 1 or more" is the keepers.

The rest of M1's keys: arrows, Home and End step; Ctrl+Z and Ctrl+Shift+Z
undo and redo marks; A turns auto-advance after a mark on and off
(remembered); Ctrl+O opens a folder. The filmstrip's filters are Bridge's
Ctrl+Alt keys: Ctrl+Alt+A all, Ctrl+Alt+0 undecided, Ctrl+Alt+1 picks and
up, Ctrl+Alt+2 to 5 that many stars and up, and Ctrl+Alt+X rejects. A
frame marked out of the filter stays on screen until the cursor leaves it.

M2's inspection keys: Z zooms to 100% (tap to toggle, hold for a look
that ends when it's let go), Shift+Z zooms to the focus point and follows
each frame's from then on; H histogram, I shooting settings, J highlight
and shadow clipping (Lightroom's), S focus peaking, F the focus point.
The mouse does the same as Z on the loupe: click to toggle, press and hold
for a look, drag to pan; the wheel pans. Which overlays are on is
remembered. The status bar has a switch for each, its key in the tooltip.

M3's keys: C compares two frames, N surveys several, Escape goes back to
the loupe (and then clears the selection). Ctrl+click and Shift+click in
the filmstrip select, as do Shift+arrows, Ctrl+A and Ctrl+D. In both views
the current frame is the active pane, outlined, which marks and the zoom
keys go to; Tab or a click makes another active.

- **Compare** takes the first two selected frames, or the current one and
  the next. Rejecting a side (or / to dismiss it unmarked) brings in the
  next candidate after both; the arrows change the active side's frame; a
  plain click in the filmstrip puts that frame on the active side.
- **Survey** takes the selection, or the current frame and the next three.
  / knocks the active frame out, without marking it, until one is left,
  which opens in the loupe; the arrows move between panes.
- **Layout:** frames go in rows, each row's frames the same height and
  filling its width, with as many rows as leave the smallest frame
  largest, so landscape frames stay big and portraits sit side by side.
- **Zoom** works in every pane, locked together by default (L unlocks):
  the same spot in each frame, or with Shift+Z each frame's own focus
  point. Every M2 overlay works in every pane. Full developments are made
  for every frame on screen, one at a time.
- **The decision log** records the view (`compare`, `survey`) and the
  other frames on screen in `compared`.

M4's keys: T shows the folder tree, M the cull summary, Shift+U goes to
the first undecided frame, and Ctrl+E (Lightroom's Edit In) opens the
folder in darktable.

- **Folder tree:** beside the loupe, rooted at the folder above the open
  one (↑ goes higher), opened out to it, each folder with how many raws
  it holds. Folders are read in the background as they're opened out,
  and again each time the tree is shown. A click opens a folder; on the
  arrow, or on a folder with no raws, it opens out instead.
- **Cull summary:** picks and up, each star, rejects and undecided, with
  their share, and buttons for the first undecided frame and darktable.
- **darktable:** every mark still queued is written first, then
  `darktable <folder>` is started, which imports the folder and reads the
  sidecars.
- **Places:** each folder's frame (by name) and filter are remembered in
  `state.toml` when another folder is opened and when Omacull closes, for
  the 200 most recent folders, and come back when it's opened again
  (unless a raw in it was opened, which wins).

M5's keys: E zooms to the eyes of the face nearest the pointer (or the
largest, with the pointer off the frame), and again to the next face, left
to right; Shift+E shows the face close-ups beside the loupe, where a click
zooms to that face.

- **Detection:** YuNet (OpenCV's model zoo, MIT) in `omacull-ai`, on the
  system's ONNX Runtime opened at run time (`/usr/lib/libonnxruntime.so`,
  or `OMACULL_ORT_LIBRARY`), on the CPU: it's small, and a folder is a few
  milliseconds a frame. It finds each face's box and both eyes. The model
  comes from `scripts/fetch-models.sh` into `~/.local/share/omacull/models`,
  or is Omapix's copy, which is the same file; both are checked by size and
  SHA-256. Omacull downloads nothing itself. Without ONNX Runtime or the
  model, everything else works and E says why there are no faces.
- **In the background:** one thread works through the whole folder from
  the time it's opened, what's on screen and near it first, finding faces
  on the preview, upright. What it finds is cached in
  `~/.cache/omacull/faces/`, keyed like the thumbnails, so a folder is
  looked through once. Faces are kept as fractions of the frame, so they
  hold for the preview and the full development alike.
- **Close-ups:** a square round the eyes, twice as wide as the face, cut
  from the full development once it's there, from the preview until then.

- Sidecars are named the way darktable names them: `DSC01234.ARW.xmp`.
- No sidecar yet: write a minimal one holding just the rating.
- Sidecar exists: change the rating field and nothing else. darktable's
  history stack must survive byte-for-byte apart from that field.
- Writes are atomic (write a temp file, rename over).
- Marks are undoable for the length of the session.

Checked in M0 against darktable 5.6.1, on copies, by importing into
scratch libraries with `darktable-cli` and reading the result from the
library database: stars arrive as stars, -1 arrives as rejected, a
sidecar with a 26-step history keeps all 26 steps and differs from the
original only in the rating's characters, and a fresh rating-only sidecar
imports with the rating and darktable's default edit. darktable left our
sidecars as we wrote them.

Known wrinkle: darktable reads sidecars on import, but for images already
in its library the database wins unless "look for updated XMP files on
startup" is turned on (it is off in Michael's config), in which case
darktable asks at startup which side to keep. Re-culling an
already-imported shoot depends on that preference; without it darktable
writes its own rating back over ours the next time it saves the sidecar.
The normal flow, cull then import, is unaffected. Still to be looked at by
eye in the darktable window.

## Architecture

Same stack and layout as Omapix: Rust, egui on wgpu, Little CMS 2, GPL-3.
A separate repo; theme, hotkeys and packaging conventions are copied from
Omapix, not shared as crates (revisit if the copies start to drift).

```
crates/
  omacull-engine   folder scan, raw preview extraction, EXIF and makernotes,
                   XMP read/write, thumbnail cache, stacking, decision log
                   (no UI or GPU dependencies; testable headless)
  omacull-ai       ONNX Runtime, face/eye detection, later scoring models
                   (added when the face milestone starts)
  omacull          the app: egui UI on wgpu, views, input, theme
```

- **Commands:** every action goes through one `Command` enum, so
  shortcuts, menus and headless test scripts never diverge (Omapix's
  `commands.rs` pattern, including an `OMACULL_SCRIPT` hook).
- **Preview pipeline:** extract the embedded JPEG, decode off the UI
  thread, upload as a GPU texture. A small ring of decoded frames around
  the cursor is kept ready, biased in the direction of travel: four ahead
  and two behind, among the frames the filter shows. The loader is a pool
  of threads with one queue the app replaces whenever the cursor moves,
  so a frame already passed is never decoded late.
- **Thumbnail cache:** `~/.cache/omacull/thumbnails/`, one small JPEG per
  raw, named for a hash of its path, size and mtime. Disposable, and not
  pruned yet.
- **Decision log:** `~/.local/share/omacull/decisions.jsonl`, append-only.
  One row per mark that reached its sidecar. This is the training set for
  auto-cull; the schema is below.
- **Writes off the UI thread:** sidecar writes (with their fsync) and log
  appends go to one disk thread, in order, so a slow disk never delays the
  next frame. A write that fails puts the frame back to what its sidecar
  holds and says so in the status bar. Closing the window waits for the
  queue to empty.

### Decision log

Settled in M1. JSON Lines, one object per mark, version 1:

```json
{"v":1,"at":1791133520394,"session":1791133520249,
 "file":{"path":"/home/michael/Pictures/2026-10-04/DSC01234.ARW","size":24771584,
         "modified":1791133349,"captured":"2026:10:04 15:43:16.123"},
 "rating":3,"was":0,"how":"mark","view":"loupe","compared":[],
 "filter":"all","dwell_ms":1840}
```

| Field | Meaning |
|-------|---------|
| `v` | Schema version. Bump it when a field changes meaning; adding a field doesn't. |
| `at` | When the mark was made, milliseconds since 1970. |
| `session` | When Omacull was started, the same for a sitting. |
| `file` | The raw: canonical `path`, `size` in bytes, `modified` (seconds since 1970), and `captured`, the Exif capture time with sub-seconds. Name, size and capture time find the raw again after a folder moves. |
| `rating`, `was` | The mark made and the mark it replaced, as `xmp:Rating` (-1 to 5). |
| `how` | `mark` for a mark key, `undo` or `redo` when those changed it. An undone mark stays in the log; the last row for a file is what it ended as. |
| `view` | `loupe` (later `compare` and `survey`). |
| `compared` | The other frames on screen, as `file` objects. Empty in the loupe. |
| `filter` | The filter in use: `all`, `undecided`, `rejects`, or `{"at_least":N}`. |
| `dwell_ms` | How long the frame had been on screen when it was marked. Undo and redo go to the frame they change, so theirs is usually 0. |

A mark that repeats the frame's mark isn't logged, nor is one whose
sidecar couldn't be written.
- **Colour:** embedded previews are sRGB or Adobe RGB per the camera
  setting (Exif ColorSpace "uncalibrated" with interop index R03 is Adobe
  RGB); developed raws are sRGB. Both are converted to the monitor's
  profile with lcms2 on the loader threads, as Omapix does: the monitor
  from `hyprctl`, its profile from `~/.config/omacull/monitors.toml` or its
  EDID, and nothing done where Hyprland manages the monitor's colours.
  Thumbnails are cached in sRGB. Moving the window to another monitor
  decodes what's on screen again.
- **Overlays:** the histogram is of the camera's rendering, the preview.
  Clipping (any channel at 254 or more is red, all at 2 or less blue) and
  focus peaking (green) are measured on the frame before colour
  conversion, as one byte of marks a pixel, and baked into the texture
  when they're on: on the preview when whole, on the full development at
  100%. Peaking marks Sobel edges at least as steep as a 40-level step and
  among the frame's crispest 1.5%, so a frame with nothing in focus has
  nothing marked. The thresholds want trying on real shoots.
- **Focus point:** Sony's FocusLocation, turned with the frame; hidden
  for manual focus (Sony's FocusMode 0, makernote tag 0x201b).

### The embedded preview is small

Answered in M0 with a real shoot (787 frames from the ILCE-7M3): every
raw embeds a 1616×1080 preview (about 840 KB) and a 160×120 thumbnail, and
nothing larger. So:

- **Stepping** uses the 1616×1080 preview. Finding it in the raw takes
  0.01 ms, reading it 0.1 ms warm and 0.7 ms cold, decoding it to RGBA
  about 10 ms on one core. Prefetched neighbours cost nothing but the
  texture upload; 500 previews decode in half a second on all cores.
- **Filmstrip thumbnails** for a 500-frame folder decode in under 20 ms
  from the camera's own thumbnails, cold. Those are 160×120 with black
  bars, too small and the wrong shape for a HiDPI filmstrip, so M1 shrinks
  the preview to 320 pixels on the long edge instead and caches it as a
  JPEG. The first open of a folder fills the cache in the background on
  every core, nearest the cursor first, after the loupe's previews; later
  opens read the cache. Until a thumbnail lands its cell is empty, and the
  loupe shows the enlarged thumbnail until the preview lands.
- **100% zoom** needs a real decode of the raw: there is no full-size
  JPEG to zoom into. With `rawler`, untuned, that is about 25 ms to decode
  and 200 ms to develop a 24 MP frame, so it has to be prefetched for the
  neighbours and cached, with the upscaled preview shown until it lands.
  Built in M2 (`develop.rs`):
  - The current frame and the next and previous ones are developed in
    the background, one at a time (a development holds a few hundred
    megabytes of float pixels while it runs), and kept in memory: about
    120 MB each with their overlay marks.
  - rawler's development is flat: no tone curve, so dull and dark next to
    the camera's JPEG. Each channel's levels are mapped to match the
    preview's histogram, so zooming in keeps the camera's brightness,
    contrast and colour near enough, with the raw's own detail and noise.
    Not exact, and not meant to be judged for colour.
  - A full frame goes to the GPU in 512-pixel tiles as they come into
    view, with nearest-neighbour filtering on whole pixels: 96 MB at once
    would stutter, and some GPUs won't take a texture that big.
  - The zoom is kept as a point of the frame (fractions of its width and
    height), so it stays on the same spot from frame to frame.
- **Focus location** is in the Sony makernotes (tag 0x2027: frame width,
  frame height, x, y) and reads correctly for 777 of the 787 frames,
  matching exiftool. The other ten were shot with tracking and record
  zeros. Manual-focus frames record the frame centre, which means nothing,
  so the overlay should be hidden for those.

Other bodies may embed a full-size JPEG; the reader lists every embedded
JPEG, so those would get instant zoom for free. Rerun the spike on a new
camera's files:

    cargo run --release -p omacull-engine --example spike -- <folder>

## Prior art

What exists, what to take and what to leave. Written from memory during
scoping; M0 includes checking the specifics that the design leans on.

| Tool | Take | Leave |
|------|------|-------|
| Photo Mechanic (proprietary) | The benchmark for speed: embedded previews, no import, single-key marks | Ingest, IPTC, the price |
| FastRawViewer (proprietary) | Raw-honest inspection: focus peaking, exposure and clipping overlays, XMP ratings | Dense settings |
| Adobe Bridge / Lightroom | Key conventions (P/X/U, 0-5), compare with synced zoom, survey view | Catalogue, everything else |
| darktable lighttable | Culling mode with synced zoom; it defines the sidecar format we write | Slow to step through raws; the reason a separate culler is needed |
| digiKam | Light table compare, face detection, similarity grouping ideas | Database, scale of the app |
| Geeqie | Fast, split-view compare, reads embedded previews | GTK look, dated marking model |
| XnView MP | Format coverage | "Too much stuff going on" |
| Rapid Photo Downloader | Nothing for now (ingest is out of scope) | — |
| gThumb, Shotwell, nomacs, qimgv | Little; general viewers, not cullers | — |
| Aftershoot, Narrative Select (proprietary) | The auto-cull target: closed-eye and blur detection, stack winners, learning from the user's own choices; eye close-ups beside the frame | Cloud, subscription |

Building blocks, settled in M0:

- **Raw container, embedded previews, makernotes:** our own TIFF directory
  reader (`omacull-engine::raw`). It reads a few hundred bytes per raw
  instead of the whole file, and needs no dependency.
- **JPEG decode:** `zune-jpeg`. libjpeg-turbo was about 20% faster on the
  previews (8.5 ms against 10.5 ms), not enough to take on a C library
  when prefetching hides the decode anyway.
- **XMP:** `quick-xml` to find the rating, then the bytes are spliced; the
  sidecar is never serialised back out.
- **Raw decode for 100% zoom:** `rawler`, added when M2 needs it.
- **Faces:** ONNX Runtime via the Omapix `omapix-ai` approach, in M5.

What doesn't exist on Linux, and is the reason for the project: a culler
with Photo Mechanic's speed, compare and survey views, face-aware zoom,
clean darktable sidecars, a native Wayland/Omarchy feel, and locally
trained auto-cull.
