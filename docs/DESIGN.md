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
  Since M9, JPEGs and PNGs too: a folder of them, or beside the raws.
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
- **Auto-cull:** signals measured of every frame, the best of a stack
  suggested from them, and a model each user trains on their own
  decisions. It suggests; it never marks. Every decision is logged from
  the first usable build so the data exists to train on.

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

M6's keys: Shift+G goes round how frames are stacked (also in the status
bar), G opens out the current stack or closes it up, Ctrl+G stacks the
selection by hand, Ctrl+Shift+G takes a hand-made stack apart, and W
chooses a winner.

- **Stacking** is a setting, remembered: off (the default), by hand, by
  time (frames taken less than 2 s apart), or by time and look (less than
  30 s apart and alike). How alike two frames look is measured on the
  camera's own 160×120 thumbnail, as a 16×12 grid of brightness with the
  overall brightness evened out, so a burst whose exposure wanders still
  counts as one. Capture times and these signatures are read when a folder
  opens, on every core: a few small reads a raw.
- **Collapsed**, a stack is one frame in the filmstrip and to the arrows:
  its best-rated frame the filter lets through (the first, when they're
  equal), so once a winner is chosen it's the one shown. It has cards
  behind it and a count. Opened out, its frames are underlined together.
- **Surveying a stack:** N with nothing selected, on a frame in a stack,
  surveys the whole stack.
- **The winner** (W): the current frame is picked, unless it has stars
  already, and the rest of the survey, or in the loupe the rest of its
  stack, are rejected. One step to undo. Each mark is in the decision log
  with the others in `compared`.
- **Stacks by hand** are kept per folder, with its place in `state.toml`,
  as the raws' names. Making one switches stacking to by hand.
- **Undo** now takes back a step at a time, a winner's marks together.

M7's keys: Q shows the signals, Shift+Q turns suggestions off and on (on
to begin with), Y takes the suggestion for the current frame, Ctrl+Alt+Y
shows only the frames with one, and Ctrl+L learns from the decision log
now. The status bar has a switch for signals and one for suggestions.

- **Signals** are what's measured of a frame with no training: how crisp
  it is where the camera focused and at the eyes of its largest face,
  whether eyes are shut, and how much of it is blown out or black.
  Sharpness is the step, in levels from one pixel to the next, that the
  crispest tenth of the edges in a window average (Sobel, as focus
  peaking measures it): a square an eighth of the frame across at the
  focus point, a box twice as wide as the eyes are apart at the eyes.
  They're measured on the preview, on the faces thread, straight after
  the faces are found, and cached in `~/.cache/omacull/signals/`. On the
  1616-pixel preview a frame only slightly out of focus won't show; how
  far out one has to be wants trying on real bursts.
- **Eyes open or shut** comes from MediaPipe's face landmarker (Apache-2.0,
  the copy Omapix uses, or `scripts/fetch-models.sh`), run on each face
  YuNet finds: how far apart the eyelids are, against the eye's width,
  both eyes together so a wink isn't a blink. On the test shoot shut eyes
  read 0.07 to 0.17 and open ones 0.26 and up, under heavy eye makeup
  too; measuring how dark the eye is, which was tried first, couldn't
  tell them apart there. A face in profile is where it goes wrong: the
  far eye reads as anything. It sees no face in the carved pumpkins YuNet
  took for some, so says nothing of their eyes. A frame's
  eyes are its least open pair, among faces at least a quarter the size
  of its largest: a bystander's blink doesn't count. Without the model,
  faces are found and nothing is said of their eyes.
- **Standing:** a frame's sharpness and its eyes are also taken as a share
  of the best among the frames it was shot with: its stack, or else its
  burst (frames less than 2 s apart, however stacking is set).
- **Shown** (Q) as words on the frame: "Eyes 64, sharpest of 5", "Focus
  41, 63% of the sharpest" (red under 80%), "Eyes shut?", "4% blown" (from
  3%), "12% black" (from 10%). In the filmstrip a red dot is shut eyes and
  a red ring a frame soft beside its neighbours.
- **The best of a stack** is suggested once every undecided frame in it
  is measured, unless one is kept already: the sharpest, marked down for
  eyes less open than the others' and for what's blown out. It stands
  for its stack in the filmstrip, so W on it confirms and W on another
  overrides; Y takes it from any frame of the stack.
- **The personal model** (`learn.rs`) is trained on the decision log: what
  each frame ended as (rejected, left unmarked, or kept, and with how
  many stars), against its signals and standing as they were logged. It's
  a weight for each thing measured and no more (softmax regression, a
  fifth of the frames held back to test it), so it trains in a moment
  and can only learn what the signals can say: that soft,
  shut-eyed and second-best frames go. It takes 100 decisions, with at
  least ten each of two kinds. It's trained when Omacull starts, if the
  log has grown, and kept in `~/.local/share/omacull/model.json`.
- **Its suggestions** are for undecided frames outside a stack's, and only
  where it's at least 80% sure (`confidence` in `config.toml`): a mark
  where the frame's mark would be, with how sure it is, and a fainter one
  in the filmstrip. Y takes one; any other mark overrides it. The cull
  summary says what it was learned from and how it did on the frames held
  back.
