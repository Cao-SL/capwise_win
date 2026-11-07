use super::*;
use cap_recording::feeds::microphone::synthetic::SyntheticMicrophone;
use futures_util::FutureExt as _;

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let project: cap_project::ProjectConfiguration = serde_json::from_value(serde_json::json!({
        "timeline": { "zoomSegments": [], "segments": [{ "recordingSegment": 0, "timescale": 1.25, "start": 0.0, "end": 30.0 }] }
    })).unwrap();
    project.write(dir.path()).unwrap();
    dir
}

fn request(project: &Path) -> VoiceoverRequest {
    VoiceoverRequest {
        project_path: project.to_path_buf(),
        id: Uuid::new_v4(),
        output_start: 7.25,
        lane: 2,
    }
}

async fn recording(
    project: &Path,
) -> (
    VoiceoverSession,
    watch::Sender<bool>,
    tokio::task::JoinHandle<()>,
) {
    let microphone = Arc::new(SyntheticMicrophone::spawn(
        "Voiceover synthetic test",
        48_000,
        1,
    ));
    let lock = Arc::new(microphone.lock().await.unwrap());
    let session = VoiceoverSession::start(request(project), lock)
        .await
        .unwrap();
    let (end, mut ended) = watch::channel(false);
    let task = tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(10));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut position = 0u64;
        loop {
            tokio::select! {
                _ = ended.changed() => return,
                _ = tick.tick() => {
                    let samples = (0..480).map(|index| {
                        let phase = (position + index) as f64 * 440.0 * std::f64::consts::TAU / 48_000.0;
                        (phase.sin() * 0.3) as f32
                    }).collect::<Vec<_>>();
                    microphone.push(&samples, Timestamp::Instant(Instant::now())).await;
                    position += 480;
                }
            }
        }
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while !session.status().first_sample_seen {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    (session, end, task)
}

async fn end_source(end: watch::Sender<bool>, task: tokio::task::JoinHandle<()>) {
    let _ = end.send(true);
    task.await.unwrap();
}

