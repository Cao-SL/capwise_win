use sha2::{Digest as _, Sha256};
use std::{
    collections::HashMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use cap_enc_ffmpeg::remux::get_media_duration;
use cap_project::{ProjectConfiguration, TimelineConfiguration};
use gpui::{
    AnyElement, Context, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder, px, svg,
};

use crate::{editor_window::EditorWindow, i18n::tr, theme::Theme, ui};

pub const AUDIO_LIBRARY: &[(&str, &str)] = &[
    ("focus-flow", "Focus Flow"),
    ("friendly-welcome", "Friendly Welcome"),
    ("building-momentum", "Building Momentum"),
    ("consistent-groove", "Consistent Groove"),
    ("instrumental-bed", "Instrumental Bed"),
    ("instrumental-fade-out", "Instrumental Fade Out"),
    ("lofi-beats-mirostar", "Lofi Beats"),
    ("raindrops-lofi-sleep-bluelike", "Raindrops"),
    ("sunday-mood-lofi-cafe-upbeat-bluelike", "Sunday Mood"),
    ("good-night-lofi-cozy-chill-fassounds", "Good Night"),
    (
        "ambient-trap-empty-streets-dreamstate-openmindaudio",
        "Empty Streets",
    ),
    ("lofi-study-calm-peaceful-chill-hop-fassounds", "Study"),
    ("lofi-cinematic-pulsebox", "Cinematic"),
    ("lofi-hip-hop-leberch", "Hip Hop"),
    ("cassette-retrositive", "Cassette"),
    ("lofi-smooth-pulsebox", "Smooth"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioPicker {
    Add { lane: u32 },
    Replace { index: usize },
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum AudioLibraryPlacement {
    #[default]
    FromPlayhead,
    FromStart,
}

impl AudioLibraryPlacement {
    pub(crate) fn start(self, playhead: f64) -> f64 {
        match self {
            Self::FromPlayhead => playhead,
            Self::FromStart => 0.,
        }
    }
}

#[derive(Clone)]
pub(crate) struct LibraryTrackMetadata {
    path: PathBuf,
    hash: [u8; 32],
    pub(crate) duration: f64,
}

#[derive(Default)]
pub(crate) struct AudioLibraryState {
    pub(crate) audition_failed: bool,
    pub(crate) placement: AudioLibraryPlacement,
    pub(crate) tracks: HashMap<&'static str, Result<LibraryTrackMetadata, String>>,
    loading: bool,
    generation: u64,
}

fn audio_timing_identity(project: &ProjectConfiguration) -> Option<String> {
    let timeline = project.timeline.as_ref()?;
    let timing = timeline
        .segments
        .iter()
        .map(|segment| {
            (
                segment.recording_clip,
                segment.start,
                segment.end,
                segment.timescale,
            )
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&(timing, timeline.hold_windows())).ok()
}

#[derive(Clone)]
struct AudioAddIntent {
    start: f64,
    total: f64,
    timing: String,
}

pub(crate) fn audio_import_range(
    playhead: f64,
    project_duration: f64,
    source_duration: f64,
) -> Option<std::ops::Range<f64>> {
    if !playhead.is_finite()
        || !project_duration.is_finite()
        || !source_duration.is_finite()
        || project_duration <= 0.0
        || source_duration <= 0.0
    {
        return None;
    }
    let start = playhead.max(0.0);
    let end = start + source_duration;
    (end.is_finite() && end > start).then_some(start..end)
}

pub(crate) fn audio_import_placement(
    segments: &[cap_project::AudioTrackSegment],
    requested_lane: u32,
    next_lane: u32,
    playhead: f64,
    project_duration: f64,
    source_duration: f64,
) -> Option<(u32, std::ops::Range<f64>)> {
    let range = audio_import_range(playhead, project_duration, source_duration)?;
    let lane = if segments.iter().any(|segment| {
        segment.track == requested_lane && segment.start < range.end && segment.end > range.start
    }) {
        segments
            .iter()
            .map(|segment| segment.track.saturating_add(1))
            .max()
            .unwrap_or(0)
            .max(next_lane)
    } else {
        requested_lane
    };
    Some((lane, range))
}

#[derive(Clone)]
pub(crate) struct AudioImportRequest {
    pub picker: AudioPicker,
    pub generation: u64,
    target_fingerprint: Option<String>,
    add: Option<AudioAddIntent>,
}

impl AudioImportRequest {
    pub fn new(
        picker: AudioPicker,
        generation: u64,
        project: &ProjectConfiguration,
    ) -> Option<Self> {
        let target_fingerprint = match picker {
            AudioPicker::Add { .. } => None,
            AudioPicker::Replace { index } => Some(
                serde_json::to_string(project.timeline.as_ref()?.audio_segments.get(index)?)
                    .ok()?,
            ),
        };
        Some(Self {
            picker,
            generation,
            target_fingerprint,
            add: None,
        })
    }

    pub(crate) fn capture_start(
        mut self,
        start: f64,
        total: f64,
        project: &ProjectConfiguration,
    ) -> Option<Self> {
        if matches!(self.picker, AudioPicker::Add { .. }) {
            audio_import_range(start, total, total)?;
            self.add = Some(AudioAddIntent {
                start: start.max(0.),
                total,
                timing: audio_timing_identity(project)?,
            });
        }
        Some(self)
    }

    pub(crate) fn start(&self) -> Option<f64> {
        self.add.as_ref().map(|intent| intent.start)
    }

    pub(crate) fn accepts_total(&self, total: f64) -> bool {
        match self.picker {
            AudioPicker::Add { .. } => self
                .add
                .as_ref()
                .is_some_and(|intent| intent.total == total),
            AudioPicker::Replace { .. } => true,
        }
    }

    pub fn accepts(
        &self,
        picker: Option<AudioPicker>,
        generation: u64,
        project: &ProjectConfiguration,
    ) -> bool {
        if picker != Some(self.picker) || generation != self.generation {
            return false;
        }
        match self.picker {
            AudioPicker::Add { .. } => self.add.as_ref().is_none_or(|intent| {
                audio_timing_identity(project).as_deref() == Some(intent.timing.as_str())
            }),
            AudioPicker::Replace { index } => {
                project
                    .timeline
                    .as_ref()
                    .and_then(|timeline| timeline.audio_segments.get(index))
                    .and_then(|segment| serde_json::to_string(segment).ok())
                    .as_deref()
                    == self.target_fingerprint.as_deref()
            }
        }
    }
}

pub(crate) fn apply_audio_add(
    project: &mut ProjectConfiguration,
    request: &AudioImportRequest,
    next_lane: u32,
    total: f64,
    asset: (String, String, f64),
) -> Result<usize, String> {
    let AudioPicker::Add { lane } = request.picker else {
        return Err(tr::editor::audio::library::timing_changed().into());
    };
    let intent = request
        .add
        .as_ref()
        .filter(|intent| intent.total == total)
        .filter(|_| request.accepts(Some(request.picker), request.generation, project))
        .ok_or_else(|| tr::editor::audio::library::timing_changed().to_string())?;
    let timeline = project
        .timeline
        .as_mut()
        .ok_or_else(|| tr::editor::audio::library::timing_changed().to_string())?;
    let (path, name, duration) = asset;
    let (lane, range) = audio_import_placement(
        &timeline.audio_segments,
        lane,
        next_lane,
        intent.start,
        total,
        duration,
    )
    .ok_or_else(|| tr::editor::window::audio_import::move_playhead().to_string())?;
    Ok(crate::editor_edits::insert_audio_segment(
        timeline,
        crate::editor_edits::default_audio_segment(
            range.start,
            range.end,
            lane,
            path,
            name,
            Some(duration),
        ),
    ))
}

pub(crate) fn replace_audio_asset(
    timeline: &mut TimelineConfiguration,
    index: usize,
    path: String,
    name: String,
    duration: f64,
) -> bool {
    let Some(segment) = timeline.audio_segments.get_mut(index) else {
        return false;
    };
    segment.path = path;
    segment.name = Some(name);
    segment.duration = (duration > 0.0).then_some(duration);
    segment.trim_start = 0.0;
    if duration > 0.0 && segment.end > segment.start + duration {
        segment.end = (segment.start + duration)
            .max(segment.start + crate::editor_edits::MIN_AUDIO_SEGMENT_DURATION);
    }
    let segment_duration = (segment.end - segment.start).max(0.0);
    segment.fade_in = segment.fade_in.min(segment_duration);
    segment.fade_out = segment.fade_out.min(segment_duration);
    true
}

pub(crate) fn probe_audio_duration(path: &Path) -> f64 {
    get_media_duration(path)
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(0.0)
}

pub(crate) fn validate_audio_duration(path: &Path) -> Result<f64, String> {
    let duration = probe_audio_duration(path);
    if !duration.is_finite() || duration <= 0.0 {
        return Err(tr::editor::window::audio_import::unreadable().to_string());
    }
    let audio =
        cap_audio::AudioData::from_file_range(path, 0, cap_audio::AudioData::SAMPLE_RATE as usize)
            .map_err(|error| {
                tracing::warn!(%error, "Could not decode imported audio");
                tr::editor::window::audio_import::unreadable().to_string()
            })?;
    if audio.sample_count() == 0 {
        return Err(tr::editor::window::audio_import::unreadable().to_string());
    }
    Ok(duration)
}

pub(crate) fn record_audio_asset(
    project: &Path,
    path: &str,
    name: &str,
) -> Result<crate::asset_library::AssetRecord, String> {
    crate::asset_library::ensure_record(
        project,
        &crate::asset_library::AssetRecord {
            path: path.to_string(),
            name: name.to_string(),
            kind: crate::asset_library::AssetKind::Audio,
            source: None,
            rendition: None,
        },
    )
}

pub fn bundled_track_path(id: &str) -> Option<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    bundled_track_path_from(id, &crate::store::bundled_resource_dirs(), &manifest)
}

fn bundled_track_path_from(
    id: &str,
    resource_dirs: &[PathBuf],
    manifest: &Path,
) -> Option<PathBuf> {
    if !AUDIO_LIBRARY.iter().any(|(known, _)| *known == id) {
        return None;
    }

    let file = format!("{id}.mp3");
    let mut candidates = resource_dirs
        .iter()
        .map(|directory| directory.join("assets/music").join(&file))
        .collect::<Vec<_>>();
    candidates.push(manifest.join("assets/music").join(&file));
    candidates.into_iter().find(|path| path.is_file())
}

/// Plays a library track on its own output stream for as long as this value
/// lives.
pub(crate) struct LibraryAudition {
    pub(crate) id: &'static str,
    started: std::time::Instant,
    finished: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    stop: Option<std::sync::mpsc::Sender<()>>,
}

impl LibraryAudition {
    fn start(id: &'static str, path: PathBuf) -> Result<Self, String> {
        let (stop, stop_requested) = std::sync::mpsc::channel();
        let finished = Arc::new(AtomicBool::new(false));
        let finished_flag = finished.clone();
        let failed = Arc::new(AtomicBool::new(false));
        let failed_flag = failed.clone();
        std::thread::Builder::new()
            .name("library-audition".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(|| play_audition(&path, &stop_requested));
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        tracing::warn!("library audition failed: {error:#}");
                        failed_flag.store(true, Ordering::Release);
                    }
                    Err(_) => failed_flag.store(true, Ordering::Release),
                }
                finished_flag.store(true, Ordering::Release);
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            id,
            started: std::time::Instant::now(),
            finished,
            failed,
            stop: Some(stop),
        })
    }
}

impl Drop for LibraryAudition {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

pub(crate) fn prepare_audition(
    path: &Path,
) -> anyhow::Result<(Option<tempfile::TempDir>, PathBuf)> {
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("m4a") || extension.eq_ignore_ascii_case("mp4")
        })
    {
        // Rodio's MP4 reader requires a byte length its adapter does not supply.
        // An ADTS stream preserves the AAC packets without changing the take.
        let directory = tempfile::tempdir()?;
        let output = directory.path().join("audition.aac");
        cap_enc_ffmpeg::remux::remux_file(path, &output)?;
        Ok((Some(directory), output))
    } else {
        Ok((None, path.to_path_buf()))
    }
}

