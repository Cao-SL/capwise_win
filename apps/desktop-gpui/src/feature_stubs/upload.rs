//! Stand-in for `src/upload.rs` when the `cap-cloud` feature is off. Nothing is
//! ever uploaded: instant uploads cannot be created, so `InstantUpload` has no
//! values, and deleting a recording only removes its local directory.

#![allow(dead_code)]

use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use cap_enc_ffmpeg::segmented_stream::SegmentCompletedEvent;
use cap_project::VideoUploadInfo;

const UNAVAILABLE: &str = "Cap cloud uploads are not part of this build";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadResult {
    Success(String),
    NotAuthenticated,
    UpgradeRequired,
}

pub struct InstantUpload {
    never: Infallible,
}

#[derive(Clone)]
pub(crate) struct CompletionControl {
    never: Infallible,
}

impl CompletionControl {
    pub(crate) fn deny(&self) {
        match self.never {}
    }
}

pub(crate) struct CompletionAuthorization;

impl CompletionAuthorization {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl InstantUpload {
    pub fn video(&self) -> &VideoUploadInfo {
        match self.never {}
    }

    pub(crate) fn metadata_lock(&self) -> &Mutex<()> {
        match self.never {}
    }

    pub(crate) fn cancellation_token(&self) -> Arc<AtomicBool> {
        match self.never {}
    }

    pub(crate) fn completion_control(&self) -> Option<CompletionControl> {
        match self.never {}
    }

    pub(crate) fn authorize_completion(&mut self) -> Result<(), String> {
        match self.never {}
    }

    pub async fn finish_segments(&mut self) -> Result<(), String> {
        match self.never {}
    }

    pub async fn cancel(&mut self, _query: Option<&str>) -> Result<(), String> {
        match self.never {}
    }

    pub(crate) async fn abort_segments(&mut self) {
        match self.never {}
    }
}

pub async fn prepare_instant_upload(
    _camera_only: bool,
    _project_name: String,
    _organization_id: Option<String>,
) -> Result<VideoUploadInfo, String> {
    Err(UNAVAILABLE.to_string())
}

pub(crate) fn start_instant_upload(
    _video: VideoUploadInfo,
    _project_path: PathBuf,
    _segment_rx: Option<std::sync::mpsc::Receiver<SegmentCompletedEvent>>,
    _metadata_lock: Arc<Mutex<()>>,
    _authorization: Option<CompletionAuthorization>,
) -> Result<InstantUpload, String> {
    Err(UNAVAILABLE.to_string())
}

pub(crate) async fn delete_instant_video_with_query(
    _video_id: &str,
    _query: Option<&str>,
) -> Result<(), String> {
    Err(UNAVAILABLE.to_string())
}

pub async fn upload_exported_video(
    _project_path: PathBuf,
    _organization_id: Option<String>,
    _progress: impl Fn(f64),
    _cancel: Arc<AtomicBool>,
) -> Result<UploadResult, String> {
    Err(UNAVAILABLE.to_string())
}

pub(crate) mod instant_cleanup {
    use std::path::Path;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum CleanupReason {
        StartFailed,
        StartCancelled,
        Interrupted,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum StartFailureCause {
        Other,
    }

    pub(crate) fn classify_start_failure(_error: &str) -> StartFailureCause {
        StartFailureCause::Other
    }

    pub(crate) fn begin(_video_id: &str, _project_dir: &Path) {}

    pub(crate) fn started(_video_id: &str) {}

    pub(crate) fn resolved(_video_id: &str) {}

    pub(crate) fn abandon(
        _video_id: &str,
        _reason: CleanupReason,
        _cause: Option<StartFailureCause>,
    ) {
    }

    pub(crate) fn delete_query(
        _reason: CleanupReason,
        _cause: Option<StartFailureCause>,
    ) -> Option<String> {
        None
    }

    pub(crate) fn project_holds_media(_project: &Path) -> bool {
        false
    }
}

pub(crate) mod queue {
    use std::convert::Infallible;
    use std::path::{Path, PathBuf};

    use cap_project::{RecordingMeta, VideoUploadInfo};

    use super::{InstantUpload, UNAVAILABLE};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum UploadPhase {
        Recording,
        Pending,
        Uploading,
        Processing,
        Retrying,
        NeedsAuthentication,
        Failed,
        Cancelled,
        Verified,
    }

    #[derive(Debug, Clone, PartialEq)]
    pub(crate) struct UploadState {
        pub phase: UploadPhase,
        pub last_error: Option<String>,
        never: Infallible,
    }

    impl UploadState {
        pub fn label(&self) -> &'static str {
            match self.never {}
        }

        pub(crate) fn can_retry(&self) -> bool {
            match self.never {}
        }

        pub fn is_pending(&self) -> bool {
            match self.never {}
        }
    }

    pub(crate) fn record_capture(
        _project: &Path,
        _video: &VideoUploadInfo,
        _segmented: bool,
        _requested_audio: bool,
    ) -> Result<(), String> {
        Err(UNAVAILABLE.to_string())
    }

    pub(crate) fn record_preserve_local(_project: &Path) -> Result<(), String> {
        Ok(())
    }

    pub(crate) fn record_failure(_project: &Path, _error: &str) -> Result<(), String> {
        Ok(())
    }

    pub(crate) async fn enqueue(
        _project: PathBuf,
        upload: InstantUpload,
        _preserve_local: bool,
    ) -> Result<(), String> {
        match upload.never {}
    }

    pub(crate) fn status(_project: &Path, _meta: &RecordingMeta) -> Option<UploadState> {
        None
    }

    pub(crate) async fn retry(_project: PathBuf) -> Result<(), String> {
        Err(UNAVAILABLE.to_string())
    }

    pub(crate) async fn delete_recording(project: PathBuf) -> Result<(), String> {
        if !project.exists() {
            return Ok(());
        }
        tokio::task::spawn_blocking(move || crate::library::delete_recording_directory(&project))
            .await
            .map_err(|error| error.to_string())?
    }
}
