use std::ops::Range;

use tokio::sync::{mpsc, oneshot, watch};

use super::*;
use crate::{
    ai_store::{self, Kind},
    mic_tap::{MicPacket, MicTapEpoch, MicTapLease, MicTapStateKind, MicTapStatus},
    teleprompter_follow::{
        self as core, AudioInput, BillingResource, FollowDecision, FollowError, FollowGeneration,
        PcmChunk, RecognitionPacket, ScriptFollower, SessionEvent, StreamingConfig, StreamingEvent,
    },
};
use tr::dialog::teleprompter::follow as copy;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PauseReason {
    Manual,
    Recording,
    Muted,
    Transition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Failure {
    MissingKey,
    Endpoint,
    Microphone,
    Permission,
    Network,
    AudioLag,
    Service,
    Script,
    Layout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum FollowPhase {
    #[default]
    Off,
    Connecting,
    Listening,
    Paused(PauseReason),
    Failed(Failure),
}

impl FollowPhase {
    fn active(self) -> bool {
        matches!(self, Self::Connecting | Self::Listening)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Off => copy::privacy(),
            Self::Connecting => copy::connecting(),
            Self::Listening => copy::listening(),
            Self::Paused(PauseReason::Manual) => copy::manual(),
            Self::Paused(PauseReason::Recording) => copy::recording_paused(),
            Self::Paused(PauseReason::Muted) => copy::muted(),
            Self::Paused(PauseReason::Transition) => copy::transition(),
            Self::Failed(Failure::MissingKey) => copy::key_missing(),
            Self::Failed(Failure::Endpoint) => copy::endpoint(),
            Self::Failed(Failure::Microphone) => copy::microphone(),
            Self::Failed(Failure::Permission) => copy::permission(),
            Self::Failed(Failure::Network) => copy::network(),
            Self::Failed(Failure::AudioLag) => copy::audio_lag(),
            Self::Failed(Failure::Service) => copy::service(),
            Self::Failed(Failure::Script) => copy::script(),
            Self::Failed(Failure::Layout) => copy::layout(),
        }
    }
}

#[derive(Default)]
pub(super) struct FollowState {
    phase: FollowPhase,
    generation: u64,
    cancel: Option<oneshot::Sender<()>>,
    lease: Option<MicTapLease>,
    task: Option<gpui::Task<()>>,
    follower: Option<ScriptFollower>,
    selection: Range<usize>,
    pub(super) body_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    pub(super) text_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl FollowState {
    fn accepts(&self, generation: FollowGeneration) -> bool {
        self.phase.active() && self.generation == generation.0
    }

    fn stop(&mut self, phase: FollowPhase) {
        self.generation = self.generation.wrapping_add(1);
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        self.lease = None;
        self.task = None;
        if let Some(follower) = &mut self.follower {
            follower.cancel_session();
        }
        self.phase = phase;
    }
}

fn official_speech_endpoint(address: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(address.trim()) else {
        return false;
    };
    url.scheme() == "https"
        && url.host_str() == Some("openspeech.bytedance.com")
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none_or(|port| port == 443)
        && url.path().trim_end_matches('/') == "/api/v3"
        && url.query().is_none()
        && url.fragment().is_none()
}

fn unchanged_official_endpoint(
    expected: &Option<ai_store::Endpoint>,
    current: &Option<ai_store::Endpoint>,
) -> bool {
    expected == current
        && official_speech_endpoint(
            current
                .as_ref()
                .map(|endpoint| endpoint.base_url.as_str())
                .unwrap_or("https://openspeech.bytedance.com/api/v3"),
        )
}

fn read_key_for_endpoint(
    expected: &Option<ai_store::Endpoint>,
    mut read_endpoint: impl FnMut() -> Option<ai_store::Endpoint>,
    read_key: impl FnOnce() -> Option<String>,
) -> Result<String, FollowPhase> {
    if !unchanged_official_endpoint(expected, &read_endpoint()) {
        return Err(FollowPhase::Failed(Failure::Endpoint));
    }
    let key = read_key();
    if !unchanged_official_endpoint(expected, &read_endpoint()) {
        return Err(FollowPhase::Failed(Failure::Endpoint));
    }
    key.ok_or(FollowPhase::Failed(Failure::MissingKey))
}

fn core_failure(error: FollowError) -> FollowPhase {
    match error {
        FollowError::InvalidKey | FollowError::Http { status: 401 | 403 } => {
            FollowPhase::Failed(Failure::Permission)
        }
        FollowError::InvalidAudio
        | FollowError::InvalidAudioClock
        | FollowError::StaleAudio
        | FollowError::AudioBackpressure
        | FollowError::ConsumerLagged => FollowPhase::Failed(Failure::AudioLag),
        FollowError::NoAudio | FollowError::AudioClosed => FollowPhase::Failed(Failure::Microphone),
        FollowError::Server { .. } | FollowError::Http { .. } | FollowError::Protocol(_) => {
            FollowPhase::Failed(Failure::Service)
        }
        FollowError::InvalidScript => FollowPhase::Failed(Failure::Script),
        FollowError::Cancelled => FollowPhase::Paused(PauseReason::Manual),
        FollowError::Network
        | FollowError::Timeout(_)
        | FollowError::Disconnected
        | FollowError::ConsumerClosed => FollowPhase::Failed(Failure::Network),
    }
}

fn mic_failure(state: &MicTapStateKind) -> FollowPhase {
    match state {
        MicTapStateKind::RecordingPaused => FollowPhase::Paused(PauseReason::Recording),
        MicTapStateKind::RecordingMuted => FollowPhase::Paused(PauseReason::Muted),
        MicTapStateKind::Disabled => FollowPhase::Paused(PauseReason::Manual),
        MicTapStateKind::Ready | MicTapStateKind::RecordingTransition => {
            FollowPhase::Paused(PauseReason::Transition)
        }
        MicTapStateKind::NoMicrophone => FollowPhase::Failed(Failure::Microphone),
        MicTapStateKind::Failed(_) => FollowPhase::Failed(Failure::AudioLag),
    }
}

fn cancelled_microphone_status(status: &MicTapStatus, generation: u64) -> FollowPhase {
    if status.generation == generation && status.state == MicTapStateKind::Ready {
        FollowPhase::Failed(Failure::AudioLag)
    } else {
        mic_failure(&status.state)
    }
}

async fn wait_for_microphone(
    status: &mut watch::Receiver<MicTapStatus>,
    packets: &flume::Receiver<MicPacket>,
) -> Result<MicTapEpoch, FollowPhase> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let current = status.borrow().clone();
        if let Some(epoch) = current.epoch.as_ref().filter(|epoch| epoch.is_current()) {
            return Ok(epoch.clone());
        }
        if !matches!(current.state, MicTapStateKind::NoMicrophone) {
            return Err(cancelled_microphone_status(&current, current.generation));
        }
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => return Err(FollowPhase::Failed(Failure::Microphone)),
            changed = status.changed() => if changed.is_err() { return Err(FollowPhase::Failed(Failure::Microphone)); },
            packet = packets.recv_async() => if packet.is_err() { return Err(FollowPhase::Failed(Failure::Microphone)); },
        }
    }
}

