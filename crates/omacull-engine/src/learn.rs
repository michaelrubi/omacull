//! The personal model: trained on the decision log, on this machine, to
//! guess whether this user would reject a frame, leave it or keep it, and
//! with how many stars. It only ever suggests, and only when it's sure.
//!
//! It sees a frame as its signals and how it stands among the frames it
//! was shot with ([`crate::signals`]), each logged with the decision, so
//! the log is all it needs. What it's given to learn is what each frame
//! ended as: its last row. The model is small and plain, a weight for each
//! thing measured, so it trains in a moment and can't learn much more than
//! "soft, shut-eyed and second-best frames go": taste in pictures is
//! beyond it.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cull::Rating;
use crate::sidecar::REJECT;
use crate::signals::{By, Signals, Standing, Suggestion};
use crate::thumbs;

/// Bumped when the model sees frames differently, so an old one isn't used.
const VERSION: u32 = 1;
/// How many decisions it takes to learn from, and how many of a kind for
/// the kind to count.
pub const NEEDED: usize = 100;
const NEEDED_OF_A_KIND: usize = 10;
/// One frame in this many is held back, to test what was learned.
const HELD_BACK: u64 = 5;
const FEATURES: usize = 14;
/// What a frame ended as: rejected, left unmarked, or kept.
const KINDS: usize = 3;
const ROUNDS: usize = 300;
const RATE: f32 = 0.5;
/// Pulls weights towards nothing, so a few odd frames don't sway it.
const DECAY: f32 = 1e-3;

/// What the model sees of a frame.
fn features(signals: &Signals, standing: &Standing) -> [f32; FEATURES] {
    // Sharpness runs from 0 to 255; what matters is how many times sharper.
    let level = |v: Option<f32>| v.map_or(0.0, |v| (1.0 + v.max(0.0)).ln() / 256f32.ln());
    let known = |v: Option<f32>| f32::from(u8::from(v.is_some()));
    [
        level(signals.focus),
        known(signals.focus),
        level(signals.eyes),
        known(signals.eyes),
        signals.open.unwrap_or(0.0),
        known(signals.open),
        // Shut is shut: it's not that eyes half as open are half as good.
        f32::from(u8::from(signals.shut())),
        signals.highlights.max(0.0).sqrt(),
        signals.shadows.max(0.0).sqrt(),
        signals.faces.min(5) as f32 / 5.0,
        signals.face.max(0.0).sqrt(),
        standing.sharp.unwrap_or(1.0),
        standing.open.unwrap_or(1.0),
        ((standing.of.max(1) as f32).ln() / 20f32.ln()).min(1.0),
    ]
}

fn kind(rating: Rating) -> usize {
    match rating {
        r if r < 0 => 0,
        0 => 1,
        _ => 2,
    }
}

/// A frame and what the user made of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Example {
    features: [f32; FEATURES],
    pub rating: Rating,
    /// Says which frames are held back, the same every time.
    key: u64,
}

impl Example {
    pub fn new(signals: &Signals, standing: &Standing, rating: Rating, key: u64) -> Self {
        Self { features: features(signals, standing), rating, key }
    }
}

/// The parts of a decision log row that are learned from.
#[derive(Deserialize)]
struct Row {
    file: RowFile,
    rating: Rating,
    #[serde(default)]
    signals: Option<Signals>,
    #[serde(default)]
    standing: Option<Standing>,
}

#[derive(Deserialize)]
struct RowFile {
    path: PathBuf,
    #[serde(default)]
    captured: Option<String>,
}

/// What each frame in the decision log ended as, for those whose signals
/// were logged (from M7 on). Rows that can't be read are skipped.
pub fn examples(log: &Path) -> io::Result<Vec<Example>> {
    let text = fs::read_to_string(log)?;
    // By path and capture time: a card's file names come round again.
    let mut frames: BTreeMap<String, (Rating, Option<[f32; FEATURES]>)> = BTreeMap::new();
    for row in text.lines().filter_map(|line| serde_json::from_str::<Row>(line).ok()) {
        let key = format!("{}|{}", row.file.path.display(), row.file.captured.unwrap_or_default());
        let seen = row.signals.map(|signals| features(&signals, &row.standing.unwrap_or_default()));
        let frame = frames.entry(key).or_insert((row.rating, None));
        *frame = (row.rating, seen.or(frame.1));
    }
    Ok(frames
        .into_iter()
        .filter_map(|(key, (rating, features))| {
            Some(Example { features: features?, rating, key: thumbs::fnv(key.as_bytes(), 0xcbf2_9ce4_8422_2325) })
        })
        .collect())
}

