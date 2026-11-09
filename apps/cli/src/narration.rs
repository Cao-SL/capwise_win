use std::{
    fs,
    future::Future,
    io::{ErrorKind, Read},
    path::{Path, PathBuf},
};

use cap_project::{
    AudioTrackSegment, CaptionSegment, CaptionWord, CaptionsData, ProjectConfiguration,
    RecordingMeta, synchronize_captions,
};
use cap_utils::speech;
use clap::Args;
use serde_json::{Value, json};

use crate::{
    OutputFormat,
    project_edit::{Result, TemporaryFile, error, io_error, revision, write_checked_with},
    resolve_format, write_json,
};

#[derive(Args)]
pub struct NarrationArgs {
    project_path: PathBuf,
    #[arg(
        long,
        required_unless_present = "text_file",
        conflicts_with = "text_file"
    )]
    text: Option<String>,
    #[arg(long, help = "Read narration text from a UTF-8 file")]
    text_file: Option<PathBuf>,
    #[arg(long, default_value_t = 0.0, help = "Start in edited output seconds")]
    start: f64,
    #[arg(
        long,
        help = "Add timed Chinese or English subtitles to a project without captions"
    )]
    captions: bool,
    #[arg(
        long,
        help = "Validate the request without contacting Doubao or writing files"
    )]
    dry_run: bool,
    #[arg(
        long,
        required_unless_present = "dry_run",
        help = "Revision from dry-run"
    )]
    expected_revision: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
}

