use std::{
    collections::{HashMap, HashSet},
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use cap_project::{ProjectConfiguration, RecordingMeta, StudioRecordingMeta};
use gpui::prelude::*;
use gpui::{Context, Hsla, IntoElement, ParentElement, SharedString, Styled, Window, div, px};
use serde::{Deserialize, Serialize};

use crate::{editor_window::EditorWindow, i18n::tr, transcription, ui};

const REQUEST_FILE: &str = "auto-captions.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub(crate) enum Request {
    Queued,
    NeedsModel,
    Running { model: String },
    Failed { message: String },
    Complete,
    Skipped,
}

pub(crate) fn read_checked(path: &Path) -> Result<Option<Request>, String> {
    match std::fs::read(path.join(REQUEST_FILE)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn read(path: &Path) -> Option<Request> {
    read_checked(path).ok().flatten()
}

pub(crate) fn keep_current_captions(path: &Path) -> Result<(), String> {
    if matches!(
        read_checked(path)?,
        Some(
            Request::Queued
                | Request::NeedsModel
                | Request::Running { .. }
                | Request::Failed { .. }
        )
    ) {
        write(path, &Request::Skipped)?;
    }
    let _ = transcription::clear_generation_error(path);
    Ok(())
}

pub(crate) fn complete_saved_recognition(path: &Path) -> Result<(), String> {
    if matches!(
        read_checked(path)?,
        Some(
            Request::Queued
                | Request::NeedsModel
                | Request::Running { .. }
                | Request::Failed { .. }
        )
    ) {
        write(path, &Request::Complete)?;
    }
    let _ = transcription::clear_generation_error(path);
    Ok(())
}

pub(crate) fn write(path: &Path, request: &Request) -> Result<(), String> {
    let staging = path.join(format!(".auto-captions-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::File::create_new(&staging).map_err(|error| error.to_string())?;
        serde_json::to_writer(&mut file, request).map_err(|error| error.to_string())?;
        file.flush().map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        drop(file);
        std::fs::rename(&staging, path.join(REQUEST_FILE)).map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(staging);
    }
    result
}

fn revoked_results() -> &'static Mutex<HashSet<PathBuf>> {
    static REVOKED: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    REVOKED.get_or_init(Default::default)
}

fn set_result_revoked(path: &Path, revoked: bool) {
    let mut paths = revoked_results()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if revoked {
        let _ = paths.insert(path.to_path_buf());
    } else {
        let _ = paths.remove(path);
    }
}

fn result_revoked(path: &Path) -> bool {
    revoked_results()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains(path)
}

fn active_models() -> &'static Mutex<HashMap<PathBuf, String>> {
    static MODELS: OnceLock<Mutex<HashMap<PathBuf, String>>> = OnceLock::new();
    MODELS.get_or_init(Default::default)
}

pub(crate) fn running_model(path: &Path, generating: bool) -> Option<String> {
    if !generating {
        return None;
    }
    active_models()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(path)
        .cloned()
}

pub(crate) fn accept_manual_generation(path: &Path) {
    set_result_revoked(path, false);
}

pub(crate) fn refinement_allowed(
    path: &Path,
    request: &transcription::RefinementRequest,
) -> Result<bool, String> {
    let automatic = read_checked(path)?;
    let processing_allowed = crate::caption_review::refinement_allowed(path)?;
    Ok(!result_revoked(path)
        && processing_allowed
        && (request.requested_manually || !matches!(automatic, Some(Request::Skipped))))
}

pub(crate) fn skip_request(path: &Path) -> Result<(), String> {
    set_result_revoked(path, true);
    let _ = transcription::clear_generation_error(path);
    write(path, &Request::Skipped).map_err(tr::editor::tabs::captions::auto_draft::skip_failed)?;
    set_result_revoked(path, false);
    Ok(())
}

fn start_request(path: &Path, model: &str) -> Result<(), String> {
    write(
        path,
        &Request::Running {
            model: model.to_string(),
        },
    )?;
    let _ = active_models()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(path.to_path_buf(), model.to_string());
    set_result_revoked(path, false);
    Ok(())
}

pub(crate) fn guard_generation_outcome<T>(
    path: &Path,
    automatic: bool,
    completing: bool,
    outcome: Result<T, String>,
) -> Result<Option<T>, String> {
    if !automatic {
        return outcome.map(Some);
    }
    if result_revoked(path) {
        return Ok(None);
    }
    match read_checked(path)? {
        Some(Request::Running { .. }) => outcome.map(Some),
        Some(Request::Complete) if completing => outcome.map(Some),
        _ => Ok(None),
    }
}

pub(crate) fn finish_generation_outcome(
    path: &Path,
    automatic: bool,
    outcome: Result<bool, String>,
) -> Result<bool, String> {
    let outcome = guard_generation_outcome(path, automatic, true, outcome);
    let outcome = match outcome {
        Ok(None) => Ok(false),
        Ok(Some(applied)) => {
            if automatic && matches!(read_checked(path), Ok(Some(Request::Running { .. }))) {
                write(path, &Request::Complete).map(|()| applied)
            } else {
                Ok(applied)
            }
        }
        Err(error) => {
            if automatic && matches!(read_checked(path), Ok(Some(Request::Running { .. }))) {
                match write(
                    path,
                    &Request::Failed {
                        message: error.clone(),
                    },
                ) {
                    Ok(()) => Err(error),
                    Err(save) => Err(format!("{error}\n{save}")),
                }
            } else {
                Err(error)
            }
        }
    };
    if automatic {
        let _ = active_models()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(path);
    }
    transcription::finish_generation(
        path,
        outcome.as_ref().err().map(|error| {
            tr::editor::tabs::captions::generate_failed(
                transcription::caption_generation_error_message(error),
            )
        }),
    );
    outcome
}

pub(crate) fn has_captions(project: &ProjectConfiguration) -> bool {
    project
        .captions
        .as_ref()
        .is_some_and(|captions| !captions.segments.is_empty())
        || project
            .timeline
            .as_ref()
            .is_some_and(|timeline| !timeline.caption_segments.is_empty())
}

pub(crate) fn enqueue(
    path: &Path,
    enabled: bool,
    project: &ProjectConfiguration,
) -> Result<(), String> {
    if !enabled || has_captions(project) || read_checked(path)?.is_some() {
        return Ok(());
    }
    let meta = RecordingMeta::load_for_project(path).map_err(|error| error.to_string())?;
    let has_audio = match meta.studio_meta() {
        Some(StudioRecordingMeta::SingleSegment { segment }) => segment.audio.is_some(),
        Some(StudioRecordingMeta::MultipleSegments { inner }) => inner
            .segments
            .iter()
            .any(|segment| segment.mic.is_some() || segment.usable_system_audio().is_some()),
        None => false,
    };
    if has_audio {
        write(path, &Request::Queued)?;
    }
    Ok(())
}

fn choose_model(preferred: &str, ready: impl Fn(&str) -> bool) -> Option<&'static str> {
    let models = [crate::local_qwen::MODEL, "medium", "small"];
    models
        .iter()
        .copied()
        .find(|model| *model == preferred && ready(model))
        .or_else(|| models.into_iter().find(|model| ready(model)))
}