fn play_audition(path: &Path, stop: &std::sync::mpsc::Receiver<()>) -> anyhow::Result<()> {
    let (_temporary, path) = prepare_audition(path)?;
    if !matches!(stop.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)) {
        return Ok(());
    }
    let (_stream, handle) = rodio::OutputStream::try_default()?;
    let sink = rodio::Sink::try_new(&handle)?;
    let file = std::io::BufReader::new(std::fs::File::open(path)?);
    sink.append(rodio::Decoder::new(file)?);
    while !sink.empty() {
        match stop.recv_timeout(std::time::Duration::from_millis(100)) {
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    sink.stop();
    Ok(())
}

fn library_source_error(_: impl std::fmt::Display) -> String {
    tr::editor::audio::library::source_conflict().into()
}

fn hash_audio_file(path: &Path) -> Result<[u8; 32], String> {
    let mut file = std::fs::File::open(path).map_err(library_source_error)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(library_source_error)?;
        if count == 0 {
            return Ok(hash.finalize().into());
        }
        hash.update(&buffer[..count]);
    }
}

fn library_track_metadata(id: &str) -> Result<LibraryTrackMetadata, String> {
    let path = bundled_track_path(id)
        .ok_or_else(|| tr::editor::audio::library::metadata_failed().to_string())?;
    let before = std::fs::metadata(&path).map_err(library_source_error)?;
    let hash = hash_audio_file(&path)?;
    let duration = probe_audio_duration(&path);
    let after = std::fs::metadata(&path).map_err(library_source_error)?;
    if !same_audio_file(&before, &after) || !duration.is_finite() || duration <= 0. {
        return Err(tr::editor::audio::library::metadata_failed().into());
    }
    Ok(LibraryTrackMetadata {
        path,
        hash,
        duration,
    })
}

