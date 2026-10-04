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
  the cursor is kept ready, biased in the direction of travel.
- **Thumbnail cache:** `~/.cache/omacull/`, keyed by path, size and mtime.
  Disposable.
- **Decision log:** `~/.local/share/omacull/`, append-only. One row per
  mark: file identity, the mark, what it replaced, the view it was made
  in, the frames it was compared against, time spent, timestamp. This is
  the training set for auto-cull, so its schema is settled in M1.
- **Colour:** embedded previews are sRGB or Adobe RGB per the camera
  setting; convert to the monitor profile with lcms2 as Omapix does.

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
  bars, so M1 decides between them and downscaled previews in the cache.
- **100% zoom** needs a real decode of the raw: there is no full-size
  JPEG to zoom into. With `rawler`, untuned, that is about 25 ms to decode
  and 200 ms to develop a 24 MP frame, so it has to be prefetched for the
  neighbours and cached, with the upscaled preview shown until it lands.
  The zoomed image won't match the camera's rendering exactly. This is
  M2's main piece of work.
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