impl EditorWindow {
    pub(crate) fn start_recording_captions(
        &mut self,
        retry: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if crate::recording_setup::pending(&self.project_path) {
            self.show_recording_setup(cx);
            return;
        }
        if let Some(error) = crate::caption_review::processing_problem(&self.project_path) {
            self.sidebar.tab = crate::editor_sidebar::SidebarTab::Captions;
            self.caption_processing_save_failed(error, cx);
            return;
        }
        let Some(request) = read(&self.project_path) else {
            return;
        };
        if matches!(request, Request::Complete | Request::Skipped)
            || transcription::ui_snapshot(&self.project_path).generating
        {
            return;
        }
        let next = if has_captions(&self.project) {
            Some(Request::Complete)
        } else if !crate::store::GeneralSettings::load().auto_captions_after_recording {
            Some(Request::Skipped)
        } else if matches!(request, Request::Running { .. }) && !retry {
            Some(Request::Failed {
                message: tr::editor::tabs::captions::auto_draft::interrupted().to_string(),
            })
        } else {
            None
        };
        if matches!(next, Some(Request::Failed { .. })) || matches!(request, Request::Failed { .. })
        {
            self.sidebar.tab = crate::editor_sidebar::SidebarTab::Captions;
            self.invalidate_sidebar(cx);
        }
        if let Some(next) = next {
            if let Err(error) = write(&self.project_path, &next) {
                tracing::warn!(%error, "Could not save automatic caption status");
            }
            cx.notify();
            return;
        }
        if matches!(request, Request::Failed { .. }) && !retry {
            return;
        }
        if !self.project_ready() {
            return;
        }
        let section = crate::store::store_section(transcription::GPUI_STORE_SECTION);
        let preferred = section
            .get(transcription::SELECTED_MODEL_KEY)
            .and_then(serde_json::Value::as_str)
            .unwrap_or(crate::local_qwen::MODEL);
        transcription::refresh_downloaded_models();
        let snapshot = transcription::ui_snapshot(&self.project_path);
        let model = choose_model(preferred, |model| {
            snapshot.downloaded.contains(model)
                && snapshot.deleting.as_deref() != Some(model)
                && !snapshot.download.as_ref().is_some_and(|download| {
                    download.model == model
                        && download.state == transcription::DownloadState::Downloading
                })
                && (model != crate::local_qwen::MODEL || crate::local_qwen::runtime_available())
        });
        self.sidebar.tab = crate::editor_sidebar::SidebarTab::Captions;
        self.invalidate_sidebar(cx);
        let Some(model) = model else {
            if let Err(error) = write(&self.project_path, &Request::NeedsModel) {
                tracing::warn!(%error, "Could not save automatic caption status");
            }
            self.invalidate_sidebar(cx);
            cx.notify();
            return;
        };
        let language = section
            .get(transcription::SELECTED_LANGUAGE_KEY)
            .and_then(serde_json::Value::as_str)
            .and_then(|saved| {
                crate::editor_tabs::CAPTION_LANGUAGES
                    .iter()
                    .find(|(code, _)| *code == saved)
            })
            .map_or("auto", |(code, _)| *code);
        if let Err(error) = start_request(&self.project_path, model) {
            self.caption_processing_save_failed(error, cx);
            return;
        }
        self.clear_caption_processing_error(cx);
        self.generate_transcript_with_model(true, model, language, true, window, cx);
    }

