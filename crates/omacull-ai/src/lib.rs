//! Faces and eyes, found with YuNet on the system's ONNX Runtime, as
//! Omapix finds them, and how open the eyes are, from MediaPipe's face
//! landmarker. No UI: the app runs this on a background thread.
//!
//! Nothing is downloaded on its own: ONNX Runtime comes from the system
//! (`/usr/lib/libonnxruntime.so`, or `OMACULL_ORT_LIBRARY`), and the models
//! from `scripts/fetch-models.sh`, or Omapix's copies if they're there.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use omacull_engine::faces::Face;
use omacull_engine::image::Image;
use ort::session::Session;
use ort::value::Tensor;
use sha2::{Digest, Sha256};

/// What went wrong, in words for the status bar.
pub type Result<T> = std::result::Result<T, String>;

/// A model on disk: the folder it's kept in, in Omacull's and in Omapix's
/// models folders, its file, and the variable that names another copy.
struct Model {
    id: &'static str,
    file: &'static str,
    bytes: u64,
    sha256: &'static str,
    variable: &'static str,
}

/// YuNet (2023 March), from OpenCV's model zoo: MIT.
const YUNET: Model = Model {
    id: "face-detect-yunet",
    file: "face_detection_yunet_2023mar.onnx",
    bytes: 232_589,
    sha256: "8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4",
    variable: "OMACULL_YUNET",
};
/// MediaPipe's Face Landmarker, exported to ONNX by senty-au: Apache-2.0.
/// 478 points of a face, the eyelids among them.
const LANDMARKER: Model = Model {
    id: "face-landmarks-mediapipe",
    file: "face_landmarks_detector.onnx",
    bytes: 4_920_995,
    sha256: "7d6e82dee82a1dca5fbddb282b3cc74571833a530de317fc22ae325c3358beeb",
    variable: "OMACULL_LANDMARKER",
};
/// YuNet sees the frame fitted into this square.
const SIZE: usize = 640;
/// Faces it's less sure of than this are left out.
const THRESHOLD: f32 = 0.6;
/// The landmarker sees a face in a square this many pixels across.
const LANDMARK_SIZE: usize = 256;
/// The eyelids, as indices into the landmarker's points: each eye's
/// corners, then three points along the upper lid, each with the one
/// below it on the lower. The eye on the left of the picture first.
const EYELIDS: [([usize; 2], [[usize; 2]; 3]); 2] =
    [([33, 133], [[160, 144], [159, 145], [158, 153]]), ([263, 362], [[387, 373], [386, 374], [385, 380]])];
/// How far apart the lids are, against the eye's width: shut, and wide
/// open. On a real shoot shut eyes read 0.07 to 0.17 and open ones 0.26
/// and up, heavy eye makeup or none.
const SHUT: f32 = 0.12;
const OPEN: f32 = 0.28;

fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Open ONNX Runtime, once.
fn init() -> Result<()> {
    static INIT: OnceLock<Result<()>> = OnceLock::new();
    INIT.get_or_init(|| {
        let library = std::env::var("OMACULL_ORT_LIBRARY").unwrap_or_else(|_| "/usr/lib/libonnxruntime.so".into());
        let builder = ort::init_from(&library).map_err(|e| format!("Couldn't open ONNX Runtime ({library}): {e}"))?;
        builder.with_name("omacull").commit();
        Ok(())
    })
    .clone()
}

fn data_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
}

impl Model {
    fn is(&self, path: &Path) -> bool {
        if !path.metadata().is_ok_and(|m| m.len() == self.bytes) {
            return false;
        }
        let Ok(mut file) = std::fs::File::open(path) else { return false };
        let mut bytes = Vec::new();
        if file.read_to_end(&mut bytes).is_err() {
            return false;
        }
        let sha: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
        sha == self.sha256
    }