async fn wait_finished(project: &Path) -> DiscoveredTake {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let mut takes = list_takes(project).unwrap();
            if takes.len() == 1 && takes[0].availability != TakeAvailability::Active {
                return takes.remove(0);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn voiceover_pause_stop_decodes_at_output_speed_without_changing_project() {
    let project = project();
    let before = fs::read(project.path().join("project-config.json")).unwrap();
    let (session, end, task) = recording(project.path()).await;
    let frozen = session.request().clone();
    tokio::time::sleep(Duration::from_millis(250)).await;
    session.pause().await.unwrap();
    session.pause().await.unwrap();
    tokio::time::sleep(Duration::from_millis(120)).await;
    let paused = session.status().duration_estimate;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(session.status().state, TakeState::Paused);
    assert_eq!(session.status().duration_estimate, paused);
    session.resume().await.unwrap();
    session.resume().await.unwrap();
    tokio::time::sleep(Duration::from_millis(250)).await;
    let ready = session.stop().await.unwrap();
    end_source(end, task).await;
    assert_eq!(ready.manifest().id, frozen.id);
    assert_eq!(ready.manifest().output_start, 7.25);
    assert_eq!(ready.manifest().lane, 2);
    assert!(
        ready.audio().duration > 0.45 && ready.audio().duration < 0.9,
        "{}",
        ready.audio().duration
    );
    let decoded = cap_audio::AudioData::from_file(ready.path()).unwrap();
    let rms = (decoded
        .samples()
        .iter()
        .map(|sample| f64::from(*sample).powi(2))
        .sum::<f64>()
        / decoded.samples().len() as f64)
        .sqrt();
    assert!(rms > 0.1);
    assert_eq!(
        fs::read(project.path().join("project-config.json")).unwrap(),
        before
    );
    assert_eq!(
        list_takes(project.path()).unwrap()[0].availability,
        TakeAvailability::Ready
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn voiceover_active_recovery_is_rejected_and_cancel_only_removes_this_take() {
    let project = project();
    let other = project.path().join("existing-audio.wav");
    fs::write(&other, b"user source stays untouched").unwrap();
    let (session, end, task) = recording(project.path()).await;
    assert_eq!(
        list_takes(project.path()).unwrap()[0].availability,
        TakeAvailability::Active
    );
    let error = recover_take(project.path().to_owned(), session.request().id)
        .await
        .unwrap_err();
    assert!(error.is::<ActiveTake>());
    session.cancel().await.unwrap();
    end_source(end, task).await;
    assert!(list_takes(project.path()).unwrap().is_empty());
    assert_eq!(fs::read(other).unwrap(), b"user source stays untouched");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn voiceover_dropped_stop_future_still_finalizes_and_attachment_requires_saved_project() {
    let project = project();
    let (session, end, task) = recording(project.path()).await;
    let id = session.request().id;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let _ = session.stop().now_or_never();
    let discovered = wait_finished(project.path()).await;
    end_source(end, task).await;
    assert_eq!(discovered.availability, TakeAvailability::Ready);
    let original_config = fs::read(project.path().join("project-config.json")).unwrap();
    let mut ready = recover_take(project.path().to_owned(), id).await.unwrap();
    assert!(ready.mark_attached().is_err());
    assert_eq!(
        fs::read(project.path().join("project-config.json")).unwrap(),
        original_config
    );
    let mut saved = read_saved_project(project.path()).unwrap();
    saved.timeline.as_mut().unwrap().audio_segments.push(
        serde_json::from_value(serde_json::json!({
            "start": ready.manifest().output_start,
            "end": ready.manifest().output_start + ready.audio().duration,
            "track": ready.manifest().lane,
            "path": ready.relative_path()
        }))
        .unwrap(),
    );
    assert!(ready.is_in_project(&saved));
    assert!(ready.mark_attached().is_err());
    saved.write(project.path()).unwrap();
    let mut legacy: serde_json::Value =
        serde_json::from_slice(&fs::read(project.path().join("project-config.json")).unwrap())
            .unwrap();
    legacy["textSizeVersion"] = serde_json::json!(0);
    legacy["futureVoiceoverSetting"] = serde_json::json!({"preserve": true});
    let bytes = format!("\n{}\n", serde_json::to_string(&legacy).unwrap()).into_bytes();
    fs::write(project.path().join("project-config.json"), &bytes).unwrap();
    ready.mark_attached().unwrap();
    ready.mark_attached().unwrap();
    assert_eq!(
        fs::read(project.path().join("project-config.json")).unwrap(),
        bytes
    );
    assert_eq!(
        list_takes(project.path()).unwrap()[0].availability,
        TakeAvailability::Attached
    );
    assert_eq!(
        recover_take(project.path().to_owned(), id)
            .await
            .unwrap()
            .manifest()
            .state,
        TakeState::Attached
    );
}

fn last_mdat(path: &Path) -> (u64, u64) {
    let bytes = fs::read(path).unwrap();
    let mut position = 0usize;
    let mut last = None;
    let mut fragments = 0;
    while position + 8 <= bytes.len() {
        let size = u32::from_be_bytes(bytes[position..position + 4].try_into().unwrap()) as usize;
        assert!(size >= 8 && position + size <= bytes.len());
        if &bytes[position + 4..position + 8] == b"mdat" {
            last = Some(((position + 8) as u64, (size - 8) as u64));
            fragments += 1;
        }
        position += size;
    }
    assert!(fragments >= 2);
    last.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn voiceover_torn_tail_recovers_complete_audio_without_overwriting_capture() {
    let project = project();
    let (session, end, task) = recording(project.path()).await;
    tokio::time::sleep(Duration::from_millis(2400)).await;
    let ready = session.stop().await.unwrap();
    end_source(end, task).await;
    let lease = TakeLease::open(project.path(), ready.manifest().id).unwrap();
    let mut manifest = lease.read().unwrap();
    manifest.state = TakeState::Recording;
    manifest.audio = None;
    lease.write(&manifest).unwrap();
    let capture = lease.dir.join(CAPTURE);
    let (start, length) = last_mdat(&capture);
    OpenOptions::new()
        .write(true)
        .open(&capture)
        .unwrap()
        .set_len(start + length / 2)
        .unwrap();
    let original = fs::read(&capture).unwrap();
    let candidate = recover_audio_prefix(&lease).unwrap();
    let candidate_path = lease.dir.join(&candidate.file);
    let saved_manifest = lease.dir.join("saved-take.json");
    fs::rename(lease.dir.join(MANIFEST), &saved_manifest).unwrap();
    fs::create_dir(lease.dir.join(MANIFEST)).unwrap();
    assert!(finalize_recovered_audio(&lease, &mut manifest, candidate).is_err());
    assert!(!candidate_path.exists());
    assert!(manifest.audio.is_none());
    assert_eq!(fs::read(&capture).unwrap(), original);
    fs::remove_dir(lease.dir.join(MANIFEST)).unwrap();
    fs::rename(saved_manifest, lease.dir.join(MANIFEST)).unwrap();
    drop(lease);
    assert_eq!(
        list_takes(project.path()).unwrap()[0].availability,
        TakeAvailability::Interrupted
    );
    let recovered = recover_take(project.path().to_owned(), ready.manifest().id)
        .await
        .unwrap();
    assert!(recovered.audio().recovered);
    assert!(recovered.audio().duration > 1.9);
    assert!(recovered.audio().duration < ready.audio().duration);
    assert_ne!(recovered.path(), capture);
    assert_eq!(fs::read(capture).unwrap(), original);
    let decoded = cap_audio::AudioData::from_file(recovered.path()).unwrap();
    assert!(!decoded.samples().is_empty());
    let retry = recover_take(project.path().to_owned(), ready.manifest().id)
        .await
        .unwrap();
    assert_eq!(retry.relative_path(), recovered.relative_path());
    assert_eq!(
        fs::read_dir(recovered.path().parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with("recovered-"))
            .count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn voiceover_owned_scope_joins_cancel_before_first_sample() {
    let project = project();
    let microphone = SyntheticMicrophone::spawn("Voiceover startup failure", 48_000, 1);
    let lock = Arc::new(microphone.lock().await.unwrap());
    let session = VoiceoverSession::start(request(project.path()), lock)
        .await
        .unwrap();
    assert!(!session.status().first_sample_seen);
    tokio::time::timeout(Duration::from_secs(3), session.cancel())
        .await
        .unwrap()
        .unwrap();
    assert!(list_takes(project.path()).unwrap().is_empty());
    assert!(project.path().join("project-config.json").is_file());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn voiceover_stop_before_first_sample_does_not_publish_encoder_padding_as_a_take() {
    let project = project();
    let microphone = SyntheticMicrophone::spawn("Voiceover empty stop", 48_000, 1);
    let lock = Arc::new(microphone.lock().await.unwrap());
    let session = VoiceoverSession::start(request(project.path()), lock)
        .await
        .unwrap();
    let error = session.stop().await.unwrap_err();
    assert!(error.to_string().contains("No microphone samples"));
    let discovered = list_takes(project.path()).unwrap();
    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0].availability, TakeAvailability::Empty);
    assert!(discovered[0].manifest.as_ref().unwrap().audio.is_none());
    assert!(
        recover_take(
            project.path().to_owned(),
            discovered[0].manifest.as_ref().unwrap().id
        )
        .await
        .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn voiceover_failed_recovery_preserves_source_and_missing_media_is_visible() {
    let project = project();
    let request = request(project.path());
    let lease = TakeLease::create(request.clone()).unwrap();
    let capture = lease.dir.join(CAPTURE);
    fs::write(&capture, b"truncated initialization").unwrap();
    drop(lease);
    assert!(
        recover_take(project.path().to_owned(), request.id)
            .await
            .is_err()
    );
    assert_eq!(fs::read(&capture).unwrap(), b"truncated initialization");
    let entry = list_takes(project.path()).unwrap().remove(0);
    assert_eq!(entry.availability, TakeAvailability::Interrupted);
    assert_eq!(entry.manifest.unwrap().state, TakeState::Failed);
    fs::remove_file(capture).unwrap();
    assert_eq!(
        list_takes(project.path()).unwrap()[0].availability,
        TakeAvailability::Missing
    );
}

#[test]
fn voiceover_manifest_path_escape_and_duplicate_identity_are_rejected() {
    let project = project();
    let request = request(project.path());
    let lease = TakeLease::create(request.clone()).unwrap();
    assert!(TakeLease::create(request.clone()).is_err());
    let mut manifest = lease.read().unwrap();
    manifest.audio = Some(TakeAudio {
        file: "../../outside.m4a".into(),
        duration: 1.0,
        sample_rate: 48_000,
        channels: 1,
        recovered: false,
    });
    manifest.state = TakeState::Ready;
    assert!(lease.write(&manifest).is_err());
    fs::write(
        lease.dir.join(MANIFEST),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    drop(lease);
    assert_eq!(
        list_takes(project.path()).unwrap()[0].availability,
        TakeAvailability::Invalid
    );
    assert!(
        TakeLease::open(project.path(), request.id)
            .unwrap()
            .read()
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn voiceover_symlinked_storage_is_rejected_without_touching_external_files() {
    let project = project();
    let outside = tempfile::tempdir().unwrap();
    fs::create_dir_all(project.path().join("content")).unwrap();
    std::os::unix::fs::symlink(outside.path(), project.path().join("content/voiceovers")).unwrap();
    assert!(TakeLease::create(request(project.path())).is_err());
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
}

#[test]
fn voiceover_clock_excludes_idempotent_pause_intervals() {
    let first = Instant::now();
    let mut clock = RecordingClock {
        first: Some(first),
        ..Default::default()
    };
    clock.set_paused(true, first + Duration::from_secs(1));
    clock.set_paused(true, first + Duration::from_secs(2));
    assert_eq!(
        clock.duration(first + Duration::from_secs(3)),
        Duration::from_secs(1)
    );
    clock.set_paused(false, first + Duration::from_secs(4));
    clock.set_paused(false, first + Duration::from_secs(5));
    assert_eq!(
        clock.duration(first + Duration::from_secs(6)),
        Duration::from_secs(3)
    );
}

#[test]
fn voiceover_first_audio_buffer_before_build_epoch_still_starts_duration_clock() {
    let epoch = Timestamps::now();
    let mut clock = RecordingClock::default();
    clock.observe_first(
        Some(Timestamp::Instant(
            epoch
                .instant()
                .checked_sub(Duration::from_millis(10))
                .unwrap(),
        )),
        epoch,
    );
    assert!(clock.first.is_some());
    assert_eq!(
        clock.duration(epoch.instant() + Duration::from_secs(1)),
        Duration::from_secs(1)
    );
    clock.observe_first(
        Some(Timestamp::Instant(
            epoch.instant() + Duration::from_secs(10),
        )),
        epoch,
    );
    assert_eq!(
        clock.duration(epoch.instant() + Duration::from_secs(1)),
        Duration::from_secs(1)
    );
}

fn finalizing_session(
    ready: &ReadyTake,
) -> (
    VoiceoverSession,
    mpsc::UnboundedReceiver<Command>,
    watch::Sender<VoiceoverStatus>,
) {
    let (commands, receiver) = mpsc::unbounded_channel();
    let (status_tx, status) = watch::channel(VoiceoverStatus {
        state: TakeState::Finalizing,
        duration_estimate: Duration::from_secs_f64(ready.audio().duration),
        first_sample_seen: true,
        health: Vec::new(),
        error: None,
    });
    (
        VoiceoverSession {
            request: VoiceoverRequest {
                project_path: ready.project_path().to_owned(),
                id: ready.manifest().id,
                output_start: ready.manifest().output_start,
                lane: ready.manifest().lane,
            },
            commands,
            status,
            closed: false,
        },
        receiver,
        status_tx,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn voiceover_terminal_request_racing_source_finalization_waits_and_preserves_attached_audio()
{
    let project = project();
    let before = fs::read(project.path().join("project-config.json")).unwrap();
    let outside = project.path().join("existing-audio.wav");
    fs::write(&outside, b"existing source").unwrap();
    let (session, end, task) = recording(project.path()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let ready = session.stop().await.unwrap();
    end_source(end, task).await;
    let (session, mut commands, status) = finalizing_session(&ready);
    let mut stop = tokio::spawn(session.stop());
    let Command::Stop(reply) = commands.recv().await.unwrap() else {
        panic!("expected stop");
    };
    drop(reply);
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut stop)
            .await
            .is_err()
    );
    assert!(ready.path().is_file());
    status.send_modify(|status| status.state = TakeState::Ready);
    let repeated = stop.await.unwrap().unwrap();
    assert_eq!(repeated.relative_path(), ready.relative_path());

    let mut saved = read_saved_project(project.path()).unwrap();
    saved.timeline.as_mut().unwrap().audio_segments.push(
        crate::editor_edits::default_audio_segment(
            7.25,
            7.25 + ready.audio().duration,
            2,
            ready.relative_path(),
            "Voiceover".into(),
            Some(ready.audio().duration),
        ),
    );
    saved.write(project.path()).unwrap();
    let attached_config = fs::read(project.path().join("project-config.json")).unwrap();
    let (session, mut commands, status) = finalizing_session(&ready);
    let cancel = tokio::spawn(session.cancel());
    let Command::Cancel(reply) = commands.recv().await.unwrap() else {
        panic!("expected cancel");
    };
    drop(reply);
    status.send_modify(|status| status.state = TakeState::Ready);
    assert!(
        cancel
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("already references")
    );
    assert!(ready.path().is_file());
    assert_eq!(
        fs::read(project.path().join("project-config.json")).unwrap(),
        attached_config
    );
    fs::write(project.path().join("project-config.json"), &before).unwrap();

    let (session, mut commands, status) = finalizing_session(&ready);
    let mut cancel = tokio::spawn(session.cancel());
    let Command::Cancel(reply) = commands.recv().await.unwrap() else {
        panic!("expected cancel");
    };
    drop(reply);
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut cancel)
            .await
            .is_err()
    );
    assert!(ready.path().is_file());
    status.send_modify(|status| status.state = TakeState::Ready);
    cancel.await.unwrap().unwrap();
    assert!(!ready.path().exists());
    assert!(list_takes(project.path()).unwrap().is_empty());
    assert_eq!(
        fs::read(project.path().join("project-config.json")).unwrap(),
        before
    );
    assert_eq!(fs::read(outside).unwrap(), b"existing source");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn voiceover_records_beyond_video_end_until_explicit_stop() {
    let project = project();
    let mut config = cap_project::ProjectConfiguration::load(project.path()).unwrap();
    config.timeline.as_mut().unwrap().segments[0].end = 0.25;
    config.write(project.path()).unwrap();
    let (session, end, task) = recording(project.path()).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(session.status().state, TakeState::Recording);
    let take = session.stop().await.unwrap();
    end_source(end, task).await;
    assert!(take.audio().duration > 0.4);
    assert_eq!(take.manifest().output_start, 7.25);
    let (_temporary, audition) = crate::editor_audio::prepare_audition(&take.path()).unwrap();
    let file = std::io::BufReader::new(fs::File::open(audition).unwrap());
    let decoded = rodio::Decoder::new(file).unwrap();
    let (samples, peak) = decoded.fold((0, 0_i32), |(count, peak), sample| {
        (count + 1, peak.max(i32::from(sample).abs()))
    });
    assert!(samples > 19_200);
    assert!(peak > 1_000);
}

#[test]
fn voiceover_manifest_accepts_long_recordings_without_a_product_duration_cap() {
    let project = project();
    let request = request(project.path());
    let lease = TakeLease::create(request).unwrap();
    let mut manifest = lease.read().unwrap();
    manifest.audio = Some(TakeAudio {
        file: "capture.m4a".into(),
        duration: 8. * 60. * 60.,
        sample_rate: 48_000,
        channels: 1,
        recovered: false,
    });
    manifest.state = TakeState::Ready;
    lease.write(&manifest).unwrap();
    assert_eq!(
        lease.read().unwrap().audio.unwrap().duration,
        8. * 60. * 60.
    );
}