fn same_audio_file(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    !after.file_type().is_symlink()
        && crate::asset_library::same_file_identity(before, after)
        && before.len() == after.len()
        && before.modified().ok() == after.modified().ok()
}

fn library_audio_directory(project: &Path) -> Result<PathBuf, String> {
    let mut directory = project.canonicalize().map_err(library_source_error)?;
    for part in ["assets", "audio"] {
        directory.push(part);
        match std::fs::create_dir(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(library_source_error(error)),
        }
        let metadata = std::fs::symlink_metadata(&directory).map_err(library_source_error)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(library_source_error("invalid audio directory"));
        }
    }
    Ok(directory)
}

fn matching_library_file(path: &Path, hash: &[u8; 32]) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            Ok(hash_audio_file(path)? == *hash)
        }
        Ok(_) => Err(library_source_error("invalid audio file")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(library_source_error(error)),
    }
}

pub(crate) struct PreparedLibraryTrack {
    pub(crate) path: String,
    pub(crate) name: String,
    pub(crate) duration: f64,
    directory: PathBuf,
    directory_identity: std::fs::Metadata,
    destination: PathBuf,
    existing: Option<std::fs::Metadata>,
    pending: Option<(tempfile::NamedTempFile, tempfile::NamedTempFile)>,
}

pub(crate) struct PublishedLibraryTrack {
    pub(crate) path: String,
    pub(crate) name: String,
    pub(crate) duration: f64,
    directory: PathBuf,
    directory_identity: std::fs::Metadata,
    created: Vec<(PathBuf, std::fs::Metadata)>,
}

impl PublishedLibraryTrack {
    pub(crate) fn keep(&mut self) {
        self.created.clear();
    }
}

