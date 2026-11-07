use super::{
    AudioSegment, ClipSpeedAudioMode, ProjectConfiguration, StereoMode, TimelineFrameMapping,
    TimelineSource,
};

const STEP_SAMPLES: usize = 480;
const ATTACK_TICKS: i64 = 8;
const HOLD_TICKS: i64 = 15;
const RELEASE_TICKS: i64 = 65;
const REDUCED_GAIN: f32 = 0.25118864;

pub(super) struct DuckingEnvelope {
    first_tick: usize,
    weights: Vec<f32>,
}

impl DuckingEnvelope {
    pub(super) fn for_audio(
        data: &[AudioSegment],
        project: &ProjectConfiguration,
        start: usize,
        count: usize,
    ) -> Option<Self> {
        let timeline = project.timeline.as_ref()?;
        if project.audio.mute
            || !project.audio.mic_volume_db.is_finite()
            || project.audio.mic_volume_db <= -30.
            || !timeline.audio_segments.iter().any(|segment| {
                segment.enabled && segment.ducking && segment.volume_db > super::MUSIC_SILENCE_DB
            })
            || !data
                .iter()
                .any(|segment| segment.tracks.iter().any(|track| track.is_microphone))
        {
            return None;
        }
        Some(Self::from_activity(start, count, |time| {
            timeline_activity(project, time, |source, transition_gain| {
                let Some(segment) = data.get(source.segment.recording_clip as usize) else {
                    return false;
                };
                let offsets = project
                    .clips
                    .iter()
                    .find(|clip| clip.index == source.segment.recording_clip)
                    .map(|clip| clip.offsets)
                    .unwrap_or_default();
                segment
                    .tracks
                    .iter()
                    .filter(|track| track.is_microphone)
                    .any(|track| {
                        let source_time = source.source_time + f64::from(track.offset(&offsets));
                        if !source_time.is_finite() || source_time < 0. {
                            return false;
                        }
                        let sample = (source_time * 48_000.).round() as usize;
                        let activity = track.data.speech_activity(sample);
                        let gain = 10.0_f32.powf(track.gain(&project.audio) / 20.)
                            * source.segment.volume() as f32
                            * transition_gain;
                        match track.stereo_mode(&project.audio) {
                            StereoMode::MonoL => activity[0].audible_speech(gain),
                            StereoMode::MonoR => activity[1].audible_speech(gain),
                            StereoMode::Stereo => activity
                                .iter()
                                .any(|activity| activity.audible_speech(gain)),
                        }
                    })
            })
        }))
    }

    fn from_activity(start: usize, count: usize, mut active: impl FnMut(f64) -> bool) -> Self {
        let first_tick = start / STEP_SAMPLES;
        let last_tick = start.saturating_add(count).div_ceil(STEP_SAMPLES);
        let from = first_tick.saturating_sub((HOLD_TICKS + RELEASE_TICKS) as usize);
        let to = last_tick.saturating_add(ATTACK_TICKS as usize);
        let voiced = (from..=to)
            .filter(|tick| active(*tick as f64 * 0.01 + 0.005))
            .collect::<Vec<_>>();
        let weights = (first_tick..=last_tick)
            .map(|tick| {
                let split = voiced.partition_point(|voice| *voice <= tick);
                let before = split.checked_sub(1).and_then(|index| voiced.get(index));
                let after = voiced.get(split);
                before
                    .into_iter()
                    .chain(after)
                    .map(|voice| envelope_weight(tick as i64 - *voice as i64))
                    .fold(0., f32::max)
            })
            .collect();
        Self {
            first_tick,
            weights,
        }
    }

    pub(super) fn gain(&self, sample: usize) -> f32 {
        let tick = sample / STEP_SAMPLES;
        let index = tick.saturating_sub(self.first_tick);
        let left = self.weights.get(index).copied().unwrap_or(0.);
        let right = self.weights.get(index + 1).copied().unwrap_or(left);
        let fraction = (sample % STEP_SAMPLES) as f32 / STEP_SAMPLES as f32;
        let weight = left + (right - left) * fraction;
        1. - weight * (1. - REDUCED_GAIN)
    }
}