- **Nothing is marked on a suggestion's word.** Every suggestion taken or
  overridden is in the decision log (`suggested`), which is how to tell
  whether they're any good.

M9's key: Shift+F goes round which of a folder's pictures are culled:
all of them, only the raws, only the JPEGs or only the PNGs, whichever of
those it holds. It's in the status bar too, where a folder holds more
than one kind.

- **JPEGs** (`.jpg`, `.jpeg`) are culled as raws are. A JPEG is its own
  preview, shrunk to 2048 pixels, and its own 100%: it's decoded whole
  for each, nothing is developed. Its settings, orientation, capture time
  and thumbnail come from its Exif, by the reader the raws use (a Fuji
  RAF's are read the same way).
- **Formats** is a setting, remembered: All (the default), RAW, JPEG or
  PNG.
  What isn't asked for isn't in the cull at all: not counted, stacked,
  measured, suggested for or given a thumbnail. As a filter on what's
  shown, a raw and the JPEG the camera wrote beside it, taken at the same
  moment, would be stacked together, and choosing the raw as the winner
  would reject its own JPEG. So changing it reads the folder again,
  opening on the frame it was on or the one named like it; marks made
  before can no longer be undone. A folder with none of what's asked for
  opens on all it has, and a picture that's opened is shown, with all
  of its folder, whatever the setting.
- **Each file has its own mark.** A raw's mark says nothing of the JPEG
  beside it, and nothing is written for a picture that isn't in the cull.
- **Colour:** a JPEG that carries a profile is in that, whatever its Exif
  says: darktable's exports can be in anything (Michael's are Linear
  ProPhoto RGB, with Exif that says sRGB). sRGB and Adobe RGB are taken as
  they are; anything else is converted to sRGB as it's decoded, so the
  histogram, clipping, faces and signals see what the picture looks like,
  and colours outside sRGB are lost. Checked on a real export against
  ImageMagick's conversion: the same to within a level.
- **The focus point** is Sony's, where the JPEG has it, unless the JPEG
  isn't the shape of the frame the camera focused in: an export keeps the
  makernote, turned or cropped or not. One cropped to the same shape
  still shows it in the wrong place.
- **Speed**, on 24 real exports of 24 MP (18 MB each): 170 ms to decode
  one, 230 ms with its colours converted for 100%, where a raw's preview
  takes 10 ms. Stepping at a walk is still ahead of the cursor, on every
  core; held down, the arrow shows enlarged thumbnails. A phone's 12 MP
  takes 120 ms.
- **PNGs** (`.png`), added after M9, are culled as JPEGs are: each is its
  own preview and its own 100%, with a sidecar of its own, and PNG is a
  format to cull alone like the others. darktable's exports carry a
  profile and no Exif, so there are no settings or capture time to show,
  and nothing to stack them by. Exif is read where a PNG has it, from a
  chunk before the pixels, which is where exiftool writes it; after them
  it isn't looked for, since that would mean reading through every big
  file to open a folder.
- **A PNG's colours:** its profile is believed, as a JPEG's is, and where
  it has 16 bits a channel they're converted before they're cut to 8: a
  linear export's shadows would come out in steps otherwise. Without a
  profile it's taken for sRGB, whatever its gamma or cICP chunks say.
  What's transparent is shown on mid grey, which neither clipping overlay
  lights up over. Checked on darktable 5.6.1's exports of a real raw (8
  and 16 bits; sRGB, Adobe RGB, linear Rec. 2020 and linear ProPhoto)
  against ImageMagick's conversion: a fiftieth of a level apart on
  average, where unconverted is 39 levels out. The 28 PNGs on Michael's
  machine, screenshots and logos, all decode.
- **A PNG's speed:** a 24 MP export takes 200 ms to decode at 8 bits,
  500 ms at 16, and 900 ms at 16 in a profile to convert from: Little CMS
  takes fifteen times as long from 16 bits as from 8. That's for each
  step and again for 100%.

### Other raw developers

`~/.config/omacull/config.toml` (M8; written with every setting commented
out the first time Omacull runs) is for people whose raw developer isn't
darktable. There are three settings and one for suggestions:

| Setting | Default | |
|---------|---------|---|
| `pick` | `1` | The stars a pick (P, and a winner) is written as. |
| `sidecar` | `"darktable"` | `"adobe"` names sidecars `DSC01234.xmp`, as Lightroom, Bridge, Capture One and most others read them. |
| `developer` | `"darktable"` | The program Ctrl+E hands the folder to. |
| `confidence` | `0.8` | How sure the model has to be to suggest a mark. |

A reject is always a rating of -1, as Bridge writes it too. A JPEG's
sidecar is `DSC01234.JPG.xmp` however `sidecar` is set, and a PNG's
`DSC01234.png.xmp`: `DSC01234.xmp` would be the raw's too, where they're
in a folder together.

### The sidecar

- Sidecars are named the way darktable names them: `DSC01234.ARW.xmp`,
  `DSC01234.JPG.xmp` for a JPEG and `DSC01234.png.xmp` for a PNG.
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