impl NarrationArgs {
    pub async fn run(self, json_output: bool) -> std::result::Result<(), String> {
        let format = resolve_format(json_output, self.format);
        match self.apply().await {
            Ok(value) if format == OutputFormat::Json => write_json(&value),
            Ok(value) => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&value).map_err(|cause| cause.to_string())?
                );
                Ok(())
            }

            Err(cause) => {
                if format == OutputFormat::Json {
                    write_json(
                        &json!({"error": cause.message, "code": cause.code, "retryable": cause.retryable()}),
                    )?;
                }
                Err(cause.message)
            }
        }
    }

    async fn apply(self) -> Result<Value> {
        self.apply_with(&speech::app_store_path(), |settings, text, captions, reuse_recognition| async move {
            let key = speech_key(reuse_recognition).ok_or_else(|| {
                error("TTS_NOT_CONFIGURED", "Set a Doubao Speech API key in Capwise AI Models or CAPWISE_DOUBAO_TTS_API_KEY")
            })?;
            speech::synthesize(&settings, &key, &text, captions)
                .await
                .map_err(|message| error("TTS_FAILED", message))
        })
        .await
    }

    async fn apply_with<F, T>(self, store_path: &Path, synthesize: F) -> Result<Value>
    where
        F: FnOnce(speech::Settings, String, bool, bool) -> T,
        T: Future<Output = Result<speech::Audio>>,
    {
        let text = if let Some(path) = self.text_file {
            let mut text = Vec::new();
            fs::File::open(path)
                .map_err(io_error)?
                .take(4_001)
                .read_to_end(&mut text)
                .map_err(io_error)?;
            if text.len() > 4_000 {
                return Err(error(
                    "INVALID_ARGUMENT",
                    "Narration text file exceeds 4,000 bytes",
                ));
            }
            String::from_utf8(text)
                .map_err(|_| error("INVALID_ARGUMENT", "Narration text file must be UTF-8"))?
        } else {
            self.text.unwrap_or_default()
        };
        let text = text.trim();
        speech::validate_text(text).map_err(|message| error("INVALID_ARGUMENT", message))?;
        if !self.start.is_finite() || self.start < 0.0 {
            return Err(error(
                "INVALID_ARGUMENT",
                "Start must be a nonnegative number of seconds",
            ));
        }

        let destination = self.project_path.join("project-config.json");
        let original = match fs::read(&destination) {
            Ok(bytes) => Some(bytes),
            Err(cause) if cause.kind() == ErrorKind::NotFound => None,
            Err(cause) => return Err(io_error(cause)),
        };
        let current_revision = revision(original.as_deref().unwrap_or_default());
        if self
            .expected_revision
            .is_some_and(|expected| expected != current_revision)
        {
            return Err(error(
                "REVISION_CONFLICT",
                "The project changed; preview the edit again",
            ));
        }
        let mut raw: Value = original
            .as_ref()
            .map_or_else(|| Ok(json!({})), |bytes| serde_json::from_slice(bytes))
            .map_err(|cause| error("INVALID_PROJECT", cause.to_string()))?;
        let mut project: ProjectConfiguration = serde_json::from_value(raw.clone())
            .map_err(|cause| error("INVALID_PROJECT", cause.to_string()))?;
        project
            .validate()
            .map_err(|cause| error("INVALID_PROJECT", cause.to_string()))?;
        let meta = RecordingMeta::load_for_project(&self.project_path)
            .map_err(|cause| error("INVALID_PROJECT", cause.to_string()))?;
        let studio = meta
            .studio_meta()
            .ok_or_else(|| error("INVALID_PROJECT", "Narration requires a studio project"))?;
        let recordings = if project.timeline.is_none() || self.captions {
            let recordings = cap_rendering::ProjectRecordingsMeta::new(&self.project_path, studio)
                .map_err(|cause| error("MEDIA_UNAVAILABLE", cause))?;
            if project.timeline.is_none() {
                cap_export::synthesize_default_timeline(&mut project, &recordings);
                raw["timeline"] = serde_json::to_value(&project.timeline).map_err(io_error)?;
            }
            Some(recordings)
        } else {
            None
        };
        let timeline = project
            .timeline
            .as_ref()
            .ok_or_else(|| error("INVALID_PROJECT", "The project has no video timeline"))?;
        if self.start > timeline.duration() {
            return Err(error(
                "INVALID_ARGUMENT",
                "Start falls after the project ends",
            ));
        }
        if self.captions
            && (!timeline.caption_segments.is_empty()
                || project
                    .captions
                    .as_ref()
                    .is_some_and(|captions| !captions.segments.is_empty()))
        {
            return Err(error(
                "CAPTIONS_EXIST",
                "This project already has captions. Add narration without --captions to keep them.",
            ));
        }
        let track = timeline
            .audio_segments
            .iter()
            .map(|segment| segment.track)
            .max()
            .map_or(Some(0), |track| track.checked_add(1))
            .ok_or_else(|| error("INVALID_PROJECT", "No audio lane is available"))?;
        let (settings, reuse_recognition) = load_settings(store_path)?;
        let mut result = json!({
            "schemaVersion": 1, "operation": "project.narrate", "provider": "doubao",
            "model": settings.resource_id, "voice": settings.speaker,
            "start": self.start, "timeBasis": "edited-output-seconds",
            "characters": text.chars().count(), "captions": self.captions, "revision": current_revision,
            "dryRun": self.dry_run, "written": !self.dry_run,
        });
        if self.dry_run {
            return Ok(result);
        }

        let audio =
            synthesize(settings, text.to_string(), self.captions, reuse_recognition).await?;
        if self.captions {
            let recordings = recordings.expect("caption timing loads recording durations");
            let durations: Vec<_> = recordings
                .segments
                .iter()
                .map(|segment| segment.display.duration)
                .collect();
            result["captionCount"] = json!(add_captions(
                &mut project,
                &mut raw,
                &durations,
                &audio,
                self.start,
            )?);
        }
        let path = format!("content/narration-{}.wav", uuid::Uuid::new_v4());
        let segment = AudioTrackSegment {
            start: self.start,
            end: self.start + audio.seconds,
            track,
            path,
            name: Some(text.chars().take(32).collect()),
            enabled: true,
            trim_start: 0.0,
            volume_db: 0.0,
            ducking: false,
            fade_in: 0.0,
            fade_out: 0.0,
            duration: Some(audio.seconds),
        };
        let row = serde_json::to_value(&segment).map_err(io_error)?;
        let rows = &mut raw["timeline"]["audioSegments"];
        if rows.is_null() {
            *rows = json!([]);
        }
        rows.as_array_mut()
            .ok_or_else(|| error("INVALID_PROJECT", "Expected an audio track array"))?
            .push(row.clone());
        let bytes = serde_json::to_vec_pretty(&raw).map_err(io_error)?;
        write_checked_with(&destination, original.as_deref(), &bytes, || {
            let path = write_audio(&self.project_path, &segment.path, &audio.wav)?;
            Ok(Some(path))
        })?;
        result["audio"] = row;
        result["resultRevision"] = json!(revision(&bytes));
        Ok(result)
    }
}

fn load_settings(store_path: &Path) -> Result<(speech::Settings, bool)> {
    let raw: Value = match fs::read(store_path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(io_error)?,
        Err(cause) if cause.kind() == ErrorKind::NotFound => {
            return Ok((speech::Settings::default(), false));
        }
        Err(cause) => return Err(io_error(cause)),
    };
    let saved = &raw[speech::SETTINGS_SECTION][speech::SETTINGS_KEY];
    let settings: speech::Settings = if saved.is_null() {
        speech::Settings::default()
    } else {
        serde_json::from_value(saved.clone()).map_err(|_| {
            error(
                "TTS_NOT_CONFIGURED",
                "Re-save the Doubao model and voice in Capwise",
            )
        })?
    };
    settings
        .validate()
        .map_err(|message| error("TTS_NOT_CONFIGURED", message))?;
    let reuse_recognition = raw[speech::SETTINGS_SECTION]
        .as_object()
        .is_some_and(speech::can_reuse_recognition_key);
    Ok((settings, reuse_recognition))
}

