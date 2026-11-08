use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result, anyhow, bail};
use cap_recording::{
    OutputPipeline, OwnedAudioPipelineScope, PipelineHealthEvent,
    feeds::microphone::MicrophoneFeedLock,
    ffmpeg::{FragmentedAudioMuxer, FragmentedAudioMuxerConfig},
    sources::Microphone,
};
use cap_timestamp::{Timestamp, Timestamps};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot, watch};
use uuid::Uuid;

const VERSION: u32 = 1;
const MANIFEST_LIMIT: u64 = 16 * 1024;
const CAPTURE_LIMIT: u64 = 8 * 1024 * 1024 * 1024;
const CAPTURE: &str = "capture.m4a";
const MANIFEST: &str = "take.json";
const LEASE: &str = "lease.lock";

#[derive(Clone, Debug)]
pub struct VoiceoverRequest {
    pub project_path: PathBuf,
    pub id: Uuid,
    pub output_start: f64,
    pub lane: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TakeState {
    Recording,
    Paused,
    Finalizing,
    Ready,
    Attached,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TakeAudio {
    pub file: String,
    pub duration: f64,
    pub sample_rate: u32,
    pub channels: u16,
    pub recovered: bool,
    #[serde(default)]
    pub peak_db: Option<f32>,
    #[serde(default)]
    pub rms_db: Option<f32>,
}

impl TakeAudio {
    pub fn suggested_gain_db(&self) -> Option<f32> {
        let peak = self.peak_db?;
        let rms = self.rms_db?;
        (peak.is_finite() && rms.is_finite() && rms > -65.0 && peak > -60.0)
            .then(|| (-6.0 - peak).clamp(-12.0, 24.0))
    }
}

#[derive(Default)]
struct AudioLevels {
    peak: f64,
    squares: f64,
    count: u64,
}

impl AudioLevels {
    fn push(&mut self, samples: &[f32]) {
        for sample in samples.iter().copied().filter(|sample| sample.is_finite()) {
            let sample = f64::from(sample);
            self.peak = self.peak.max(sample.abs());
            self.squares += sample * sample;
            self.count += 1;
        }
    }

    fn db(&self) -> (f32, f32) {
        let rms = (self.squares / self.count.max(1) as f64).sqrt();
        (
            (20.0 * self.peak.max(1e-6).log10()) as f32,
            (20.0 * rms.max(1e-6).log10()) as f32,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TakeManifest {
    pub version: u32,
    pub id: Uuid,
    pub output_start: f64,
    pub lane: u32,
    pub created_at_ms: u64,
    pub state: TakeState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_received: Option<bool>,
    pub audio: Option<TakeAudio>,
    pub last_error: Option<String>,
}

impl TakeManifest {
    fn validate(&self, id: Uuid) -> Result<()> {
        if self.version != VERSION || self.id != id || id.is_nil() {
            bail!("Invalid voiceover take identity or version");
        }
        validate_destination(self.output_start, self.lane)?;
        if self
            .last_error
            .as_ref()
            .is_some_and(|message| message.len() > 2048)
        {
            bail!("Invalid voiceover error metadata");
        }
        if let Some(audio) = &self.audio {
            if !valid_media_name(&audio.file)
                || !audio.duration.is_finite()
                || audio.duration <= 0.0
                || !(8_000..=192_000).contains(&audio.sample_rate)
                || !(1..=8).contains(&audio.channels)
            {
                bail!("Invalid voiceover audio metadata");
            }
        } else if matches!(self.state, TakeState::Ready | TakeState::Attached) {
            bail!("A finished voiceover is missing its audio metadata");
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct VoiceoverStatus {
    pub state: TakeState,
    pub duration_estimate: Duration,
    pub first_sample_seen: bool,
    pub health: Vec<PipelineHealthEvent>,
    pub error: Option<String>,
}

pub struct VoiceoverSession {
    request: VoiceoverRequest,
    commands: mpsc::UnboundedSender<Command>,
    status: watch::Receiver<VoiceoverStatus>,
    closed: bool,
}

enum Command {
    Pause(bool, oneshot::Sender<Result<()>>),
    Stop(oneshot::Sender<Result<ReadyTake>>),
    Cancel(oneshot::Sender<Result<()>>),
    Abandon,
}

impl VoiceoverSession {
    pub async fn start(
        request: VoiceoverRequest,
        microphone: Arc<MicrophoneFeedLock>,
    ) -> Result<Self> {
        let prepared_request = request.clone();
        let lease =
            tokio::task::spawn_blocking(move || TakeLease::create(prepared_request)).await??;
        let request = VoiceoverRequest {
            project_path: lease.project.clone(),
            ..request
        };
        let manifest = lease.read()?;
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (status_tx, status) = watch::channel(VoiceoverStatus {
            state: TakeState::Recording,
            duration_estimate: Duration::ZERO,
            first_sample_seen: false,
            health: Vec::new(),
            error: None,
        });
        let session = Self {
            request,
            commands,
            status,
            closed: false,
        };
        let (ready_tx, ready_rx) = oneshot::channel();
        tokio::spawn(async move {
            let epoch = Timestamps::now();
            let scope = OwnedAudioPipelineScope::default();
            let pipeline = scope
                .build::<FragmentedAudioMuxer>(
                    OutputPipeline::builder(lease.dir.join(CAPTURE))
                        .with_audio_source::<Microphone>(microphone)
                        .with_timestamps(epoch),
                    FragmentedAudioMuxerConfig {
                        shared_pause_state: None,
                        write_behind: cfg!(target_os = "macos"),
                    },
                )
                .await;
            match pipeline {
                Ok(pipeline) => {
                    let _ = ready_tx.send(Ok(()));
                    run_session(
                        lease, manifest, pipeline, scope, epoch, command_rx, status_tx,
                    )
                    .await;
                }
                Err(error) => {
                    let joined = scope.shutdown().await;
                    let error = record_failure(&lease, manifest, error);
                    let _ = ready_tx.send(Err(error));
                    if !joined.quiescent {
                        retain_unconfirmed_lease(lease);
                    }
                }
            }
        });
        ready_rx
            .await
            .context("Voiceover startup ended unexpectedly")??;
        Ok(session)
    }

    pub fn request(&self) -> &VoiceoverRequest {
        &self.request
    }

    pub fn status(&self) -> VoiceoverStatus {
        self.status.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<VoiceoverStatus> {
        self.status.clone()
    }

    pub async fn pause(&self) -> Result<()> {
        self.set_paused(true).await
    }

    pub async fn resume(&self) -> Result<()> {
        self.set_paused(false).await
    }

    async fn set_paused(&self, paused: bool) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.commands
            .send(Command::Pause(paused, tx))
            .map_err(|_| anyhow!("Voiceover has stopped"))?;
        rx.await
            .context("Voiceover pause request ended unexpectedly")?
    }

    pub async fn stop(mut self) -> Result<ReadyTake> {
        let (tx, rx) = oneshot::channel();
        let sent = self.commands.send(Command::Stop(tx)).is_ok();
        self.closed = true;
        if sent && let Ok(result) = rx.await {
            return result;
        }
        let status = terminal_status(self.status.clone()).await?;
        if status.state != TakeState::Ready {
            bail!(
                "{}",
                status.error.unwrap_or_else(|| {
                    "Voiceover ended without a ready take; its files are preserved".into()
                })
            );
        }
        recover_take(self.request.project_path.clone(), self.request.id).await
    }

    pub async fn cancel(mut self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let sent = self.commands.send(Command::Cancel(tx)).is_ok();
        self.closed = true;
        if sent && let Ok(result) = rx.await {
            return result;
        }
        let status = terminal_status(self.status.clone()).await?;
        if status.state == TakeState::Cancelled {
            return Ok(());
        }
        let request = self.request.clone();
        tokio::task::spawn_blocking(move || discard_finished_take(&request)).await?
    }
}

async fn terminal_status(mut status: watch::Receiver<VoiceoverStatus>) -> Result<VoiceoverStatus> {
    loop {
        let current = status.borrow_and_update().clone();
        if matches!(
            current.state,
            TakeState::Ready | TakeState::Failed | TakeState::Cancelled
        ) {
            return Ok(current);
        }
        status
            .changed()
            .await
            .context("Voiceover shutdown could not be confirmed; its files are preserved")?;
    }
}

fn read_saved_project(project: &Path) -> Result<cap_project::ProjectConfiguration> {
    serde_json::from_reader(File::open(project.join("project-config.json"))?)
        .context("Could not read saved project audio references")
}

fn discard_finished_take(request: &VoiceoverRequest) -> Result<()> {
    let lease = TakeLease::open(&request.project_path, request.id)?;
    let mut manifest = lease.read()?;
    if !matches!(
        manifest.state,
        TakeState::Ready | TakeState::Failed | TakeState::Cancelled
    ) {
        bail!("This voiceover has not finished or is already attached");
    }
    let project = read_saved_project(&lease.project)?;
    let prefix = format!("content/voiceovers/{}/", request.id);
    if project.timeline.as_ref().is_some_and(|timeline| {
        timeline
            .audio_segments
            .iter()
            .any(|segment| segment.path.starts_with(&prefix))
    }) {
        bail!("The project already references this voiceover; its files are preserved");
    }
    manifest.state = TakeState::Cancelled;
    lease.write(&manifest)?;
    lease.remove_owned()
}

impl Drop for VoiceoverSession {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.commands.send(Command::Abandon);
        }
    }
}

#[derive(Default)]
struct RecordingClock {
    first: Option<Instant>,
    pause_started: Option<Instant>,
    paused: Duration,
}

impl RecordingClock {
    fn observe_first(&mut self, timestamp: Option<Timestamp>, epoch: Timestamps) {
        if self.first.is_none() {
            self.first = timestamp.map(|timestamp| {
                epoch
                    .instant()
                    .checked_add(
                        timestamp
                            .checked_duration_since(epoch)
                            .unwrap_or(Duration::ZERO),
                    )
                    .unwrap_or(epoch.instant())
            });
        }
    }

    fn set_paused(&mut self, paused: bool, now: Instant) {
        if paused {
            self.pause_started.get_or_insert(now);
        } else if let Some(start) = self.pause_started.take() {
            if let Some(first) = self.first {
                self.paused += now.saturating_duration_since(start.max(first));
            }
        }
    }

    fn duration(&self, now: Instant) -> Duration {
        let Some(first) = self.first else {
            return Duration::ZERO;
        };
        let current_pause = self.pause_started.map_or(Duration::ZERO, |start| {
            now.saturating_duration_since(start.max(first))
        });
        now.saturating_duration_since(first)
            .saturating_sub(self.paused)
            .saturating_sub(current_pause)
    }
}

async fn run_session(
    lease: TakeLease,
    mut manifest: TakeManifest,
    mut pipeline: OutputPipeline,
    scope: OwnedAudioPipelineScope,
    epoch: Timestamps,
    mut commands: mpsc::UnboundedReceiver<Command>,
    status: watch::Sender<VoiceoverStatus>,
) {
    let mut health = pipeline.take_health_rx();
    let first = pipeline.first_timestamp_watch();
    let done = pipeline.done_fut();
    let mut clock = RecordingClock::default();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let command = tokio::select! {
            command = commands.recv() => command.unwrap_or(Command::Abandon),
            _ = tick.tick() => {
                clock.observe_first(*first.borrow(), epoch);
                status.send_modify(|value| {
                    value.first_sample_seen = clock.first.is_some();
                    value.duration_estimate = clock.duration(Instant::now());
                });
                continue;
            },
            event = async { match &mut health { Some(rx) => rx.recv().await, None => std::future::pending().await } } => {
                if let Some(event) = event {
                    status.send_modify(|value| {
                        if value.health.len() == 16 { value.health.remove(0); }
                        value.health.push(event);
                    });
                } else { health = None; }
                continue;
            },
            _ = done.clone() => Command::Abandon,
        };
        match command {
            Command::Pause(paused, reply) => {
                let next = if paused {
                    TakeState::Paused
                } else {
                    TakeState::Recording
                };
                let result = if manifest.state == next {
                    Ok(())
                } else {
                    let mut updated = manifest.clone();
                    updated.state = next;
                    lease.write(&updated).map(|()| {
                        if paused {
                            pipeline.pause();
                        } else {
                            pipeline.resume();
                        }
                        clock.set_paused(paused, Instant::now());
                        manifest = updated;
                        status.send_modify(|value| value.state = next);
                    })
                };
                let _ = reply.send(result);
            }
            command => {
                status.send_modify(|value| value.state = TakeState::Finalizing);
                let cleanup = matches!(&command, Command::Cancel(_));
                let result = finish_session(lease, manifest, pipeline, scope, cleanup).await;
                status.send_modify(|value| match &result {
                    Ok(Some(take)) => {
                        value.state = TakeState::Ready;
                        value.duration_estimate = Duration::from_secs_f64(take.audio().duration);
                    }
                    Ok(None) => value.state = TakeState::Cancelled,
                    Err(error) => {
                        value.state = TakeState::Failed;
                        value.error = Some(bounded_error(error));
                    }
                });
                match command {
                    Command::Stop(reply) => {
                        let _ = reply.send(result.and_then(|take| {
                            take.ok_or_else(|| anyhow!("Voiceover was cancelled"))
                        }));
                    }
                    Command::Cancel(reply) => {
                        let _ = reply.send(result.map(|_| ()));
                    }
                    Command::Abandon => {}
                    Command::Pause(..) => unreachable!(),
                }
                return;
            }
        }
    }
}

async fn finish_session(
    lease: TakeLease,
    mut manifest: TakeManifest,
    pipeline: OutputPipeline,
    scope: OwnedAudioPipelineScope,
    cleanup: bool,
) -> Result<Option<ReadyTake>> {
    manifest.state = TakeState::Finalizing;
    let manifest_error = lease.write(&manifest).err();
    let first_sample = pipeline.first_timestamp_watch();
    let stopped = pipeline.stop().await;
    let joined = scope.shutdown().await;
    if !joined.quiescent {
        let error = record_failure(
            &lease,
            manifest,
            anyhow!(
                "Voiceover stop could not be confirmed; files remain locked until this application exits: {}",
                joined.error.unwrap_or_default()
            ),
        );
        retain_unconfirmed_lease(lease);
        return Err(error);
    }
    manifest.input_received = Some(first_sample.borrow().is_some());
    if !cleanup {
        if manifest.input_received == Some(false) {
            return Err(record_failure(
                &lease,
                manifest,
                anyhow!("No microphone samples were recorded; the take is preserved"),
            ));
        }
        if let Err(error) = stopped {
            return Err(record_failure(&lease, manifest, error));
        }
        if let Some(error) = joined.error {
            return Err(record_failure(&lease, manifest, anyhow!(error)));
        }
    }
    if cleanup {
        return tokio::task::spawn_blocking(move || {
            manifest.state = TakeState::Cancelled;
            lease.write(&manifest)?;
            lease.remove_owned()?;
            Ok(None)
        })
        .await?;
    }
    if let Some(error) = manifest_error {
        return Err(record_failure(&lease, manifest, error));
    }
    tokio::task::spawn_blocking(move || {
        match finalize_audio(&lease, &mut manifest, CAPTURE, false) {
            Ok(take) => Ok(Some(take)),
            Err(error) => Err(record_failure(&lease, manifest, error)),
        }
    })
    .await?
}

fn retain_unconfirmed_lease(lease: TakeLease) {
    tokio::spawn(async move {
        let _lease = lease;
        std::future::pending::<()>().await;
    });
}

#[derive(Clone, Debug)]
pub struct ReadyTake {
    project: PathBuf,
    manifest: TakeManifest,
}

impl ReadyTake {
    pub fn manifest(&self) -> &TakeManifest {
        &self.manifest
    }

    pub fn audio(&self) -> &TakeAudio {
        self.manifest.audio.as_ref().expect("validated ready take")
    }

    pub fn project_path(&self) -> &Path {
        &self.project
    }

    pub fn relative_path(&self) -> String {
        format!(
            "content/voiceovers/{}/{}",
            self.manifest.id,
            self.audio().file
        )
    }

    pub fn path(&self) -> PathBuf {
        self.project.join(self.relative_path())
    }

    pub fn is_in_project(&self, project: &cap_project::ProjectConfiguration) -> bool {
        let path = self.relative_path();
        project.timeline.as_ref().is_some_and(|timeline| {
            timeline
                .audio_segments
                .iter()
                .any(|segment| segment.path == path)
        })
    }

    pub fn mark_attached(&mut self) -> Result<()> {
        let lease = TakeLease::open(&self.project, self.manifest.id)?;
        let mut manifest = lease.read()?;
        if !matches!(manifest.state, TakeState::Ready | TakeState::Attached) {
            bail!("Voiceover is not ready to attach");
        }
        if manifest.audio != self.manifest.audio
            || manifest.output_start != self.manifest.output_start
            || manifest.lane != self.manifest.lane
        {
            bail!("Voiceover take changed before attachment");
        }
        safe_file(&lease.dir.join(&self.audio().file))?;
        let project = read_saved_project(&self.project)
            .context("Save the project before marking the voiceover attached")?;
        if !self.is_in_project(&project) {
            bail!("The saved project does not reference this voiceover");
        }
        if manifest.state != TakeState::Attached {
            manifest.state = TakeState::Attached;
            lease.write(&manifest)?;
        }
        self.manifest = manifest;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TakeAvailability {
    Active,
    Interrupted,
    Ready,
    Attached,
    Missing,
    Empty,
    Invalid,
    Cancelled,
}

#[derive(Debug)]
pub struct DiscoveredTake {
    pub directory: PathBuf,
    pub manifest: Option<TakeManifest>,
    pub availability: TakeAvailability,
    pub problem: Option<String>,
}

pub fn list_takes(project: &Path) -> Result<Vec<DiscoveredTake>> {
    let project = project.canonicalize()?;
    let root = project.join("content/voiceovers");
    if !root.try_exists()? {
        return Ok(Vec::new());
    }
    safe_directory(&project.join("content"))?;
    safe_directory(&root)?;
    let mut takes = Vec::new();
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        if takes.len() == 4096 {
            bail!("Too many voiceover takes to enumerate safely");
        }
        let mut discovered = DiscoveredTake {
            directory: entry.path(),
            manifest: None,
            availability: TakeAvailability::Invalid,
            problem: None,
        };
        let inspected = (|| -> Result<()> {
            let id = Uuid::parse_str(&entry.file_name().to_string_lossy())?;
            let manifest = read_manifest(&entry.path(), id)?;
            discovered.manifest = Some(manifest.clone());
            match TakeLease::open(&project, id) {
                Err(error) if error.is::<ActiveTake>() => {
                    discovered.availability = TakeAvailability::Active;
                    return Ok(());
                }
                Err(error) => return Err(error),
                Ok(_lease) => {
                    let file = manifest
                        .audio
                        .as_ref()
                        .map_or(CAPTURE, |audio| audio.file.as_str());
                    if !entry.path().join(file).try_exists()? {
                        discovered.availability = TakeAvailability::Missing;
                        discovered.problem = Some("Voiceover audio file is missing".into());
                    } else {
                        safe_file(&entry.path().join(file))?;
                        discovered.availability = if manifest.input_received == Some(false) {
                            TakeAvailability::Empty
                        } else {
                            match manifest.state {
                                TakeState::Ready => TakeAvailability::Ready,
                                TakeState::Attached => TakeAvailability::Attached,
                                TakeState::Cancelled => TakeAvailability::Cancelled,
                                _ => TakeAvailability::Interrupted,
                            }
                        };
                    }
                }
            }
            Ok(())
        })();
        if let Err(error) = inspected {
            discovered.problem = Some(bounded_error(&error));
        }
        takes.push(discovered);
    }
    takes.sort_by(|a, b| a.directory.cmp(&b.directory));
    Ok(takes)
}

pub async fn recover_take(project: PathBuf, id: Uuid) -> Result<ReadyTake> {
    tokio::task::spawn_blocking(move || {
        let lease = TakeLease::open(&project, id)?;
        let mut manifest = lease.read()?;
        if manifest.input_received == Some(false) {
            bail!("No microphone samples were recorded; there is no voiceover to recover");
        }
        if matches!(manifest.state, TakeState::Ready | TakeState::Attached) {
            validate_audio(&lease.dir.join(&manifest.audio.as_ref().unwrap().file))?;
            return Ok(ReadyTake {
                project: lease.project.clone(),
                manifest,
            });
        }
        if manifest.state == TakeState::Cancelled {
            bail!("This voiceover was explicitly cancelled");
        }
        let recovered = if let Some(audio) = manifest.audio.clone() {
            finalize_audio(&lease, &mut manifest, &audio.file, audio.recovered)
        } else {
            recover_audio_prefix(&lease)
                .and_then(|candidate| finalize_recovered_audio(&lease, &mut manifest, candidate))
        };
        match recovered {
            Ok(take) => Ok(take),
            Err(error) => Err(record_failure(&lease, manifest, error)),
        }
    })
    .await?
}

#[derive(Debug)]
struct ActiveTake;

impl std::fmt::Display for ActiveTake {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("This voiceover take is still active")
    }
}

impl std::error::Error for ActiveTake {}

struct TakeLease {
    project: PathBuf,
    dir: PathBuf,
    lock: File,
}

impl TakeLease {
    fn create(request: VoiceoverRequest) -> Result<Self> {
        validate_destination(request.output_start, request.lane)?;
        if request.id.is_nil() {
            bail!("Voiceover take ID cannot be empty");
        }
        let project = request.project_path.canonicalize()?;
        safe_directory(&project)?;
        let content = project.join("content");
        create_directory(&content)?;
        let root = content.join("voiceovers");
        create_directory(&root)?;
        let dir = root.join(request.id.to_string());
        fs::create_dir(&dir).context("Voiceover take already exists or cannot be created")?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(dir.join(LEASE))?;
        lock.try_lock()
            .map_err(|error| anyhow!("Cannot lock new voiceover: {error}"))?;
        let lease = Self { project, dir, lock };
        lease.write(&TakeManifest {
            version: VERSION,
            id: request.id,
            output_start: request.output_start,
            lane: request.lane,
            created_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)?
                .as_millis()
                .try_into()?,
            state: TakeState::Recording,
            input_received: None,
            audio: None,
            last_error: None,
        })?;
        Ok(lease)
    }

    fn open(project: &Path, id: Uuid) -> Result<Self> {
        let project = project.canonicalize()?;
        safe_directory(&project.join("content"))?;
        safe_directory(&project.join("content/voiceovers"))?;
        let dir = project.join("content/voiceovers").join(id.to_string());
        safe_directory(&dir)?;
        safe_file(&dir.join(LEASE))?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(dir.join(LEASE))?;
        match lock.try_lock() {
            Ok(()) => Ok(Self { project, dir, lock }),
            Err(std::fs::TryLockError::WouldBlock) => Err(ActiveTake.into()),
            Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
        }
    }

    fn read(&self) -> Result<TakeManifest> {
        let id = Uuid::parse_str(
            self.dir
                .file_name()
                .unwrap()
                .to_str()
                .ok_or_else(|| anyhow!("Invalid take directory"))?,
        )?;
        read_manifest(&self.dir, id)
    }

    fn write(&self, manifest: &TakeManifest) -> Result<()> {
        manifest.validate(Uuid::parse_str(
            self.dir.file_name().unwrap().to_str().unwrap(),
        )?)?;
        atomic_manifest(&self.dir, manifest)
    }

    fn remove_owned(self) -> Result<()> {
        let audio = self.read()?.audio.map(|audio| audio.file);
        let entries = fs::read_dir(&self.dir)?.collect::<std::io::Result<Vec<_>>>()?;
        for entry in &entries {
            let name = entry.file_name();
            let name = name
                .to_str()
                .ok_or_else(|| anyhow!("Unknown file in voiceover take; preserving it"))?;
            if !matches!(name, MANIFEST | LEASE | CAPTURE) && audio.as_deref() != Some(name) {
                bail!("Unknown file in voiceover take; preserving it");
            }
            safe_file(&entry.path())?;
        }
        for entry in entries {
            if entry.file_name() != LEASE && entry.file_name() != MANIFEST {
                fs::remove_file(entry.path())?;
            }
        }
        fs::remove_file(self.dir.join(MANIFEST))?;
        drop(self.lock);
        fs::remove_file(self.dir.join(LEASE))?;
        fs::remove_dir(self.dir)?;
        Ok(())
    }
}

fn validate_destination(start: f64, lane: u32) -> Result<()> {
    if !start.is_finite() || start < 0.0 || start > 7.0 * 24.0 * 3600.0 || lane > 4095 {
        bail!("Invalid voiceover output position or audio lane");
    }
    Ok(())
}

fn valid_media_name(name: &str) -> bool {
    name == CAPTURE
        || name
            .strip_prefix("recovered-")
            .and_then(|value| value.strip_suffix(".m4a"))
            .is_some_and(|value| Uuid::parse_str(value).is_ok())
}

fn safe_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        bail!("Voiceover directory is not a real directory");
    }
    Ok(())
}

fn create_directory(path: &Path) -> Result<()> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => safe_directory(path),
        Err(error) => Err(error.into()),
    }
}

