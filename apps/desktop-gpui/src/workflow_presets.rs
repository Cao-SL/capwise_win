use std::path::Path;

use cap_project::{ClipSpeedAudioMode, ProjectConfiguration};
use serde::{Deserialize, Serialize};

pub(crate) const QIAOMU_ID: &str = "qiaomu";
pub(crate) const SPEEDS: &[f64] = &[1.0, 1.1, 1.25, 1.5, 1.75, 2.0];
const REQUEST_FILE: &str = "workflow-request.json";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct WorkflowPreset {
    pub id: String,
    pub name: String,
    pub speed: f64,
    pub bilingual_english: bool,
    pub translation_language: String,
    pub ai_caption_review: bool,
    pub enhance_voice: bool,
    pub camera_beauty: f32,
}

pub(crate) fn default_translation_language() -> String {
    "en".into()
}

impl Default for WorkflowPreset {
    fn default() -> Self {
        Self {
            id: QIAOMU_ID.into(),
            name: "乔木预设".into(),
            speed: 1.25,
            bilingual_english: true,
            translation_language: default_translation_language(),
            ai_caption_review: true,
            enhance_voice: true,
            camera_beauty: 1.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct WorkflowPresets {
    pub enabled: bool,
    pub selected_id: String,
    pub presets: Vec<WorkflowPreset>,
}

impl Default for WorkflowPresets {
    fn default() -> Self {
        Self {
            enabled: true,
            selected_id: QIAOMU_ID.into(),
            presets: vec![WorkflowPreset::default()],
        }
    }
}

impl WorkflowPresets {
    pub fn load() -> Self {
        let Some(value) = crate::store::store_section("workflow_presets").remove("catalog") else {
            return Self::default();
        };
        match serde_json::from_value::<Self>(value) {
            Ok(mut catalog) => {
                for preset in &mut catalog.presets {
                    if !preset.speed.is_finite() || !(0.25..=8.0).contains(&preset.speed) {
                        preset.speed = 1.0;
                    }
                    if !preset.camera_beauty.is_finite() {
                        preset.camera_beauty = 0.0;
                    }
                    preset.camera_beauty = preset.camera_beauty.clamp(0.0, 1.0);
                    if !crate::caption_translation::LANGUAGES
                        .iter()
                        .any(|(code, _)| *code == preset.translation_language)
                    {
                        preset.translation_language = default_translation_language();
                    }
                }
                catalog
            }
            Err(error) => {
                tracing::warn!(%error, "Workflow presets did not parse; leaving automatic processing disabled");
                Self {
                    enabled: false,
                    ..Self::default()
                }
            }
        }
    }

    pub fn save(&self) -> bool {
        serde_json::to_value(self).is_ok_and(|value| {
            crate::store::set_store_setting("workflow_presets", "catalog", value)
        })
    }

    pub fn selected(&self) -> Option<&WorkflowPreset> {
        self.presets
            .iter()
            .find(|preset| preset.id == self.selected_id)
    }

    pub fn selected_mut(&mut self) -> Option<&mut WorkflowPreset> {
        self.presets
            .iter_mut()
            .find(|preset| preset.id == self.selected_id)
    }

    pub fn duplicate_selected(&mut self, name: String) {
        let Some(mut preset) = self.selected().cloned() else {
            return;
        };
        preset.id = uuid::Uuid::new_v4().to_string();
        preset.name = name;
        self.selected_id = preset.id.clone();
        self.presets.push(preset);
    }

    pub fn delete_selected(&mut self) {
        if self.selected_id == QIAOMU_ID {
            return;
        }
        self.presets.retain(|preset| preset.id != self.selected_id);
        self.selected_id = self
            .presets
            .first()
            .map_or_else(String::new, |preset| preset.id.clone());
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppliedWorkflow {
    pub preset_id: String,
    pub preset_name: String,
    pub speed: f64,
    pub bilingual_english: bool,
    #[serde(default = "default_translation_language")]
    pub translation_language: String,
    pub ai_caption_review: bool,
    pub enhance_voice: bool,
    pub camera_beauty: f32,
}

pub(crate) fn request_for_project(path: &Path) -> Option<AppliedWorkflow> {
    request_for_project_checked(path).ok().flatten()
}

pub(crate) fn request_for_project_checked(path: &Path) -> Result<Option<AppliedWorkflow>, String> {
    match std::fs::read(path.join(REQUEST_FILE)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn save_request(path: &Path, request: &AppliedWorkflow) -> anyhow::Result<()> {
    let staging = path.join(format!(".workflow-{}.tmp", uuid::Uuid::new_v4()));
    let result = std::fs::write(&staging, serde_json::to_vec(request)?)
        .and_then(|()| std::fs::rename(&staging, path.join(REQUEST_FILE)));
    if result.is_err() {
        let _ = std::fs::remove_file(&staging);
    }
    result?;
    Ok(())
}

pub(crate) fn apply_to_new_recording(
    project: &mut ProjectConfiguration,
    catalog: &WorkflowPresets,
    voice_enabled: bool,
    has_camera: bool,
    camera_preference: Option<bool>,
) -> Option<AppliedWorkflow> {
    let preset = catalog.selected().filter(|_| catalog.enabled)?;
    if !preset.speed.is_finite()
        || !(0.25..=8.0).contains(&preset.speed)
        || project.timeline.is_none()
    {
        return None;
    }
    if let Some(timeline) = project.timeline.as_mut() {
        crate::editor_edits::set_all_clips_timescale(timeline, preset.speed);
        for segment in &mut timeline.segments {
            if (segment.timescale - 1.0).abs() > f64::EPSILON && segment.speed_audio_mode.is_none()
            {
                segment.speed_audio_mode = Some(ClipSpeedAudioMode::MaintainPitch);
            }
        }
    }
    if project.captions.is_none() {
        project.captions = Some(cap_project::CaptionsData {
            settings: cap_project::CaptionSettings {
                enabled: true,
                font: "Noto Sans SC".into(),
                font_weight: 700,
                size: 42,
                translation_gap: Some(10),
                bottom_margin: Some(60),
                translation_style: Some(cap_project::CaptionTextStyle {
                    font: "Inter".into(),
                    size: 30,
                }),
                color: "#FFFFFF".into(),
                background_opacity: 0,
                outline: true,
                outline_color: "#000000".into(),
                outline_width: 200.0 / 42.0,
                shadow: false,
                active_word_highlight: false,
                preset: "white-outline".into(),
                animation: "none".into(),
                ..Default::default()
            },
            ..Default::default()
        });
    }
    project.audio.improve = preset.enhance_voice && voice_enabled;
    project.camera.skin_smoothing = if has_camera && camera_preference != Some(false) {
        preset.camera_beauty.clamp(0.0, 1.0)
    } else {
        0.0
    };
    Some(AppliedWorkflow {
        preset_id: preset.id.clone(),
        preset_name: preset.name.clone(),
        speed: preset.speed,
        bilingual_english: preset.bilingual_english,
        translation_language: preset.translation_language.clone(),
        ai_caption_review: preset.ai_caption_review,
        enhance_voice: project.audio.improve,
        camera_beauty: project.camera.skin_smoothing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> ProjectConfiguration {
        serde_json::from_value(serde_json::json!({"timeline":{"segments":[{"recordingSegment":0,"start":2.0,"end":12.0,"timescale":1.0}],"zoomSegments":[],"audioSegments":[{"start":3.0,"end":8.0,"track":0,"path":"music.wav","trimStart":1.0,"fadeIn":0.5}]}})).unwrap()
    }

    #[test]
    fn translation_language_defaults_to_english_and_follows_the_preset() {
        let old: AppliedWorkflow = serde_json::from_value(serde_json::json!({"presetId":"qiaomu","presetName":"乔木预设","speed":1.25,"bilingualEnglish":true,"aiCaptionReview":true,"enhanceVoice":true,"cameraBeauty":1.0})).unwrap();
        assert_eq!(old.translation_language, "en");
        let old_preset: WorkflowPreset =
            serde_json::from_value(serde_json::json!({"id":"qiaomu","bilingualEnglish":true}))
                .unwrap();
        assert_eq!(old_preset.translation_language, "en");
        let mut catalog = WorkflowPresets::default();
        catalog.presets[0].translation_language = "ja".into();
        let applied = apply_to_new_recording(&mut project(), &catalog, true, true, None).unwrap();
        assert_eq!(applied.translation_language, "ja");
    }

    #[test]
    fn qiaomu_retimes_source_once_and_preserves_pitch_and_independent_music() {
        let mut project = project();
        let music =
            serde_json::to_value(&project.timeline.as_ref().unwrap().audio_segments).unwrap();
        let catalog = WorkflowPresets::default();
        apply_to_new_recording(&mut project, &catalog, true, true, None).unwrap();
        let timeline = project.timeline.as_ref().unwrap();
        assert_eq!(timeline.duration(), 8.0);
        assert_eq!(timeline.segments[0].timescale, 1.25);
        assert_eq!(
            timeline.segments[0].speed_audio_mode,
            Some(ClipSpeedAudioMode::MaintainPitch)
        );
        let Some(cap_project::TimelineFrameMapping::Single { source, .. }) =
            timeline.get_frame_mapping(4.0)
        else {
            panic!("single source");
        };
        assert_eq!(source.source_time, 7.0);
        assert_eq!(
            serde_json::to_value(&timeline.audio_segments).unwrap(),
            music
        );
        assert!(project.audio.improve);
        assert_eq!(project.camera.skin_smoothing, 1.0);
        let first = serde_json::to_value(&project).unwrap();
        apply_to_new_recording(&mut project, &catalog, true, true, None).unwrap();
        assert_eq!(serde_json::to_value(&project).unwrap(), first);
    }

    #[test]
    fn qiaomu_caption_fonts_seed_new_projects_without_replacing_existing_styles() {
        let mut fresh = project();
        apply_to_new_recording(&mut fresh, &WorkflowPresets::default(), true, false, None).unwrap();
        let settings = &fresh.captions.as_ref().unwrap().settings;
        assert_eq!(
            (settings.font.as_str(), settings.size),
            ("Noto Sans SC", 42)
        );
        assert_eq!(
            settings.resolved_translation_style(),
            cap_project::CaptionTextStyle {
                font: "Inter".into(),
                size: 30
            }
        );
        for translation_style in [
            None,
            Some(cap_project::CaptionTextStyle {
                font: "Montserrat".into(),
                size: 32,
            }),
        ] {
            let mut existing = project();
            existing.captions = Some(cap_project::CaptionsData {
                settings: cap_project::CaptionSettings {
                    font: "Noto Serif SC".into(),
                    size: 64,
                    translation_style,
                    ..Default::default()
                },
                ..Default::default()
            });
            let before = serde_json::to_value(&existing.captions).unwrap();
            apply_to_new_recording(
                &mut existing,
                &WorkflowPresets::default(),
                true,
                false,
                None,
            )
            .unwrap();
            assert_eq!(serde_json::to_value(&existing.captions).unwrap(), before);
        }
    }

    #[test]
    fn disabled_workflows_and_explicit_choices_are_respected() {
        let mut project = project();
        let before = serde_json::to_value(&project).unwrap();
        let mut catalog = WorkflowPresets {
            enabled: false,
            ..Default::default()
        };
        assert!(apply_to_new_recording(&mut project, &catalog, true, true, None).is_none());
        assert_eq!(serde_json::to_value(&project).unwrap(), before);
        catalog.enabled = true;
        catalog.presets[0].speed = 1.5;
        apply_to_new_recording(&mut project, &catalog, false, true, Some(false)).unwrap();
        assert_eq!(
            project.timeline.as_ref().unwrap().segments[0].timescale,
            1.5
        );
        assert!(!project.audio.improve);
        assert_eq!(project.camera.skin_smoothing, 0.0);
        apply_to_new_recording(&mut project, &catalog, true, false, None).unwrap();
        assert_eq!(project.camera.skin_smoothing, 0.0);
    }

    #[test]
    fn user_presets_can_be_created_selected_renamed_and_deleted() {
        let mut catalog = WorkflowPresets::default();
        catalog.duplicate_selected("My preset".into());
        catalog.selected_mut().unwrap().speed = 1.5;
        catalog.selected_mut().unwrap().name = "Tutorial".into();
        assert_ne!(catalog.selected_id, QIAOMU_ID);
        assert_eq!(catalog.presets[0].speed, 1.25);
        let restored: WorkflowPresets =
            serde_json::from_value(serde_json::to_value(&catalog).unwrap()).unwrap();
        assert_eq!(restored.selected().unwrap().name, "Tutorial");
        catalog.delete_selected();
        assert_eq!(catalog.selected_id, QIAOMU_ID);
        catalog.delete_selected();
        assert_eq!(catalog.presets.len(), 1);
    }

    #[test]
    fn workflow_intent_is_only_present_after_a_recording_is_prepared() {
        let dir = tempfile::tempdir().unwrap();
        assert!(request_for_project(dir.path()).is_none());
        let request = apply_to_new_recording(
            &mut project(),
            &WorkflowPresets::default(),
            true,
            false,
            None,
        )
        .unwrap();
        save_request(dir.path(), &request).unwrap();
        assert_eq!(request_for_project(dir.path()), Some(request));
    }
}
