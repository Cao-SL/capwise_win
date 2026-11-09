use std::path::Path;

use cap_project::{
    AspectRatio, ProjectConfiguration, RecordingMeta, SceneMode, SceneSegment, SplitLayout,
    StudioRecordingMeta,
};
use serde::{Deserialize, Serialize};

use crate::recording_setup::SetupInputs;

const STUDIO_LAYOUT_FILE: &str = "studio-layout.json";
const STUDIO_LAYOUT_SETTING: &str = "studioLayout";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum StudioLayout {
    #[default]
    PictureInPicture,
    ScreenLeft,
    CameraLeft,
}

impl StudioLayout {
    pub(crate) const OPTIONS: [Self; 3] =
        [Self::PictureInPicture, Self::ScreenLeft, Self::CameraLeft];

    pub(crate) fn preferred() -> Self {
        crate::store::store_section(crate::store::RECORDING_SETTINGS)
            .get(STUDIO_LAYOUT_SETTING)
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default()
    }

    pub(crate) fn remember(self) -> bool {
        serde_json::to_value(self).is_ok_and(|value| {
            crate::store::set_store_setting(
                crate::store::RECORDING_SETTINGS,
                STUDIO_LAYOUT_SETTING,
                value,
            )
        })
    }

    pub(crate) fn save_intent(self, path: &Path) -> std::io::Result<()> {
        if self == Self::PictureInPicture {
            return Ok(());
        }
        std::fs::write(path.join(STUDIO_LAYOUT_FILE), serde_json::to_vec(&self)?)
    }