    pub(crate) fn skip_recording_captions(&mut self, cx: &mut Context<Self>) {
        self.cancel_caption_export_wait(cx);
        let skipped = skip_request(&self.project_path);
        self.clear_caption_processing_error(cx);
        transcription::cancel_refinement(&self.project_path);
        self.cancel_caption_processing(cx);
        if let Err(error) = skipped {
            self.caption_processing_save_failed(error, cx);
        }
        self.invalidate_sidebar(cx);
        cx.notify();
    }

    pub(crate) fn render_recording_captions(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let request = read(&self.project_path)?;
        let label = match &request {
            Request::Queued => tr::editor::tabs::captions::auto_draft::queued().to_string(),
            Request::NeedsModel => {
                tr::editor::tabs::captions::auto_draft::needs_model().to_string()
            }
            Request::Running { .. } => running_model(&self.project_path, true).map_or_else(
                || tr::editor::tabs::captions::auto_draft::interrupted().to_string(),
                |model| {
                    tr::editor::tabs::captions::auto_draft::running(
                        crate::editor_tabs::caption_model_label(&model),
                    )
                },
            ),
            Request::Failed { message } => message.clone(),
            Request::Complete | Request::Skipped => return None,
        };
        let theme = self.theme;
        let busy = transcription::ui_snapshot(&self.project_path).generating;
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(Hsla::from(theme.gray_11))
                        .child(SharedString::from(label)),
                )
                .when(!busy, |panel| {
                    panel.child(
                        ui::Button::plain(
                            &theme,
                            "auto-caption-retry",
                            ui::ButtonVariant::Gray,
                            ui::ButtonSize::Sm,
                        )
                        .label(tr::editor::tabs::captions::auto_draft::retry())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.cancel_caption_export_wait(cx);
                            this.start_recording_captions(true, window, cx)
                        })),
                    )
                })
                .child(
                    ui::Button::plain(
                        &theme,
                        "auto-caption-skip",
                        ui::ButtonVariant::Gray,
                        ui::ButtonSize::Sm,
                    )
                    .label(tr::editor::tabs::captions::auto_draft::skip())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.skip_recording_captions(cx);
                    })),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(audio: bool) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let mut take = serde_json::json!({"display": {"path": "display.mp4", "fps": 30}});
        if audio {
            take["mic"] = serde_json::json!({"path": "mic.mp4"});
        }
        std::fs::write(
            dir.path().join("recording-meta.json"),
            serde_json::to_vec(&serde_json::json!({"pretty_name":"test", "segments":[take]}))
                .unwrap(),
        )
        .unwrap();
        dir
    }

    #[test]
    fn only_new_recordings_with_audio_and_enabled_preference_are_queued() {
        let dir = fixture(true);
        assert_eq!(read(dir.path()), None);
        enqueue(dir.path(), false, &ProjectConfiguration::default()).unwrap();
        assert_eq!(read(dir.path()), None);
        enqueue(dir.path(), true, &ProjectConfiguration::default()).unwrap();
        assert_eq!(read(dir.path()), Some(Request::Queued));
        let silent = fixture(false);
        enqueue(silent.path(), true, &ProjectConfiguration::default()).unwrap();
        assert_eq!(read(silent.path()), None);
    }

    #[test]
    fn lifecycle_survives_reopen_and_enqueue_does_not_reset_it() {
        let dir = fixture(true);
        for state in [
            Request::Queued,
            Request::NeedsModel,
            Request::Running {
                model: "small".into(),
            },
            Request::Failed {
                message: "retry".into(),
            },
            Request::Complete,
            Request::Skipped,
        ] {
            write(dir.path(), &state).unwrap();
            enqueue(dir.path(), true, &ProjectConfiguration::default()).unwrap();
            assert_eq!(read(dir.path()), Some(state));
        }
    }

    #[test]
    fn existing_captions_are_never_enqueued() {
        let dir = fixture(true);
        let mut project = ProjectConfiguration::default();
        project.captions = Some(cap_project::CaptionsData {
            segments: vec![cap_project::CaptionSegment {
                text: "Keep this edit".into(),
                ..Default::default()
            }],
            ..Default::default()
        });
        enqueue(dir.path(), true, &project).unwrap();
        assert_eq!(read(dir.path()), None);
    }

    #[test]
    fn model_selection_never_uses_cloud_or_english_only_models() {
        assert_eq!(
            choose_model("doubao", |_| true),
            Some(crate::local_qwen::MODEL)
        );
        assert_eq!(choose_model("medium", |_| true), Some("medium"));
        assert_eq!(choose_model("best", |model| model == "best"), None);
        assert_eq!(choose_model("doubao", |model| model == "doubao"), None);
        assert_eq!(choose_model("small", |_| false), None);
        assert_eq!(
            choose_model("small", |model| model == "medium"),
            Some("medium")
        );
    }

    #[test]
    fn skipped_success_and_failure_do_not_apply_or_refine_and_clear_old_errors() {
        for result in [Ok(true), Err("late recognition error".to_string())] {
            let dir = fixture(true);
            start_request(dir.path(), "small").unwrap();
            assert!(transcription::begin_generation(dir.path()));
            skip_request(dir.path()).unwrap();
            assert!(!transcription::begin_generation(dir.path()));
            assert!(
                guard_generation_outcome(dir.path(), true, false, result.clone())
                    .unwrap()
                    .is_none()
            );
            assert!(!finish_generation_outcome(dir.path(), true, result).unwrap());
            assert_eq!(read_checked(dir.path()).unwrap(), Some(Request::Skipped));
            assert!(transcription::clear_generation_error(dir.path()).is_none());
            assert!(transcription::begin_generation(dir.path()));
            transcription::finish_generation(dir.path(), None);
        }
    }

    #[test]
    fn a_skip_between_saved_result_and_deferred_followup_prevents_refinement() {
        let dir = fixture(true);
        start_request(dir.path(), "small").unwrap();
        assert_eq!(
            guard_generation_outcome(dir.path(), true, false, Ok(true)).unwrap(),
            Some(true)
        );
        complete_saved_recognition(dir.path()).unwrap();
        assert!(finish_generation_outcome(dir.path(), true, Ok(true)).unwrap());
        skip_request(dir.path()).unwrap();
        assert!(
            guard_generation_outcome(dir.path(), true, true, Ok(()))
                .unwrap()
                .is_none()
        );
        assert_eq!(read_checked(dir.path()).unwrap(), Some(Request::Skipped));
    }

    #[test]
    fn failed_skip_write_revokes_this_result_without_pretending_the_worker_stopped() {
        let dir = fixture(true);
        start_request(dir.path(), "small").unwrap();
        assert!(transcription::begin_generation(dir.path()));
        let request = dir.path().join(REQUEST_FILE);
        let saved = dir.path().join("saved-request.json");
        std::fs::rename(&request, &saved).unwrap();
        std::fs::create_dir(&request).unwrap();
        assert!(skip_request(dir.path()).is_err());
        assert!(!transcription::begin_generation(dir.path()));
        std::fs::remove_dir(&request).unwrap();
        std::fs::rename(&saved, &request).unwrap();
        assert!(matches!(
            read_checked(dir.path()).unwrap(),
            Some(Request::Running { .. })
        ));
        assert!(
            guard_generation_outcome(dir.path(), true, false, Ok("late transcript"))
                .unwrap()
                .is_none()
        );
        assert!(!finish_generation_outcome(dir.path(), true, Err("late error".into())).unwrap());
        assert!(matches!(
            read_checked(dir.path()).unwrap(),
            Some(Request::Running { .. })
        ));
        assert!(transcription::clear_generation_error(dir.path()).is_none());
        start_request(dir.path(), "small").unwrap();
        assert_eq!(
            guard_generation_outcome(dir.path(), true, false, Ok(true)).unwrap(),
            Some(true)
        );
        assert!(finish_generation_outcome(dir.path(), true, Ok(true)).unwrap());
    }

    #[test]
    fn missing_or_replaced_requests_do_not_receive_an_old_workers_failure() {
        for state in [
            None,
            Some(Request::Queued),
            Some(Request::NeedsModel),
            Some(Request::Skipped),
            Some(Request::Failed {
                message: "different attempt".into(),
            }),
        ] {
            let dir = fixture(true);
            if let Some(state) = &state {
                write(dir.path(), state).unwrap();
            }
            assert!(
                !finish_generation_outcome(dir.path(), true, Err("old worker failed".into()))
                    .unwrap()
            );
            assert_eq!(read_checked(dir.path()).unwrap(), state);
            assert!(transcription::clear_generation_error(dir.path()).is_none());
        }
    }

    #[test]
    fn manual_generation_after_an_automatic_skip_keeps_its_own_result_and_errors() {
        let dir = fixture(true);
        skip_request(dir.path()).unwrap();
        assert_eq!(
            guard_generation_outcome(dir.path(), false, false, Ok(true)).unwrap(),
            Some(true)
        );
        assert!(finish_generation_outcome(dir.path(), false, Ok(true)).unwrap());
        assert!(
            finish_generation_outcome(dir.path(), false, Err("manual failure".into())).is_err()
        );
        assert!(transcription::clear_generation_error(dir.path()).is_some());
        assert_eq!(read_checked(dir.path()).unwrap(), Some(Request::Skipped));
    }

    #[test]
    fn invalid_persistent_state_blocks_result_and_preserves_the_invalid_file() {
        let dir = fixture(true);
        std::fs::write(dir.path().join(REQUEST_FILE), b"invalid state").unwrap();
        assert!(guard_generation_outcome(dir.path(), true, false, Ok(true)).is_err());
        assert!(finish_generation_outcome(dir.path(), true, Ok(true)).is_err());
        assert_eq!(
            std::fs::read(dir.path().join(REQUEST_FILE)).unwrap(),
            b"invalid state"
        );
        assert!(transcription::clear_generation_error(dir.path()).is_some());
    }

    #[test]
    fn active_generation_still_completes_or_reports_a_real_failure() {
        let dir = fixture(true);
        start_request(dir.path(), "small").unwrap();
        assert!(finish_generation_outcome(dir.path(), true, Ok(true)).unwrap());
        assert_eq!(read_checked(dir.path()).unwrap(), Some(Request::Complete));
        start_request(dir.path(), "small").unwrap();
        assert!(finish_generation_outcome(dir.path(), true, Err("active failure".into())).is_err());
        assert_eq!(
            read_checked(dir.path()).unwrap(),
            Some(Request::Failed {
                message: "active failure".into()
            })
        );
        assert!(transcription::clear_generation_error(dir.path()).is_some());
    }

    #[test]
    fn actual_running_model_survives_skip_until_the_underlying_worker_returns() {
        let dir = fixture(true);
        start_request(dir.path(), crate::local_qwen::MODEL).unwrap();
        assert_eq!(
            running_model(dir.path(), true).as_deref(),
            Some(crate::local_qwen::MODEL)
        );
        assert!(running_model(dir.path(), false).is_none());
        skip_request(dir.path()).unwrap();
        assert_eq!(
            running_model(dir.path(), true).as_deref(),
            Some(crate::local_qwen::MODEL)
        );
        assert!(!finish_generation_outcome(dir.path(), true, Ok(true)).unwrap());
        assert!(running_model(dir.path(), true).is_none());
    }

    #[test]
    fn failed_skip_and_finished_worker_do_not_mislabel_a_manual_cloud_generation() {
        let dir = fixture(true);
        start_request(dir.path(), "small").unwrap();
        assert!(transcription::begin_generation(dir.path()));
        let request = dir.path().join(REQUEST_FILE);
        let saved = dir.path().join("saved-request.json");
        std::fs::rename(&request, &saved).unwrap();
        std::fs::create_dir(&request).unwrap();
        assert!(skip_request(dir.path()).is_err());
        std::fs::remove_dir(&request).unwrap();
        std::fs::rename(&saved, &request).unwrap();
        assert!(!finish_generation_outcome(dir.path(), true, Ok(true)).unwrap());
        assert!(matches!(
            read_checked(dir.path()).unwrap(),
            Some(Request::Running { .. })
        ));
        assert!(transcription::begin_generation(dir.path()));
        assert!(running_model(dir.path(), true).is_none());
        assert!(finish_generation_outcome(dir.path(), false, Ok(true)).unwrap());
    }

    fn write_processing_status(path: &Path, status: &str) {
        std::fs::write(
            path.join("caption-processing.json"),
            serde_json::to_vec(&serde_json::json!({
                "review": true,
                "bilingual": true,
                "status": status,
                "fingerprint": "saved-captions"
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn pending_refinement_resumes_after_close_but_not_after_persisted_skip_or_cancel() {
        let dir = fixture(true);
        let request = transcription::RefinementRequest::new("small", "zh", &[0.0..10.0]);
        std::fs::write(
            dir.path().join("caption-refinement.json"),
            br#"{"model":"small","language":"zh","ranges":[[0.0,10.0]]}"#,
        )
        .unwrap();
        write(dir.path(), &Request::Complete).unwrap();
        write_processing_status(dir.path(), "queued");
        let reopened = transcription::refinement_request_checked(dir.path())
            .unwrap()
            .unwrap();
        assert!(refinement_allowed(dir.path(), &reopened).unwrap());
        skip_request(dir.path()).unwrap();
        let reopened = transcription::refinement_request_checked(dir.path())
            .unwrap()
            .unwrap();
        assert!(!refinement_allowed(dir.path(), &reopened).unwrap());
        assert_eq!(reopened, request);
        write(dir.path(), &Request::Complete).unwrap();
        write_processing_status(dir.path(), "cancelled");
        assert!(!refinement_allowed(dir.path(), &reopened).unwrap());
    }

    #[test]
    fn a_new_manual_refinement_after_skip_survives_reopen_without_restarting_automatic_work() {
        let dir = fixture(true);
        start_request(dir.path(), "small").unwrap();
        assert!(transcription::begin_generation(dir.path()));
        let previous = transcription::RefinementRequest::new("small", "zh", &[0.0..10.0]);
        transcription::save_refinement_request(dir.path(), &previous).unwrap();
        skip_request(dir.path()).unwrap();
        write_processing_status(dir.path(), "cancelled");
        assert!(!finish_generation_outcome(dir.path(), true, Ok(true)).unwrap());
        assert!(!refinement_allowed(dir.path(), &previous).unwrap());
        assert!(transcription::begin_generation(dir.path()));
        let mut manual = transcription::RefinementRequest::new("medium", "zh", &[0.0..10.0]);
        manual.requested_manually = true;
        transcription::save_refinement_request(dir.path(), &manual).unwrap();
        write_processing_status(dir.path(), "queued");
        accept_manual_generation(dir.path());
        assert!(finish_generation_outcome(dir.path(), false, Ok(true)).unwrap());
        let reopened = transcription::refinement_request_checked(dir.path())
            .unwrap()
            .unwrap();
        assert!(refinement_allowed(dir.path(), &reopened).unwrap());
        assert_eq!(read_checked(dir.path()).unwrap(), Some(Request::Skipped));
        write_processing_status(dir.path(), "cancelled");
        assert!(!refinement_allowed(dir.path(), &reopened).unwrap());
    }

    #[test]
    fn refinement_does_not_hide_broken_states_or_ignore_a_session_revocation() {
        let dir = fixture(true);
        let request = transcription::RefinementRequest::new("small", "zh", &[0.0..10.0]);
        write(dir.path(), &Request::Complete).unwrap();
        set_result_revoked(dir.path(), true);
        assert!(!refinement_allowed(dir.path(), &request).unwrap());
        set_result_revoked(dir.path(), false);
        std::fs::write(dir.path().join("caption-processing.json"), b"broken").unwrap();
        assert!(refinement_allowed(dir.path(), &request).is_err());
        write_processing_status(dir.path(), "queued");
        std::fs::write(dir.path().join(REQUEST_FILE), b"broken").unwrap();
        assert!(refinement_allowed(dir.path(), &request).is_err());
    }

    #[test]
    fn skipping_a_failed_request_clears_its_existing_generation_error() {
        let dir = fixture(true);
        write(
            dir.path(),
            &Request::Failed {
                message: "previous failure".into(),
            },
        )
        .unwrap();
        transcription::finish_generation(dir.path(), Some("previous failure".into()));
        skip_request(dir.path()).unwrap();
        assert!(transcription::clear_generation_error(dir.path()).is_none());
        assert_eq!(read_checked(dir.path()).unwrap(), Some(Request::Skipped));
    }
}