    /// The model on disk: the one its variable names, Omacull's own (where
    /// scripts/fetch-models.sh puts it), or Omapix's, which is the same
    /// file. Checked by size and checksum.
    fn find(&self) -> Option<PathBuf> {
        let named = std::env::var_os(self.variable).map(PathBuf::from);
        let kept = |app: &str| data_dir().map(|d| d.join(app).join("models").join(self.id).join(self.file));
        [named, kept("omacull"), kept("omapix")].into_iter().flatten().find(|p| self.is(p))
    }

    fn load(&self, path: &Path) -> Result<Session> {
        Session::builder()
            .and_then(|mut b| b.commit_from_file(path))
            .map_err(|e| format!("Couldn't load {}: {e}", path.display()))
    }
}

/// A square of the frame, turned by `angle` (radians, clockwise on screen)
/// about its centre: what the landmarker sees. As in Omapix.
struct Crop {
    centre: [f32; 2],
    side: f32,
    angle: f32,
}

impl Crop {
    /// Where `(u, v)`, 0 to 1 across and down the crop, is in the frame.
    fn to_image(&self, u: f32, v: f32) -> [f32; 2] {
        let (dx, dy) = ((u - 0.5) * self.side, (v - 0.5) * self.side);
        let (s, c) = self.angle.sin_cos();
        [self.centre[0] + dx * c - dy * s, self.centre[1] + dx * s + dy * c]
    }

    /// The square `scale` times the size of the box round `points` (in the
    /// frame turned by `angle`), as MediaPipe crops faces.
    fn around(points: impl Iterator<Item = [f32; 2]>, angle: f32, scale: f32) -> Self {
        let (s, c) = angle.sin_cos();
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for [x, y] in points {
            let turned = [x * c + y * s, -x * s + y * c];
            for i in 0..2 {
                lo[i] = lo[i].min(turned[i]);
                hi[i] = hi[i].max(turned[i]);
            }
        }
        let (mx, my) = ((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0);
        Self { centre: [mx * c - my * s, mx * s + my * c], side: (hi[0] - lo[0]).max(hi[1] - lo[1]) * scale, angle }
    }

    /// The crop as the landmarker takes it: [`LANDMARK_SIZE`] square, rows
    /// of red, green, blue, 0 to 1; outside the frame is black. Each pixel
    /// averages enough of the frame to cover what it stands for.
    fn sample(&self, image: &Image) -> Vec<f32> {
        let size = LANDMARK_SIZE;
        let taps = (self.side / size as f32).ceil().max(1.0) as usize;
        let at = |x: f32, y: f32| -> [f32; 3] {
            if x < 0.0 || y < 0.0 || x >= image.width as f32 || y >= image.height as f32 {
                return [0.0; 3];
            }
            let p = image.pixel(x as usize, y as usize);
            [0, 1, 2].map(|c| f32::from(p[c]))
        };
        let bilinear = |x: f32, y: f32| -> [f32; 3] {
            let (x0, y0) = (x.floor(), y.floor());
            let (fx, fy) = (x - x0, y - y0);
            let (a, b, c, d) = (at(x0, y0), at(x0 + 1.0, y0), at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0));
            [0, 1, 2].map(|i| (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy)
        };
        let mut out = Vec::with_capacity(size * size * 3);
        for i in 0..size * size {
            let (x, y) = ((i % size) as f32, (i / size) as f32);
            let mut sum = [0.0f32; 3];
            for ty in 0..taps {
                for tx in 0..taps {
                    let u = (x + (tx as f32 + 0.5) / taps as f32) / size as f32;
                    let v = (y + (ty as f32 + 0.5) / taps as f32) / size as f32;
                    let [px, py] = self.to_image(u, v);
                    let p = bilinear(px - 0.5, py - 0.5);
                    for c in 0..3 {
                        sum[c] += p[c];
                    }
                }
            }
            out.extend(sum.map(|s| s / (taps * taps) as f32 / 255.0));
        }
        out
    }
}

/// The angle that levels the line from `a` to `b`.
fn level(a: [f32; 2], b: [f32; 2]) -> f32 {
    (b[1] - a[1]).atan2(b[0] - a[0])
}

/// How far apart each eye's lids are, against its width, from the
/// landmarker's points.
fn lids(points: &[[f32; 2]]) -> [f32; 2] {
    let apart = |a: usize, b: usize| (points[a][0] - points[b][0]).hypot(points[a][1] - points[b][1]);
    EYELIDS.map(|([outer, inner], pairs)| {
        pairs.iter().map(|&[upper, lower]| apart(upper, lower)).sum::<f32>() / 3.0 / apart(outer, inner).max(1e-6)
    })
}

/// 0 (shut) to 1 for how far apart the lids are, the two eyes together:
/// a wink comes out half open, not shut.
fn open([left, right]: [f32; 2]) -> f32 {
    (((left + right) / 2.0 - SHUT) / (OPEN - SHUT)).clamp(0.0, 1.0)
}

/// Finds faces in frames.
pub struct Detector {
    session: Session,
    /// Tells how open the eyes are; faces are found without it.
    landmarker: Option<Session>,
}

impl Detector {
    /// Ready to find faces, or why not: no ONNX Runtime, or no model.
    pub fn new() -> Result<Self> {
        let model = YUNET.find().ok_or("Face detection needs YuNet: run scripts/fetch-models.sh")?;
        init()?;
        let session = YUNET.load(&model)?;
        let landmarker = match LANDMARKER.find().map(|path| LANDMARKER.load(&path)) {
            Some(Ok(landmarker)) => Some(landmarker),
            Some(Err(e)) => {
                log::warn!("{e}");
                None
            }
            None => {
                log::info!("no face landmarker, so no telling open eyes from shut: run scripts/fetch-models.sh");
                None
            }
        };
        Ok(Self { session, landmarker })
    }

