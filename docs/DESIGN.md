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
| Unmarked  | 0            | U             |
| Pick      | 1            | P             |
| Stars     | 1 to 5       | 1 to 5        |

A pick is one star. In darktable, "rated 1 or more" is the keepers.

- Sidecars are named the way darktable names them: `DSC01234.ARW.xmp`.
- No sidecar yet: write a minimal one holding just the rating.
- Sidecar exists: change the rating field and nothing else. darktable's
  history stack must survive byte-for-byte apart from that field.
- Writes are atomic (write a temp file, rename over).
- Marks are undoable for the length of the session.

Known wrinkle: darktable reads sidecars on import, but for images already
in its library it only picks up changes when "look for updated XMP files
on startup" is turned on. Re-culling an already-imported shoot depends on
that preference. To be confirmed in the M0 round-trip test.

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

### Open technical question: how big is the embedded preview?

Everything about "instant 100% zoom" depends on it. Many Sony bodies embed
only a 1616×1080 preview in the ARW; some newer ones embed a full-size
JPEG. Which one Michael's camera writes decides the design:

- **Full-size preview:** 100% zoom, peaking and eye checks all run on the
  embedded JPEG. Simple and instant.
- **Small preview only:** stepping still uses the embedded JPEG, but
  zooming to 100% triggers a fast real decode of the raw (half-size or
  full demosaic, cached, prefetched for neighbours). More work, and the
  zoomed image won't match the camera's rendering exactly.

M0 answers this with real files before any UI is built.

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

Likely building blocks, to be confirmed in M0: `rawler` for ARW
containers, embedded previews and makernotes; `zune-jpeg` or libjpeg-turbo
for decoding; `quick-xml` for surgical sidecar edits; ONNX Runtime via the
Omapix `omapix-ai` approach for faces.

What doesn't exist on Linux, and is the reason for the project: a culler
with Photo Mechanic's speed, compare and survey views, face-aware zoom,
clean darktable sidecars, a native Wayland/Omarchy feel, and locally
trained auto-cull.