/// What was learned.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Model {
    v: u32,
    /// When it was trained, in milliseconds since 1970.
    pub trained: u64,
    /// How big the decision log was then: bigger now, there's more to
    /// learn.
    pub log_bytes: u64,
    /// How many frames it learned from: rejected, left unmarked, kept.
    pub learned: [usize; KINDS],
    /// How it did on the frames held back: how many there were, for how
    /// many it would have suggested a mark, and for how many of those the
    /// mark was the user's.
    pub held: usize,
    pub sure: usize,
    pub right: usize,
    /// Each feature's middle and spread, to even them out.
    middle: [f32; FEATURES],
    spread: [f32; FEATURES],
    /// For each kind, a weight a feature and one over.
    weights: [[f32; FEATURES + 1]; KINDS],
    /// The same for how many stars a kept frame gets.
    stars: [f32; FEATURES + 1],
}

fn evened(features: &[f32; FEATURES], middle: &[f32; FEATURES], spread: &[f32; FEATURES]) -> [f32; FEATURES] {
    std::array::from_fn(|i| (features[i] - middle[i]) / spread[i])
}

fn weighed(weights: &[f32; FEATURES + 1], x: &[f32; FEATURES]) -> f32 {
    weights[FEATURES] + weights.iter().zip(x).map(|(w, x)| w * x).sum::<f32>()
}

/// The chance of each kind.
fn chances(weights: &[[f32; FEATURES + 1]; KINDS], x: &[f32; FEATURES]) -> [f32; KINDS] {
    let scores = weights.each_ref().map(|w| weighed(w, x));
    let top = scores.iter().copied().fold(f32::MIN, f32::max);
    let raised = scores.map(|s| (s - top).exp());
    let sum: f32 = raised.iter().sum();
    raised.map(|r| r / sum)
}

/// Weights fitted to frames by walking down the slope of how wrong they
/// are, `slope` giving for a frame how wrong each output is.
fn fit<const OUT: usize>(
    xs: &[[f32; FEATURES]],
    slope: impl Fn(&[[f32; FEATURES + 1]; OUT], usize) -> [f32; OUT],
) -> [[f32; FEATURES + 1]; OUT] {
    let mut weights = [[0.0; FEATURES + 1]; OUT];
    let n = xs.len().max(1) as f32;
    for _ in 0..ROUNDS {
        let mut down = [[0.0; FEATURES + 1]; OUT];
        for (i, x) in xs.iter().enumerate() {
            for (out, wrong) in slope(&weights, i).into_iter().enumerate() {
                for (d, x) in down[out].iter_mut().zip(x) {
                    *d += wrong * x;
                }
                down[out][FEATURES] += wrong;
            }
        }
        for (w, d) in weights.iter_mut().zip(&down) {
            for j in 0..=FEATURES {
                let decay = if j < FEATURES { DECAY * w[j] } else { 0.0 };
                w[j] -= RATE * (d[j] / n + decay);
            }
        }
    }
    weights
}

impl Model {
    fn fitted(examples: &[&Example]) -> Self {
        let n = examples.len().max(1) as f32;
        let middle: [f32; FEATURES] = std::array::from_fn(|j| examples.iter().map(|e| e.features[j]).sum::<f32>() / n);
        let spread: [f32; FEATURES] = std::array::from_fn(|j| {
            let variance = examples.iter().map(|e| (e.features[j] - middle[j]).powi(2)).sum::<f32>() / n;
            // A feature that never varies says nothing.
            if variance > 1e-12 { variance.sqrt() } else { 1.0 }
        });
        let xs: Vec<[f32; FEATURES]> = examples.iter().map(|e| evened(&e.features, &middle, &spread)).collect();
        let weights = fit(&xs, |weights, i| {
            let mut wrong = chances(weights, &xs[i]);
            wrong[kind(examples[i].rating)] -= 1.0;
            wrong
        });
        // Stars, among the kept.
        let kept: Vec<usize> = (0..examples.len()).filter(|&i| examples[i].rating > 0).collect();
        let kept_xs: Vec<[f32; FEATURES]> = kept.iter().map(|&i| xs[i]).collect();
        // Half the rate: being wrong by a star counts for more than being
        // wrong about a kind.
        let [stars] = fit(&kept_xs, |weights, i| [(weighed(&weights[0], &kept_xs[i]) - examples[kept[i]].rating as f32) / 2.0]);
        let mut learned = [0; KINDS];
        for example in examples {
            learned[kind(example.rating)] += 1;
        }
        Self { v: VERSION, trained: 0, log_bytes: 0, learned, held: 0, sure: 0, right: 0, middle, spread, weights, stars }
    }

