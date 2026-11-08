use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use cap_media_info::{AudioInfo, ffmpeg_sample_format_for};
use cap_recording::feeds::microphone::{self, MicrophoneFeed, MicrophoneSamples};
use cap_timestamp::{Timestamp, Timestamps};
use gpui::{App, AppContext as _, Context, Entity, Subscription};
use kameo::actor::ActorRef;
use tokio::sync::watch;

use crate::{
    feeds::Feeds,
    session::{Phase, RecordingSession},
};

const SAMPLE_RATE: u32 = 16_000;
const PACKET_SAMPLES: usize = 3_200;
const MAX_AUDIO_AGE: Duration = Duration::from_secs(1);

#[derive(Clone, Debug)]
pub(crate) struct MicTapEpoch(Arc<EpochInner>);

#[derive(Debug)]
struct EpochInner {
    generation: u64,
    active: AtomicBool,
    muted: Option<Arc<AtomicBool>>,
    cancelled: watch::Sender<bool>,
    opened: Timestamps,
}

impl MicTapEpoch {
    fn new(generation: u64, muted: Option<Arc<AtomicBool>>) -> Self {
        Self(Arc::new(EpochInner {
            generation,
            active: AtomicBool::new(true),
            muted,
            cancelled: watch::channel(false).0,
            opened: Timestamps::now(),
        }))
    }

    pub(crate) fn generation(&self) -> u64 {
        self.0.generation
    }

    pub(crate) fn is_current(&self) -> bool {
        if self
            .0
            .muted
            .as_ref()
            .is_some_and(|muted| muted.load(Ordering::Acquire))
        {
            self.invalidate();
        }
        self.0.active.load(Ordering::Acquire)
    }

    pub(crate) async fn cancelled(&self) {
        let mut receiver = self.0.cancelled.subscribe();
        if !self.is_current() {
            return;
        }
        while receiver.changed().await.is_ok() {
            if *receiver.borrow_and_update() {
                return;
            }
        }
    }

    fn invalidate(&self) {
        self.0.active.store(false, Ordering::Release);
        self.0.cancelled.send_replace(true);
    }
}

pub(crate) struct MicPacket {
    pub samples: Vec<i16>,
    pub captured_at: Instant,
    pub epoch: MicTapEpoch,
}