impl Drop for PublishedLibraryTrack {
    fn drop(&mut self) {
        if !std::fs::symlink_metadata(&self.directory).is_ok_and(|current| {
            current.is_dir()
                && crate::asset_library::same_file_identity(&self.directory_identity, &current)
        }) {
            return;
        }
        for (path, original) in self.created.iter().rev() {
            if std::fs::symlink_metadata(path)
                .is_ok_and(|current| same_audio_file(original, &current))
            {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

impl PreparedLibraryTrack {
    pub(crate) fn publish(self) -> Result<PublishedLibraryTrack, String> {
        let current = std::fs::symlink_metadata(&self.directory).map_err(library_source_error)?;
        if !current.is_dir()
            || !crate::asset_library::same_file_identity(&self.directory_identity, &current)
        {
            return Err(library_source_error("audio directory changed"));
        }
        let mut published = PublishedLibraryTrack {
            path: self.path,
            name: self.name,
            duration: self.duration,
            directory: self.directory,
            directory_identity: self.directory_identity,
            created: Vec::new(),
        };
        if let Some((audio, sidecar)) = self.pending {
            for (file, path) in [
                (audio, self.destination.clone()),
                (
                    sidecar,
                    crate::asset_library::record_path(&self.destination),
                ),
            ] {
                let identity = file.as_file().metadata().map_err(library_source_error)?;
                file.persist_noclobber(&path)
                    .map_err(library_source_error)?;
                published.created.push((path, identity));
            }
        } else if !self.existing.as_ref().is_some_and(|expected| {
            std::fs::symlink_metadata(&self.destination)
                .is_ok_and(|current| same_audio_file(expected, &current))
        }) {
            return Err(tr::editor::audio::library::source_changed().into());
        }
        Ok(published)
    }
}

pub(crate) fn prepare_library_track(
    project: &Path,
    id: &str,
    name: &str,
    source: &LibraryTrackMetadata,
) -> Result<PreparedLibraryTrack, String> {
    if !AUDIO_LIBRARY.iter().any(|(known, _)| *known == id) {
        return Err(library_source_error("unknown library track"));
    }
    let directory = library_audio_directory(project)?;
    let directory_identity = std::fs::symlink_metadata(&directory).map_err(library_source_error)?;
    let hash = source
        .hash
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let legacy = directory.join(format!("library-{id}.mp3"));
    let mut destination = directory.join(format!("library-{id}-{hash}.mp3"));
    if matching_library_file(&legacy, &source.hash)? {
        destination = legacy;
    }
    let existing = if matching_library_file(&destination, &source.hash)? {
        Some(std::fs::symlink_metadata(&destination).map_err(library_source_error)?)
    } else {
        if destination.try_exists().map_err(library_source_error)?
            || crate::asset_library::record_path(&destination)
                .try_exists()
                .map_err(library_source_error)?
        {
            destination =
                directory.join(format!("library-{id}-{hash}-{}.mp3", uuid::Uuid::new_v4()));
        }
        None
    };
    let filename = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| library_source_error("invalid library filename"))?;
    let path = format!("assets/audio/{filename}");
    let pending = if existing.is_some() {
        if hash_audio_file(&source.path)? != source.hash {
            return Err(tr::editor::audio::library::source_changed().into());
        }
        validate_audio_duration(&destination)?;
        None
    } else {
        let mut audio = tempfile::Builder::new()
            .prefix(".library-")
            .suffix(".mp3")
            .tempfile_in(&directory)
            .map_err(library_source_error)?;
        let mut input = std::fs::File::open(&source.path).map_err(library_source_error)?;
        std::io::copy(&mut input, audio.as_file_mut()).map_err(library_source_error)?;
        audio
            .as_file_mut()
            .sync_all()
            .map_err(library_source_error)?;
        if hash_audio_file(audio.path())? != source.hash {
            return Err(tr::editor::audio::library::source_changed().into());
        }
        validate_audio_duration(audio.path())?;
        let record = crate::asset_library::AssetRecord {
            path: path.clone(),
            name: name.into(),
            kind: crate::asset_library::AssetKind::Audio,
            source: None,
            rendition: None,
        };
        let mut sidecar = tempfile::Builder::new()
            .prefix(".library-source-")
            .tempfile_in(&directory)
            .map_err(library_source_error)?;
        sidecar
            .write_all(&serde_json::to_vec_pretty(&record).map_err(library_source_error)?)
            .map_err(library_source_error)?;
        sidecar
            .as_file_mut()
            .sync_all()
            .map_err(library_source_error)?;
        Some((audio, sidecar))
    };
    Ok(PreparedLibraryTrack {
        path,
        name: name.into(),
        duration: source.duration,
        directory,
        directory_identity,
        destination,
        existing,
        pending,
    })
}

#[cfg(test)]
pub fn copy_library_track(
    project: &Path,
    id: &str,
    name: &str,
) -> Result<(String, String, f64), String> {
    std::fs::create_dir_all(project).map_err(library_source_error)?;
    let metadata = library_track_metadata(id)?;
    let mut track = prepare_library_track(project, id, name, &metadata)?.publish()?;
    track.keep();
    Ok((track.path.clone(), track.name.clone(), track.duration))
}

impl EditorWindow {
    pub(crate) fn load_audio_library(&mut self, refresh: bool, cx: &mut Context<Self>) {
        if self.audio_library.loading || (!refresh && !self.audio_library.tracks.is_empty()) {
            return;
        }
        self.audio_library.loading = true;
        self.audio_library.generation = self.audio_library.generation.wrapping_add(1);
        let generation = self.audio_library.generation;
        cx.spawn(async move |this, cx| {
            let tracks = cx
                .background_executor()
                .spawn(async {
                    AUDIO_LIBRARY
                        .iter()
                        .map(|(id, _)| (*id, library_track_metadata(id)))
                        .collect()
                })
                .await;
            this.update(cx, |this, cx| {
                if this.audio_library.generation == generation {
                    this.audio_library.tracks = tracks;
                    this.audio_library.loading = false;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn toggle_library_audition(&mut self, id: &'static str, cx: &mut Context<Self>) {
        if let Some(path) = bundled_track_path(id) {
            self.toggle_audio_audition(id, path, cx);
        }
    }

    pub(crate) fn toggle_audio_audition(
        &mut self,
        id: &'static str,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) {
        self.audio_library.audition_failed = false;
        let was_playing = self
            .library_audition
            .take()
            .is_some_and(|audition| audition.id == id);
        if !was_playing {
            if self.playing {
                self.stop_playback(cx);
            }
            match LibraryAudition::start(id, path) {
                Ok(audition) => {
                    let started = audition.started;
                    self.library_audition = Some(audition);
                    cx.spawn(async move |this, cx| {
                        loop {
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(250))
                                .await;
                            let running = this.update(cx, |this, cx| {
                                let Some(audition) = &this.library_audition else {
                                    return false;
                                };
                                if audition.started != started {
                                    return false;
                                }
                                if audition.finished.load(Ordering::Acquire) {
                                    this.audio_library.audition_failed =
                                        audition.failed.load(Ordering::Acquire);
                                    this.library_audition = None;
                                    cx.notify();
                                    return false;
                                }
                                true
                            });
                            if !matches!(running, Ok(true)) {
                                break;
                            }
                        }
                    })
                    .detach();
                }
                Err(error) => {
                    self.audio_library.audition_failed = true;
                    tracing::warn!("library audition unavailable: {error}");
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn render_audio_library(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let is_replace = matches!(self.audio_picker, Some(AudioPicker::Replace { .. }));
        let importing = self.audio_import_pending == Some(self.audio_picker_generation);
        let start = if importing {
            self.audio_picker_request
                .as_ref()
                .and_then(AudioImportRequest::start)
                .unwrap_or_else(|| self.audio_library.placement.start(self.playhead))
        } else {
            self.audio_library.placement.start(self.playhead)
        };
        let close = ui::EditorButton::plain(&theme, "audio-library-close")
            .filled(&theme)
            .padding_x(px(12.))
            .text_size(px(12.))
            .label(tr::editor::audio::library::done())
            .on_click(cx.listener(|this, _, _, cx| {
                this.audio_picker = None;
                cx.notify();
            }))
            .into_any_element();
        let sources = div()
            .flex()
            .flex_row()
            .gap(px(8.))
            .child(
                ui::EditorButton::plain(&theme, "audio-library-import")
                    .tinted(&theme)
                    .grow()
                    .height(px(36.))
                    .left_icon("icons/import.svg")
                    .label(tr::editor::audio::library::import())
                    .disabled(importing)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.import_audio_from_picker(window, cx);
                    })),
            )
            .when(!is_replace, |row| {
                row.child(
                    ui::EditorButton::plain(&theme, "audio-library-voiceover")
                        .tinted(&theme)
                        .grow()
                        .height(px(36.))
                        .left_icon("icons/microphone.svg")
                        .label(tr::editor::voiceover::title())
                        .disabled(importing)
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some(AudioPicker::Add { lane }) = this.audio_picker {
                                this.open_voiceover(lane, window, cx);
                            }
                        })),
                )
            });
        let placement = (!is_replace).then(|| {
            ui::SegmentedControl::editor(
                &theme,
                "audio-library-placement",
                vec![
                    ui::SegmentOption::new(
                        tr::editor::audio::library::placement_playhead(),
                        self.audio_library.placement == AudioLibraryPlacement::FromPlayhead,
                    )
                    .disabled(importing),
                    ui::SegmentOption::new(
                        tr::editor::audio::library::placement_start(),
                        self.audio_library.placement == AudioLibraryPlacement::FromStart,
                    )
                    .disabled(importing),
                ],
            )
            .text_size(px(12.))
            .on_select(cx.listener(|this, index: &usize, _, cx| {
                this.audio_library.placement = if *index == 0 {
                    AudioLibraryPlacement::FromPlayhead
                } else {
                    AudioLibraryPlacement::FromStart
                };
                cx.notify();
            }))
            .into_any_element()
        });
        let playhead_past_end = !is_replace
            && self.audio_library.placement == AudioLibraryPlacement::FromPlayhead
            && audio_import_range(start, self.total_duration(), self.total_duration()).is_none();

        div()
            .id("audio-library")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(16.))
            .gap(px(16.))
            .child(crate::editor_sidebar::tab_header(
                &theme,
                if is_replace {
                    tr::editor::audio::library::change_title()
                } else {
                    tr::editor::audio::library::add_title()
                },
                Some(close),
            ))
            .when(self.audio_library.audition_failed, |this| {
                this.child(
                    div()
                        .text_size(px(12.))
                        .text_color(Hsla::from(theme.red_9))
                        .child(tr::editor::audio::library::audition_failed()),
                )
            })
            .child(sources)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap(px(8.))
                            .min_h(px(28.))
                            .child(crate::editor_sidebar::section_title(
                                &theme,
                                tr::editor::audio::library::bundled_title(),
                            ))
                            .children(placement),
                    )
                    .when(playhead_past_end, |content| {
                        content.child(
                            div()
                                .text_size(px(12.))
                                .text_color(Hsla::from(theme.editor.text_2))
                                .child(tr::editor::window::audio_import::move_playhead()),
                        )
                    })
                    .when(
                        self.audio_library.tracks.values().any(Result::is_err),
                        |content| {
                            content.child(
                                ui::EditorButton::plain(&theme, "audio-library-retry-metadata")
                                    .tinted(&theme)
                                    .label(tr::editor::audio::library::retry_metadata())
                                    .disabled(self.audio_library.loading || importing)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.load_audio_library(true, cx)
                                    })),
                            )
                        },
                    )
                    .child(div().flex().flex_col().gap(px(2.)).children(
                        AUDIO_LIBRARY.iter().copied().map(|(id, name)| {
                            let auditioning = self
                                .library_audition
                                .as_ref()
                                .is_some_and(|audition| audition.id == id);
                            let metadata = self.audio_library.tracks.get(id);
                            let full = metadata
                                .and_then(|result| result.as_ref().ok())
                                .map(|track| track.duration);
                            let range = full.and_then(|duration| {
                                audio_import_range(start, self.total_duration(), duration)
                            });
                            let description: SharedString = match (metadata, full) {
                                (_, Some(duration)) => {
                                    crate::editor_timeline::format_time(duration).into()
                                }
                                (Some(Err(_)), _) => {
                                    tr::editor::audio::library::metadata_failed().into()
                                }
                                _ => tr::editor::audio::library::metadata_loading().into(),
                            };
                            render_library_row(
                                &theme,
                                id,
                                name,
                                importing || full.is_none() || (!is_replace && range.is_none()),
                                auditioning,
                                description,
                                cx,
                            )
                        }),
                    )),
            )
            .into_any_element()
    }
}