fn add_captions(
    project: &mut ProjectConfiguration,
    raw: &mut Value,
    durations: &[f64],
    audio: &speech::Audio,
    start: f64,
) -> Result<usize> {
    let mut segments: Vec<_> = audio
        .sentences
        .iter()
        .filter_map(|sentence| {
            let words: Vec<_> = sentence
                .words
                .iter()
                .filter(|word| {
                    word.start_time.is_finite()
                        && word.end_time.is_finite()
                        && word.start_time >= 0.0
                        && word.end_time > word.start_time
                        && word.start_time < audio.seconds
                })
                .map(|word| CaptionWord {
                    text: word.word.clone(),
                    start: (start + word.start_time) as f32,
                    end: (start + word.end_time.min(audio.seconds)) as f32,
                    hidden: false,
                })
                .collect();
            Some(CaptionSegment {
                id: uuid::Uuid::new_v4().to_string(),
                start: words.first()?.start,
                end: words.last()?.end,
                text: sentence.text.trim().to_string(),
                words,
            })
            .filter(|segment| !segment.text.is_empty())
        })
        .collect();
    segments.sort_by(|left, right| left.start.total_cmp(&right.start));
    segments.dedup_by(|left, right| {
        left.start == right.start && left.end == right.end && left.text == right.text
    });
    if segments.is_empty() {
        return Err(error(
            "TTS_CAPTIONS_UNAVAILABLE",
            "Doubao returned no timed subtitles. Try a Chinese or English voice, or omit --captions.",
        ));
    }
    let expected: Vec<_> = segments
        .iter()
        .map(|segment| (segment.start, segment.end))
        .collect();
    let mut captions = project
        .captions
        .take()
        .unwrap_or_else(CaptionsData::default);
    captions.segments = segments;
    captions.source_timed = false;
    captions.settings.enabled = true;
    captions.settings.export_with_subtitles = true;
    project.captions = Some(captions);
    synchronize_captions(project, durations);
    let captions = project.captions.as_ref().unwrap();
    let track = &project.timeline.as_ref().unwrap().caption_segments;
    if !captions.source_timed
        || track.len() != expected.len()
        || track.iter().zip(expected).any(|(mapped, (start, end))| {
            (mapped.start - f64::from(start)).abs() > 0.02
                || (mapped.end - f64::from(end)).abs() > 0.02
        })
    {
        return Err(error(
            "TTS_CAPTIONS_OUTSIDE_CLIPS",
            "The generated subtitles do not fit the editable video clips at this start time. Move the start earlier or omit --captions.",
        ));
    }
    raw["captions"]["segments"] = serde_json::to_value(&captions.segments).map_err(io_error)?;
    raw["captions"]["sourceTimed"] = json!(true);
    raw["captions"]["settings"]["enabled"] = json!(true);
    raw["captions"]["settings"]["exportWithSubtitles"] = json!(true);
    raw["timeline"]["captionSegments"] = serde_json::to_value(track).map_err(io_error)?;
    Ok(track.len())
}

fn speech_key(reuse_recognition: bool) -> Option<String> {
    let env = std::env::var("CAPWISE_DOUBAO_TTS_API_KEY")
        .ok()
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty());
    #[cfg(target_os = "macos")]
    let env = env.or_else(|| {
        let read = |service, account| {
            security_framework::passwords::get_generic_password(service, account)
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .filter(|key| !key.is_empty())
        };
        read(speech::KEYCHAIN_SERVICE, speech::KEYCHAIN_ACCOUNT).or_else(|| {
            reuse_recognition
                .then(|| {
                    read(speech::KEYCHAIN_SERVICE, speech::RECOGNITION_ACCOUNT)
                        .or_else(|| read(speech::LEGACY_RECOGNITION_SERVICE, "doubao"))
                })
                .flatten()
        })
    });
    #[cfg(not(target_os = "macos"))]
    let _ = reuse_recognition;
    env
}