impl MicPacket {
    pub(crate) fn is_current(&self) -> bool {
        self.epoch.is_current()
            && self.captured_at <= Instant::now()
            && self.captured_at.elapsed() <= MAX_AUDIO_AGE
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MicTapStateKind {
    Disabled,
    NoMicrophone,
    RecordingPaused,
    RecordingMuted,
    RecordingTransition,
    Ready,
    Failed(String),
}

#[derive(Clone, Debug)]
pub(crate) struct MicTapStatus {
    pub generation: u64,
    pub state: MicTapStateKind,
    pub epoch: Option<MicTapEpoch>,
}

pub(crate) struct MicTapLease {
    state: Entity<MicTapState>,
    status: watch::Receiver<MicTapStatus>,
    packets: flume::Receiver<MicPacket>,
}

impl MicTapLease {
    pub(crate) fn new(cx: &mut App) -> Self {
        let feeds = Feeds::global(cx);
        let session = RecordingSession::global(cx);
        feeds.update(cx, |feeds, cx| feeds.acquire_mic_tap(cx));
        let (packet_tx, packets) = flume::bounded(4);
        let (status_tx, status) = watch::channel(MicTapStatus {
            generation: 0,
            state: MicTapStateKind::Disabled,
            epoch: None,
        });
        let drain = packets.clone();
        let state = cx.new(|cx| {
            let subscriptions = vec![
                cx.observe(&feeds, |this: &mut MicTapState, _, cx| this.reconcile(cx)),
                cx.observe(&session, |this: &mut MicTapState, _, cx| this.reconcile(cx)),
                cx.on_release(|this: &mut MicTapState, cx| {
                    this.close_epoch();
                    this.feeds.update(cx, |feeds, cx| feeds.release_mic_tap(cx));
                }),
            ];
            MicTapState {
                feeds,
                session,
                enabled: false,
                route_key: None,
                status: status_tx,
                packets: packet_tx,
                drain,
                worker: None,
                _subscriptions: subscriptions,
            }
        });
        Self {
            state,
            status,
            packets,
        }
    }

    pub(crate) fn set_enabled(&self, enabled: bool, cx: &mut App) {
        self.state.update(cx, |state, cx| {
            let retry =
                enabled && matches!(state.status.borrow().state, MicTapStateKind::Failed(_));
            if state.enabled != enabled || retry {
                if retry {
                    state.route_key = None;
                }
                state.enabled = enabled;
                state.reconcile(cx);
            }
        });
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<MicTapStatus> {
        self.status.clone()
    }

    pub(crate) fn packets(&self) -> flume::Receiver<MicPacket> {
        self.packets.clone()
    }
}

impl Drop for MicTapLease {
    fn drop(&mut self) {
        if let Some(epoch) = &self.status.borrow().epoch {
            epoch.invalidate();
        }
        while self.packets.try_recv().is_ok() {}
    }
}

struct MicTapState {
    feeds: Entity<Feeds>,
    session: Entity<RecordingSession>,
    enabled: bool,
    route_key: Option<(String, u64, bool)>,
    status: watch::Sender<MicTapStatus>,
    packets: flume::Sender<MicPacket>,
    drain: flume::Receiver<MicPacket>,
    worker: Option<gpui::Task<()>>,
    _subscriptions: Vec<Subscription>,
}

struct Route {
    actor: ActorRef<MicrophoneFeed>,
    key: (String, u64, bool),
    muted: Option<Arc<AtomicBool>>,
}

fn recording_gate(phase: Phase, pause_unavailable: bool) -> Option<MicTapStateKind> {
    match phase {
        Phase::Starting | Phase::Stopping => Some(MicTapStateKind::RecordingTransition),
        Phase::Recording { paused } if paused || pause_unavailable => {
            Some(MicTapStateKind::RecordingPaused)
        }
        _ => None,
    }
}

impl MicTapState {
    fn route(&self, cx: &App) -> Result<Route, MicTapStateKind> {
        if !self.enabled {
            return Err(MicTapStateKind::Disabled);
        }
        let session = self.session.read(cx);
        if let Some(blocked) = recording_gate(session.phase, session.pause_unavailable()) {
            return Err(blocked);
        }
        match session.phase {
            Phase::Starting | Phase::Stopping => Err(MicTapStateKind::RecordingTransition),
            Phase::Recording { .. } => {
                let (actor, muted, generation) = session
                    .mic_tap_route()
                    .ok_or(MicTapStateKind::NoMicrophone)?;
                if muted.load(Ordering::Acquire) {
                    return Err(MicTapStateKind::RecordingMuted);
                }
                Ok(Route {
                    key: (actor.id().to_string(), generation, true),
                    actor,
                    muted: Some(muted),
                })
            }
            Phase::Idle => {
                let (actor, generation) = self
                    .feeds
                    .read(cx)
                    .mic_tap_route()
                    .ok_or(MicTapStateKind::NoMicrophone)?;
                Ok(Route {
                    key: (actor.id().to_string(), generation, false),
                    actor,
                    muted: None,
                })
            }
        }
    }

    fn close_epoch(&mut self) {
        if let Some(epoch) = &self.status.borrow().epoch {
            epoch.invalidate();
        }
        self.worker = None;
        while self.drain.try_recv().is_ok() {}
    }

    fn reconcile(&mut self, cx: &mut Context<Self>) {
        let route = self.route(cx);
        let (key, state) = match &route {
            Ok(route) => (Some(route.key.clone()), MicTapStateKind::Ready),
            Err(state) => (None, state.clone()),
        };
        if self.route_key == key
            && (self.status.borrow().state == state
                || matches!(self.status.borrow().state, MicTapStateKind::Failed(_)))
        {
            return;
        }
        self.close_epoch();
        self.route_key = key;
        let generation = self.status.borrow().generation.wrapping_add(1);
        let epoch = route
            .as_ref()
            .ok()
            .map(|route| MicTapEpoch::new(generation, route.muted.clone()));
        self.status.send_replace(MicTapStatus {
            generation,
            state,
            epoch: epoch.clone(),
        });
        let (Ok(route), Some(epoch)) = (route, epoch) else {
            return;
        };
        let packets = self.packets.clone();
        let task =
            gpui_tokio::Tokio::spawn(
                cx,
                async move { run_tap(route.actor, epoch, packets).await },
            );
        self.worker = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, _| {
                if this.status.borrow().generation != generation {
                    return;
                }
                let error = match result {
                    Ok(Ok(())) => return,
                    Ok(Err(error)) => error,
                    Err(error) => error.to_string(),
                };
                if let Some(epoch) = &this.status.borrow().epoch {
                    epoch.invalidate();
                }
                while this.drain.try_recv().is_ok() {}
                this.status.send_replace(MicTapStatus {
                    generation,
                    state: MicTapStateKind::Failed(error),
                    epoch: None,
                });
            })
            .ok();
        }));
    }
}