fn upload_allowed(ready_at: Option<Instant>, captured_at: Instant, epoch_current: bool) -> bool {
    epoch_current && ready_at.is_some_and(|ready| captured_at >= ready)
}

pub(super) fn reading_scroll_position(
    offset: f32,
    text_top: f32,
    row: Range<f32>,
    cue_y: f32,
    maximum: f32,
) -> Option<f32> {
    if ![offset, text_top, row.start, row.end, cue_y, maximum]
        .iter()
        .all(|value| value.is_finite())
        || maximum < 0.
        || row.end <= row.start
    {
        return None;
    }
    Some((-offset + text_top + (row.start + row.end) / 2. - cue_y).clamp(0., maximum))
}

async fn discard_until<T, P>(
    result: impl std::future::Future<Output = T>,
    packets: &flume::Receiver<P>,
) -> Result<T, FollowPhase> {
    tokio::pin!(result);
    loop {
        tokio::select! {
            biased;
            result = &mut result => return Ok(result),
            packet = packets.recv_async() => if packet.is_err() {
                return Err(FollowPhase::Failed(Failure::Microphone));
            },
        }
    }
}

async fn drive_session(
    generation: FollowGeneration,
    endpoint: Option<ai_store::Endpoint>,
    mut status: watch::Receiver<MicTapStatus>,
    packets: flume::Receiver<MicPacket>,
    updates: mpsc::Sender<SessionEvent>,
) -> Result<(), FollowPhase> {
    let expected = endpoint.clone();
    let lookup = tokio::task::spawn_blocking(move || {
        read_key_for_endpoint(
            &expected,
            || ai_store::endpoint(Kind::Asr, "doubao"),
            || ai_store::key(Kind::Asr, "doubao"),
        )
    });
    let key = discard_until(lookup, &packets)
        .await?
        .map_err(|_| FollowPhase::Failed(Failure::MissingKey))??;
    let config = StreamingConfig::new(key, BillingResource::Duration).map_err(core_failure)?;
    let epoch = wait_for_microphone(&mut status, &packets).await?;
    if !unchanged_official_endpoint(&endpoint, &ai_store::endpoint(Kind::Asr, "doubao")) {
        return Err(FollowPhase::Failed(Failure::Endpoint));
    }
    let (audio, receiver) = core::audio_channel();
    let (events, mut received) = core::event_channel();
    let (_cancel, cancelled) = oneshot::channel();
    let network = core::run_session(generation, config, receiver, events, cancelled);
    tokio::pin!(network);
    let mut ready_at = None;
    loop {
        tokio::select! {
            biased;
            _ = epoch.cancelled() => return Err(cancelled_microphone_status(&status.borrow(), epoch.generation())),
            changed = status.changed() => {
                if changed.is_err() || status.borrow().generation != epoch.generation() {
                    return Err(mic_failure(&status.borrow().state));
                }
            }
            result = &mut network => return result.map_err(core_failure),
            event = received.recv() => {
                let Some(event) = event else { return Err(FollowPhase::Failed(Failure::Network)); };
                if event.generation != generation { continue; }
                if matches!(event.event, StreamingEvent::Ready) { ready_at = Some(Instant::now()); }
                updates.try_send(event).map_err(|_| FollowPhase::Failed(Failure::AudioLag))?;
            }
            packet = packets.recv_async() => {
                let packet = packet.map_err(|_| FollowPhase::Failed(Failure::Microphone))?;
                if !upload_allowed(ready_at, packet.captured_at, packet.epoch.generation() == epoch.generation() && epoch.is_current()) {
                    continue;
                }
                if !packet.is_current() { return Err(FollowPhase::Failed(Failure::AudioLag)); }
                let chunk = PcmChunk::captured_16khz_mono(packet.samples, packet.captured_at).map_err(core_failure)?;
                core::try_enqueue_audio(&audio, AudioInput::Pcm(chunk)).map_err(core_failure)?;
            }
        }
    }
}