fn write_audio(project: &Path, relative: &str, bytes: &[u8]) -> Result<PathBuf> {
    let directory = project.join("content");
    if !fs::symlink_metadata(&directory).map_err(io_error)?.is_dir() {
        return Err(error(
            "INVALID_PROJECT",
            "The project audio directory is not a local directory",
        ));
    }
    let path = project.join(relative);
    let temporary = TemporaryFile::write(&path, bytes)?;
    fs::hard_link(&temporary.path, &path).map_err(io_error)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project_config() -> Value {
        json!({
            "futureSetting": true,
            "timeline": {
                "segments": [{"start": 0.0, "end": 4.0, "timescale": 1.25}],
                "zoomSegments": [],
                "audioSegments": [{"start": 0.0, "end": 1.0, "track": 2, "path": "content/existing.wav", "futureAudioField": true}]
            }
        })
    }

    fn synthetic_audio() -> speech::Audio {
        speech::Audio {
            wav: vec![1, 2, 3, 4],
            seconds: 1.0,
            sentences: vec![speech::Sentence {
                text: "你好".into(),
                words: vec![speech::Word {
                    word: "你好".into(),
                    start_time: 0.1,
                    end_time: 0.8,
                }],
            }],
        }
    }

    #[tokio::test]
    async fn dry_run_and_stale_revision_skip_synthesis_and_apply_preserves_existing_audio() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("narration.cap");
        fs::create_dir_all(project.join("content")).unwrap();
        fs::write(
            project.join("recording-meta.json"),
            br#"{"pretty_name":"Test","display":{"path":"content/display.mp4","fps":30}}"#,
        )
        .unwrap();
        let config = project.join("project-config.json");
        let original = serde_json::to_vec(&project_config()).unwrap();
        fs::write(&config, &original).unwrap();
        let store = directory.path().join("store");
        fs::write(
            &store,
            br#"{"capwise_ai":{"asr":{"doubao":{"baseUrl":"https://openspeech.bytedance.com/api/v3"}},"doubaoTts":{"resourceId":"seed-tts-2.0","speaker":"test_voice"}}}"#,
        )
        .unwrap();
        let args = |dry_run, expected_revision| NarrationArgs {
            project_path: project.clone(),
            text: Some("你好".into()),
            text_file: None,
            start: 0.5,
            captions: false,
            dry_run,
            expected_revision,
            format: OutputFormat::Json,
        };
        let skip =
            |_, _, _, _| async { panic!("preview and revision conflicts must skip synthesis") };
        let preview = args(true, None)
            .apply_with(&store, skip)
            .await
            .ok()
            .unwrap();
        assert_eq!(preview["voice"], "test_voice");
        assert_eq!(fs::read(&config).unwrap(), original);
        let conflict = args(false, Some("stale".into()))
            .apply_with(&store, skip)
            .await;
        assert_eq!(conflict.err().unwrap().code, "REVISION_CONFLICT");
        assert_eq!(fs::read_dir(project.join("content")).unwrap().count(), 0);

        let applied = args(false, Some(revision(&original)))
            .apply_with(
                &store,
                |settings, text, captions, reuse_recognition| async move {
                    assert_eq!(settings.speaker, "test_voice");
                    assert_eq!(text, "你好");
                    assert!(!captions);
                    assert!(reuse_recognition);
                    Ok(synthetic_audio())
                },
            )
            .await
            .ok()
            .unwrap();
        assert_eq!(applied["audio"]["start"], 0.5);
        assert_eq!(applied["audio"]["end"], 1.5);
        assert_eq!(applied["audio"]["track"], 3);
        let saved: Value = serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
        assert_eq!(saved["futureSetting"], true);
        assert_eq!(
            saved["timeline"]["audioSegments"][0]["futureAudioField"],
            true
        );
        assert_eq!(saved["timeline"]["audioSegments"][1], applied["audio"]);
        let asset = project.join(applied["audio"]["path"].as_str().unwrap());
        assert_eq!(fs::read(asset).unwrap(), synthetic_audio().wav);
    }

    #[test]
    fn narration_captions_preserve_output_times_at_an_edited_playback_speed() {
        let mut raw = project_config();
        let mut project: ProjectConfiguration = serde_json::from_value(raw.clone()).unwrap();
        let count = add_captions(&mut project, &mut raw, &[4.0], &synthetic_audio(), 0.5)
            .ok()
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(raw["captions"]["sourceTimed"], true);
        assert_eq!(raw["captions"]["settings"]["exportWithSubtitles"], true);
        let caption = &raw["timeline"]["captionSegments"][0];
        assert_eq!(caption["text"], "你好");
        assert!((caption["start"].as_f64().unwrap() - 0.6).abs() < 0.001);
        assert!((caption["end"].as_f64().unwrap() - 1.3).abs() < 0.001);
        let rebuilt: ProjectConfiguration = serde_json::from_value(raw).unwrap();
        assert!(rebuilt.validate().is_ok());
    }
}