    /// The mark it would make on a frame, if it's at least `confidence`
    /// sure of one.
    pub fn suggest(&self, signals: &Signals, standing: &Standing, confidence: f32) -> Option<Suggestion> {
        self.suggest_for(&features(signals, standing), confidence)
    }

    fn suggest_for(&self, features: &[f32; FEATURES], confidence: f32) -> Option<Suggestion> {
        let x = evened(features, &self.middle, &self.spread);
        let [reject, _, keep] = chances(&self.weights, &x);
        if reject >= confidence {
            Some(Suggestion { rating: REJECT, by: By::Model, confidence: reject })
        } else if keep >= confidence {
            let stars = weighed(&self.stars, &x).round().clamp(1.0, 5.0) as Rating;
            Some(Suggestion { rating: stars, by: By::Model, confidence: keep })
        } else {
            None
        }
    }

    pub fn load(path: &Path) -> Option<Self> {
        let model: Self = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
        (model.v == VERSION).then_some(model)
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(path, serde_json::to_vec(self)?)
    }
}

/// Learn from what frames ended as, suggesting only when `confidence`
/// sure. Says why not, if there isn't enough to learn from.
pub fn train(examples: &[Example], confidence: f32) -> Result<Model, String> {
    if examples.len() < NEEDED {
        return Err(format!("Not enough decisions to learn from yet: {} of the {NEEDED} it takes", examples.len()));
    }
    let mut of_a_kind = [0; KINDS];
    for example in examples {
        of_a_kind[kind(example.rating)] += 1;
    }
    if of_a_kind.iter().filter(|&&n| n >= NEEDED_OF_A_KIND).count() < 2 {
        let all = ["rejected", "left unmarked", "kept"][(0..KINDS).max_by_key(|&k| of_a_kind[k]).unwrap_or(1)];
        return Err(format!("Nothing to learn yet: nearly every frame so far was {all}"));
    }
    // Learn from four in five, and see how that does on the fifth; then
    // learn from them all.
    let (held, taught): (Vec<&Example>, Vec<&Example>) = examples.iter().partition(|e| e.key % HELD_BACK == 0);
    let trial = Model::fitted(&taught);
    let guesses: Vec<(Rating, &Example)> =
        held.iter().filter_map(|e| Some((trial.suggest_for(&e.features, confidence)?.rating, *e))).collect();
    let mut model = Model::fitted(&examples.iter().collect::<Vec<_>>());
    model.held = held.len();
    model.sure = guesses.len();
    model.right = guesses.iter().filter(|(rating, e)| kind(*rating) == kind(e.rating)).count();
    Ok(model)
}