impl TeleprompterWindow {
    pub(super) fn stop_follow(&mut self) {
        self.follow.stop(FollowPhase::Off);
    }

    pub(super) fn pause_follow(&mut self) {
        if self.follow.phase.active() {
            self.follow.stop(FollowPhase::Paused(PauseReason::Manual));
        }
    }

    pub(super) fn follow_selection_changed(&mut self, cx: &Context<Self>) {
        let selection = self.script_input.read(cx).selection();
        if selection != self.follow.selection {
            self.pause_follow();
            self.follow.selection = selection;
        }
    }

    pub(super) fn follow_recording_changed(&mut self, phase: Phase, muted: bool, uncertain: bool) {
        if !self.follow.phase.active() {
            return;
        }
        let reason = match phase {
            Phase::Recording { paused: true } => Some(PauseReason::Recording),
            Phase::Starting | Phase::Stopping => Some(PauseReason::Transition),
            _ if uncertain => Some(PauseReason::Transition),
            Phase::Recording { .. } if muted => Some(PauseReason::Muted),
            _ => None,
        };
        if let Some(reason) = reason {
            self.follow.stop(FollowPhase::Paused(reason));
        }
    }

    pub(super) fn follow_active(&self) -> bool {
        self.follow.phase.active()
    }

    pub(super) fn reading_offset(&self, cx: &Context<Self>) -> Option<usize> {
        let body = self.follow.body_bounds.get()?;
        let text = self.follow.text_bounds.get()?;
        let y = body.origin.y + body.size.height / 2. - text.origin.y;
        self.script_input.read(cx).byte_offset_for_visual_y(y)
    }