fn render_library_row(
    theme: &Theme,
    id: &'static str,
    name: &'static str,
    unavailable: bool,
    auditioning: bool,
    description: SharedString,
    cx: &mut Context<EditorWindow>,
) -> AnyElement {
    let group = SharedString::from(format!("audio-lib-row-{id}"));
    let accent = Hsla::from(theme.editor.accent);
    let mut accent_wash = accent;
    accent_wash.a = 0.12;
    div()
        .id(SharedString::from(format!("audio-lib-{id}")))
        .group(group.clone())
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.))
        .h(px(44.))
        .px(px(6.))
        .rounded(px(8.))
        .when(auditioning, |row| row.bg(accent_wash))
        .when(!unavailable, |row| {
            row.cursor_pointer()
                .when(!auditioning, |row| {
                    row.hover(|this| this.bg(Hsla::from(theme.editor.ctl)))
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.add_library_track(id, name, window, cx);
                }))
        })
        .child(
            div()
                .id(SharedString::from(format!("audio-lib-play-{id}")))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .size(px(30.))
                .rounded_full()
                .bg(if auditioning {
                    accent
                } else {
                    Hsla::from(theme.editor.ctl)
                })
                .cursor_pointer()
                .when(!auditioning, |this| {
                    this.hover(|this| this.bg(Hsla::from(theme.editor.ctl_active)))
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle_library_audition(id, cx);
                }))
                .child(
                    svg()
                        .path(if auditioning {
                            "icons/square.svg"
                        } else {
                            "icons/play.svg"
                        })
                        .size(px(12.))
                        .text_color(if auditioning {
                            Hsla::from(gpui::rgb(0xffffff))
                        } else {
                            Hsla::from(theme.editor.text_1)
                        }),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(Hsla::from(if unavailable {
                    theme.editor.text_3
                } else {
                    theme.editor.text_1
                }))
                .child(name),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(12.))
                .text_color(Hsla::from(theme.editor.text_3))
                .child(description),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .size(px(24.))
                .rounded(px(6.))
                .when(!unavailable, |this| {
                    this.group_hover(group.clone(), move |style| style.bg(accent))
                })
                .child(
                    svg()
                        .path("icons/plus.svg")
                        .size(px(14.))
                        .text_color(Hsla::from(if unavailable {
                            theme.editor.text_3
                        } else {
                            theme.editor.text_2
                        }))
                        .when(!unavailable, |this| {
                            this.group_hover(group, |style| {
                                style.text_color(Hsla::from(gpui::rgb(0xffffff)))
                            })
                        }),
                ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{
        AUDIO_LIBRARY, AudioImportRequest, AudioPicker, audio_import_range, bundled_track_path,
        bundled_track_path_from, copy_library_track, probe_audio_duration, replace_audio_asset,
    };
    use cap_project::ProjectConfiguration;

    fn music_project() -> ProjectConfiguration {
        ProjectConfiguration {
            timeline: Some(
                serde_json::from_value(serde_json::json!({
                    "segments": [{"start": 0, "end": 60, "timescale": 1}], "zoomSegments": []
                }))
                .unwrap(),
            ),
            ..Default::default()
        }
    }

    #[test]
    fn audio_library_add_uses_clicked_position_and_keeps_full_source_duration() {
        for (placement, source, expected) in [
            (
                super::AudioLibraryPlacement::FromPlayhead,
                204.32,
                50.0..254.32,
            ),
            (super::AudioLibraryPlacement::FromStart, 204.32, 0.0..204.32),
            (super::AudioLibraryPlacement::FromPlayhead, 5., 50.0..55.0),
            (super::AudioLibraryPlacement::FromStart, 5., 0.0..5.0),
        ] {
            let mut project = music_project();
            let request = AudioImportRequest::new(AudioPicker::Add { lane: 0 }, 4, &project)
                .unwrap()
                .capture_start(placement.start(50.), 60., &project)
                .unwrap();
            let original_video =
                serde_json::to_value(&project.timeline.as_ref().unwrap().segments).unwrap();
            let index = super::apply_audio_add(
                &mut project,
                &request,
                1,
                60.,
                ("full.mp3".into(), "Full song".into(), source),
            )
            .unwrap();
            let timeline = project.timeline.as_ref().unwrap();
            let inserted = &timeline.audio_segments[index];
            assert_eq!(inserted.start..inserted.end, expected);
            assert_eq!(inserted.duration, Some(source));
            assert_eq!(inserted.trim_start, 0.);
            assert_eq!(timeline.audio_segments.len(), 1);
            assert_eq!(
                serde_json::to_value(&timeline.segments).unwrap(),
                original_video
            );
        }
    }

    #[test]
    fn audio_library_add_rejects_changed_timing_but_accepts_cosmetic_and_volume_edits() {
        for change in 0..7 {
            let mut project = music_project();
            let request = AudioImportRequest::new(AudioPicker::Add { lane: 0 }, 1, &project)
                .unwrap()
                .capture_start(50., 60., &project)
                .unwrap();
            let timeline = project.timeline.as_mut().unwrap();
            let mut total = 60.;
            match change {
                0 => timeline.segments[0].start = 1.,
                1 => timeline.segments[0].timescale = 2.,
                2 => timeline.segments[0].recording_clip = 1,
                3 => timeline.text_segments.push(
                    serde_json::from_value(serde_json::json!({
                        "start": 5, "end": 7, "layout": "fullscreen", "content": "Hold"
                    }))
                    .unwrap(),
                ),
                4 => total = 59.,
                _ => {
                    timeline.segments[0].name = Some("Renamed".into());
                    timeline.segments[0].volume = Some(0.25);
                    timeline.segments[0].hide_cursor = Some(true);
                    timeline.text_segments.push(
                        serde_json::from_value(serde_json::json!({
                            "start": 5, "end": 7, "layout": "overlay", "content": "Title"
                        }))
                        .unwrap(),
                    );
                }
            }
            let before = serde_json::to_value(&project).unwrap();
            let result = super::apply_audio_add(
                &mut project,
                &request,
                1,
                total,
                ("full.mp3".into(), "Full song".into(), 204.32),
            );
            if change < 5 {
                assert!(result.is_err());
                assert_eq!(serde_json::to_value(&project).unwrap(), before);
            } else {
                assert!(result.is_ok());
                assert_eq!(
                    project.timeline.as_ref().unwrap().audio_segments[0].start,
                    50.
                );
            }
        }
    }

    fn write_short_wave(path: &std::path::Path) {
        let data_size = 16_000_u32 * 10;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(data_size + 36).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&8_000_u32.to_le_bytes());
        wav.extend_from_slice(&16_000_u32.to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_size.to_le_bytes());
        wav.resize(data_size as usize + 44, 0);
        std::fs::write(path, wav).unwrap();
    }

    #[test]
    fn bundled_music_short_legacy_source_is_preserved_and_full_source_is_reused() {
        let root = tempfile::tempdir().unwrap();
        let directory = super::library_audio_directory(root.path()).unwrap();
        let legacy = directory.join("library-focus-flow.mp3");
        write_short_wave(&legacy);
        assert_eq!(probe_audio_duration(&legacy), 10.);
        let old_hash = super::hash_audio_file(&legacy).unwrap();
        let old_path = "assets/audio/library-focus-flow.mp3";
        super::record_audio_asset(root.path(), old_path, "Old short song").unwrap();
        let old_sidecar = std::fs::read(crate::asset_library::record_path(&legacy)).unwrap();
        let mut project = music_project();
        project.timeline.as_mut().unwrap().audio_segments.push(
            crate::editor_edits::default_audio_segment(
                0.,
                10.,
                0,
                old_path.into(),
                "Old short song".into(),
                Some(10.),
            ),
        );
        let old_segment =
            serde_json::to_value(&project.timeline.as_ref().unwrap().audio_segments[0]).unwrap();
        let source = super::library_track_metadata("focus-flow").unwrap();
        assert!(source.duration > 200.);
        let mut published =
            super::prepare_library_track(root.path(), "focus-flow", "Focus Flow", &source)
                .unwrap()
                .publish()
                .unwrap();
        assert_ne!(published.path, old_path);
        assert_eq!(
            super::hash_audio_file(&root.path().join(&published.path)).unwrap(),
            source.hash
        );
        let request = AudioImportRequest::new(AudioPicker::Add { lane: 0 }, 1, &project)
            .unwrap()
            .capture_start(0., 60., &project)
            .unwrap();
        super::apply_audio_add(
            &mut project,
            &request,
            1,
            60.,
            (
                published.path.clone(),
                published.name.clone(),
                published.duration,
            ),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&project.timeline.as_ref().unwrap().audio_segments[0]).unwrap(),
            old_segment
        );
        assert_eq!(super::hash_audio_file(&legacy).unwrap(), old_hash);
        assert_eq!(
            std::fs::read(crate::asset_library::record_path(&legacy)).unwrap(),
            old_sidecar
        );
        let again =
            super::prepare_library_track(root.path(), "focus-flow", "Focus Flow", &source).unwrap();
        assert_eq!(again.path, published.path);
        assert!(again.pending.is_none());
        published.keep();
        assert_eq!(std::fs::read_dir(directory).unwrap().count(), 4);
    }

    #[test]
    fn bundled_music_cancel_publish_collision_and_changed_source_leave_no_partial_media() {
        let root = tempfile::tempdir().unwrap();
        let source = super::library_track_metadata("focus-flow").unwrap();
        let pending =
            super::prepare_library_track(root.path(), "focus-flow", "Focus Flow", &source).unwrap();
        let directory = pending.directory.clone();
        drop(pending);
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 0);
        let pending =
            super::prepare_library_track(root.path(), "focus-flow", "Focus Flow", &source).unwrap();
        let destination = pending.destination.clone();
        std::fs::write(&destination, b"concurrent file").unwrap();
        assert!(pending.publish().is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), b"concurrent file");
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        let mut changed = source.clone();
        changed.hash[0] ^= 1;
        assert!(
            super::prepare_library_track(root.path(), "focus-flow", "Focus Flow", &changed)
                .is_err()
        );
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        let published =
            super::prepare_library_track(root.path(), "focus-flow", "Focus Flow", &source)
                .unwrap()
                .publish()
                .unwrap();
        let saved = root.path().join(&published.path);
        assert!(saved.is_file());
        drop(published);
        assert!(!saved.exists());
        assert!(!crate::asset_library::record_path(&saved).exists());
        assert_eq!(std::fs::read(&destination).unwrap(), b"concurrent file");
    }

    #[cfg(unix)]
    #[test]
    fn bundled_music_does_not_follow_project_audio_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("assets")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("assets/audio")).unwrap();
        let source = super::library_track_metadata("focus-flow").unwrap();
        assert!(
            super::prepare_library_track(root.path(), "focus-flow", "Focus Flow", &source).is_err()
        );
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[test]
    fn audio_reuse_allocates_a_new_lane_without_copying_or_overwriting_existing_media() {
        let root = tempfile::tempdir().unwrap();
        let (path, name, duration) =
            copy_library_track(root.path(), "focus-flow", "Focus Flow").unwrap();
        let record = super::record_audio_asset(root.path(), &path, &name).unwrap();
        let media = root.path().join(&path);
        let before = std::fs::read(&media).unwrap();
        let source_before = std::fs::read(crate::asset_library::record_path(&media)).unwrap();
        let mut project = ProjectConfiguration {
            timeline: Some(
                serde_json::from_value(serde_json::json!({
                    "segments": [{"start":0,"end":8,"timescale":1}], "zoomSegments":[]
                }))
                .unwrap(),
            ),
            ..Default::default()
        };
        project.timeline.as_mut().unwrap().audio_segments.push(
            crate::editor_edits::default_audio_segment(
                0.,
                8.,
                0,
                path.clone(),
                name.clone(),
                Some(duration),
            ),
        );
        let mut history = crate::editor_edits::ProjectHistory::new(project.clone());
        let timeline = project.timeline.as_mut().unwrap();
        let (lane, range) =
            super::audio_import_placement(&timeline.audio_segments, 0, 1, 3., 8., duration)
                .unwrap();
        assert_eq!((lane, range.clone()), (1, 3.0..3.0 + duration));
        crate::editor_edits::insert_audio_segment(
            timeline,
            crate::editor_edits::default_audio_segment(
                range.start,
                range.end,
                lane,
                path.clone(),
                name,
                Some(duration),
            ),
        );
        crate::asset_library::ensure_record(root.path(), &record).unwrap();
        history.record(&project);
        assert_eq!(
            crate::asset_library::collect_project_records(root.path(), &project)
                .records
                .len(),
            1
        );
        assert_eq!(
            std::fs::read_dir(media.parent().unwrap()).unwrap().count(),
            2
        );
        assert_eq!(std::fs::read(&media).unwrap(), before);
        assert_eq!(
            std::fs::read(crate::asset_library::record_path(&media)).unwrap(),
            source_before
        );
        let undone = &history
            .undo()
            .unwrap()
            .timeline
            .as_ref()
            .unwrap()
            .audio_segments;
        assert_eq!(undone.len(), 1);
        assert_eq!(
            (undone[0].start, undone[0].end, undone[0].track),
            (0., 8., 0)
        );
        let redone = &history
            .redo()
            .unwrap()
            .timeline
            .as_ref()
            .unwrap()
            .audio_segments;
        assert_eq!(redone.len(), 2);
        assert!(redone.iter().all(|segment| segment.path == path));
        assert_eq!(
            super::audio_import_placement(redone, 0, 2, 8., 12., duration),
            Some((0, 8.0..8.0 + duration))
        );
        assert_eq!(
            super::audio_import_placement(redone, 0, 2, 8., 8., duration),
            Some((0, 8.0..8.0 + duration))
        );
    }

    #[test]
    fn project_audio_asset_record_is_idempotent_and_survives_timeline_undo() {
        let root = tempfile::tempdir().unwrap();
        let (path, name, duration) =
            copy_library_track(root.path(), "focus-flow", "Focus Flow").unwrap();
        let before = std::fs::read(root.path().join(&path)).unwrap();
        super::record_audio_asset(root.path(), &path, &name).unwrap();
        super::record_audio_asset(root.path(), &path, &name).unwrap();
        assert_eq!(
            std::fs::read_dir(root.path().join("assets/audio"))
                .unwrap()
                .count(),
            2
        );
        assert_eq!(std::fs::read(root.path().join(&path)).unwrap(), before);
        let initial = ProjectConfiguration {
            timeline: Some(
                serde_json::from_value(serde_json::json!({"segments": [], "zoomSegments": []}))
                    .unwrap(),
            ),
            ..Default::default()
        };
        let mut history = crate::editor_edits::ProjectHistory::new(initial.clone());
        let mut added = initial;
        added.timeline.as_mut().unwrap().audio_segments.push(
            crate::editor_edits::default_audio_segment(
                0.,
                duration.min(5.),
                0,
                path.clone(),
                name,
                Some(duration),
            ),
        );
        history.record(&added);
        let undone = history.undo().unwrap().clone();
        undone.write(root.path()).unwrap();
        let reopened = ProjectConfiguration::load(root.path()).unwrap();
        let inventory = crate::asset_library::collect_project_records(root.path(), &reopened);
        assert_eq!(inventory.records.len(), 1);
        assert_eq!(
            inventory.records[0].kind,
            crate::asset_library::AssetKind::Audio
        );
        assert_eq!(
            history
                .redo()
                .unwrap()
                .timeline
                .as_ref()
                .unwrap()
                .audio_segments[0]
                .path,
            path
        );
    }

    #[test]
    fn project_audio_asset_record_failure_preserves_existing_source_and_bad_sidecar() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("assets/audio")).unwrap();
        let path = "assets/audio/source.wav";
        let media = root.path().join(path);
        std::fs::write(&media, b"source").unwrap();
        std::fs::write(
            crate::asset_library::record_path(&media),
            b"damaged source record",
        )
        .unwrap();
        assert!(super::record_audio_asset(root.path(), path, "Source").is_err());
        assert_eq!(std::fs::read(media).unwrap(), b"source");
        assert_eq!(
            std::fs::read(crate::asset_library::record_path(&root.path().join(path))).unwrap(),
            b"damaged source record"
        );
    }

    #[test]
    fn audio_import_preserves_full_source_even_at_or_after_video_end() {
        assert_eq!(audio_import_range(15.0, 20.0, 10.0), Some(15.0..25.0));
        assert_eq!(audio_import_range(2.0, 20.0, 0.2), Some(2.0..2.2));
        assert_eq!(audio_import_range(20., 20., 10.), Some(20.0..30.0));
        assert_eq!(audio_import_range(21., 20., 10.), Some(21.0..31.0));
        assert_eq!(audio_import_range(-1.0, 20.0, 4.0), Some(0.0..4.0));
        for (playhead, total, source) in [
            (0.0, 0.0, 10.0),
            (0.0, 20.0, 0.0),
            (f64::NAN, 20.0, 10.0),
            (0.0, f64::INFINITY, 10.0),
            (0.0, 20.0, f64::NAN),
        ] {
            assert_eq!(audio_import_range(playhead, total, source), None);
        }
    }

    fn audio_project() -> ProjectConfiguration {
        serde_json::from_value(serde_json::json!({
            "timeline": {
                "segments": [],
                "zoomSegments": [],
                "audioSegments": [
                    {"start": 2.0, "end": 22.0, "track": 1, "path": "old.wav", "name": "Old", "trimStart": 10.0, "volumeDb": -6.0, "fadeIn": 8.0, "fadeOut": 7.0, "duration": 30.0},
                    {"start": 25.0, "end": 29.0, "track": 2, "path": "other.wav", "name": "Other", "duration": 4.0}
                ]
            }
        }))
        .unwrap()
    }

    fn finish_replacement(
        request: &AudioImportRequest,
        picker: Option<AudioPicker>,
        generation: u64,
        project: &mut ProjectConfiguration,
    ) -> bool {
        if !request.accepts(picker, generation, project) {
            return false;
        }
        let AudioPicker::Replace { index } = request.picker else {
            return false;
        };
        replace_audio_asset(
            project.timeline.as_mut().unwrap(),
            index,
            "short.wav".into(),
            "Short".into(),
            3.0,
        )
    }

    #[test]
    fn audio_import_rejects_closed_reopened_and_changed_picker_sessions() {
        let mut project = audio_project();
        let picker = AudioPicker::Replace { index: 0 };
        let request = AudioImportRequest::new(picker, 1, &project).unwrap();
        let before = serde_json::to_value(&project).unwrap();
        for (active, generation) in [
            (None, 1),
            (Some(picker), 2),
            (Some(AudioPicker::Replace { index: 1 }), 1),
            (Some(AudioPicker::Add { lane: 1 }), 1),
        ] {
            assert!(!finish_replacement(
                &request,
                active,
                generation,
                &mut project
            ));
            assert_eq!(serde_json::to_value(&project).unwrap(), before);
        }
    }

    #[test]
    fn audio_import_rejects_deleted_reordered_and_edited_targets() {
        for change in 0..3 {
            let mut project = audio_project();
            let picker = AudioPicker::Replace { index: 0 };
            let request = AudioImportRequest::new(picker, 1, &project).unwrap();
            let segments = &mut project.timeline.as_mut().unwrap().audio_segments;
            match change {
                0 => {
                    segments.remove(0);
                }
                1 => segments.swap(0, 1),
                _ => segments[0].trim_start = 11.0,
            }
            let before = serde_json::to_value(&project).unwrap();
            assert!(!finish_replacement(&request, Some(picker), 1, &mut project));
            assert_eq!(serde_json::to_value(&project).unwrap(), before);
        }
    }

    #[test]
    fn replacing_trimmed_audio_starts_new_source_and_clamps_short_source() {
        let mut project = audio_project();
        let picker = AudioPicker::Replace { index: 0 };
        let request = AudioImportRequest::new(picker, 1, &project).unwrap();
        let mut expected = serde_json::to_value(&project).unwrap();
        let segment = &mut expected["timeline"]["audioSegments"][0];
        segment["path"] = "short.wav".into();
        segment["name"] = "Short".into();
        segment["duration"] = 3.0.into();
        segment["trimStart"] = 0.0.into();
        segment["end"] = 5.0.into();
        segment["fadeIn"] = 3.0.into();
        segment["fadeOut"] = 3.0.into();
        assert!(finish_replacement(&request, Some(picker), 1, &mut project));
        assert_eq!(serde_json::to_value(&project).unwrap(), expected);
        assert!(!finish_replacement(&request, Some(picker), 1, &mut project));
    }

    #[test]
    fn replacement_preserves_shorter_timeline_and_unknown_duration() {
        for duration in [60.0, 0.0] {
            let mut project = audio_project();
            let timeline = project.timeline.as_mut().unwrap();
            assert!(replace_audio_asset(
                timeline,
                0,
                "new.wav".into(),
                "New".into(),
                duration,
            ));
            let segment = &timeline.audio_segments[0];
            assert_eq!(segment.end, 22.0);
            assert_eq!(segment.trim_start, 0.0);
            assert_eq!(segment.fade_in, 8.0);
            assert_eq!(segment.fade_out, 7.0);
            assert_eq!(segment.volume_db, -6.0);
            assert_eq!(segment.duration, (duration > 0.0).then_some(duration));
        }
    }

    #[test]
    fn add_audio_import_is_bound_to_its_lane_and_picker_session() {
        let project = audio_project();
        let picker = AudioPicker::Add { lane: 4 };
        let request = AudioImportRequest::new(picker, 7, &project).unwrap();
        assert!(request.accepts(Some(picker), 7, &project));
        assert!(!request.accepts(None, 7, &project));
        assert!(!request.accepts(Some(picker), 8, &project));
        assert!(!request.accepts(Some(AudioPicker::Add { lane: 5 }), 7, &project));
        assert!(AudioImportRequest::new(AudioPicker::Replace { index: 9 }, 7, &project).is_none());
    }

    #[test]
    fn copied_library_audio_includes_source_duration() {
        let root =
            std::env::temp_dir().join(format!("cap-gpui-audio-duration-{}", std::process::id()));
        let (path, name, duration) =
            copy_library_track(&root, "lofi-beats-mirostar", "Lofi Beats").unwrap();
        assert_eq!(name, "Lofi Beats");
        assert!(duration > 0.0);
        assert_eq!(duration, probe_audio_duration(&root.join(path)));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn built_in_music_resolves_from_an_installed_bundle() {
        let root =
            std::env::temp_dir().join(format!("cap-gpui-installed-music-{}", std::process::id()));
        let resources = root.join("Cap.app/Contents/Resources");
        let music = resources.join("assets/music");
        std::fs::create_dir_all(&music).unwrap();
        let track = music.join("lofi-beats-mirostar.mp3");
        std::fs::write(&track, b"test track").unwrap();

        assert_eq!(
            bundled_track_path_from("lofi-beats-mirostar", &[resources], &root.join("missing")),
            Some(track)
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn built_in_music_remains_available_from_the_development_checkout() {
        for (id, _) in AUDIO_LIBRARY {
            assert!(
                bundled_track_path(id).is_some(),
                "missing bundled track {id}"
            );
        }
    }

    #[test]
    fn built_in_music_rejects_unknown_and_traversal_identifiers() {
        assert_eq!(bundled_track_path("unknown-track"), None);
        assert_eq!(bundled_track_path("../lofi-beats-mirostar"), None);
    }
}
