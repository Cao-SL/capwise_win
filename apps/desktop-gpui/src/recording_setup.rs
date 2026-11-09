use std::{
    collections::{BTreeMap, HashMap},
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use cap_project::{CaptionSettings, ProjectConfiguration};
use gpui::prelude::*;
use gpui::{Context, Hsla, IntoElement, ParentElement, SharedString, Styled, Window, div, px};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{editor_window::EditorWindow, i18n::tr, ui};

const FILE: &str = "recording-setup.json";
const FALLBACK: &str = "recording_setup_pending";

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct SetupInputs {
    pub workflow: Option<crate::workflow_presets::WorkflowPreset>,
    pub voice_enabled: bool,
    pub camera_preference: Option<bool>,
    pub auto_zoom: bool,
    pub zoom_amount: f32,
    pub keyboard: bool,
    pub auto_captions: bool,
    pub caption_defaults: Option<CaptionSettings>,
    #[serde(default)]
    pub studio_layout: crate::recording_defaults::StudioLayout,
}

impl SetupInputs {
    pub(crate) fn capture(settings: &crate::store::GeneralSettings) -> Self {
        let catalog = crate::workflow_presets::WorkflowPresets::load();
        let mut styles = ProjectConfiguration::default();
        crate::style_presets::seed_default_captions(&mut styles);
        Self {
            workflow: catalog.selected().filter(|_| catalog.enabled).cloned(),
            voice_enabled: settings.studio_sound_by_default,
            camera_preference: crate::store::camera_skin_smoothing_preference(),
            auto_zoom: settings.auto_zoom_on_clicks,
            zoom_amount: settings
                .default_zoom_amount
                .unwrap_or(crate::editor_edits::DEFAULT_AUTO_ZOOM_AMOUNT as f32),
            keyboard: settings.capture_keyboard_events,
            auto_captions: settings.auto_captions_after_recording,
            caption_defaults: styles.captions.map(|captions| captions.settings),
            studio_layout: Default::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Phase {
    Planning,
    ProjectPending,
    ProjectSaved,
    WorkflowSaved,
    Complete,
    Stopped,
}

impl Phase {
    fn terminal(self) -> bool {
        matches!(self, Self::Complete | Self::Stopped)
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Setup {
    version: u32,
    id: String,
    inputs: SetupInputs,
    baseline: Option<String>,
    baseline_raw: Option<String>,
    prepared: Option<ProjectConfiguration>,
    workflow: Option<crate::workflow_presets::AppliedWorkflow>,
    has_audio: bool,
    #[serde(default)]
    fallback_pending: bool,
    phase: Phase,
    error: Option<String>,
}

fn errors() -> &'static Mutex<HashMap<PathBuf, String>> {
    static ERRORS: OnceLock<Mutex<HashMap<PathBuf, String>>> = OnceLock::new();
    ERRORS.get_or_init(Default::default)
}

fn transient_error(path: &Path) -> Option<String> {
    errors()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(path)
        .cloned()
}

fn set_error(path: &Path, error: Option<String>) {
    let mut errors = errors()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(error) = error {
        let _ = errors.insert(path.to_path_buf(), error);
    } else {
        let _ = errors.remove(path);
    }
}

fn fingerprint(project: &ProjectConfiguration) -> String {
    fn canonical(value: serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(values) => {
                let ordered: BTreeMap<_, _> = values.into_iter().collect();
                serde_json::Value::Object(
                    ordered
                        .into_iter()
                        .map(|(key, value)| (key, canonical(value)))
                        .collect(),
                )
            }
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.into_iter().map(canonical).collect())
            }
            value => value,
        }
    }
    format!(
        "{:x}",
        Sha256::digest(
            canonical(serde_json::to_value(project).expect("project serializes"))
                .to_string()
                .as_bytes()
        )
    )
}

fn raw_fingerprint(path: &Path) -> Option<String> {
    std::fs::read(path.join("project-config.json"))
        .ok()
        .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
}

fn initial(path: &Path, inputs: SetupInputs) -> Setup {
    let baseline_raw = raw_fingerprint(path);
    let loaded = ProjectConfiguration::load(path);
    Setup {
        version: 1,
        id: uuid::Uuid::new_v4().to_string(),
        inputs,
        baseline: loaded.as_ref().ok().map(fingerprint),
        baseline_raw,
        prepared: None,
        workflow: None,
        has_audio: false,
        fallback_pending: false,
        phase: Phase::Planning,
        error: loaded.err().map(|error| error.to_string()),
    }
}

fn decode(bytes: &[u8]) -> Result<Setup, String> {
    let setup: Setup = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    if setup.version != 1 {
        return Err(tr::editor::setup::unsupported().into());
    }
    Ok(setup)
}

fn load(path: &Path) -> Result<Option<Setup>, String> {
    match std::fs::read(path.join(FILE)) {
        Ok(bytes) => decode(&bytes).map(Some),
        Err(error) => {
            let bootstrap = error.kind() == std::io::ErrorKind::NotFound
                || std::fs::symlink_metadata(path.join(FILE))
                    .is_ok_and(|metadata| metadata.is_dir());
            if bootstrap
                && let Some(value) = crate::store::store_section_checked(FALLBACK)?
                    .get(&path.to_string_lossy().to_string())
            {
                return decode(&serde_json::to_vec(value).map_err(|error| error.to_string())?)
                    .map(Some);
            }
            if error.kind() == std::io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(error.to_string())
            }
        }
    }
}

fn write(path: &Path, setup: &Setup) -> Result<(), String> {
    let staging = path.join(format!(".recording-setup-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::File::create_new(&staging).map_err(|error| error.to_string())?;
        serde_json::to_writer(&mut file, setup).map_err(|error| error.to_string())?;
        file.flush().map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        drop(file);
        std::fs::rename(&staging, path.join(FILE)).map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(staging);
    }
    result
}

fn persist_initial(path: &Path, setup: &Setup) -> Result<(), String> {
    if let Err(error) = write(path, setup) {
        let mut fallback = setup.clone();
        fallback.error = Some(error.clone());
        fallback.fallback_pending = true;
        let saved = serde_json::to_value(&fallback).is_ok_and(|value| {
            crate::store::set_store_setting(FALLBACK, &path.to_string_lossy(), value)
        });
        let error = if saved {
            error
        } else {
            tr::editor::setup::not_persisted(error)
        };
        set_error(path, Some(error.clone()));
        return Err(error);
    }
    Ok(())
}

fn clear_fallback(path: &Path, setup: &mut Setup) -> Result<(), String> {
    if !setup.fallback_pending {
        return Ok(());
    }
    let key = path.to_string_lossy().to_string();
    if crate::store::store_section_checked(FALLBACK)?.contains_key(&key)
        && !crate::store::update_store_section(FALLBACK, |section| {
            let _ = section.remove(&key);
            true
        })
    {
        return Err(tr::editor::setup::cleanup_failed().into());
    }
    setup.fallback_pending = false;
    write(path, setup)
}

fn finish_terminal(path: &Path, setup: &mut Setup) -> Result<(), String> {
    setup.error = None;
    let result = write(path, setup).and_then(|()| clear_fallback(path, setup));
    result.map_err(|error| save_failure(path, setup, error))
}

fn save_failure(path: &Path, setup: &mut Setup, error: String) -> String {
    setup.error = Some(error.clone());
    let error = match write(path, setup) {
        Ok(()) => error,
        Err(save) => format!("{error}\n{save}"),
    };
    set_error(path, Some(error.clone()));
    error
}

fn prepare_candidate(
    path: &Path,
    setup: &mut Setup,
    current: &ProjectConfiguration,
) -> Result<(), String> {
    if setup.phase != Phase::Planning {
        return Ok(());
    }
    let matches = setup.baseline.as_ref().map_or_else(
        || setup.baseline_raw.is_some() && setup.baseline_raw == raw_fingerprint(path),
        |baseline| *baseline == fingerprint(current),
    );
    if !matches {
        return Err(tr::editor::setup::changed().into());
    }
    let (prepared, workflow, has_audio) =
        crate::recording_defaults::prepare_project(path, current.clone(), &setup.inputs)?;
    setup.baseline = Some(fingerprint(current));
    setup.prepared = Some(prepared);
    setup.workflow = workflow;
    setup.has_audio = has_audio;
    setup.phase = Phase::ProjectPending;
    setup.error = None;
    write(path, setup)
}

fn project_to_apply(
    setup: &Setup,
    current: &ProjectConfiguration,
) -> Result<Option<ProjectConfiguration>, String> {
    if setup.phase != Phase::ProjectPending {
        return Ok(None);
    }
    let prepared = setup
        .prepared
        .as_ref()
        .ok_or_else(|| tr::editor::setup::unsupported().to_string())?;
    let current_hash = fingerprint(current);
    if current_hash == fingerprint(prepared) {
        Ok(None)
    } else if setup.baseline.as_ref() == Some(&current_hash) {
        Ok(Some(prepared.clone()))
    } else {
        Err(tr::editor::setup::changed().into())
    }
}

fn confirm_project_saved(path: &Path, setup: &mut Setup) -> Result<(), String> {
    if setup.phase != Phase::ProjectPending {
        return Ok(());
    }
    let saved = ProjectConfiguration::load(path).map_err(|error| error.to_string())?;
    let prepared = setup
        .prepared
        .as_ref()
        .ok_or_else(|| tr::editor::setup::unsupported().to_string())?;
    if fingerprint(&saved) != fingerprint(prepared) {
        return Err(tr::editor::setup::changed().into());
    }
    setup.phase = Phase::ProjectSaved;
    setup.error = None;
    write(path, setup)
}

fn finish_requests(
    path: &Path,
    setup: &mut Setup,
    project: &ProjectConfiguration,
    auto_captions_allowed: bool,
) -> Result<(), String> {
    if setup.phase == Phase::ProjectSaved {
        if crate::workflow_presets::request_for_project_checked(path)?.is_none()
            && let Some(workflow) = &setup.workflow
        {
            crate::workflow_presets::save_request(path, workflow)
                .map_err(|error| error.to_string())?;
        }
        setup.phase = Phase::WorkflowSaved;
        setup.error = None;
        write(path, setup)?;
    }
    if setup.phase == Phase::WorkflowSaved {
        let existing = crate::recording_autocaptions::read_checked(path)?;
        let has_processing = path
            .join("caption-processing.json")
            .try_exists()
            .map_err(|error| error.to_string())?;
        if existing.is_none()
            && !has_processing
            && setup.has_audio
            && setup.inputs.auto_captions
            && auto_captions_allowed
            && !crate::recording_autocaptions::has_captions(project)
        {
            crate::recording_autocaptions::enqueue(path, true, project)?;
        }
        setup.phase = Phase::Complete;
        setup.error = None;
        write(path, setup)?;
    }
    Ok(())
}

fn run_on_disk(path: &Path, setup: &mut Setup, auto_captions_allowed: bool) -> Result<(), String> {
    if setup.phase.terminal() {
        return Ok(());
    }
    let result = (|| {
        let mut current = ProjectConfiguration::load(path).map_err(|error| error.to_string())?;
        prepare_candidate(path, setup, &current)?;
        if let Some(prepared) = project_to_apply(setup, &current)? {
            prepared.write(path).map_err(|error| error.to_string())?;
            current = prepared;
        }
        confirm_project_saved(path, setup)?;
        finish_requests(path, setup, &current, auto_captions_allowed)
    })();
    result.map_err(|error| save_failure(path, setup, error))
}

pub(crate) fn has_preparation_evidence(path: &Path) -> bool {
    [
        FILE,
        "workflow-request.json",
        "auto-captions.json",
        "caption-processing.json",
    ]
    .into_iter()
    .any(|file| path.join(file).try_exists().unwrap_or(true))
        || crate::store::store_section_checked(FALLBACK).map_or(true, |section| {
            section.contains_key(&path.to_string_lossy().to_string())
        })
        || transient_error(path).is_some()
}

pub(crate) fn prepare_new(path: &Path, inputs: SetupInputs) -> Result<(), String> {
    set_error(path, None);
    prepare_new_inner(path, inputs).inspect_err(|error| {
        set_error(path, Some(error.clone()));
    })
}

fn prepare_new_inner(path: &Path, inputs: SetupInputs) -> Result<(), String> {
    let allowed = inputs.auto_captions;
    let mut setup = match load(path) {
        Ok(Some(setup)) => setup,
        Ok(None) => {
            if crate::workflow_presets::request_for_project_checked(path)?.is_some()
                || crate::recording_autocaptions::read_checked(path)?.is_some()
                || path
                    .join("caption-processing.json")
                    .try_exists()
                    .map_err(|error| error.to_string())?
            {
                return Ok(());
            }
            let setup = initial(path, inputs);
            persist_initial(path, &setup)?;
            setup
        }
        Err(error) => {
            set_error(path, Some(error.clone()));
            return Err(error);
        }
    };
    if setup.phase.terminal() {
        return if setup.error.is_some() || setup.fallback_pending {
            finish_terminal(path, &mut setup)
        } else {
            Ok(())
        };
    }
    write(path, &setup)?;
    if let Err(error) = clear_fallback(path, &mut setup) {
        return Err(save_failure(path, &mut setup, error));
    }
    run_on_disk(path, &mut setup, allowed)
}

pub(crate) fn pending(path: &Path) -> bool {
    match load(path) {
        Ok(Some(setup)) => {
            !setup.phase.terminal()
                || setup.fallback_pending
                || setup.error.is_some()
                || transient_error(path).is_some()
        }
        Ok(None) => transient_error(path).is_some(),
        Err(_) => true,
    }
}

impl EditorWindow {
    pub(crate) fn show_recording_setup(&mut self, cx: &mut Context<Self>) {
        if pending(&self.project_path) {
            self.sidebar.tab = crate::editor_sidebar::SidebarTab::Captions;
            self.invalidate_sidebar(cx);
        }
    }

    fn retry_recording_setup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.project_ready() {
            return;
        }
        set_error(&self.project_path, None);
        let result = (|| {
            self.flush_pending_saves(cx)?;
            let Some(mut setup) = load(&self.project_path)? else {
                return Err(
                    tr::editor::setup::not_persisted(tr::editor::setup::unsupported()).into(),
                );
            };
            if setup.phase.terminal() {
                return finish_terminal(&self.project_path, &mut setup);
            }
            let result = (|| {
                write(&self.project_path, &setup)?;
                clear_fallback(&self.project_path, &mut setup)?;
                prepare_candidate(&self.project_path, &mut setup, &self.project)?;
                if let Some(prepared) = project_to_apply(&setup, &self.project)? {
                    self.edit_project_global(
                        "resume recording setup",
                        window,
                        cx,
                        move |project| {
                            *project = prepared;
                            true
                        },
                    );
                    self.flush_pending_saves(cx)?;
                }
                confirm_project_saved(&self.project_path, &mut setup)?;
                finish_requests(
                    &self.project_path,
                    &mut setup,
                    &self.project,
                    crate::store::GeneralSettings::load().auto_captions_after_recording,
                )
            })();
            result.map_err(|error| save_failure(&self.project_path, &mut setup, error))
        })();
        if let Err(error) = result {
            set_error(&self.project_path, Some(error));
        } else {
            self.start_recording_captions(false, window, cx);
        }
        self.invalidate_sidebar(cx);
    }

    fn stop_recording_setup(&mut self, cx: &mut Context<Self>) {
        let result = (|| {
            self.flush_pending_saves(cx)?;
            let Some(mut setup) = load(&self.project_path)? else {
                return Ok(());
            };
            crate::recording_autocaptions::keep_current_captions(&self.project_path)?;
            setup.phase = Phase::Stopped;
            setup.error = None;
            finish_terminal(&self.project_path, &mut setup)
        })();
        set_error(&self.project_path, result.err());
        self.invalidate_sidebar(cx);
    }

    pub(crate) fn render_recording_setup(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let loaded = load(&self.project_path);
        let transient = transient_error(&self.project_path);
        let (message, can_stop) = match &loaded {
            Ok(Some(setup))
                if !setup.phase.terminal()
                    || setup.fallback_pending
                    || setup.error.is_some()
                    || transient.is_some() =>
            {
                (
                    transient.or_else(|| setup.error.clone()),
                    !setup.phase.terminal(),
                )
            }
            Err(error) => (Some(error.clone()), false),
            Ok(None) if transient.is_some() => (transient, false),
            _ => return None,
        };
        let theme = self.theme;
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .mb(px(12.))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(Hsla::from(theme.gray_11))
                        .child(tr::editor::setup::notice()),
                )
                .when_some(message, |panel, message| {
                    panel.child(
                        div()
                            .text_size(px(11.))
                            .text_color(Hsla::from(theme.gray_11))
                            .child(SharedString::from(message)),
                    )
                })
                .child(
                    ui::Button::plain(
                        &theme,
                        "retry-recording-setup",
                        ui::ButtonVariant::Gray,
                        ui::ButtonSize::Sm,
                    )
                    .label(tr::editor::setup::retry())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.retry_recording_setup(window, cx)),
                    ),
                )
                .when(can_stop, |panel| {
                    panel
                        .child(
                            ui::Button::plain(
                                &theme,
                                "stop-recording-setup",
                                ui::ButtonVariant::Gray,
                                ui::ButtonSize::Sm,
                            )
                            .label(tr::editor::setup::keep())
                            .on_click(cx.listener(|this, _, _, cx| this.stop_recording_setup(cx))),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(Hsla::from(theme.gray_11))
                                .child(tr::editor::setup::keep_note()),
                        )
                })
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        directory: tempfile::TempDir,
        project: PathBuf,
    }

    impl Fixture {
        fn new(audio: bool) -> Self {
            let directory = tempfile::tempdir().unwrap();
            let project = directory.path().join("recording.cap");
            std::fs::create_dir(&project).unwrap();
            let store = directory.path().join("store.json");
            std::fs::write(&store, b"{}").unwrap();
            crate::store::set_test_tauri_store_path(Some(store));
            let config: ProjectConfiguration = serde_json::from_value(serde_json::json!({
                "timeline": {"segments": [{"recordingSegment": 0, "start": 0.0, "end": 10.0, "timescale": 1.0}], "zoomSegments": []}
            })).unwrap();
            config.write(&project).unwrap();
            let mut segment = serde_json::json!({"display": {"path": "display.mp4", "fps": 30}});
            if audio {
                segment["mic"] = serde_json::json!({"path": "mic.mp4"});
            }
            std::fs::write(
                project.join("recording-meta.json"),
                serde_json::to_vec(&serde_json::json!({
                    "pretty_name": "Setup fixture", "platform": "MacOS", "segments": [segment]
                }))
                .unwrap(),
            )
            .unwrap();
            Self { directory, project }
        }

        fn inputs(&self) -> SetupInputs {
            SetupInputs {
                workflow: Some(crate::workflow_presets::WorkflowPreset::default()),
                voice_enabled: true,
                camera_preference: None,
                auto_zoom: false,
                zoom_amount: 2.0,
                keyboard: false,
                auto_captions: true,
                caption_defaults: None,
                studio_layout: Default::default(),
            }
        }

        fn plan(&self) -> Setup {
            let setup = initial(&self.project, self.inputs());
            write(&self.project, &setup).unwrap();
            setup
        }

        fn config_bytes(&self) -> Vec<u8> {
            std::fs::read(self.project.join("project-config.json")).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            set_error(&self.project, None);
            crate::store::set_test_tauri_store_path(None);
        }
    }

    #[test]
    fn failed_project_write_reopens_pending_and_applies_speed_once() {
        let fixture = Fixture::new(true);
        let mut setup = fixture.plan();
        let original = ProjectConfiguration::load(&fixture.project).unwrap();
        prepare_candidate(&fixture.project, &mut setup, &original).unwrap();
        let path = fixture.project.join("project-config.json");
        let saved = fixture.project.join("original.json");
        std::fs::rename(&path, &saved).unwrap();
        std::fs::create_dir(&path).unwrap();
        let prepared = project_to_apply(&setup, &original).unwrap().unwrap();
        assert!(prepared.write(&fixture.project).is_err());
        assert!(run_on_disk(&fixture.project, &mut setup, true).is_err());
        assert_eq!(
            load(&fixture.project).unwrap().unwrap().phase,
            Phase::ProjectPending
        );
        assert!(!fixture.project.join("workflow-request.json").exists());
        assert!(!fixture.project.join("auto-captions.json").exists());
        std::fs::remove_dir(&path).unwrap();
        std::fs::rename(&saved, &path).unwrap();
        let mut reopened = load(&fixture.project).unwrap().unwrap();
        run_on_disk(&fixture.project, &mut reopened, true).unwrap();
        let finished = ProjectConfiguration::load(&fixture.project).unwrap();
        let timeline = finished.timeline.as_ref().unwrap();
        assert_eq!(timeline.duration(), 8.0);
        assert_eq!(timeline.segments[0].timescale, 1.25);
        assert_eq!(
            timeline.segments[0].speed_audio_mode,
            Some(cap_project::ClipSpeedAudioMode::MaintainPitch)
        );
        let bytes = fixture.config_bytes();
        run_on_disk(&fixture.project, &mut reopened, true).unwrap();
        assert_eq!(fixture.config_bytes(), bytes);
    }

    #[test]
    fn failed_workflow_retry_keeps_manual_edits_and_original_recording_intent() {
        let fixture = Fixture::new(true);
        let mut setup = fixture.plan();
        std::fs::create_dir(fixture.project.join("workflow-request.json")).unwrap();
        assert!(run_on_disk(&fixture.project, &mut setup, true).is_err());
        assert_eq!(
            load(&fixture.project).unwrap().unwrap().phase,
            Phase::ProjectSaved
        );
        let mut edited = ProjectConfiguration::load(&fixture.project).unwrap();
        edited.audio.improve = false;
        edited.timeline.as_mut().unwrap().segments[0].timescale = 1.75;
        edited.captions.as_mut().unwrap().settings.enabled = false;
        edited.captions.as_mut().unwrap().settings.size = 61;
        edited.write(&fixture.project).unwrap();
        let bytes = fixture.config_bytes();
        std::fs::remove_dir(fixture.project.join("workflow-request.json")).unwrap();
        let mut next_recording = fixture.inputs();
        next_recording.workflow.as_mut().unwrap().speed = 2.0;
        next_recording.workflow.as_mut().unwrap().bilingual_english = false;
        prepare_new(&fixture.project, next_recording).unwrap();
        assert_eq!(fixture.config_bytes(), bytes);
        let workflow = crate::workflow_presets::request_for_project_checked(&fixture.project)
            .unwrap()
            .unwrap();
        assert_eq!(workflow.speed, 1.25);
        assert!(workflow.bilingual_english);
        assert_eq!(
            crate::recording_autocaptions::read_checked(&fixture.project).unwrap(),
            Some(crate::recording_autocaptions::Request::Queued)
        );
    }

    #[test]
    fn failed_caption_request_retry_preserves_manual_captions_and_completed_tasks() {
        let fixture = Fixture::new(true);
        let mut setup = fixture.plan();
        std::fs::create_dir(fixture.project.join("auto-captions.json")).unwrap();
        assert!(run_on_disk(&fixture.project, &mut setup, true).is_err());
        assert_eq!(
            load(&fixture.project).unwrap().unwrap().phase,
            Phase::WorkflowSaved
        );
        std::fs::remove_dir(fixture.project.join("auto-captions.json")).unwrap();
        let mut edited = ProjectConfiguration::load(&fixture.project).unwrap();
        edited.captions.as_mut().unwrap().segments = serde_json::from_value(serde_json::json!([
            {"id":"manual", "text":"用户字幕", "start":0.0, "end":1.0, "words":[]}
        ]))
        .unwrap();
        edited.write(&fixture.project).unwrap();
        let bytes = fixture.config_bytes();
        let mut reopened = load(&fixture.project).unwrap().unwrap();
        run_on_disk(&fixture.project, &mut reopened, true).unwrap();
        assert!(
            crate::recording_autocaptions::read_checked(&fixture.project)
                .unwrap()
                .is_none()
        );
        assert_eq!(fixture.config_bytes(), bytes);
        for state in [
            crate::recording_autocaptions::Request::Skipped,
            crate::recording_autocaptions::Request::Complete,
        ] {
            crate::recording_autocaptions::write(&fixture.project, &state).unwrap();
            reopened.phase = Phase::WorkflowSaved;
            finish_requests(&fixture.project, &mut reopened, &edited, true).unwrap();
            assert_eq!(
                crate::recording_autocaptions::read_checked(&fixture.project).unwrap(),
                Some(state)
            );
        }
    }

    #[test]
    fn crash_between_project_write_and_phase_write_never_reapplies_over_user_changes() {
        let fixture = Fixture::new(true);
        let mut setup = fixture.plan();
        let original = ProjectConfiguration::load(&fixture.project).unwrap();
        prepare_candidate(&fixture.project, &mut setup, &original).unwrap();
        let prepared = project_to_apply(&setup, &original).unwrap().unwrap();
        prepared.write(&fixture.project).unwrap();
        let mut reopened = load(&fixture.project).unwrap().unwrap();
        assert!(project_to_apply(&reopened, &prepared).unwrap().is_none());
        let mut edited = prepared;
        edited.audio.improve = false;
        edited.write(&fixture.project).unwrap();
        let bytes = fixture.config_bytes();
        assert!(run_on_disk(&fixture.project, &mut reopened, true).is_err());
        assert_eq!(fixture.config_bytes(), bytes);
        assert!(!fixture.project.join("workflow-request.json").exists());
        assert!(!fixture.project.join("auto-captions.json").exists());
    }

    #[test]
    fn keyboard_read_failure_remains_pending_until_repaired() {
        let fixture = Fixture::new(true);
        let meta = fixture.project.join("recording-meta.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&meta).unwrap()).unwrap();
        value["segments"][0]["keyboard"] = "keyboard.json".into();
        std::fs::write(&meta, serde_json::to_vec(&value).unwrap()).unwrap();
        std::fs::write(fixture.project.join("keyboard.json"), b"damaged").unwrap();
        let mut setup = fixture.plan();
        setup.inputs.keyboard = true;
        write(&fixture.project, &setup).unwrap();
        let bytes = fixture.config_bytes();
        assert!(run_on_disk(&fixture.project, &mut setup, true).is_err());
        assert_eq!(
            load(&fixture.project).unwrap().unwrap().phase,
            Phase::Planning
        );
        assert_eq!(fixture.config_bytes(), bytes);
        assert!(pending(&fixture.project));
        std::fs::write(fixture.project.join("keyboard.json"), br#"{"presses":[]}"#).unwrap();
        let mut reopened = load(&fixture.project).unwrap().unwrap();
        run_on_disk(&fixture.project, &mut reopened, true).unwrap();
        assert_eq!(
            load(&fixture.project).unwrap().unwrap().phase,
            Phase::Complete
        );
    }

    #[test]
    fn bootstrap_fallback_preserves_intent_and_only_clears_after_project_state_exists() {
        let fixture = Fixture::new(true);
        let setup = initial(&fixture.project, fixture.inputs());
        std::fs::create_dir(fixture.project.join(FILE)).unwrap();
        let bytes = fixture.config_bytes();
        assert!(persist_initial(&fixture.project, &setup).is_err());
        assert_eq!(fixture.config_bytes(), bytes);
        assert!(fixture.directory.path().join("store.json").exists());
        assert!(
            crate::store::store_section(FALLBACK)
                .contains_key(&fixture.project.to_string_lossy().to_string())
        );
        assert!(pending(&fixture.project));
        std::fs::remove_dir(fixture.project.join(FILE)).unwrap();
        let mut next = fixture.inputs();
        next.workflow.as_mut().unwrap().speed = 2.0;
        prepare_new(&fixture.project, next).unwrap();
        assert!(crate::store::store_section(FALLBACK).is_empty());
        assert_eq!(
            ProjectConfiguration::load(&fixture.project)
                .unwrap()
                .timeline
                .unwrap()
                .segments[0]
                .timescale,
            1.25
        );
        assert!(!pending(&fixture.project));
    }

    #[test]
    fn damaged_requests_are_not_overwritten_and_old_projects_are_not_created_by_reading_status() {
        let fixture = Fixture::new(true);
        let bytes = fixture.config_bytes();
        assert!(!pending(&fixture.project));
        assert!(load(&fixture.project).unwrap().is_none());
        assert!(!fixture.project.join(FILE).exists());
        assert_eq!(fixture.config_bytes(), bytes);
        let mut setup = fixture.plan();
        std::fs::write(
            fixture.project.join("workflow-request.json"),
            b"corrupt workflow",
        )
        .unwrap();
        assert!(run_on_disk(&fixture.project, &mut setup, true).is_err());
        assert_eq!(
            std::fs::read(fixture.project.join("workflow-request.json")).unwrap(),
            b"corrupt workflow"
        );
        assert!(!fixture.project.join("auto-captions.json").exists());
        std::fs::write(fixture.project.join(FILE), b"corrupt setup").unwrap();
        assert!(load(&fixture.project).is_err());
        assert!(prepare_new(&fixture.project, fixture.inputs()).is_err());
        assert_eq!(
            std::fs::read(fixture.project.join(FILE)).unwrap(),
            b"corrupt setup"
        );
    }

    #[test]
    fn no_audio_optout_and_existing_manual_processing_finish_without_new_asr() {
        for (audio, enabled, currently_allowed, manual) in [
            (false, true, true, false),
            (true, false, true, false),
            (true, true, false, false),
            (true, true, true, true),
        ] {
            let fixture = Fixture::new(audio);
            let mut setup = fixture.plan();
            setup.inputs.auto_captions = enabled;
            if manual {
                std::fs::write(
                    fixture.project.join("caption-processing.json"),
                    b"{\"status\":\"cancelled\"}",
                )
                .unwrap();
            }
            run_on_disk(&fixture.project, &mut setup, currently_allowed).unwrap();
            assert_eq!(setup.phase, Phase::Complete);
            assert!(!fixture.project.join("auto-captions.json").exists());
        }
    }

    #[test]
    fn recovered_defaults_form_one_undo_step_and_request_retries_add_none() {
        let fixture = Fixture::new(true);
        let mut setup = fixture.plan();
        let original = ProjectConfiguration::load(&fixture.project).unwrap();
        prepare_candidate(&fixture.project, &mut setup, &original).unwrap();
        let prepared = project_to_apply(&setup, &original).unwrap().unwrap();
        let mut history = crate::editor_edits::ProjectHistory::new(original.clone());
        history.record(&prepared);
        prepared.write(&fixture.project).unwrap();
        confirm_project_saved(&fixture.project, &mut setup).unwrap();
        finish_requests(&fixture.project, &mut setup, &prepared, true).unwrap();
        assert_eq!(fingerprint(history.undo().unwrap()), fingerprint(&original));
        assert!(history.undo().is_none());
        assert_eq!(fingerprint(history.redo().unwrap()), fingerprint(&prepared));
        assert!(history.redo().is_none());
        assert!(project_to_apply(&setup, &original).unwrap().is_none());
    }

    #[test]
    fn missing_original_fingerprint_never_adopts_current_edits_as_a_new_baseline() {
        let fixture = Fixture::new(true);
        let mut setup = fixture.plan();
        setup.baseline = None;
        setup.baseline_raw = None;
        write(&fixture.project, &setup).unwrap();
        let bytes = fixture.config_bytes();
        assert!(run_on_disk(&fixture.project, &mut setup, true).is_err());
        assert_eq!(fixture.config_bytes(), bytes);
        assert_eq!(setup.phase, Phase::Planning);
        assert!(setup.prepared.is_none());
    }

    #[test]
    fn terminal_cleanup_failure_stays_visible_after_reopen_and_retries_only_cleanup() {
        for phase in [Phase::Stopped, Phase::Complete] {
            let fixture = Fixture::new(true);
            let mut setup = fixture.plan();
            setup.phase = phase;
            setup.fallback_pending = true;
            let key = fixture.project.to_string_lossy().to_string();
            assert!(crate::store::set_store_setting(
                FALLBACK,
                &key,
                serde_json::to_value(&setup).unwrap()
            ));
            write(&fixture.project, &setup).unwrap();
            let store = fixture.directory.path().join("store.json");
            let saved_store = fixture.directory.path().join("saved-store.json");
            std::fs::rename(&store, &saved_store).unwrap();
            std::fs::create_dir(&store).unwrap();
            let bytes = fixture.config_bytes();
            assert!(finish_terminal(&fixture.project, &mut setup).is_err());
            set_error(&fixture.project, None);
            let reopened = load(&fixture.project).unwrap().unwrap();
            assert_eq!(reopened.phase, phase);
            assert!(reopened.error.is_some());
            assert!(reopened.fallback_pending);
            assert!(pending(&fixture.project));
            std::fs::remove_dir(&store).unwrap();
            std::fs::rename(&saved_store, &store).unwrap();
            prepare_new(&fixture.project, fixture.inputs()).unwrap();
            assert!(!pending(&fixture.project));
            assert!(
                crate::store::store_section_checked(FALLBACK)
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(fixture.config_bytes(), bytes);
            assert!(!fixture.project.join("workflow-request.json").exists());
            assert!(!fixture.project.join("auto-captions.json").exists());
        }
    }
}