    pub(super) fn start_follow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_follow();
        self.stop_playback();
        let endpoint = ai_store::endpoint(Kind::Asr, "doubao");
        if !official_speech_endpoint(
            endpoint
                .as_ref()
                .map(|endpoint| endpoint.base_url.as_str())
                .unwrap_or("https://openspeech.bytedance.com/api/v3"),
        ) {
            self.follow.phase = FollowPhase::Failed(Failure::Endpoint);
            cx.notify();
            return;
        }
        let Ok(mut follower) = ScriptFollower::new(&self.state.script) else {
            self.follow.phase = FollowPhase::Failed(Failure::Script);
            cx.notify();
            return;
        };
        let Some(offset) = self.reading_offset(cx) else {
            self.follow.phase = FollowPhase::Failed(Failure::Layout);
            cx.notify();
            return;
        };
        let generation = FollowGeneration(self.follow.generation);
        if follower.begin_session(generation).is_err()
            || follower
                .resume_at(follower.sentence_at_byte(offset))
                .is_err()
        {
            self.follow.phase = FollowPhase::Failed(Failure::Script);
            cx.notify();
            return;
        }
        self.follow.selection = self.script_input.read(cx).selection();
        self.follow.follower = Some(follower);
        self.follow.phase = FollowPhase::Connecting;
        let lease = MicTapLease::new(cx);
        lease.set_enabled(true, cx);
        let status = lease.subscribe();
        let packets = lease.packets();
        self.follow.lease = Some(lease);
        let (cancel, cancelled) = oneshot::channel();
        self.follow.cancel = Some(cancel);
        let (updates, mut received) = mpsc::channel(16);
        let backend = gpui_tokio::Tokio::spawn(cx, async move {
            tokio::select! {
                biased;
                _ = cancelled => Err(FollowPhase::Paused(PauseReason::Manual)),
                result = drive_session(generation, endpoint, status, packets, updates) => result,
            }
        });
        self.follow.task = Some(cx.spawn_in(window, async move |this, cx| {
            while let Some(event) = received.recv().await {
                let active = this
                    .update_in(cx, |this, window, cx| {
                        if !this.follow.accepts(event.generation) {
                            return false;
                        }
                        match event.event {
                            StreamingEvent::Ready => this.follow.phase = FollowPhase::Listening,
                            StreamingEvent::Recognition(packet) => {
                                this.follow_recognition(generation, packet, window, cx)
                            }
                            StreamingEvent::Connected { .. } | StreamingEvent::Finished => {}
                        }
                        cx.notify();
                        window.refresh();
                        true
                    })
                    .unwrap_or(false);
                if !active {
                    return;
                }
            }
            let phase = backend
                .await
                .ok()
                .and_then(Result::err)
                .unwrap_or(FollowPhase::Failed(Failure::Network));
            let _ = this.update_in(cx, |this, window, cx| {
                if !this.follow.accepts(generation) {
                    return;
                }
                this.follow.stop(phase);
                cx.notify();
                window.refresh();
            });
        }));
        cx.notify();
    }

    fn follow_recognition(
        &mut self,
        generation: FollowGeneration,
        packet: RecognitionPacket,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.follow.accepts(generation) || self.follow.phase != FollowPhase::Listening {
            return;
        }
        let Some(follower) = &mut self.follow.follower else {
            return;
        };
        let mut offset = None;
        for update in packet.updates {
            if let FollowDecision::Move(anchor) | FollowDecision::Follow(anchor) =
                follower.update_for(generation, &update)
            {
                offset = Some(anchor.read_offset);
            }
        }
        let Some(row) = offset.and_then(|offset| {
            self.script_input
                .read(cx)
                .visual_row_bounds_for_offset(offset)
        }) else {
            return;
        };
        let (Some(body), Some(text)) =
            (self.follow.body_bounds.get(), self.follow.text_bounds.get())
        else {
            return;
        };
        let Some(position) = reading_scroll_position(
            f32::from(self.scroll.offset().y),
            f32::from(text.origin.y),
            f32::from(row.start)..f32::from(row.end),
            f32::from(body.origin.y + body.size.height / 2.),
            self.maximum_scroll(),
        ) else {
            return;
        };
        self.position = position;
        self.apply_scroll();
        window.refresh();
        cx.notify();
    }

    /// Voice follow's state over the reading bar, with the one fix a failure
    /// needs (settings, or choosing a microphone). Nothing while off.
    pub(super) fn render_follow_status(&self, _cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let phase = self.follow.phase;
        if phase == FollowPhase::Off {
            return None;
        }
        let fix = match phase {
            FollowPhase::Failed(Failure::MissingKey | Failure::Endpoint | Failure::Permission) => {
                Some(self.button("prompter-follow-settings", copy::settings()).on_click(
                    |_, _, cx| {
                        crate::app_windows::open_settings(
                            crate::settings_window::Page::Transcription,
                            cx,
                        )
                    },
                ))
            }
            FollowPhase::Failed(Failure::Microphone) => Some(
                self.button("prompter-follow-microphone", copy::choose_microphone())
                    .on_click(|_, _, cx| crate::app_windows::show_main_window(cx)),
            ),
            _ => None,
        };
        Some(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .max_w(px(420.))
                .px(px(10.))
                .py(px(4.))
                .rounded(px(12.))
                .bg(Hsla::from(self.theme.gray_2))
                .child(
                    div()
                        .min_w_0()
                        .text_size(px(11.))
                        .text_color(Hsla::from(if matches!(phase, FollowPhase::Failed(_)) {
                            self.theme.red_10
                        } else {
                            self.theme.gray_10
                        }))
                        .child(phase.label()),
                )
                .children(fix)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follow_only_uses_the_explicit_official_speech_endpoint() {
        for address in [
            "https://openspeech.bytedance.com/api/v3",
            "https://openspeech.bytedance.com:443/api/v3/",
        ] {
            assert!(official_speech_endpoint(address));
        }
        for address in [
            "https://example.com/api/v3",
            "https://openspeech.bytedance.com.example.com/api/v3",
            "http://openspeech.bytedance.com/api/v3",
            "https://key@openspeech.bytedance.com/api/v3",
            "https://openspeech.bytedance.com:8443/api/v3",
            "https://openspeech.bytedance.com/api/v3?key=secret",
            "https://openspeech.bytedance.com/api/v3#other",
            "https://ark.cn-beijing.volces.com/api/v3",
        ] {
            assert!(!official_speech_endpoint(address));
        }
    }

    #[test]
    fn follow_ready_discards_audio_captured_during_connection() {
        let before = Instant::now();
        let ready = before + Duration::from_millis(600);
        assert!(!upload_allowed(None, before, true));
        assert!(!upload_allowed(Some(ready), before, true));
        assert!(upload_allowed(Some(ready), ready, true));
        assert!(!upload_allowed(Some(ready), ready, false));
    }

    #[test]
    fn follow_manual_stop_invalidates_late_ready_and_recognition() {
        let mut state = FollowState::default();
        assert!(!state.phase.active());
        state.generation = 4;
        state.phase = FollowPhase::Connecting;
        let (cancel, mut receiver) = oneshot::channel();
        state.cancel = Some(cancel);
        assert!(state.accepts(FollowGeneration(4)));
        state.stop(FollowPhase::Paused(PauseReason::Manual));
        assert!(receiver.try_recv().is_ok());
        assert!(!state.accepts(FollowGeneration(4)));
        assert!(!state.accepts(FollowGeneration(5)));
        assert_eq!(state.phase, FollowPhase::Paused(PauseReason::Manual));
        state.phase = FollowPhase::Connecting;
        assert!(state.accepts(FollowGeneration(5)));
    }

    #[test]
    fn follow_mute_and_pause_require_explicit_restart() {
        for (state, expected) in [
            (
                MicTapStateKind::RecordingPaused,
                FollowPhase::Paused(PauseReason::Recording),
            ),
            (
                MicTapStateKind::RecordingMuted,
                FollowPhase::Paused(PauseReason::Muted),
            ),
            (
                MicTapStateKind::RecordingTransition,
                FollowPhase::Paused(PauseReason::Transition),
            ),
        ] {
            let mut follow = FollowState::default();
            follow.phase = FollowPhase::Listening;
            follow.stop(mic_failure(&state));
            assert_eq!(follow.phase, expected);
            assert!(!follow.phase.active());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn follow_discards_capture_while_credentials_are_pending_and_cancels_cleanly() {
        let (tx, rx) = flume::bounded(4);
        let (ready, result) = oneshot::channel::<()>();
        let task = tokio::spawn(async move { discard_until(result, &rx).await });
        for sequence in 0..20 {
            tx.try_send(sequence).unwrap();
            tokio::task::yield_now().await;
        }
        ready.send(()).unwrap();
        assert_eq!(task.await.unwrap(), Ok(Ok(())));
        assert!(tx.is_disconnected());
        let (tx, rx) = flume::bounded::<u8>(4);
        let (_ready, result) = oneshot::channel::<()>();
        let (cancel, cancelled) = oneshot::channel();
        let task = tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = cancelled => None,
                result = discard_until(result, &rx) => Some(result),
            }
        });
        tx.try_send(1).unwrap();
        cancel.send(()).unwrap();
        assert_eq!(task.await.unwrap(), None);
        assert!(tx.is_disconnected());
    }

    #[test]
    fn follow_visual_row_center_uses_actual_scrolled_bounds_without_caret_changes() {
        assert_eq!(
            reading_scroll_position(-60., 90., 120. ..160., 190., 500.),
            Some(100.)
        );
        assert_eq!(
            reading_scroll_position(-100., 50., 120. ..160., 190., 500.),
            Some(100.)
        );
        assert_eq!(
            reading_scroll_position(0., 150., 0. ..40., 190., 500.),
            Some(0.)
        );
        assert_eq!(
            reading_scroll_position(-60., 90., 900. ..940., 190., 500.),
            Some(500.)
        );
        assert_eq!(
            reading_scroll_position(0., f32::NAN, 0. ..40., 190., 500.),
            None
        );
    }

    #[test]
    fn follow_window_release_cancels_even_without_close_callback() {
        let (cancel, mut receiver) = oneshot::channel();
        let state = FollowState {
            cancel: Some(cancel),
            ..Default::default()
        };
        drop(state);
        assert!(matches!(
            receiver.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        ));
    }

    #[test]
    fn follow_endpoint_changes_before_or_during_credential_lookup_never_release_a_key() {
        let official = Some(ai_store::Endpoint {
            base_url: "https://openspeech.bytedance.com/api/v3".into(),
            model: "bigmodel".into(),
        });
        let proxy = Some(ai_store::Endpoint {
            base_url: "https://example.com/api/v3".into(),
            model: "bigmodel".into(),
        });
        let reads = Cell::new(0);
        let rejected = read_key_for_endpoint(
            &official,
            || proxy.clone(),
            || {
                reads.set(reads.get() + 1);
                Some("synthetic".into())
            },
        );
        assert_eq!(rejected, Err(FollowPhase::Failed(Failure::Endpoint)));
        assert_eq!(reads.get(), 0);
        let mut endpoints = std::collections::VecDeque::from([official.clone(), proxy]);
        let rejected = read_key_for_endpoint(
            &official,
            || endpoints.pop_front().unwrap(),
            || {
                reads.set(reads.get() + 1);
                Some("synthetic-third-party-key".into())
            },
        );
        assert_eq!(rejected, Err(FollowPhase::Failed(Failure::Endpoint)));
        assert_eq!(reads.get(), 1);
        assert_eq!(
            read_key_for_endpoint(
                &official,
                || official.clone(),
                || Some("synthetic-official-key".into())
            ),
            Ok("synthetic-official-key".into())
        );
    }

    #[test]
    fn follow_connection_recheck_rejects_endpoint_model_and_removal_changes() {
        let official = Some(ai_store::Endpoint {
            base_url: "https://openspeech.bytedance.com/api/v3".into(),
            model: "bigmodel".into(),
        });
        let changed_model = Some(ai_store::Endpoint {
            base_url: "https://openspeech.bytedance.com/api/v3".into(),
            model: "new-model".into(),
        });
        assert!(unchanged_official_endpoint(&official, &official));
        assert!(!unchanged_official_endpoint(&official, &changed_model));
        assert!(!unchanged_official_endpoint(&official, &None));
        assert!(!unchanged_official_endpoint(&None, &official));
    }

    #[test]
    fn follow_worker_failure_before_status_publication_is_not_a_recording_transition() {
        let mut status = MicTapStatus {
            generation: 7,
            state: MicTapStateKind::Ready,
            epoch: None,
        };
        assert_eq!(
            cancelled_microphone_status(&status, 7),
            FollowPhase::Failed(Failure::AudioLag)
        );
        status.generation = 8;
        assert_eq!(
            cancelled_microphone_status(&status, 7),
            FollowPhase::Paused(PauseReason::Transition)
        );
        status.generation = 7;
        status.state = MicTapStateKind::RecordingMuted;
        assert_eq!(
            cancelled_microphone_status(&status, 7),
            FollowPhase::Paused(PauseReason::Muted)
        );
        status.state = MicTapStateKind::RecordingPaused;
        assert_eq!(
            cancelled_microphone_status(&status, 7),
            FollowPhase::Paused(PauseReason::Recording)
        );
        status.state = MicTapStateKind::Failed("synthetic converter failure".into());
        assert_eq!(
            cancelled_microphone_status(&status, 7),
            FollowPhase::Failed(Failure::AudioLag)
        );
    }
}