struct FeedSubscription {
    actor: ActorRef<MicrophoneFeed>,
    sender: flume::Sender<MicrophoneSamples>,
    runtime: tokio::runtime::Handle,
}

struct EpochExitGuard(MicTapEpoch);

impl Drop for EpochExitGuard {
    fn drop(&mut self) {
        self.0.invalidate();
    }
}

impl Drop for FeedSubscription {
    fn drop(&mut self) {
        let actor = self.actor.clone();
        let sender = self.sender.clone();
        drop(self.runtime.spawn(async move {
            let _ = actor.ask(microphone::RemoveSender(sender)).await;
        }));
    }
}

async fn run_tap(
    actor: ActorRef<MicrophoneFeed>,
    epoch: MicTapEpoch,
    packets: flume::Sender<MicPacket>,
) -> Result<(), String> {
    let _epoch_guard = EpochExitGuard(epoch.clone());
    let (sender, receiver) = flume::bounded(8);
    let _subscription = FeedSubscription {
        actor: actor.clone(),
        sender: sender.clone(),
        runtime: tokio::runtime::Handle::current(),
    };
    tokio::select! {
        biased;
        _ = epoch.cancelled() => return Ok(()),
        result = actor.ask(microphone::AddSender(sender)) => result.map_err(|error| error.to_string())?,
    }
    let mut converter = Packetizer::new(epoch.clone());
    let mut stream_id = None;
    loop {
        let samples = tokio::select! {
            biased;
            _ = epoch.cancelled() => return Ok(()),
            samples = tokio::time::timeout(Duration::from_secs(3), receiver.recv_async()) => samples
                .map_err(|_| "Microphone stopped delivering audio".to_string())?
                .map_err(|_| "Microphone subscription ended".to_string())?,
        };
        if !epoch.is_current() {
            return Ok(());
        }
        if samples
            .timestamp
            .checked_duration_since(epoch.0.opened)
            .is_none()
        {
            continue;
        }
        if stream_id.is_some_and(|previous| previous != samples.stream_id()) {
            return Err("Microphone source changed; restart speech follow".to_string());
        }
        stream_id = Some(samples.stream_id());
        for packet in converter.push(
            &samples.data,
            samples.format,
            samples.sample_rate,
            samples.channels,
            samples.timestamp,
        )? {
            if !epoch.is_current() {
                return Ok(());
            }
            packets.try_send(packet).map_err(|error| match error {
                flume::TrySendError::Full(_) => {
                    "Speech follow cannot keep up with microphone audio".to_string()
                }
                flume::TrySendError::Disconnected(_) => {
                    "Speech follow audio consumer closed".to_string()
                }
            })?;
        }
    }
}

struct Packetizer {
    epoch: MicTapEpoch,
    context: Option<ffmpeg::software::resampling::Context>,
    format: Option<(cpal::SampleFormat, u32, u16)>,
    origin: Option<Instant>,
    next_capture: Option<Instant>,
    emitted: usize,
    pending: VecDeque<i16>,
}