fn envelope_weight(distance: i64) -> f32 {
    if distance < 0 {
        (1. + distance as f32 / ATTACK_TICKS as f32).clamp(0., 1.)
    } else if distance <= HOLD_TICKS {
        1.
    } else {
        (1. - (distance - HOLD_TICKS) as f32 / RELEASE_TICKS as f32).clamp(0., 1.)
    }
}

fn timeline_activity(
    project: &ProjectConfiguration,
    time: f64,
    mut active: impl FnMut(TimelineSource<'_>, f32) -> bool,
) -> bool {
    if project.audio.mute
        || !project.audio.mic_volume_db.is_finite()
        || project.audio.mic_volume_db <= -30.
    {
        return false;
    }
    let Some(mapping) = project
        .timeline
        .as_ref()
        .and_then(|timeline| timeline.get_frame_mapping(time))
    else {
        return false;
    };
    let mut audible = |source: TimelineSource<'_>, gain: f32| {
        source.segment.speed_audio_mode != Some(ClipSpeedAudioMode::Mute)
            && source.segment.timescale.is_finite()
            && (0.25..=8.).contains(&source.segment.timescale)
            && source.segment.volume() > 0.
            && gain > 0.
            && active(source, gain)
    };
    match mapping {
        TimelineFrameMapping::Single { source, .. } => audible(source, 1.),
        TimelineFrameMapping::Hold { .. } => false,
        TimelineFrameMapping::Transition {
            outgoing,
            incoming,
            kind,
            progress,
            ..
        } => {
            let progress = progress.clamp(0., 1.);
            let (left, right) = if kind.dips() {
                (
                    (1. - progress * 2.).max(0.) as f32,
                    (progress * 2. - 1.).max(0.) as f32,
                )
            } else {
                let angle = progress * std::f64::consts::FRAC_PI_2;
                (angle.cos() as f32, angle.sin() as f32)
            };
            audible(outgoing, left) || audible(incoming, right)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> ProjectConfiguration {
        serde_json::from_value(serde_json::json!({"timeline": {
            "segments": [{"start":2,"end":7,"timescale":1.25,"recordingSegment":0},
                         {"start":2,"end":4,"timescale":1,"recordingSegment":0}],
            "zoomSegments": [],
            "textSegments": [{"start":1,"end":3,"layout":"fullscreen"}]
        }}))
        .unwrap()
    }

    #[test]
    fn ducking_source_mapping_respects_speed_cuts_repeats_holds_and_mutes() {
        let mut project = fixture();
        let source_at = |project: &ProjectConfiguration, time: f64| {
            let mut source = None;
            timeline_activity(project, time, |mapping, _| {
                source = Some(mapping.source_time);
                true
            });
            source
        };
        assert_eq!(source_at(&project, 0.8), Some(3.));
        assert_eq!(source_at(&project, 2.), None);
        assert!(source_at(&project, 3.2).is_some_and(|time| (time - 3.5).abs() < 0.00000001));
        assert_eq!(source_at(&project, 6.5), Some(2.5));
        project.audio.mute = true;
        assert_eq!(source_at(&project, 0.8), None);
        project.audio.mute = false;
        project.audio.mic_volume_db = -30.;
        assert_eq!(source_at(&project, 0.8), None);
        project.audio.mic_volume_db = 0.;
        project.timeline.as_mut().unwrap().segments[0].speed_audio_mode =
            Some(ClipSpeedAudioMode::Mute);
        assert_eq!(source_at(&project, 0.8), None);
    }

    #[test]
    fn ducking_envelope_is_seek_and_chunk_independent_and_recovers_during_hold() {
        let active = |time: f64| (0.5..1.).contains(&time) || (3.5..4.).contains(&time);
        let full = DuckingEnvelope::from_activity(0, 5 * 48_000, active);
        assert_eq!(full.gain(48_000 * 2), 1.);
        assert!((full.gain(36_000) - REDUCED_GAIN).abs() < 0.00001);
        assert!(full.gain(22_000) > REDUCED_GAIN && full.gain(22_000) < 1.);
        for start in [0, 479, 24_001, 45_003, 48_002, 76_031, 144_119, 190_013] {
            let partial = DuckingEnvelope::from_activity(start, 1024, active);
            for sample in start..start + 1024 {
                assert_eq!(partial.gain(sample), full.gain(sample));
            }
        }
    }

    #[test]
    fn ducking_music_gain_multiplies_existing_volume_and_fades_only_when_enabled() {
        let music: ProjectConfiguration = serde_json::from_value(serde_json::json!({"timeline": {
            "segments": [{"start":0,"end":3,"timescale":1}], "zoomSegments": [],
            "audioSegments": [{"start":0,"end":3,"path":"music","fadeIn":1,"fadeOut":1}]
        }}))
        .unwrap();
        let mut enabled = music.clone();
        enabled.timeline.as_mut().unwrap().audio_segments[0].ducking = true;
        enabled.timeline.as_mut().unwrap().audio_segments[0].volume_db = -6.;
        let tracks = std::collections::HashMap::from([(
            "music".into(),
            std::sync::Arc::new(cap_audio::AudioData::from_samples(
                vec![0.25; 48_000 * 3 * 2],
                2,
            )),
        )]);
        let start = 24_000;
        let envelope = DuckingEnvelope::from_activity(start, 512, |_| true);
        let mut unchanged = vec![0.; 1024];
        let mut disabled = unchanged.clone();
        let mut lowered = unchanged.clone();
        super::super::mix_music(
            &tracks,
            music.timeline.as_ref().unwrap(),
            start,
            512,
            &mut unchanged,
            None,
        );
        super::super::mix_music(
            &tracks,
            music.timeline.as_ref().unwrap(),
            start,
            512,
            &mut disabled,
            Some(&envelope),
        );
        super::super::mix_music(
            &tracks,
            enabled.timeline.as_ref().unwrap(),
            start,
            512,
            &mut lowered,
            Some(&envelope),
        );
        assert_eq!(unchanged, disabled);
        for (before, after) in unchanged.iter().zip(lowered) {
            assert!((after - before * 10.0_f32.powf(-6. / 20.) * REDUCED_GAIN).abs() < 0.000001);
        }
    }

    #[test]
    fn ducking_missing_or_silent_microphone_keeps_music_unchanged() {
        let project: ProjectConfiguration =
            serde_json::from_value(serde_json::json!({"timeline": {
                "segments": [{"start":0,"end":3,"timescale":1}], "zoomSegments": [],
                "audioSegments": [{"start":0,"end":3,"path":"music","ducking":true}]
            }}))
            .unwrap();
        assert!(DuckingEnvelope::for_audio(&[], &project, 0, 1024).is_none());
        let silent =
            std::sync::Arc::new(cap_audio::AudioData::from_samples(vec![0.; 48_000 * 3], 1));
        let microphone = super::super::AudioSegmentTrack::new(
            silent,
            |audio| audio.mic_volume_db,
            |_| StereoMode::Stereo,
            |offsets| offsets.mic,
        )
        .with_microphone_enhancement();
        let data = vec![AudioSegment {
            tracks: vec![microphone],
        }];
        let envelope = DuckingEnvelope::for_audio(&data, &project, 0, 1024).unwrap();
        assert!((0..1024).all(|sample| envelope.gain(sample) == 1.));
        let mut muted = project;
        muted.audio.mic_volume_db = -30.;
        assert!(DuckingEnvelope::for_audio(&data, &muted, 0, 1024).is_none());
    }

    #[test]
    fn ducking_settings_invalidate_the_shared_playback_audio_cache() {
        let mut project: ProjectConfiguration =
            serde_json::from_value(serde_json::json!({"timeline": {
                "segments": [{"start":0,"end":3,"timescale":1}], "zoomSegments": [],
                "audioSegments": [{"start":0,"end":3,"path":"music"}]
            }}))
            .unwrap();
        let before = super::super::audio_project_key(&project).unwrap();
        project.timeline.as_mut().unwrap().audio_segments[0].ducking = true;
        assert_ne!(super::super::audio_project_key(&project).unwrap(), before);
        project.timeline.as_mut().unwrap().audio_segments[0].ducking = false;
        assert_eq!(super::super::audio_project_key(&project).unwrap(), before);
    }

    #[test]
    #[ignore = "requires CAP_DUCKING_FIXTURE_DIR containing controlled.wav and controlled.json"]
    fn ducking_real_chinese_fixture_speech_silence_pcm_and_seek_match() {
        use super::super::{AudioRenderer, AudioSegmentTrack};
        use cap_audio::{AudioData, DecodedAudio};
        use std::{collections::HashMap, sync::Arc};
        let directory = std::path::PathBuf::from(
            std::env::var("CAP_DUCKING_FIXTURE_DIR")
                .expect("Set the controlled voice fixture directory"),
        );
        let truth: serde_json::Value =
            serde_json::from_slice(&std::fs::read(directory.join("controlled.json")).unwrap())
                .unwrap();
        let voice = Arc::new(DecodedAudio::from(Arc::new(
            AudioData::from_file(directory.join("controlled.wav")).unwrap(),
        )));
        let frames = voice.sample_count();
        let duration = frames as f64 / 48_000.;
        for phrase in truth["phrase_audio_windows"].as_array().unwrap() {
            let start = (phrase["start"].as_f64().unwrap() * 48_000.).ceil() as usize;
            let end = (phrase["end"].as_f64().unwrap() * 48_000.).floor() as usize;
            let voiced = (start..end)
                .step_by(480)
                .filter(|sample| voice.speech_activity(*sample)[0].audible_speech(1.))
                .count();
            assert!(
                voiced >= 10,
                "No sustained speech detected in {}",
                phrase["text"]
            );
        }
        for silence in truth["inserted_exact_silences"].as_array().unwrap() {
            let start = ((silence[0].as_f64().unwrap() + 0.02) * 48_000.).ceil() as usize;
            let end = ((silence[1].as_f64().unwrap() - 0.02) * 48_000.).floor() as usize;
            assert!(
                (start..end)
                    .step_by(480)
                    .all(|sample| !voice.speech_activity(sample)[0].audible_speech(1.)),
                "Inserted silence activated ducking"
            );
        }
        let project: ProjectConfiguration =
            serde_json::from_value(serde_json::json!({"timeline": {
                "segments": [{"start":0,"end":duration,"timescale":1}], "zoomSegments": [],
                "audioSegments": [{"start":0,"end":duration,"path":"music","ducking":true}]
            }}))
            .unwrap();
        let data = vec![AudioSegment {
            tracks: vec![
                AudioSegmentTrack::from_decoded(
                    voice,
                    |audio| audio.mic_volume_db,
                    |_| StereoMode::Stereo,
                    |offsets| offsets.mic,
                )
                .with_microphone_enhancement(),
            ],
        }];
        let music = HashMap::from([(
            "music".into(),
            Arc::new(AudioData::from_samples(vec![0.03; frames * 2], 2)),
        )]);
        let full = AudioRenderer::new(data.clone())
            .with_music(music.clone())
            .render_frame_raw(frames, &project)
            .unwrap()
            .1;
        let recording = AudioRenderer::new(data.clone())
            .render_frame_raw(frames, &project)
            .unwrap()
            .1;
        let envelope = DuckingEnvelope::for_audio(&data, &project, 0, frames).unwrap();
        assert!(
            (0..frames)
                .step_by(480)
                .any(|sample| envelope.gain(sample) < 0.3)
        );
        assert_eq!(envelope.gain(6 * 48_000 + 12_000), 1.);
        for sample in (0..frames).step_by(479) {
            for channel in 0..2 {
                let expected =
                    (recording[sample * 2 + channel] + 0.03 * envelope.gain(sample)).clamp(-1., 1.);
                assert!((full[sample * 2 + channel] - expected).abs() < 0.000001);
            }
        }
        for start in [24_000, 84_000, 144_000, 300_000, 432_000] {
            let mut renderer = AudioRenderer::new(data.clone()).with_music(music.clone());
            renderer.set_playhead(start as f64 / 48_000., &project);
            let chunk = renderer.render_frame_raw(1024, &project).unwrap().1;
            for (actual, expected) in chunk.iter().zip(&full[start * 2..]) {
                assert!(
                    (actual - expected).abs() < 0.000001,
                    "Seek differs at {start}"
                );
            }
        }
    }
}