/// Learn from the decision log as it stands.
pub fn learn(log: &Path, confidence: f32, now: u64) -> Result<Model, String> {
    let examples = examples(log).map_err(|e| format!("Couldn't read the decision log: {e}"))?;
    let mut model = train(&examples, confidence)?;
    model.trained = now;
    model.log_bytes = fs::metadata(log).map_or(0, |m| m.len());
    Ok(model)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cull::Filter;
    use crate::disk::{Disk, How, Mark};
    use crate::testing::{Arw, Folder};
    use std::time::{Duration, UNIX_EPOCH};

    fn seen(sharp: f32, open: f32) -> (Signals, Standing) {
        let signals = Signals { focus: Some(60.0 * sharp), eyes: Some(80.0 * sharp), open: Some(open), ..Signals::default() };
        (signals, Standing { of: 4, sharp: Some(sharp), open: Some(open) })
    }

    /// A cull where the soft and the shut-eyed go, the sharpest are kept
    /// with stars for the sharpest of all, and the middling are left.
    fn cull(frames: usize) -> Vec<Example> {
        (0..frames)
            .map(|i| {
                // Spread over sharpness and openness, evenly and repeatably.
                let sharp = 0.3 + 0.7 * (i * 37 % 100) as f32 / 99.0;
                let open = if i % 7 == 0 { 0.05 } else { 0.6 + 0.4 * (i * 13 % 10) as f32 / 9.0 };
                let rating = match () {
                    () if open < 0.3 || sharp < 0.55 => REJECT,
                    () if sharp > 0.93 => 3,
                    () if sharp > 0.8 => 1,
                    () => 0,
                };
                let (signals, standing) = seen(sharp, open);
                Example::new(&signals, &standing, rating, i as u64)
            })
            .collect()
    }

    #[test]
    fn it_learns_what_goes_and_what_stays_and_keeps_quiet_between() {
        let model = train(&cull(600), 0.8).unwrap();
        let suggest = |sharp, open| {
            let (signals, standing) = seen(sharp, open);
            model.suggest(&signals, &standing, 0.8).map(|s| s.rating)
        };
        assert_eq!(suggest(0.35, 0.9), Some(REJECT), "soft");
        assert_eq!(suggest(0.9, 0.02), Some(REJECT), "shut eyes");
        assert_eq!(suggest(1.0, 1.0), Some(3), "the sharpest, with the stars they got");
        assert_eq!(suggest(0.72, 0.9), None, "not sure of the middling");
        assert_eq!(model.learned.iter().sum::<usize>(), 600);
        assert_eq!(model.held, 120, "a fifth held back");
        assert!(model.sure > 30 && model.right * 10 >= model.sure * 9, "{} sure, {} right", model.sure, model.right);
        // The surer it has to be, the less it suggests.
        let (signals, standing) = seen(0.5, 0.9);
        assert!(model.suggest(&signals, &standing, 0.999).is_none());
        assert!(model.suggest(&signals, &standing, 0.5).is_some_and(|s| s.by == By::Model && s.confidence >= 0.5));
    }

    #[test]
    fn it_wont_learn_from_too_little() {
        let few = train(&cull(50), 0.8).unwrap_err();
        assert!(few.contains("50 of the 100"), "{few}");
        let (signals, standing) = seen(0.9, 0.9);
        let kept: Vec<Example> = (0..200).map(|i| Example::new(&signals, &standing, 1, i)).collect();
        let one_kind = train(&kept, 0.8).unwrap_err();
        assert!(one_kind.contains("kept"), "{one_kind}");
    }

    #[test]
    fn a_model_is_saved_and_read_back() {
        let folder = Folder::new("learn-model");
        let path = folder.0.join("data").join("model.json");
        assert_eq!(Model::load(&path), None);
        let model = train(&cull(200), 0.8).unwrap();
        model.save(&path).unwrap();
        assert_eq!(Model::load(&path), Some(model.clone()));
        // One that saw frames another way isn't used.
        Model { v: VERSION + 1, ..model }.save(&path).unwrap();
        assert_eq!(Model::load(&path), None);
    }

    #[test]
    fn the_log_is_read_for_what_each_frame_ended_as() {
        let folder = Folder::with_raws("learn-log", 4, &Arw::default());
        let log = folder.0.join("decisions.jsonl");
        let mut disk = Disk::new(Some(log.clone()), 0, || {});
        let mark = |i: usize, rating, how, seen: Option<(Signals, Standing)>| Mark {
            path: folder.raw(i),
            rating,
            was: 0,
            how,
            view: "loupe",
            compared: Vec::new(),
            filter: Filter::All,
            dwell: Duration::ZERO,
            at: UNIX_EPOCH,
            signals: seen.map(|s| s.0),
            standing: seen.map(|s| s.1),
            suggested: None,
        };
        // Picked, then rejected after all; measured only the first time.
        disk.write(mark(1, 1, How::Mark, Some(seen(0.9, 0.9))));
        disk.write(mark(1, REJECT, How::Mark, None));
        disk.write(mark(2, 0, How::Pass, Some(seen(0.5, 0.5))));
        // Never measured: nothing to learn from.
        disk.write(mark(3, 4, How::Mark, None));
        disk.finish();
        fs::write(&log, fs::read_to_string(&log).unwrap() + "not a row\n").unwrap();

        let examples = examples(&log).unwrap();
        let ratings: Vec<Rating> = examples.iter().map(|e| e.rating).collect();
        assert_eq!(ratings, [REJECT, 0]);
        let (signals, standing) = seen(0.9, 0.9);
        assert_eq!(examples[0].features, features(&signals, &standing));
        assert!(learn(&log, 0.8, 0).unwrap_err().contains("2 of the 100"));
        assert!(learn(&folder.0.join("missing.jsonl"), 0.8, 0).unwrap_err().contains("Couldn't read"));
    }
}