impl Packetizer {
    fn new(epoch: MicTapEpoch) -> Self {
        Self {
            epoch,
            context: None,
            format: None,
            origin: None,
            next_capture: None,
            emitted: 0,
            pending: VecDeque::new(),
        }
    }

    fn push(
        &mut self,
        data: &[u8],
        format: cpal::SampleFormat,
        rate: u32,
        channels: u16,
        timestamp: Timestamp,
    ) -> Result<Vec<MicPacket>, String> {
        self.push_at(data, format, rate, channels, timestamp, Instant::now())
    }

    fn push_at(
        &mut self,
        data: &[u8],
        format: cpal::SampleFormat,
        rate: u32,
        channels: u16,
        timestamp: Timestamp,
        now: Instant,
    ) -> Result<Vec<MicPacket>, String> {
        if !self.epoch.is_current() {
            self.pending.clear();
            return Ok(Vec::new());
        }
        let Some(offset) = timestamp.checked_duration_since(self.epoch.0.opened) else {
            return Ok(Vec::new());
        };
        let captured_at = self
            .epoch
            .0
            .opened
            .instant()
            .checked_add(offset)
            .ok_or_else(|| "Invalid microphone capture timestamp".to_string())?;
        if captured_at > now || now.saturating_duration_since(captured_at) > MAX_AUDIO_AGE {
            return Err("Microphone audio is not current".to_string());
        }
        if !(8_000..=384_000).contains(&rate) || !(1..=32).contains(&channels) {
            return Err("Unsupported microphone audio format".to_string());
        }
        let stride = format.sample_size() * usize::from(channels);
        if data.is_empty()
            || data.len() > 4 * 1024 * 1024
            || !data.len().is_multiple_of(stride)
            || data.len() / stride > rate as usize
        {
            return Err("Invalid microphone audio buffer".to_string());
        }
        if self
            .format
            .is_some_and(|previous| previous != (format, rate, channels))
        {
            return Err("Microphone audio format changed; restart speech follow".to_string());
        }
        if let Some(expected) = self.next_capture
            && (captured_at.saturating_duration_since(expected) > Duration::from_millis(50)
                || expected.saturating_duration_since(captured_at) > Duration::from_millis(50))
        {
            return Err("Microphone audio was interrupted; restart speech follow".to_string());
        }
        self.next_capture = captured_at.checked_add(Duration::from_secs_f64(
            (data.len() / stride) as f64 / f64::from(rate),
        ));
        let input_format = ffmpeg_sample_format_for(format)
            .ok_or_else(|| "Unsupported microphone sample format".to_string())?;
        let source = AudioInfo::new_raw(input_format, rate, channels);
        if self.context.is_none() {
            self.context = Some(
                ffmpeg::software::resampler(
                    (
                        input_format,
                        source.wrapped_frame_layout(AudioInfo::MAX_AUDIO_CHANNELS as usize),
                        rate,
                    ),
                    (
                        ffmpeg::format::Sample::I16(ffmpeg::format::sample::Type::Packed),
                        ffmpeg::ChannelLayout::MONO,
                        SAMPLE_RATE,
                    ),
                )
                .map_err(|error| error.to_string())?,
            );
            self.format = Some((format, rate, channels));
            self.origin = Some(captured_at);
        }
        let context = self
            .context
            .as_mut()
            .ok_or_else(|| "Microphone converter unavailable".to_string())?;
        let input =
            source.wrap_frame_with_max_channels(data, AudioInfo::MAX_AUDIO_CHANNELS as usize);
        let delay = context
            .delay()
            .map_or(0, |delay| delay.output.max(0) as usize);
        let capacity =
            delay + (input.samples() * SAMPLE_RATE as usize).div_ceil(rate as usize) + 32;
        let mut output = ffmpeg::frame::Audio::new(
            ffmpeg::format::Sample::I16(ffmpeg::format::sample::Type::Packed),
            capacity,
            ffmpeg::ChannelLayout::MONO,
        );
        context
            .run(&input, &mut output)
            .map_err(|error| error.to_string())?;
        if output.samples() > 0 {
            self.pending.extend(
                output.data(0)[..output.samples() * 2]
                    .chunks_exact(2)
                    .map(|bytes| i16::from_ne_bytes([bytes[0], bytes[1]])),
            );
        }
        let mut packets = Vec::new();
        let origin_instant = self
            .origin
            .ok_or_else(|| "Microphone timestamp unavailable".to_string())?;
        while self.pending.len() >= PACKET_SAMPLES {
            let offset = Duration::from_secs_f64(self.emitted as f64 / f64::from(SAMPLE_RATE));
            packets.push(MicPacket {
                samples: self.pending.drain(..PACKET_SAMPLES).collect(),
                captured_at: origin_instant + offset,
                epoch: self.epoch.clone(),
            });
            self.emitted += PACKET_SAMPLES;
        }
        Ok(packets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cap_recording::feeds::microphone::synthetic::SyntheticMicrophone;

    fn pcm(samples: impl Iterator<Item = i16>) -> Vec<u8> {
        samples.flat_map(i16::to_ne_bytes).collect()
    }

    #[test]
    fn mic_tap_recording_transition_and_unconfirmed_pause_close_the_gate() {
        for phase in [Phase::Starting, Phase::Stopping] {
            assert_eq!(
                recording_gate(phase, false),
                Some(MicTapStateKind::RecordingTransition)
            );
        }
        for (phase, pending_or_uncertain) in [
            (Phase::Recording { paused: true }, false),
            (Phase::Recording { paused: false }, true),
        ] {
            assert_eq!(
                recording_gate(phase, pending_or_uncertain),
                Some(MicTapStateKind::RecordingPaused)
            );
        }
        assert_eq!(
            recording_gate(Phase::Recording { paused: false }, false),
            None
        );
        assert_eq!(recording_gate(Phase::Idle, false), None);
    }

    #[test]
    fn mic_tap_packets_preserve_capture_time_and_pcm_samples() {
        let epoch = MicTapEpoch::new(7, None);
        let start = epoch.0.opened.instant() + Duration::from_millis(10);
        let input = pcm((0..6_400).map(|sample| (sample % 256) as i16));
        let mut packetizer = Packetizer::new(epoch);
        let packets = packetizer
            .push_at(
                &input,
                cpal::SampleFormat::I16,
                16_000,
                1,
                Timestamp::Instant(start),
                start + Duration::from_millis(450),
            )
            .unwrap();
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0].samples.len(), 3_200);
        assert_eq!(packets[0].captured_at, start);
        assert_eq!(packets[1].captured_at, start + Duration::from_millis(200));
        assert_eq!(
            packets[0].samples,
            (0..3_200)
                .map(|sample| (sample % 256) as i16)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn mic_tap_resamples_48khz_stereo_without_mixing_channels_or_resetting_time() {
        let epoch = MicTapEpoch::new(1, None);
        let start = epoch.0.opened.instant() + Duration::from_millis(1);
        let data: Vec<_> = (0..10_560)
            .flat_map(|_| [0.3_f32, -0.3_f32])
            .flat_map(f32::to_ne_bytes)
            .collect();
        let mut packetizer = Packetizer::new(epoch);
        let mut packets = packetizer
            .push_at(
                &data,
                cpal::SampleFormat::F32,
                48_000,
                2,
                Timestamp::Instant(start),
                start + Duration::from_millis(500),
            )
            .unwrap();
        packets.extend(
            packetizer
                .push_at(
                    &data,
                    cpal::SampleFormat::F32,
                    48_000,
                    2,
                    Timestamp::Instant(start + Duration::from_millis(220)),
                    start + Duration::from_millis(500),
                )
                .unwrap(),
        );
        assert_eq!(packets.len(), 2);
        assert!(
            packets
                .iter()
                .flat_map(|packet| &packet.samples)
                .all(|sample| sample.abs() <= 1)
        );
        assert_eq!(packets[1].captured_at, start + Duration::from_millis(200));
    }

    #[tokio::test]
    async fn mic_tap_mute_invalidates_queued_packets_permanently_and_wakes_cancel() {
        let muted = Arc::new(AtomicBool::new(false));
        let epoch = MicTapEpoch::new(1, Some(muted.clone()));
        let start = epoch.0.opened.instant();
        let mut packetizer = Packetizer::new(epoch.clone());
        let data = pcm(std::iter::repeat_n(1, 3_200));
        let packets = packetizer
            .push_at(
                &data,
                cpal::SampleFormat::I16,
                16_000,
                1,
                Timestamp::Instant(start),
                start + Duration::from_millis(200),
            )
            .unwrap();
        muted.store(true, Ordering::Release);
        assert!(!packets[0].epoch.is_current());
        muted.store(false, Ordering::Release);
        assert!(!packets[0].epoch.is_current());
        tokio::time::timeout(Duration::from_millis(100), epoch.cancelled())
            .await
            .unwrap();
        assert!(
            packetizer
                .push_at(
                    &data,
                    cpal::SampleFormat::I16,
                    16_000,
                    1,
                    Timestamp::Instant(start),
                    start + Duration::from_millis(400)
                )
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn mic_tap_resume_rejects_pre_epoch_audio_and_partial_old_packet() {
        let first = MicTapEpoch::new(1, None);
        let start = first.0.opened.instant();
        let data = pcm(std::iter::repeat_n(900, 1_600));
        let mut packetizer = Packetizer::new(first.clone());
        assert!(
            packetizer
                .push_at(
                    &data,
                    cpal::SampleFormat::I16,
                    16_000,
                    1,
                    Timestamp::Instant(start),
                    start + Duration::from_millis(100)
                )
                .unwrap()
                .is_empty()
        );
        first.invalidate();
        let second = MicTapEpoch::new(2, None);
        let next = second.0.opened.instant() + Duration::from_millis(1);
        let mut resumed = Packetizer::new(second);
        assert!(
            resumed
                .push_at(
                    &data,
                    cpal::SampleFormat::I16,
                    16_000,
                    1,
                    Timestamp::Instant(start),
                    next + Duration::from_millis(200)
                )
                .unwrap()
                .is_empty()
        );
        let new_data = pcm(std::iter::repeat_n(-700, 3_200));
        let packets = resumed
            .push_at(
                &new_data,
                cpal::SampleFormat::I16,
                16_000,
                1,
                Timestamp::Instant(next),
                next + Duration::from_millis(200),
            )
            .unwrap();
        assert_eq!(packets.len(), 1);
        assert!(packets[0].samples.iter().all(|sample| *sample == -700));
        assert_eq!(packets[0].epoch.generation(), 2);
    }

    #[test]
    fn mic_tap_rejects_stale_future_malformed_and_discontinuous_audio() {
        let epoch = MicTapEpoch::new(1, None);
        let start = epoch.0.opened.instant();
        let data = pcm(std::iter::repeat_n(0, 1_600));
        let mut packetizer = Packetizer::new(epoch);
        for (timestamp, now) in [
            (start, start + Duration::from_secs(2)),
            (start + Duration::from_secs(1), start),
        ] {
            assert!(
                packetizer
                    .push_at(
                        &data,
                        cpal::SampleFormat::I16,
                        16_000,
                        1,
                        Timestamp::Instant(timestamp),
                        now
                    )
                    .is_err()
            );
        }
        assert!(
            packetizer
                .push_at(
                    &[0, 1, 2],
                    cpal::SampleFormat::I16,
                    16_000,
                    1,
                    Timestamp::Instant(start),
                    start
                )
                .is_err()
        );
        assert!(
            packetizer
                .push_at(
                    &data,
                    cpal::SampleFormat::I16,
                    16_000,
                    0,
                    Timestamp::Instant(start),
                    start
                )
                .is_err()
        );
        assert!(
            packetizer
                .push_at(
                    &data,
                    cpal::SampleFormat::I16,
                    16_000,
                    1,
                    Timestamp::Instant(start),
                    start + Duration::from_millis(100)
                )
                .unwrap()
                .is_empty()
        );
        assert!(
            packetizer
                .push_at(
                    &data,
                    cpal::SampleFormat::I16,
                    16_000,
                    1,
                    Timestamp::Instant(start + Duration::from_millis(300)),
                    start + Duration::from_millis(400)
                )
                .is_err()
        );
    }

    async fn subscribers(mic: &SyntheticMicrophone, count: usize) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while mic.subscriber_count().await.unwrap() != count {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn mic_tap_subscription_is_additive_nonblocking_and_explicitly_removed_without_new_samples()
     {
        let mic = SyntheticMicrophone::spawn("owned-synthetic-mic", 16_000, 1);
        let lock = mic.lock().await.unwrap();
        let actor = (*lock).clone();
        let (recording, recorded) = flume::bounded(16);
        actor.ask(microphone::AddSender(recording)).await.unwrap();
        let (slow, slow_receiver) = flume::bounded(1);
        actor
            .ask(microphone::AddSender(slow.clone()))
            .await
            .unwrap();
        let subscription = FeedSubscription {
            actor: actor.clone(),
            sender: slow,
            runtime: tokio::runtime::Handle::current(),
        };
        for _ in 0..8 {
            mic.push(&[0.25], Timestamp::Instant(Instant::now())).await;
        }
        tokio::time::timeout(Duration::from_secs(1), async {
            for _ in 0..8 {
                recorded.recv_async().await.unwrap();
            }
        })
        .await
        .unwrap();
        assert_eq!(slow_receiver.len(), 1);
        drop(subscription);
        subscribers(&mic, 1).await;
        assert!(slow_receiver.is_disconnected());
        assert!(!recorded.is_disconnected());
    }

    #[tokio::test]
    async fn mic_tap_cancelled_worker_detaches_even_when_microphone_has_stopped_delivering() {
        let mic = SyntheticMicrophone::spawn("private-recording-route", 16_000, 1);
        let lock = mic.lock().await.unwrap();
        let epoch = MicTapEpoch::new(9, None);
        let (tx, _rx) = flume::bounded(4);
        let task = tokio::spawn(run_tap((*lock).clone(), epoch.clone(), tx));
        subscribers(&mic, 1).await;
        epoch.invalidate();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        subscribers(&mic, 0).await;
        let (tx, _rx) = flume::bounded(4);
        let aborted = tokio::spawn(run_tap((*lock).clone(), MicTapEpoch::new(10, None), tx));
        subscribers(&mic, 1).await;
        aborted.abort();
        assert!(aborted.await.unwrap_err().is_cancelled());
        subscribers(&mic, 0).await;
        assert!(lock.is_alive());
    }

    #[tokio::test]
    async fn mic_tap_uses_given_private_route_and_backpressure_does_not_stop_recording() {
        let selected = SyntheticMicrophone::spawn("selected-preview", 16_000, 1);
        let private = SyntheticMicrophone::spawn("actual-recording", 16_000, 1);
        let lock = private.lock().await.unwrap();
        let actor = (*lock).clone();
        let (recording, recorded) = flume::bounded(4);
        actor.ask(microphone::AddSender(recording)).await.unwrap();
        let (tx, queued) = flume::bounded(1);
        let task = tokio::spawn(run_tap(actor, MicTapEpoch::new(1, None), tx));
        subscribers(&private, 2).await;
        assert_eq!(selected.subscriber_count().await.unwrap(), 0);
        private
            .push(&[0.25; 6_400], Timestamp::Instant(Instant::now()))
            .await;
        let failure = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(failure.contains("cannot keep up"));
        assert!(!queued.try_recv().unwrap().epoch.is_current());
        assert_eq!(recorded.recv_async().await.unwrap().data.len(), 6_400 * 4);
        subscribers(&private, 1).await;
        private
            .push(&[0.5], Timestamp::Instant(Instant::now()))
            .await;
        assert_eq!(recorded.recv_async().await.unwrap().data.len(), 4);
    }
}