    /// How far apart each eye's lids are, against its width (the eye on
    /// the left of the picture first), or None without the landmarker, or
    /// if it doesn't see a face there. The crop is found from the
    /// detection, then again from the first pass's points, as MediaPipe
    /// tracks.
    pub fn eyelids(&mut self, image: &Image, face: &Face) -> Result<Option<[f32; 2]>> {
        let Some(landmarker) = &mut self.landmarker else { return Ok(None) };
        let (w, h) = (image.width as f32, image.height as f32);
        let [l, t, r, b] = [face.bounds[0] * w, face.bounds[1] * h, face.bounds[2] * w, face.bounds[3] * h];
        let eyes = face.eyes.map(|[x, y]| [x * w, y * h]);
        let mut crop = Crop::around([[l, t], [r, t], [l, b], [r, b]].into_iter(), level(eyes[0], eyes[1]), 1.5);
        let mut points: Vec<[f32; 2]> = Vec::new();
        for _ in 0..2 {
            let n = LANDMARK_SIZE as i64;
            let input = Tensor::from_array((vec![1, n, n, 3], crop.sample(image))).map_err(error)?;
            let outputs = landmarker.run(ort::inputs![input]).map_err(error)?;
            let (_, raw) = outputs[0].try_extract_tensor::<f32>().map_err(error)?;
            let (_, presence) = outputs[1].try_extract_tensor::<f32>().map_err(error)?;
            if raw.len() < 478 * 3 || 1.0 / (1.0 + (-presence[0]).exp()) < 0.5 {
                return Ok(None);
            }
            let size = LANDMARK_SIZE as f32;
            points = raw.chunks(3).map(|p| crop.to_image(p[0] / size, p[1] / size)).collect();
            // The eyes' outer corners level the next crop.
            crop = Crop::around(points.iter().copied(), level(points[33], points[263]), 1.5);
        }
        Ok(Some(lids(&points)))
    }

