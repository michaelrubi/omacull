//! Faces and eyes, found with YuNet on the system's ONNX Runtime, as
//! Omapix finds them. No UI: the app runs this on a background thread.
//!
//! Nothing is downloaded on its own: ONNX Runtime comes from the system
//! (`/usr/lib/libonnxruntime.so`, or `OMACULL_ORT_LIBRARY`), and the model
//! from `scripts/fetch-models.sh`, or Omapix's copy if it's there.

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

/// YuNet (2023 March), from OpenCV's model zoo: MIT.
pub const MODEL: &str = "face_detection_yunet_2023mar.onnx";
const MODEL_BYTES: u64 = 232_589;
const MODEL_SHA256: &str = "8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4";
/// The folder it's kept in, in Omacull's and in Omapix's models folders.
const MODEL_ID: &str = "face-detect-yunet";
/// YuNet sees the frame fitted into this square.
const SIZE: usize = 640;
/// Faces it's less sure of than this are left out.
const THRESHOLD: f32 = 0.6;

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

/// Where scripts/fetch-models.sh puts the model.
pub fn own_model() -> Option<PathBuf> {
    Some(data_dir()?.join("omacull/models").join(MODEL_ID).join(MODEL))
}

fn is_model(path: &Path) -> bool {
    if !path.metadata().is_ok_and(|m| m.len() == MODEL_BYTES) {
        return false;
    }
    let Ok(mut file) = std::fs::File::open(path) else { return false };
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return false;
    }
    let sha: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    sha == MODEL_SHA256
}

/// The model on disk: `OMACULL_YUNET`, Omacull's own, or Omapix's, which
/// is the same file. Checked by size and checksum.
pub fn find_model() -> Option<PathBuf> {
    let named = std::env::var_os("OMACULL_YUNET").map(PathBuf::from);
    let omapix = data_dir().map(|d| d.join("omapix/models").join(MODEL_ID).join(MODEL));
    [named, own_model(), omapix].into_iter().flatten().find(|p| is_model(p))
}

/// Finds faces in frames.
pub struct Detector {
    session: Session,
}

impl Detector {
    /// Ready to find faces, or why not: no ONNX Runtime, or no model.
    pub fn new() -> Result<Self> {
        let model = find_model().ok_or("Face detection needs YuNet: run scripts/fetch-models.sh")?;
        init()?;
        let session = Session::builder()
            .and_then(|mut b| b.commit_from_file(&model))
            .map_err(|e| format!("Couldn't load {}: {e}", model.display()))?;
        Ok(Self { session })
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
                });
            }
        }
        Ok(suppressed(found))
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
        Face { score, bounds, eyes: [[0.0; 2]; 2] }
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
        let wrong = dir.join(MODEL);
        std::fs::write(&wrong, vec![0u8; MODEL_BYTES as usize]).unwrap();
        assert!(!is_model(&wrong), "the right size, the wrong bytes");
        assert!(!is_model(&dir.join("missing.onnx")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// With ONNX Runtime and the model to hand, and a photo of people:
    ///
    ///     OMACULL_ORT_LIBRARY=… OMACULL_YUNET=… OMACULL_FACES_PHOTO=… \
    ///         cargo test -p omacull-ai finds_faces -- --nocapture
    ///
    /// Skipped otherwise.
    #[test]
    fn finds_faces_and_their_eyes_in_a_photo() {
        let Some(photo) = std::env::var_os("OMACULL_FACES_PHOTO") else {
            eprintln!("skipped: OMACULL_FACES_PHOTO isn't set");
            return;
        };
        let jpeg = std::fs::read(photo).unwrap();
        let image = Image::decode_jpeg(&jpeg).unwrap();
        let mut detector = Detector::new().unwrap();
        let faces = detector.detect(&image).unwrap();
        eprintln!("{} faces: {faces:?}", faces.len());
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