fn safe_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        bail!("Voiceover file is not a regular local file");
    }
    Ok(())
}

fn read_manifest(dir: &Path, id: Uuid) -> Result<TakeManifest> {
    safe_directory(dir)?;
    let path = dir.join(MANIFEST);
    safe_file(&path)?;
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MANIFEST_LIMIT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MANIFEST_LIMIT {
        bail!("Voiceover manifest is too large");
    }
    let manifest: TakeManifest = serde_json::from_slice(&bytes)?;
    manifest.validate(id)?;
    Ok(manifest)
}

fn atomic_manifest(dir: &Path, manifest: &TakeManifest) -> Result<()> {
    let path = dir.join(format!(".take-{}.tmp", Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let result = (|| -> Result<()> {
        let bytes = serde_json::to_vec_pretty(manifest)?;
        if bytes.len() as u64 > MANIFEST_LIMIT {
            bail!("Voiceover manifest is too large");
        }
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        let destination = dir.join(MANIFEST);
        if destination.try_exists()? {
            safe_file(&destination)?;
        }
        fs::rename(&path, destination)?;
        #[cfg(unix)]
        File::open(dir)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}

fn bounded_error(error: &anyhow::Error) -> String {
    let message = format!("{error:#}");
    let end = message
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= 2000)
        .last()
        .unwrap_or(0);
    if message.len() <= 2000 {
        message
    } else {
        message[..end].to_owned()
    }
}

fn record_failure(
    lease: &TakeLease,
    mut manifest: TakeManifest,
    error: anyhow::Error,
) -> anyhow::Error {
    manifest.state = TakeState::Failed;
    manifest.last_error = Some(bounded_error(&error));
    match lease.write(&manifest) {
        Ok(()) => error,
        Err(save) => error.context(format!("Could not save voiceover failure state: {save}")),
    }
}

fn finalize_audio(
    lease: &TakeLease,
    manifest: &mut TakeManifest,
    file: &str,
    recovered: bool,
) -> Result<ReadyTake> {
    if !valid_media_name(file) {
        bail!("Invalid voiceover media name");
    }
    let path = lease.dir.join(file);
    let mut audio = validate_audio(&path)?;
    cap_enc_ffmpeg::sync_media_file(&path)?;
    audio.file = file.to_owned();
    audio.recovered = recovered;
    let mut ready = manifest.clone();
    ready.audio = Some(audio);
    ready.state = TakeState::Ready;
    ready.last_error = None;
    let published = lease.write(&ready);
    if published.is_ok() || lease.read().is_ok_and(|saved| saved == ready) {
        *manifest = ready;
    }
    published?;
    Ok(ReadyTake {
        project: lease.project.clone(),
        manifest: manifest.clone(),
    })
}

fn validate_audio(path: &Path) -> Result<TakeAudio> {
    safe_file(path)?;
    if fs::metadata(path)?.len() > CAPTURE_LIMIT {
        bail!("Voiceover audio exceeds its size limit");
    }
    ffmpeg::init()?;
    let mut input = ffmpeg::format::input(path)?;
    if input
        .streams()
        .any(|stream| stream.parameters().medium() == ffmpeg::media::Type::Video)
    {
        bail!("Voiceover media contains unexpected video");
    }
    let stream = input
        .streams()
        .best(ffmpeg::media::Type::Audio)
        .ok_or_else(|| anyhow!("Voiceover has no audio stream"))?;
    let index = stream.index();
    let mut decoder = ffmpeg::codec::Context::from_parameters(stream.parameters())?
        .decoder()
        .audio()?;
    let sample_rate = decoder.rate();
    let channels = decoder.channels();
    if !(8_000..=192_000).contains(&sample_rate) || !(1..=8).contains(&channels) {
        bail!("Invalid voiceover audio format");
    }
    let mut samples = 0u64;
    if decoder.channel_layout().is_empty() {
        decoder.set_channel_layout(ffmpeg::ChannelLayout::default(i32::from(channels)));
    }
    let mut resampler = ffmpeg::software::resampling::Context::get(
        decoder.format(),
        decoder.channel_layout(),
        sample_rate,
        ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Planar),
        decoder.channel_layout(),
        sample_rate,
    )?;
    let mut levels = AudioLevels::default();
    let mut drain = |decoder: &mut ffmpeg::decoder::Audio| -> Result<()> {
        let mut frame = ffmpeg::frame::Audio::empty();
        loop {
            match decoder.receive_frame(&mut frame) {
                Ok(()) => {
                    if frame.rate() != sample_rate || frame.channels() != channels {
                        bail!("Voiceover format changed within the take");
                    }
                    samples = samples
                        .checked_add(frame.samples() as u64)
                        .ok_or_else(|| anyhow!("Voiceover sample count overflow"))?;
                    let mut floats = ffmpeg::frame::Audio::empty();
                    resampler.run(&frame, &mut floats)?;
                    for channel in 0..usize::from(floats.channels()) {
                        levels.push(floats.plane::<f32>(channel));
                    }
                }
                Err(ffmpeg::Error::Eof) => return Ok(()),
                Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::ffi::EAGAIN => {
                    return Ok(());
                }
                Err(error) => return Err(error.into()),
            }
        }
    };
    let mut packet = ffmpeg::Packet::empty();
    loop {
        match packet.read(&mut input) {
            Ok(()) if packet.stream() == index => {
                decoder.send_packet(&packet)?;
                drain(&mut decoder)?;
            }
            Ok(()) => {}
            Err(ffmpeg::Error::Eof) => break,
            Err(error) => return Err(error.into()),
        }
    }
    decoder.send_eof()?;
    drain(&mut decoder)?;
    if samples == 0 {
        bail!("Voiceover contains no decodable samples");
    }
    Ok(TakeAudio {
        file: String::new(),
        duration: samples as f64 / f64::from(sample_rate),
        sample_rate,
        channels,
        recovered: false,
        peak_db: Some(levels.db().0),
        rms_db: Some(levels.db().1),
    })
}

struct RecoveryFiles(Vec<PathBuf>);

impl Drop for RecoveryFiles {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

struct RecoveredAudio {
    file: String,
    files: RecoveryFiles,
}

fn finalize_recovered_audio(
    lease: &TakeLease,
    manifest: &mut TakeManifest,
    mut candidate: RecoveredAudio,
) -> Result<ReadyTake> {
    let result = finalize_audio(lease, manifest, &candidate.file, true);
    if manifest
        .audio
        .as_ref()
        .is_some_and(|audio| audio.file == candidate.file)
    {
        candidate.files.0.clear();
    }
    result
}

fn recover_audio_prefix(lease: &TakeLease) -> Result<RecoveredAudio> {
    let source = lease.dir.join(CAPTURE);
    safe_file(&source)?;
    let mut input = File::open(&source)?;
    let metadata = input.metadata()?;
    let size = metadata.len();
    if size > CAPTURE_LIMIT {
        bail!("Voiceover audio exceeds its size limit");
    }
    let mut offset = 0u64;
    let mut has_header = false;
    let mut has_movie = false;
    loop {
        if size.saturating_sub(offset) < 8 || offset > 16 * 1024 * 1024 {
            bail!("No recoverable voiceover initialization");
        }
        input.seek(SeekFrom::Start(offset))?;
        let mut header = [0; 8];
        input.read_exact(&mut header)?;
        if &header[4..] == b"moof" && has_header && has_movie {
            break;
        }
        let mut length = u64::from(u32::from_be_bytes(header[..4].try_into()?));
        let minimum = if length == 1 {
            let mut large = [0; 8];
            input.read_exact(&mut large)?;
            length = u64::from_be_bytes(large);
            16
        } else {
            8
        };
        if length < minimum || length > size.saturating_sub(offset) {
            bail!("Incomplete voiceover initialization");
        }
        has_header |= &header[4..] == b"ftyp";
        has_movie |= &header[4..] == b"moov";
        offset += length;
    }
    let token = Uuid::new_v4();
    let init = lease.dir.join(format!(".recover-{token}-init.mp4"));
    let media = lease.dir.join(format!(".recover-{token}-media.m4s"));
    let temporary = RecoveryFiles(vec![init.clone(), media.clone()]);
    copy_range(&mut input, 0, offset, &init)?;
    copy_range(&mut input, offset, size - offset, &media)?;
    let prefix = cap_enc_ffmpeg::fragmented_mp4::read_fragment_prefix(&init, &media)?;
    if prefix.duration.is_zero() {
        bail!("Voiceover has no recoverable audio duration");
    }
    let file = format!("recovered-{token}.m4a");
    let output_path = lease.dir.join(&file);
    let candidate = RecoveryFiles(vec![output_path.clone()]);
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output_path)?;
    input.seek(SeekFrom::Start(0))?;
    if std::io::copy(&mut input.take(offset + prefix.length), &mut output)?
        != offset + prefix.length
    {
        bail!("Voiceover changed while recovering");
    }
    output.sync_all()?;
    drop(output);
    let after = fs::metadata(source)?;
    if after.len() != size || after.modified()? != metadata.modified()? {
        bail!("Voiceover changed while recovering");
    }
    validate_audio(&output_path)?;
    drop(temporary);
    Ok(RecoveredAudio {
        file,
        files: candidate,
    })
}

fn copy_range(input: &mut File, offset: u64, length: u64, path: &Path) -> Result<()> {
    input.seek(SeekFrom::Start(offset))?;
    let mut output = OpenOptions::new().write(true).create_new(true).open(path)?;
    if std::io::copy(&mut input.take(length), &mut output)? != length {
        bail!("Voiceover changed while reading");
    }
    output.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests;