    pub(crate) fn for_project(path: &Path) -> std::io::Result<Self> {
        match std::fs::read(path.join(STUDIO_LAYOUT_FILE)) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn apply(self, project: &mut ProjectConfiguration, has_camera: bool) -> bool {
        if self == Self::PictureInPicture || !has_camera {
            return false;
        }
        let Some(timeline) = project.timeline.as_mut() else {
            return false;
        };
        let end = timeline.duration();
        if !timeline.scene_segments.is_empty() || !end.is_finite() || end <= 0.0 {
            return false;
        }
        project.aspect_ratio = Some(AspectRatio::Wide);
        project.camera.hide = false;
        timeline.scene_segments.push(SceneSegment {
            start: 0.0,
            end,
            mode: SceneMode::Floating,
            split_layout: Some(SplitLayout {
                camera_first: self == Self::CameraLeft,
                ..Default::default()
            }),
            transition_in: 0.0,
            transition_out: 0.0,
        });
        true
    }

    pub(crate) fn apply_recovered(path: &Path, has_camera: bool) -> std::io::Result<()> {
        let layout = Self::for_project(path)?;
        if layout != Self::PictureInPicture && has_camera {
            let mut project = ProjectConfiguration::load(path)?;
            if layout.apply(&mut project, has_camera) {
                project.write(path)?;
            }
        }
        Ok(())
    }
}

pub(crate) fn has_studio_camera(meta: &StudioRecordingMeta) -> bool {
    match meta {
        StudioRecordingMeta::SingleSegment { segment } => segment.camera.is_some(),
        StudioRecordingMeta::MultipleSegments { inner } => inner
            .segments
            .iter()
            .any(|segment| segment.camera.is_some()),
    }
}

pub(crate) fn prepare_tracks(path: &Path) -> anyhow::Result<()> {
    prepare_tracks_with_settings(path, &crate::store::GeneralSettings::load())
}

fn prepare_tracks_with_settings(
    path: &Path,
    settings: &crate::store::GeneralSettings,
) -> anyhow::Result<()> {
    let mut inputs = SetupInputs::capture(settings);
    inputs.studio_layout = StudioLayout::for_project(path)?;
    crate::recording_setup::prepare_new(path, inputs).map_err(anyhow::Error::msg)
}

pub(crate) fn prepare_project(
    path: &Path,
    mut project: ProjectConfiguration,
    inputs: &SetupInputs,
) -> Result<
    (
        ProjectConfiguration,
        Option<crate::workflow_presets::AppliedWorkflow>,
        bool,
    ),
    String,
> {
    let meta = RecordingMeta::load_for_project(path).map_err(|error| error.to_string())?;
    let has_camera = meta.studio_meta().is_some_and(has_studio_camera);
    let has_audio = match meta.studio_meta() {
        Some(StudioRecordingMeta::SingleSegment { segment }) => segment.audio.is_some(),
        Some(StudioRecordingMeta::MultipleSegments { inner }) => inner
            .segments
            .iter()
            .any(|segment| segment.mic.is_some() || segment.usable_system_audio().is_some()),
        None => false,
    };
    if project.captions.is_none()
        && let Some(settings) = &inputs.caption_defaults
    {
        project.captions = Some(cap_project::CaptionsData {
            settings: settings.clone(),
            ..Default::default()
        });
    }
    let catalog = crate::workflow_presets::WorkflowPresets {
        enabled: inputs.workflow.is_some(),
        selected_id: inputs
            .workflow
            .as_ref()
            .map_or_else(String::new, |preset| preset.id.clone()),
        presets: inputs.workflow.iter().cloned().collect(),
    };
    let workflow = crate::workflow_presets::apply_to_new_recording(
        &mut project,
        &catalog,
        inputs.voice_enabled,
        has_camera,
        inputs.camera_preference,
    );
    let captions = project.captions.get_or_insert_with(Default::default);
    captions.settings.enabled = true;
    if captions.settings.font == "System Sans-Serif" {
        captions.settings.font = "Noto Sans SC".to_string();
    }
    if let Some(timeline) = project.timeline.as_mut() {
        if inputs.auto_zoom && timeline.zoom_segments.is_empty() {
            let load = |cursor: Option<std::path::PathBuf>| -> Result<Vec<cap_project::CursorClickEvent>, String> {
                match cursor {
                    Some(cursor) => cap_project::CursorEvents::load_clicks_from_file(&cursor)
                        .map_err(|error| error.to_string()),
                    None => Ok(Vec::new()),
                }
            };
            let clicks = match meta.studio_meta() {
                Some(StudioRecordingMeta::SingleSegment { segment }) => vec![load(
                    segment.cursor.as_ref().map(|cursor| meta.path(cursor)),
                )?],
                Some(StudioRecordingMeta::MultipleSegments { inner }) => inner
                    .segments
                    .iter()
                    .map(|segment| load(segment.cursor.as_ref().map(|cursor| meta.path(cursor))))
                    .collect::<Result<Vec<_>, _>>()?,
                None => Vec::new(),
            };
            timeline.zoom_segments = crate::editor_edits::generate_zoom_segments_for_timeline(
                &clicks,
                timeline,
                f64::from(inputs.zoom_amount),
            );
        }
        if inputs.keyboard && timeline.keyboard_segments.is_empty() {
            let keyboard = project.keyboard.get_or_insert_with(Default::default);
            timeline.keyboard_segments = cap_project::generate_project_keyboard_segments(
                &meta,
                timeline,
                &keyboard.settings,
            )?
            .into_iter()
            .filter(|segment| {
                segment.keys.len() == 1
                    && segment
                        .display_text
                        .chars()
                        .any(|ch| matches!(ch, '⌘' | '⌃'))
                    && !matches!(
                        segment.keys[0].key.as_str(),
                        "LMeta"
                            | "RMeta"
                            | "Meta"
                            | "Command"
                            | "RCommand"
                            | "LControl"
                            | "RControl"
                    )
            })
            .collect();
            keyboard.settings.enabled = !timeline.keyboard_segments.is_empty();
        }
    }
    inputs.studio_layout.apply(&mut project, has_camera);
    Ok((project, workflow, has_audio))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn studio_layout_follows_the_recording_and_covers_the_retimed_project() {
        for (layout, camera_first) in [
            (StudioLayout::ScreenLeft, false),
            (StudioLayout::CameraLeft, true),
        ] {
            let dir = fixture(false);
            let path = dir.path();
            assert_eq!(
                StudioLayout::for_project(path).unwrap(),
                StudioLayout::PictureInPicture
            );
            layout.save_intent(path).unwrap();
            let inputs = SetupInputs {
                workflow: None,
                voice_enabled: false,
                camera_preference: None,
                auto_zoom: false,
                zoom_amount: 2.0,
                keyboard: false,
                auto_captions: false,
                caption_defaults: None,
                studio_layout: StudioLayout::for_project(path).unwrap(),
            };
            let mut project = ProjectConfiguration::load(path).unwrap();
            project.timeline.as_mut().unwrap().segments[0].timescale = 2.0;
            let (screen_only, _, _) = prepare_project(path, project.clone(), &inputs).unwrap();
            assert!(screen_only.timeline.unwrap().scene_segments.is_empty());

            let meta = path.join("recording-meta.json");
            let mut recording: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&meta).unwrap()).unwrap();
            recording["segments"][0]["camera"] = serde_json::json!({"path":"camera.mp4","fps":30});
            std::fs::write(&meta, serde_json::to_vec(&recording).unwrap()).unwrap();
            let (prepared, _, _) = prepare_project(path, project, &inputs).unwrap();
            prepared.write(path).unwrap();
            let mut loaded = ProjectConfiguration::load(path).unwrap();
            let scene = &loaded.timeline.as_ref().unwrap().scene_segments[0];
            assert!(matches!(loaded.aspect_ratio, Some(AspectRatio::Wide)));
            assert!(matches!(scene.mode, SceneMode::Floating));
            assert_eq!((scene.start, scene.end), (0.0, 5.0));
            assert_eq!((scene.transition_in, scene.transition_out), (0.0, 0.0));
            assert_eq!(scene.split_layout.unwrap().camera_first, camera_first);
            assert!(!layout.apply(&mut loaded, true));
        }
    }

    fn fixture(with_events: bool) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let project: ProjectConfiguration = serde_json::from_value(serde_json::json!({
            "timeline": {"segments": [{"recordingSegment": 0, "timescale": 1.0, "start": 0.0, "end": 10.0}], "zoomSegments": []}
        })).unwrap();
        project.write(dir.path()).unwrap();
        let mut take = serde_json::json!({"display": {"path": "display.mp4", "fps": 30}});
        if with_events {
            take["cursor"] = "cursor.json".into();
            take["keyboard"] = "keyboard.json".into();
            std::fs::write(
                dir.path().join("cursor.json"),
                serde_json::to_vec(&cap_project::CursorEvents {
                    clicks: vec![cap_project::CursorClickEvent {
                        active_modifiers: Vec::new(),
                        cursor_num: 1,
                        cursor_id: "arrow".into(),
                        time_ms: 3000.0,
                        down: true,
                    }],
                    moves: Vec::new(),
                })
                .unwrap(),
            )
            .unwrap();
            let presses = [
                ("a", 1000.0, true),
                ("a", 1100.0, false),
                ("LMeta", 3000.0, true),
                ("c", 3100.0, true),
                ("c", 3200.0, false),
                ("LMeta", 3300.0, false),
            ]
            .map(|(key, time_ms, down)| cap_project::KeyPressEvent {
                key: key.into(),
                key_code: key.into(),
                time_ms,
                down,
            });
            std::fs::write(
                dir.path().join("keyboard.json"),
                serde_json::to_vec(&cap_project::KeyboardEvents {
                    presses: presses.to_vec(),
                })
                .unwrap(),
            )
            .unwrap();
        }
        std::fs::write(
            dir.path().join("recording-meta.json"),
            serde_json::to_vec(&serde_json::json!({
                "pretty_name": "Recording defaults", "platform": "MacOS", "segments": [take]
            }))
            .unwrap(),
        )
        .unwrap();
        dir
    }

    fn settings() -> crate::store::GeneralSettings {
        let mut settings = crate::store::GeneralSettings::default();
        settings.auto_zoom_on_clicks = true;
        settings.capture_keyboard_events = true;
        settings
    }

    #[test]
    fn eventless_recordings_get_captions_and_audio_without_fake_generated_tracks() {
        let dir = fixture(false);
        prepare_tracks_with_settings(dir.path(), &settings()).unwrap();
        let project = ProjectConfiguration::load(dir.path()).unwrap();
        let lanes = crate::editor_timeline::TrackLanes::from_project(&project, false);
        assert!(lanes.caption);
        assert_eq!(lanes.audio, 1);
        assert!(!lanes.keyboard);
        assert!(!lanes.zoom);
        assert!(project.timeline.unwrap().audio_segments.is_empty());
    }

    #[test]
    fn click_and_shortcut_events_generate_once_and_exclude_plain_typing() {
        let dir = fixture(true);
        prepare_tracks_with_settings(dir.path(), &settings()).unwrap();
        let project = ProjectConfiguration::load(dir.path()).unwrap();
        let timeline = project.timeline.as_ref().unwrap();
        assert!(!timeline.zoom_segments.is_empty());
        assert_eq!(timeline.keyboard_segments.len(), 1);
        assert_eq!(timeline.keyboard_segments[0].display_text, "⌘C");
        assert!(project.keyboard.as_ref().unwrap().settings.enabled);
        prepare_tracks_with_settings(dir.path(), &settings()).unwrap();
        assert_eq!(
            serde_json::to_value(&project).unwrap(),
            serde_json::to_value(ProjectConfiguration::load(dir.path()).unwrap()).unwrap()
        );
    }

    #[test]
    fn disabled_generation_preferences_are_respected() {
        let dir = fixture(true);
        let mut settings = settings();
        settings.auto_zoom_on_clicks = false;
        settings.capture_keyboard_events = false;
        prepare_tracks_with_settings(dir.path(), &settings).unwrap();
        let project = ProjectConfiguration::load(dir.path()).unwrap();
        let timeline = project.timeline.unwrap();
        assert!(timeline.zoom_segments.is_empty());
        assert!(timeline.keyboard_segments.is_empty());
    }
}