And another, found in M7: darktable gives every frame it imports a rating
of its own (one star, by default), so a folder it has already imported
opens in Omacull with every frame a pick: all 787 of the test shoot.
Nothing there is undecided, so nothing is suggested, and the filters for
picks say nothing. Culling before importing avoids it; so does setting
darktable's rating on import to none.

And for JPEGs, from M9: a JPEG can hold a rating itself (darktable's
exports carry their raw's), which Omacull doesn't read. Such a JPEG
opens undecided; darktable gives it the rating in its sidecar if it has
one, and the one inside it if not. Checked against darktable 5.6.1 as in
M0: a sidecar's 4 stars over 2 inside the JPEG arrived as 4, its -1 as
rejected, and with no sidecar the 2. So what's left unmarked in a folder
of exports shows in darktable with the stars it was exported with. A PNG
is the same, checked the same way: 5 stars in its sidecar over 2 inside
it arrived as 5.

## Architecture

Same stack and layout as Omapix: Rust, egui on wgpu, Little CMS 2, GPL-3.
A separate repo; theme, hotkeys and packaging conventions are copied from
Omapix, not shared as crates (revisit if the copies start to drift).

```
crates/
  omacull-engine   folder scan, raw preview extraction, JPEGs and PNGs, EXIF and makernotes,
                   XMP read/write, thumbnail cache, stacking, signals,
                   decision log and the model trained on it
                   (no UI or GPU dependencies; testable headless)
  omacull-ai       ONNX Runtime, face and eye detection, eyelids
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
| `how` | `mark` for a mark key, `undo` or `redo` when those changed it, `pass` for a frame looked at and left unmarked. An undone mark stays in the log; the last row for a file is what it ended as. |
| `view` | `loupe` (later `compare` and `survey`). |
| `compared` | The other frames on screen, as `file` objects. Empty in the loupe. |
| `filter` | The filter in use: `all`, `undecided`, `rejects`, `suggested`, or `{"at_least":N}`. |
| `dwell_ms` | How long the frame had been on screen when it was marked. Undo and redo go to the frame they change, so theirs is usually 0. |
| `signals` | What was measured of the frame, or null if it hadn't been yet: `focus` and `eyes` (sharpness), `open` (0 shut to 1), `highlights` and `shadows` (shares of the frame), `faces` (how many) and `face` (the largest's share of the frame). Added in M7. |
| `standing` | How it stood among the frames it was shot with: `of` how many, and its `sharp` and `open` as shares of the best. Added in M7. |
| `suggested` | The mark Omacull was suggesting for it, or null: its `rating`, `by` (`signals` for the best of a stack, `model`) and `confidence`. A row whose `rating` differs is an override. Added in M7. |

A mark that repeats the frame's mark isn't logged, nor is one whose
sidecar couldn't be written.

Frames looked at and left unmarked are logged as `pass` rows (rating 0)
when their folder is left, if any mark was made in it that sitting: for
someone who only picks, what's passed over is the other half of every
decision, and the model can't learn to tell keepers without it. A folder
only looked through logs nothing.
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

### Other cameras

Built in M8 from descriptions of the formats, and tested on made-up files
shaped like each (`testing.rs`): no real NEF, CR3 or RAF has been opened
yet.

- **Nikon NEF** is a TIFF like an ARW, read by the same code. Its small
  JPEG is in the makernote, a TIFF of its own.
- **Canon CR3** is boxes, as in an MP4: the settings are small TIFFs in
  Canon's box under `moov` (CMT1, CMT2), the thumbnail is beside them
  (THMB) and the 1620×1080 preview in a box of its own (PRVW).
- **Fuji RAF** is a header pointing at a whole JPEG file, whose own Exif
  holds the settings and a thumbnail.
- **The preview** is the smallest embedded JPEG at least 1400 pixels on
  its long edge, or the biggest there is. Where a camera embeds only a
  full-size one, it's shrunk to 2048 pixels once decoded: slower to step
  through than Sony's, but no more to hold or upload. 100% zoom still
  comes from developing the raw (rawler reads all four); using a
  full-size embedded JPEG for it instead is left for later.
- **The focus point** is only read from Sony's makernotes.
- A raw that rawler panics on, rather than refusing, no longer takes the
  thread developing it with it.

Rerun the spike on a new camera's files:

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
- **PNG decode:** `png`, which rawler brings in already.
- **XMP:** `quick-xml` to find the rating, then the bytes are spliced; the
  sidecar is never serialised back out.
- **Raw decode for 100% zoom:** `rawler`, added when M2 needs it.
- **Faces:** ONNX Runtime via the Omapix `omapix-ai` approach, in M5.
- **Shut eyes:** MediaPipe's face landmarker, as Omapix runs it, in M7.
- **The personal model:** our own few lines of softmax regression. With a
  dozen numbers a frame there's nothing for a library to do.

What doesn't exist on Linux, and is the reason for the project: a culler
with Photo Mechanic's speed, compare and survey views, face-aware zoom,
clean darktable sidecars, a native Wayland/Omarchy feel, and locally
trained auto-cull.