    /// The faces in an upright frame, the surest first, in fractions of
    /// its width and height.
    pub fn detect(&mut self, image: &Image) -> Result<Vec<Face>> {
        // Fitted into the top left of the square, the rest black, as blue,
        // green, red planes of 0 to 255.
        let small = image.shrunk(SIZE);
        let mut planar = vec![0f32; 3 * SIZE * SIZE];
        for y in 0..small.height {
            for x in 0..small.width {
                let p = small.pixel(x, y);
                for (plane, c) in [2, 1, 0].into_iter().enumerate() {
                    planar[plane * SIZE * SIZE + y * SIZE + x] = f32::from(p[c]);
                }
            }
        }
        let n = SIZE as i64;
        let input = Tensor::from_array((vec![1, 3, n, n], planar)).map_err(error)?;
        let outputs = self.session.run(ort::inputs!["input" => input]).map_err(error)?;
        // From the square's pixels to fractions of the frame.
        let (fw, fh) = (small.width as f32, small.height as f32);
        let mut found = Vec::new();
        for stride in [8usize, 16, 32] {
            let get = |name: &str| -> Result<Vec<f32>> {
                let (_, data) =
                    outputs[format!("{name}_{stride}").as_str()].try_extract_tensor::<f32>().map_err(error)?;
                Ok(data.to_vec())
            };
            let (cls, obj, bbox, kps) = (get("cls")?, get("obj")?, get("bbox")?, get("kps")?);
            let cols = SIZE / stride;
            let s = stride as f32;
            for (i, (&cls, &obj)) in cls.iter().zip(&obj).enumerate() {
                let score = (cls.clamp(0.0, 1.0) * obj.clamp(0.0, 1.0)).sqrt();
                if score < THRESHOLD {
                    continue;
                }
                let (col, row) = ((i % cols) as f32, (i / cols) as f32);
                let b = &bbox[i * 4..i * 4 + 4];
                let (cx, cy) = ((col + b[0]) * s, (row + b[1]) * s);
                let (w, h) = (b[2].exp() * s, b[3].exp() * s);
                let k = &kps[i * 10..i * 10 + 10];
                let point = |p: usize| [(k[2 * p] + col) * s / fw, (k[2 * p + 1] + row) * s / fh];
                found.push(Face {
                    score,
                    bounds: [(cx - w / 2.0) / fw, (cy - h / 2.0) / fh, (cx + w / 2.0) / fw, (cy + h / 2.0) / fh],
                    eyes: [point(0), point(1)],
                    open: None,
                });
            }
        }
        drop(outputs);
        let mut faces = suppressed(found);
        for face in &mut faces {
            match self.eyelids(image, face) {
                Ok(lids) => face.open = lids.map(open),
                Err(e) => log::warn!("eyelids: {e}"),
            }
        }
        Ok(faces)
    }
}

/// How much two boxes overlap: their intersection over their union.
fn overlap(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let w = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let h = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let area = |r: &[f32; 4]| (r[2] - r[0]) * (r[3] - r[1]);
    w * h / (area(a) + area(b) - w * h)
}

/// The surest of each cluster of overlapping detections.
fn suppressed(mut found: Vec<Face>) -> Vec<Face> {
    found.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Face> = Vec::new();
    for face in found {
        if kept.iter().all(|k| overlap(&k.bounds, &face.bounds) < 0.3) {
            kept.push(face);
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(score: f32, bounds: [f32; 4]) -> Face {
        Face { score, bounds, eyes: [[0.0; 2]; 2], open: None }
    }

    #[test]
    fn overlapping_detections_keep_the_surest() {
        let kept = suppressed(vec![
            face(0.7, [0.1, 0.1, 0.3, 0.3]),
            face(0.9, [0.11, 0.1, 0.31, 0.3]),
            face(0.8, [0.6, 0.6, 0.7, 0.7]),
        ]);
        let scores: Vec<f32> = kept.iter().map(|f| f.score).collect();
        assert_eq!(scores, [0.9, 0.8]);
    }

    #[test]
    fn only_the_right_file_is_taken_for_the_model() {
        let dir = std::env::temp_dir().join(format!("omacull-ai-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wrong = dir.join(YUNET.file);
        std::fs::write(&wrong, vec![0u8; YUNET.bytes as usize]).unwrap();
        assert!(!YUNET.is(&wrong), "the right size, the wrong bytes");
        assert!(!YUNET.is(&dir.join("missing.onnx")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_crop_is_centred_on_its_points_and_levelled() {
        // A box leaning a quarter turn: its crop is turned with it.
        let corners = [[10.0, 10.0], [10.0, 30.0], [50.0, 10.0], [50.0, 30.0]];
        let crop = Crop::around(corners.into_iter(), level([0.0, 0.0], [0.0, 1.0]), 1.5);
        assert!((crop.centre[0] - 30.0).abs() < 1e-4 && (crop.centre[1] - 20.0).abs() < 1e-4, "{:?}", crop.centre);
        assert!((crop.side - 60.0).abs() < 1e-4);
        let [x, y] = crop.to_image(1.0, 0.5);
        assert!((x - 30.0).abs() < 1e-3 && (y - 50.0).abs() < 1e-3, "right in the crop is down the frame");
    }

    #[test]
    fn lids_far_apart_are_open_and_together_shut() {
        // An eye 30 wide on each side, its lids `gap` apart.
        let face = |gap: f32| {
            let mut points = vec![[0.0f32; 2]; 478];
            for (at, ([outer, inner], pairs)) in [100.0f32, 200.0].into_iter().zip(EYELIDS) {
                (points[outer], points[inner]) = ([at, 50.0], [at + 30.0, 50.0]);
                for [upper, lower] in pairs {
                    (points[upper], points[lower]) = ([at + 15.0, 50.0 - gap / 2.0], [at + 15.0, 50.0 + gap / 2.0]);
                }
            }
            points
        };
        assert_eq!(open(lids(&face(10.0))), 1.0);
        assert_eq!(open(lids(&face(1.0))), 0.0);
        let half = open(lids(&face(6.0)));
        assert!(half > 0.3 && half < 0.7, "{half}");
        // A wink isn't a blink.
        let wink = open([0.1, 0.3]);
        assert!(wink > 0.4 && wink < 0.6, "{wink}");
    }

    /// With ONNX Runtime and the model to hand, and a photo of people:
    ///
    ///     OMACULL_ORT_LIBRARY=… OMACULL_YUNET=… OMACULL_FACES_PHOTO=… \
    ///         cargo test -p omacull-ai finds_faces -- --nocapture
    ///
    /// The photo can be a raw, whose preview is looked at.
    ///
    /// Skipped otherwise.
    #[test]
    fn finds_faces_and_their_eyes_in_a_photo() {
        let Some(photo) = std::env::var_os("OMACULL_FACES_PHOTO") else {
            eprintln!("skipped: OMACULL_FACES_PHOTO isn't set");
            return;
        };
        let photo = PathBuf::from(photo);
        let image = match omacull_engine::image::preview(&photo) {
            Ok(preview) => preview,
            Err(_) => Image::decode_jpeg(&std::fs::read(&photo).unwrap()).unwrap(),
        };
        let mut detector = Detector::new().unwrap();
        let faces = detector.detect(&image).unwrap();
        eprintln!("{} faces: {faces:?}", faces.len());
        for face in &faces {
            eprintln!("eyelids: {:?}", detector.eyelids(&image, face).unwrap());
        }
        assert!(!faces.is_empty());
        for f in &faces {
            let [l, t, r, b] = f.bounds;
            assert!(f.score >= THRESHOLD && l < r && t < b);
            for [x, y] in f.eyes {
                assert!(x > l && x < r && y > t && y < b, "the eyes are in the face");
            }
            assert!(f.eyes[0][0] < f.eyes[1][0], "the left of the picture first");
        }
    }
}
