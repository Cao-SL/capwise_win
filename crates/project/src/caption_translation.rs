use crate::{CaptionSettings, CaptionTrackSegment, caption_display_text, source_caption_id};
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Type, Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CaptionDisplayMode {
    #[default]
    Original,
    Translated,
    Bilingual,
}

#[derive(Type, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CaptionTranslation {
    pub source_text: String,
    pub text: String,
    pub language: String,
}

impl CaptionSettings {
    pub fn translation_for(&self, id: &str, source_text: &str) -> Option<&CaptionTranslation> {
        let source_text = caption_display_text(source_text);
        [id, source_caption_id(id)].into_iter().find_map(|key| {
            self.translations.get(key).filter(|translation| {
                !translation.text.trim().is_empty()
                    && caption_display_text(&translation.source_text) == source_text
            })
        })
    }

    pub fn set_translation(&mut self, id: &str, source_text: &str, text: String, language: String) {
        self.translations.insert(
            id.to_string(),
            CaptionTranslation {
                source_text: caption_display_text(source_text),
                text: caption_display_text(&text),
                language,
            },
        );
    }
}

pub fn caption_presentation_text(
    segment: &CaptionTrackSegment,
    settings: &CaptionSettings,
) -> String {
    let Some(display) =
        crate::caption_words::caption_display_segment(segment, settings.hides_fillers())
    else {
        return String::new();
    };
    let original = caption_display_text(&display.text);
    let translation = settings.translation_for(&segment.id, &original);
    let text = match (settings.display_mode, translation) {
        (CaptionDisplayMode::Translated, Some(translation)) => {
            caption_display_text(&translation.text)
        }
        (CaptionDisplayMode::Bilingual, Some(translation)) => {
            format!("{original}\n{}", caption_display_text(&translation.text))
        }
        _ => original,
    };
    if settings.uppercase {
        text.to_uppercase()
    } else {
        text
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubtitleFormat {
    Srt,
    Vtt,
}

fn subtitle_timestamp(seconds: f64, format: SubtitleFormat) -> String {
    let milliseconds = (seconds.max(0.0) * 1000.0).round() as u64;
    let separator = match format {
        SubtitleFormat::Srt => ',',
        SubtitleFormat::Vtt => '.',
    };
    format!(
        "{:02}:{:02}:{:02}{separator}{:03}",
        milliseconds / 3_600_000,
        milliseconds / 60_000 % 60,
        milliseconds / 1000 % 60,
        milliseconds % 1000,
    )
}

pub fn export_subtitles(
    segments: &[CaptionTrackSegment],
    settings: &CaptionSettings,
    format: SubtitleFormat,
) -> String {
    let mut ordered: Vec<_> = segments.iter().collect();
    ordered.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut output = match format {
        SubtitleFormat::Srt => String::new(),
        SubtitleFormat::Vtt => "WEBVTT\n\n".to_string(),
    };
    let mut index = 1;
    for segment in ordered {
        let Some(display) =
            crate::caption_words::caption_display_segment(segment, settings.hides_fillers())
        else {
            continue;
        };
        let end = display.words.last().map_or(display.end, |word| {
            display
                .end
                .min(f64::from(word.start) + crate::MAX_CAPTION_WORD_DURATION as f64)
        });
        let text = caption_presentation_text(&display, settings);
        if !display.start.is_finite() || !end.is_finite() || end <= display.start || text.is_empty()
        {
            continue;
        }
        output.push_str(&format!(
            "{index}\n{} --> {}\n{text}\n\n",
            subtitle_timestamp(display.start, format),
            subtitle_timestamp(end, format),
        ));
        index += 1;
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(id: &str, text: &str) -> CaptionTrackSegment {
        serde_json::from_value(serde_json::json!({
            "id": id, "start": 1.25, "end": 3.5, "text": text
        }))
        .unwrap()
    }

    fn bilingual() -> CaptionSettings {
        let mut settings = CaptionSettings {
            display_mode: CaptionDisplayMode::Bilingual,
            ..Default::default()
        };
        settings.set_translation("intro", "开始录制", "Start recording".into(), "en".into());
        settings
    }

    #[test]
    fn legacy_settings_keep_original_captions_and_translation_roundtrips() {
        let legacy: CaptionSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.display_mode, CaptionDisplayMode::Original);
        assert!(legacy.translations.is_empty());
        let settings = bilingual();
        let restored: CaptionSettings =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(restored.display_mode, CaptionDisplayMode::Bilingual);
        assert_eq!(restored.translations, settings.translations);
    }

    #[test]
    fn translations_follow_unchanged_projected_captions_and_reject_source_edits() {
        let settings = bilingual();
        assert_eq!(
            caption_presentation_text(&segment("intro::edl2", "开 始 录 制"), &settings),
            "开始录制\nStart recording"
        );
        assert_eq!(
            caption_presentation_text(&segment("intro::edl2", "录制"), &settings),
            "录制"
        );
        assert_eq!(
            caption_presentation_text(&segment("intro", "停止录制"), &settings),
            "停止录制"
        );
    }

    #[test]
    fn translated_mode_does_not_change_source_or_timing() {
        let mut settings = bilingual();
        settings.display_mode = CaptionDisplayMode::Translated;
        let source = segment("intro", "开始录制");
        assert_eq!(
            caption_presentation_text(&source, &settings),
            "Start recording"
        );
        assert_eq!(source.text, "开始录制");
        assert_eq!((source.start, source.end), (1.25, 3.5));
        assert!(source.words.is_empty());
    }

    #[test]
    fn subtitle_files_keep_one_timed_cue_for_both_languages() {
        let settings = bilingual();
        let segments = [segment("intro", "开始录制")];
        assert_eq!(
            export_subtitles(&segments, &settings, SubtitleFormat::Srt),
            "1\n00:00:01,250 --> 00:00:03,500\n开始录制\nStart recording\n\n"
        );
        assert_eq!(
            export_subtitles(&segments, &settings, SubtitleFormat::Vtt),
            "WEBVTT\n\n1\n00:00:01.250 --> 00:00:03.500\n开始录制\nStart recording\n\n"
        );
    }

    #[test]
    fn real_projection_reuses_whole_captions_but_not_partially_cut_words() {
        let source: crate::CaptionSegment = serde_json::from_value(serde_json::json!({
            "id": "intro", "start": 1.0, "end": 3.0, "text": "开始录制",
            "words": [
                {"text": "开始", "start": 1.0, "end": 1.5},
                {"text": "录制", "start": 2.0, "end": 3.0}
            ]
        }))
        .unwrap();
        let timeline: crate::TimelineConfiguration = serde_json::from_value(serde_json::json!({
            "segments": [
                {"recordingSegment": 0, "timescale": 1.0, "start": 0.0, "end": 4.0},
                {"recordingSegment": 0, "timescale": 1.0, "start": 2.0, "end": 4.0}
            ], "zoomSegments": []
        }))
        .unwrap();
        let derived = crate::derive_caption_track_segments(&[source], &timeline, &[4.0]);
        assert_eq!(derived.len(), 2);
        let settings = bilingual();
        assert_eq!(
            caption_presentation_text(&derived[0], &settings),
            "开始录制\nStart recording"
        );
        assert_eq!(caption_presentation_text(&derived[1], &settings), "录制");
        assert_eq!(derived[1].start, 4.0);
    }
}
