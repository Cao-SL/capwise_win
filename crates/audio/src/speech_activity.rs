use crate::AudioSampleSource;
use cap_rnnoise::{DenoiseState, FRAME_SIZE};
use std::{
    collections::BTreeMap,
    sync::{Mutex, PoisonError},
};

const BLOCK_FRAMES: usize = 200;
const WARMUP_FRAMES: usize = 50;
const MAX_BLOCKS: usize = 256;

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct SpeechActivity {
    pub probability: f32,
    pub rms: f32,
}

impl SpeechActivity {
    pub fn audible_speech(self, gain: f32) -> bool {
        self.probability.is_finite()
            && self.probability >= 0.6
            && gain.is_finite()
            && gain > 0.
            && self.rms * gain >= 0.0025
    }
}

#[derive(Default)]
pub(crate) struct SpeechActivityCache {
    blocks: Mutex<BTreeMap<usize, Vec<[SpeechActivity; 2]>>>,
}

impl SpeechActivityCache {
    pub(crate) fn at(&self, source: &impl AudioSampleSource, sample: usize) -> [SpeechActivity; 2] {
        if sample >= source.sample_count() || !(1..=2).contains(&source.channels()) {
            return [SpeechActivity::default(); 2];
        }
        let frame = sample / FRAME_SIZE;
        let block = frame / BLOCK_FRAMES;
        let mut blocks = self.blocks.lock().unwrap_or_else(PoisonError::into_inner);
        if blocks.len() >= MAX_BLOCKS && !blocks.contains_key(&block) {
            let _ = blocks.pop_first();
        }
        blocks
            .entry(block)
            .or_insert_with(|| analyze_block(source, block))
            .get(frame % BLOCK_FRAMES)
            .copied()
            .unwrap_or_default()
    }
}

fn analyze_block(source: &impl AudioSampleSource, block: usize) -> Vec<[SpeechActivity; 2]> {
    let channels = usize::from(source.channels());
    let start = block * BLOCK_FRAMES;
    let end = (start + BLOCK_FRAMES).min(source.sample_count().div_ceil(FRAME_SIZE));
    let warm_start = start.saturating_sub(WARMUP_FRAMES);
    let mut states = (0..channels)
        .map(|_| DenoiseState::new())
        .collect::<Vec<_>>();
    let mut frames = Vec::with_capacity(end.saturating_sub(start));
    let mut input = [0.; FRAME_SIZE];
    let mut output = [0.; FRAME_SIZE];
    // Fixed source blocks and pre-roll make VAD independent of playback/seek order.
    for frame in warm_start..end {
        let mut activity = [SpeechActivity::default(); 2];
        for (channel, state) in states.iter_mut().enumerate() {
            let mut energy = 0.;
            for (index, sample) in input.iter_mut().enumerate() {
                let raw = source
                    .sample((frame * FRAME_SIZE + index) * channels + channel)
                    .copied()
                    .filter(|sample| sample.is_finite())
                    .unwrap_or(0.)
                    .clamp(-1., 1.);
                energy += raw * raw;
                *sample = raw * 32768.;
            }
            activity[channel] = SpeechActivity {
                probability: state.process_frame(&mut output, &input),
                rms: (energy / FRAME_SIZE as f32).sqrt(),
            };
        }
        if channels == 1 {
            activity[1] = activity[0];
        }
        if frame >= start {
            frames.push(activity);
        }
    }
    frames
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AudioData;

    #[test]
    fn speech_activity_requires_probability_and_audible_energy() {
        assert!(
            !SpeechActivity {
                probability: 0.1,
                rms: 0.5
            }
            .audible_speech(1.)
        );
        assert!(
            !SpeechActivity {
                probability: 0.9,
                rms: 0.0001
            }
            .audible_speech(1.)
        );
        assert!(
            !SpeechActivity {
                probability: 0.9,
                rms: 0.1
            }
            .audible_speech(0.)
        );
        assert!(
            SpeechActivity {
                probability: 0.9,
                rms: 0.1
            }
            .audible_speech(1.)
        );
    }

    #[test]
    fn speech_activity_real_rnnoise_silence_nonfinite_and_random_access_are_deterministic() {
        let data = AudioData::from_samples(
            (0..48_000 * 5)
                .flat_map(|sample| {
                    let tone = (sample as f32 * 0.013).sin() * 0.1;
                    [if sample % 701 == 0 { f32::NAN } else { tone }, 0.]
                })
                .collect(),
            2,
        );
        let forward = SpeechActivityCache::default();
        let reverse = SpeechActivityCache::default();
        let positions = [0, 480, 47_999, 95_999, 96_000, 144_000, 192_001, 239_999];
        let expected = positions.map(|position| forward.at(&data, position));
        for (position, expected) in positions.into_iter().zip(expected).rev() {
            let actual = reverse.at(&data, position);
            assert_eq!(actual, expected);
            assert!(actual[0].probability.is_finite());
            assert!(actual[0].rms.is_finite());
            assert!(!actual[1].audible_speech(1.));
        }
        assert_eq!(
            forward.at(&data, data.sample_count()),
            [SpeechActivity::default(); 2]
        );
    }
}
