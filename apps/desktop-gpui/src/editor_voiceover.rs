use std::{
    path::PathBuf,
    sync::{Arc, Weak},
};

use cap_editor::EditorInstance;
use gpui::{
    AnyElement, AppContext as _, AsyncWindowContext, Context, Entity, FontWeight, Hsla,
    InteractiveElement as _, IntoElement as _, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use tokio::sync::{Mutex, watch};
use uuid::Uuid;

use crate::{
    editor_window::EditorWindow,
    feeds::Feeds,
    i18n::tr,
    ui,
    voiceover_recording::{
        self, DiscoveredTake, ReadyTake, TakeAvailability, TakeState, VoiceoverRequest,
        VoiceoverSession, VoiceoverStatus,
    },
};

type Session = Arc<Mutex<Option<VoiceoverSession>>>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    Starting,
    Recording,
    Paused,
    Finalizing,
    SourceFinalizing,
    Recovering,
    Ready,
    Attached,
    Error,
}

impl Phase {
    fn busy(self) -> bool {
        matches!(
            self,
            Self::Starting
                | Self::Recording
                | Self::Paused
                | Self::Finalizing
                | Self::SourceFinalizing
                | Self::Recovering
        )
    }
}

#[derive(Debug, PartialEq)]
enum StatusAction {
    Continue,
    Recover,
    Failed,
    Cancelled,
    Ignore,
}

fn observe_recording_phase(phase: &mut Phase, state: TakeState) -> StatusAction {
    if !matches!(
        phase,
        Phase::Recording | Phase::Paused | Phase::SourceFinalizing
    ) {
        return StatusAction::Ignore;
    }
    match state {
        TakeState::Finalizing => {
            *phase = Phase::SourceFinalizing;
            StatusAction::Continue
        }
        TakeState::Ready => {
            *phase = Phase::Ready;
            StatusAction::Recover
        }
        TakeState::Failed => {
            *phase = Phase::Error;
            StatusAction::Failed
        }
        TakeState::Cancelled => {
            *phase = Phase::Idle;
            StatusAction::Cancelled
        }
        _ => StatusAction::Continue,
    }
}

fn audio_destination(
    project: &cap_project::ProjectConfiguration,
    path: &str,
    lane: u32,
    next_lane: u32,
    start: f64,
    total: f64,
    duration: f64,
) -> Option<(u32, std::ops::Range<f64>)> {
    let segments = project.timeline.as_ref()?.audio_segments.as_slice();
    if let Some(segment) = segments.iter().find(|segment| segment.path == path) {
        return Some((segment.track, segment.start..segment.end));
    }
    crate::editor_audio::audio_import_placement(segments, lane, next_lane, start, total, duration)
}

#[derive(Clone)]
struct Ticket {
    generation: u64,
    project: PathBuf,
    instance: Weak<EditorInstance>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Placement {
    start: f64,
    lane: u32,
}

fn placement(
    project: &cap_project::ProjectConfiguration,
    start: f64,
    duration: f64,
    lane: u32,
    next_lane: u32,
) -> Option<Placement> {
    let segments = project.timeline.as_ref()?.audio_segments.as_slice();
    let (lane, range) = crate::editor_audio::audio_import_placement(
        segments,
        lane,
        next_lane,
        start,
        duration,
        (duration - start).max(1.),
    )?;
    (lane <= 4095).then_some(Placement {
        start: range.start,
        lane,
    })
}

struct MicrophonePreview;

struct Panel {
    ticket: Ticket,
    placement: Placement,
    phase: Phase,
    devices: Vec<String>,
    selected: Option<String>,
    device_menu: bool,
    meter: Entity<ui::MicrophoneLevel>,
    _microphone_preview: Entity<MicrophonePreview>,
    normalize: bool,
    session: Option<Session>,
    take_id: Option<Uuid>,
    startup_cancel: Option<watch::Sender<bool>>,
    status: Option<VoiceoverStatus>,
    ready: Option<ReadyTake>,
    takes: Vec<DiscoveredTake>,
    recovery_open: bool,
    pause_pending: bool,
    close_after_cancel: bool,
    confirming: bool,
    error: Option<String>,
}

#[derive(Default)]
pub(crate) struct VoiceoverUi {
    generation: u64,
    panel: Option<Panel>,
}

impl VoiceoverUi {
    pub(crate) fn is_open(&self) -> bool {
        self.panel.is_some()
    }
    pub(crate) fn is_busy(&self) -> bool {
        self.panel.as_ref().is_some_and(|panel| panel.phase.busy())
    }
}

impl EditorWindow {
    fn voiceover_matches(&self, ticket: &Ticket) -> bool {
        self.voiceover
            .panel
            .as_ref()
            .is_some_and(|panel| panel.ticket.generation == ticket.generation)
            && self.project_path == ticket.project
            && ticket
                .instance
                .upgrade()
                .zip(self.instance.as_ref())
                .is_some_and(|(old, current)| Arc::ptr_eq(&old, current))
    }

    pub(crate) fn open_voiceover(
        &mut self,
        lane: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.voiceover.is_busy()
            || self.busy_reason().is_some()
            || self.audio_import_pending.is_some()
        {
            return;
        }
        let Some(instance) = self.instance.as_ref() else {
            return;
        };
        let Some(placement) = placement(
            &self.project,
            self.playhead,
            self.total_duration(),
            lane,
            self.tracks.audio,
        ) else {
            self.show_notice(tr::editor::voiceover::move_playhead(), window, cx);
            return;
        };
        let instance = Arc::downgrade(instance);
        if let Err(error) = self.flush_pending_saves(cx) {
            self.show_notice(error, window, cx);
            return;
        }
        self.library_audition = None;
        self.audio_library.audition_failed = false;
        self.stop_playback(cx);
        self.seek_to_time(placement.start, cx);
        self.library_audition = None;
        self.audio_picker = None;
        self.cancel_asset_drag(cx);
        let feeds = Feeds::global(cx);
        let level = feeds.read(cx).mic_level().clone();
        let selected = feeds.read(cx).microphone.clone();
        let meter = cx.new(|cx| {
            ui::MicrophoneLevel::new(ui::MicrophoneLevelKind::Voiceover, &level, window, cx)
        });
        let theme = self.theme;
        meter.update(cx, |meter, cx| {
            meter.configure(
                true,
                Hsla::from(theme.gray_11),
                Hsla::from(theme.gray_4),
                cx,
            )
        });
        self.voiceover.generation = self.voiceover.generation.wrapping_add(1);
        let ticket = Ticket {
            generation: self.voiceover.generation,
            project: self.project_path.clone(),
            instance,
        };
        feeds.update(cx, |feeds, cx| {
            feeds.acquire_mic_preview(selected.clone(), cx)
        });
        let preview = cx.new(|cx| {
            cx.on_release(move |_: &mut MicrophonePreview, cx| {
                feeds.update(cx, |feeds, cx| feeds.release_mic_tap(cx));
            })
            .detach();
            MicrophonePreview
        });
        self.voiceover.panel = Some(Panel {
            ticket: ticket.clone(),
            placement,
            phase: Phase::Idle,
            devices: Vec::new(),
            selected,
            device_menu: false,
            meter,
            _microphone_preview: preview,
            normalize: false,
            session: None,
            take_id: None,
            startup_cancel: None,
            status: None,
            ready: None,
            takes: Vec::new(),
            recovery_open: false,
            pause_pending: false,
            close_after_cancel: false,
            confirming: false,
            error: None,
        });
        window.focus(&self.focus_handle_for_menu(), cx);
        cx.spawn_in(window, async move |this, cx: &mut AsyncWindowContext| {
            let project = ticket.project.clone();
            let (devices, takes) = cx
                .background_executor()
                .spawn(async move {
                    (
                        cap_recording::MicrophoneFeed::list_names(),
                        voiceover_recording::list_takes(&project)
                            .map_err(|error| error.to_string()),
                    )
                })
                .await;
            let _ = this.update_in(cx, |this, _, cx| {
                if !this.voiceover_matches(&ticket) {
                    return;
                }
                let panel = this.voiceover.panel.as_mut().unwrap();
                if panel.selected.is_none() {
                    panel.selected = devices.first().cloned();
                }
                panel.devices = devices;
                let selected = panel.selected.clone();
                Feeds::global(cx).update(cx, |feeds, cx| feeds.set_microphone(selected, cx));
                match takes {
                    Ok(takes) => panel.takes = takes,
                    Err(error) => panel.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn start_voiceover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = self.voiceover.panel.as_ref() else {
            return;
        };
        if panel.phase.busy() || panel.ready.is_some() {
            return;
        }
        let Some(selected) = panel.selected.clone() else {
            return;
        };
        let ticket = panel.ticket.clone();
        if !self.voiceover_matches(&ticket) {
            return;
        }
        let Some(placement) = placement(
            &self.project,
            panel.placement.start,
            self.total_duration(),
            panel.placement.lane,
            self.tracks.audio,
        ) else {
            self.voiceover.panel.as_mut().unwrap().error =
                Some(tr::editor::voiceover::move_playhead().into());
            cx.notify();
            return;
        };
        self.voiceover.panel.as_mut().unwrap().placement = placement;
        if let Err(error) = self.flush_pending_saves(cx) {
            self.voiceover.panel.as_mut().unwrap().error = Some(error);
            cx.notify();
            return;
        }
        self.library_audition = None;
        self.audio_library.audition_failed = false;
        self.stop_playback(cx);
        let feeds = Feeds::global(cx);
        feeds.update(cx, |feeds, cx| {
            feeds.set_microphone(Some(selected.clone()), cx)
        });
        let (ready, actor) = {
            let feeds = feeds.read(cx);
            (feeds.input_readiness().microphone, feeds.mic_actor())
        };
        let (Some(ready), Some(actor)) = (ready, actor) else {
            self.voiceover.panel.as_mut().unwrap().error =
                Some(tr::editor::voiceover::microphone_unavailable().into());
            cx.notify();
            return;
        };
        self.voiceover.generation = self.voiceover.generation.wrapping_add(1);
        let panel = self.voiceover.panel.as_mut().unwrap();
        panel.ticket.generation = self.voiceover.generation;
        let ticket = panel.ticket.clone();
        let request = VoiceoverRequest {
            project_path: self.project_path.clone(),
            id: Uuid::new_v4(),
            output_start: panel.placement.start,
            lane: panel.placement.lane,
        };
        panel.take_id = Some(request.id);
        let (cancel, mut cancelled) = watch::channel(false);
        panel.startup_cancel = Some(cancel);
        panel.phase = Phase::Starting;
        panel.close_after_cancel = false;
        panel.pause_pending = false;
        panel.error = None;
        panel.status = None;
        panel.device_menu = false;
        cx.spawn_in(window, async move |this, cx: &mut AsyncWindowContext| {
            let result = gpui_tokio::Tokio::spawn(cx, async move {
                let lock = tokio::select! {
                    _ = cancelled.changed() => return Ok(None),
                    result = async {
                        ready.await?;
                        let lock = actor.ask(cap_recording::feeds::microphone::Lock).await.map_err(|error| error.to_string())?;
                        if lock.device_name() != selected { return Err(tr::editor::voiceover::microphone_changed().to_string()); }
                        Ok::<_, String>(Arc::new(lock))
                    } => result?,
                };
                if *cancelled.borrow() { return Ok(None); }
                let session = VoiceoverSession::start(request, lock).await.map_err(|error| format!("{error:#}"))?;
                if *cancelled.borrow() || cancelled.has_changed().is_err() {
                    session.cancel().await.map_err(|error| format!("{error:#}"))?;
                    Ok(None)
                } else { Ok(Some(session)) }
            }).await.unwrap_or_else(|error| Err(error.to_string()));
            let mut result = Some(result);
            let accepted = this.update_in(cx, |this, window, cx| {
                if !this.voiceover_matches(&ticket) { return false; }
                let panel = this.voiceover.panel.as_mut().unwrap();
                panel.startup_cancel = None;
                match result.take().unwrap() {
                    Ok(Some(session)) => {
                        let status = session.subscribe();
                        panel.status = Some(session.status());
                        panel.session = Some(Arc::new(Mutex::new(Some(session))));
                        panel.phase = Phase::Recording;
                        if panel.close_after_cancel {
                            this.finish_voiceover(true, window, cx);
                        } else {
                            this.watch_voiceover(ticket.clone(), status, window, cx);
                        }
                    }
                    Ok(None) => {
                        let close = panel.close_after_cancel;
                        panel.phase = Phase::Idle;
                        if close { this.voiceover.panel = None; }
                    }
                    Err(error) => {
                        panel.phase = Phase::Error;
                        panel.error = Some(error);
                    }
                }
                cx.notify();
                true
            }).unwrap_or(false);
            if !accepted && let Some(Ok(Some(session))) = result {
                let _ = gpui_tokio::Tokio::spawn(cx, async move { session.cancel().await }).await;
            }
        }).detach();
        cx.notify();
    }

    fn watch_voiceover(
        &mut self,
        ticket: Ticket,
        mut status: watch::Receiver<VoiceoverStatus>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx: &mut AsyncWindowContext| {
            loop {
                let value = status.borrow_and_update().clone();
                let keep = this
                    .update_in(cx, |this, window, cx| {
                        if !this.voiceover_matches(&ticket) {
                            return false;
                        }
                        let panel = this.voiceover.panel.as_mut().unwrap();
                        let action = observe_recording_phase(&mut panel.phase, value.state);
                        if action == StatusAction::Ignore {
                            return false;
                        }
                        panel.status = Some(value.clone());
                        if panel.confirming
                            && matches!(action, StatusAction::Recover | StatusAction::Failed)
                        {
                            panel.phase = Phase::SourceFinalizing;
                            cx.notify();
                            return false;
                        }
                        match action {
                            StatusAction::Recover => {
                                let id = panel.take_id;
                                panel.session = None;
                                if let Some(id) = id {
                                    this.recover_voiceover(id, window, cx);
                                }
                            }
                            StatusAction::Failed => {
                                panel.session = None;
                                panel.error = Some(value.error.clone().unwrap_or_else(|| {
                                    tr::editor::voiceover::recording_ended().into()
                                }));
                                this.reload_voiceover_takes(window, cx);
                            }
                            StatusAction::Cancelled => {
                                panel.session = None;
                            }
                            _ => {}
                        }
                        cx.notify();
                        action == StatusAction::Continue
                    })
                    .unwrap_or(false);
                if !keep || status.changed().await.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn pause_voiceover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = self.voiceover.panel.as_mut() else {
            return;
        };
        if panel.pause_pending || !matches!(panel.phase, Phase::Recording | Phase::Paused) {
            return;
        }
        let Some(session) = panel.session.clone() else {
            return;
        };
        let paused = panel.phase == Phase::Recording;
        let ticket = panel.ticket.clone();
        panel.pause_pending = true;
        cx.spawn_in(window, async move |this, cx: &mut AsyncWindowContext| {
            let result = gpui_tokio::Tokio::spawn(cx, async move {
                let session = session.lock().await;
                let session = session
                    .as_ref()
                    .ok_or_else(|| tr::editor::voiceover::recording_ended().to_string())?;
                if paused {
                    session.pause().await
                } else {
                    session.resume().await
                }
                .map_err(|error| format!("{error:#}"))
            })
            .await
            .unwrap_or_else(|error| Err(error.to_string()));
            let _ = this.update_in(cx, |this, _, cx| {
                if !this.voiceover_matches(&ticket) {
                    return;
                }
                let panel = this.voiceover.panel.as_mut().unwrap();
                panel.pause_pending = false;
                if matches!(panel.phase, Phase::Recording | Phase::Paused) {
                    match result {
                        Ok(()) => {
                            panel.phase = if paused {
                                Phase::Paused
                            } else {
                                Phase::Recording
                            }
                        }
                        Err(error) => panel.error = Some(error),
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn finish_voiceover(&mut self, cancel: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = self.voiceover.panel.as_mut() else {
            return;
        };
        if panel.phase == Phase::Starting {
            if cancel {
                if let Some(signal) = &panel.startup_cancel {
                    let _ = signal.send(true);
                }
                panel.phase = Phase::Finalizing;
                cx.notify();
            }
            return;
        }
        if !matches!(
            panel.phase,
            Phase::Recording | Phase::Paused | Phase::SourceFinalizing
        ) {
            return;
        }
        let Some(session) = panel.session.take() else {
            return;
        };
        let ticket = panel.ticket.clone();
        panel.phase = Phase::Finalizing;
        panel.error = None;
        cx.spawn_in(window, async move |this, cx: &mut AsyncWindowContext| {
            let result = gpui_tokio::Tokio::spawn(cx, async move {
                let session = session
                    .lock()
                    .await
                    .take()
                    .ok_or_else(|| tr::editor::voiceover::recording_ended().to_string())?;
                if cancel {
                    session.cancel().await.map(|()| None)
                } else {
                    session.stop().await.map(Some)
                }
                .map_err(|error| format!("{error:#}"))
            })
            .await
            .unwrap_or_else(|error| Err(error.to_string()));
            let _ = this.update_in(cx, |this, window, cx| {
                if !this.voiceover_matches(&ticket) {
                    return;
                }
                let panel = this.voiceover.panel.as_mut().unwrap();
                match result {
                    Ok(Some(take)) => {
                        panel.ready = Some(take);
                        panel.phase = Phase::Ready;
                    }
                    Ok(None) => {
                        if panel.close_after_cancel {
                            this.voiceover.panel = None;
                        } else {
                            panel.phase = Phase::Idle;
                            panel.status = None;
                        }
                    }
                    Err(error) => {
                        panel.phase = Phase::Error;
                        panel.error = Some(error);
                    }
                }
                this.reload_voiceover_takes(window, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn attach_voiceover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = self.voiceover.panel.as_ref() else {
            return;
        };
        if panel.phase != Phase::Ready || !self.voiceover_matches(&panel.ticket) {
            return;
        }
        let Some(mut take) = panel.ready.clone() else {
            return;
        };
        let Some((lane, range)) = audio_destination(
            &self.project,
            &take.relative_path(),
            take.manifest().lane,
            self.tracks.audio,
            take.manifest().output_start,
            self.total_duration(),
            take.audio().duration,
        ) else {
            self.voiceover.panel.as_mut().unwrap().error =
                Some(tr::editor::voiceover::move_playhead().into());
            cx.notify();
            return;
        };
        let (start, end) = (range.start, range.end);
        let gain = if panel.normalize {
            take.audio().suggested_gain_db().unwrap_or(0.0)
        } else {
            0.0
        };
        let result = (|| -> Result<(), String> {
            if !take.is_in_project(&self.project) {
                self.commit_project_audio_asset(
                    crate::asset_library::AssetRecord {
                        path: take.relative_path(),
                        name: tr::editor::voiceover::asset_name(clock(start)).into(),
                        kind: crate::asset_library::AssetKind::Audio,
                        source: None,
                        rendition: None,
                    },
                    take.audio().duration,
                    start,
                    Some(lane),
                    gain,
                    window,
                    cx,
                )?;
            }
            self.flush_pending_saves(cx)?;
            take.mark_attached().map_err(|error| format!("{error:#}"))
        })();
        let panel = self.voiceover.panel.as_mut().unwrap();
        panel.ready = Some(take);
        match result {
            Ok(()) => {
                panel.phase = Phase::Attached;
                panel.placement = Placement { start, lane };
                panel.error = None;
                self.seek_to_time(start, cx);
                self.reveal_timeline_range(start, end, cx);
                self.reveal_timeline_row(crate::editor_timeline::TrackKind::Audio, lane, cx);
            }
            Err(error) => panel.error = Some(error),
        }
        cx.notify();
    }

    fn reload_voiceover_takes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ticket) = self
            .voiceover
            .panel
            .as_ref()
            .map(|panel| panel.ticket.clone())
        else {
            return;
        };
        cx.spawn_in(window, async move |this, cx: &mut AsyncWindowContext| {
            let project = ticket.project.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    voiceover_recording::list_takes(&project).map_err(|error| error.to_string())
                })
                .await;
            let _ = this.update_in(cx, |this, _, cx| {
                if !this.voiceover_matches(&ticket) {
                    return;
                }
                let panel = this.voiceover.panel.as_mut().unwrap();
                match result {
                    Ok(takes) => panel.takes = takes,
                    Err(error) => panel.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn recover_voiceover(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = self.voiceover.panel.as_mut() else {
            return;
        };
        if panel.phase.busy() {
            return;
        }
        self.voiceover.generation = self.voiceover.generation.wrapping_add(1);
        panel.ticket.generation = self.voiceover.generation;
        panel.phase = Phase::Recovering;
        panel.error = None;
        let ticket = panel.ticket.clone();
        cx.spawn_in(window, async move |this, cx: &mut AsyncWindowContext| {
            let project = ticket.project.clone();
            let result = gpui_tokio::Tokio::spawn(cx, async move {
                voiceover_recording::recover_take(project, id)
                    .await
                    .map_err(|error| format!("{error:#}"))
            })
            .await
            .unwrap_or_else(|error| Err(error.to_string()));
            let _ = this.update_in(cx, |this, window, cx| {
                if !this.voiceover_matches(&ticket) {
                    return;
                }
                let panel = this.voiceover.panel.as_mut().unwrap();
                match result {
                    Ok(take) => {
                        panel.ready = Some(take);
                        panel.phase = Phase::Ready;
                        this.attach_voiceover(window, cx);
                    }
                    Err(error) => {
                        panel.error = Some(error);
                        panel.phase = Phase::Error;
                    }
                }
                this.reload_voiceover_takes(window, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn close_voiceover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.library_audition = None;
        let Some(panel) = self.voiceover.panel.as_ref() else {
            return;
        };
        if panel.confirming
            || matches!(
                panel.phase,
                Phase::Finalizing | Phase::SourceFinalizing | Phase::Recovering
            )
        {
            return;
        }
        if !panel.phase.busy() {
            self.voiceover.panel = None;
            self.focus_root(window, cx);
            cx.notify();
            return;
        }
        let ticket = panel.ticket.clone();
        self.voiceover.panel.as_mut().unwrap().confirming = true;
        #[cfg(target_os = "linux")]
        let answer = crate::editor_modal::confirm_action(
            tr::editor::voiceover::cancel_title(),
            tr::editor::voiceover::cancel_message(),
            tr::editor::voiceover::discard(),
            tr::editor::voiceover::keep_recording(),
            window,
            cx,
        );
        cx.spawn_in(window, async move |this, cx: &mut AsyncWindowContext| {
            #[cfg(target_os = "linux")]
            let confirmed = answer.await;
            #[cfg(not(target_os = "linux"))]
            let confirmed = crate::platform::confirm_dialog(
                tr::editor::voiceover::cancel_title(),
                tr::editor::voiceover::cancel_message(),
                tr::editor::voiceover::discard(),
                tr::editor::voiceover::keep_recording(),
                false,
            );
            let _ = this.update_in(cx, |this, window, cx| {
                if !this.voiceover_matches(&ticket) {
                    return;
                }
                this.voiceover.panel.as_mut().unwrap().confirming = false;
                if confirmed {
                    this.voiceover.panel.as_mut().unwrap().close_after_cancel = true;
                    this.finish_voiceover(true, window, cx);
                } else if this
                    .voiceover
                    .panel
                    .as_ref()
                    .is_some_and(|panel| panel.phase == Phase::SourceFinalizing)
                {
                    this.finish_voiceover(false, window, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn render_voiceover(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let panel = self.voiceover.panel.as_ref()?;
        let theme = self.theme;
        let busy = panel.phase.busy();
        let elapsed = panel
            .ready
            .as_ref()
            .map(|take| take.audio().duration)
            .unwrap_or_else(|| {
                panel
                    .status
                    .as_ref()
                    .map_or(0., |status| status.duration_estimate.as_secs_f64())
            });
        let (start, lane) = if panel.phase == Phase::Attached {
            (panel.placement.start, panel.placement.lane)
        } else {
            panel
                .ready
                .as_ref()
                .map_or((panel.placement.start, panel.placement.lane), |take| {
                    (take.manifest().output_start, take.manifest().lane)
                })
        };
        let phase = match panel.phase {
            Phase::Idle => tr::editor::voiceover::ready(),
            Phase::Starting => tr::editor::voiceover::preparing(),
            Phase::Recording
                if panel
                    .status
                    .as_ref()
                    .is_none_or(|status| !status.first_sample_seen) =>
            {
                tr::editor::voiceover::preparing()
            }
            Phase::Recording => tr::editor::voiceover::recording(),
            Phase::Paused => tr::editor::voiceover::paused(),
            Phase::Finalizing | Phase::SourceFinalizing => tr::editor::voiceover::saving(),
            Phase::Recovering => tr::editor::voiceover::recovering(),
            Phase::Ready => tr::editor::voiceover::save_pending(),
            Phase::Attached => tr::editor::voiceover::added(),
            Phase::Error => tr::editor::voiceover::stopped(),
        };
        let takes = panel
            .takes
            .iter()
            .filter(|take| take.availability != TakeAvailability::Attached)
            .collect::<Vec<_>>();
        let mut content = div()
            .id("voiceover-panel")
            .occlude()
            .flex()
            .flex_col()
            .w(px(440.))
            .max_w(gpui::relative(0.94))
            .max_h(gpui::relative(0.9))
            .overflow_y_scroll()
            .p(px(24.))
            .gap(px(14.))
            .rounded(px(16.))
            .bg(Hsla::from(theme.editor.card))
            .text_color(Hsla::from(theme.gray_12))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(17.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(tr::editor::voiceover::title()),
                    )
                    .child(
                        ui::EditorButton::plain(&theme, "voiceover-close")
                            .label(tr::common::close())
                            .disabled(matches!(
                                panel.phase,
                                Phase::Finalizing | Phase::SourceFinalizing | Phase::Recovering
                            ))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.close_voiceover(window, cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(Hsla::from(theme.gray_10))
                    .child(tr::editor::voiceover::hint()),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .child(tr::editor::voiceover::position(clock(start), lane + 1)),
            )
            .child(
                ui::Select::plain(
                    &theme,
                    "voiceover-microphone",
                    panel
                        .selected
                        .clone()
                        .unwrap_or_else(|| tr::editor::voiceover::choose_microphone().into()),
                )
                .stretch_label()
                .disabled(busy || panel.ready.is_some())
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(panel) = this.voiceover.panel.as_mut() {
                        panel.device_menu = !panel.device_menu;
                    }
                    cx.notify();
                })),
            )
            .child(panel.meter.clone());
        #[cfg(target_os = "macos")]
        if matches!(panel.phase, Phase::Idle | Phase::Error) {
            content = content.child(
                ui::EditorButton::plain(&theme, "voiceover-input-settings")
                    .tinted(&theme)
                    .label(tr::editor::voiceover::input_settings())
                    .on_click(|_, _, cx| {
                        cx.open_url("x-apple.systempreferences:com.apple.Sound-Settings.extension")
                    }),
            );
        }
        if panel.phase == Phase::Ready {
            content = content.child(
                div()
                    .text_size(px(12.))
                    .child(tr::editor::voiceover::review_hint()),
            );
            if let Some(gain) = panel
                .ready
                .as_ref()
                .and_then(|take| take.audio().suggested_gain_db())
            {
                content = content.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(format!(
                            "{} ({gain:+.1} dB)",
                            tr::editor::voiceover::normalize()
                        ))
                        .child(
                            ui::Toggle::plain(&theme, "voiceover-normalize", panel.normalize)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.library_audition = None;
                                    if let Some(panel) = this.voiceover.panel.as_mut() {
                                        panel.normalize = !panel.normalize;
                                    }
                                    cx.notify();
                                })),
                        ),
                );
            }
        }
        if panel.device_menu {
            content = content.child(
                div()
                    .id("voiceover-devices")
                    .max_h(px(150.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .children(panel.devices.iter().enumerate().map(|(index, name)| {
                        let name = name.clone();
                        ui::EditorButton::plain(
                            &theme,
                            SharedString::from(format!("voiceover-mic-{index}")),
                        )
                        .label(name.clone())
                        .pressed(panel.selected.as_ref() == Some(&name))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(panel) = this.voiceover.panel.as_mut() {
                                panel.selected = Some(name.clone());
                                panel.device_menu = false;
                                panel.error = None;
                            }
                            Feeds::global(cx).update(cx, |feeds, cx| {
                                feeds.set_microphone(Some(name.clone()), cx)
                            });
                            cx.notify();
                        }))
                    })),
            );
        }
        content = content.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .py(px(10.))
                .child(div().text_size(px(14.)).child(phase))
                .child(
                    div()
                        .text_size(px(26.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(clock(elapsed)),
                ),
        );
        if matches!(
            panel.phase,
            Phase::Finalizing | Phase::SourceFinalizing | Phase::Recovering
        ) {
            content = content.child(
                div()
                    .text_size(px(12.))
                    .text_color(Hsla::from(theme.gray_10))
                    .child(tr::editor::voiceover::wait_hint()),
            );
        }
        if let Some(error) = &panel.error {
            content = content.child(
                div()
                    .text_size(px(12.))
                    .text_color(Hsla::from(theme.gray_11))
                    .child(error.clone()),
            );
        }
        if self.audio_library.audition_failed {
            content = content.child(
                div()
                    .text_size(px(12.))
                    .child(tr::editor::audio::library::audition_failed()),
            );
        }
        let actions = div().flex().items_center().gap(px(8.));
        content = content.child(match panel.phase {
            Phase::Idle | Phase::Error => actions.child(
                ui::EditorButton::plain(&theme, "voiceover-start")
                    .filled(&theme)
                    .grow()
                    .height(px(36.))
                    .left_icon("icons/microphone.svg")
                    .label(tr::editor::voiceover::start())
                    .disabled(panel.selected.is_none() || panel.ready.is_some())
                    .on_click(cx.listener(|this, _, window, cx| this.start_voiceover(window, cx))),
            ),
            Phase::Recording | Phase::Paused => actions
                .child(
                    ui::EditorButton::plain(&theme, "voiceover-pause")
                        .label(if panel.phase == Phase::Paused {
                            tr::editor::voiceover::resume()
                        } else {
                            tr::editor::voiceover::pause()
                        })
                        .disabled(panel.pause_pending)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.pause_voiceover(window, cx)),
                        ),
                )
                .child(
                    ui::EditorButton::plain(&theme, "voiceover-finish")
                        .filled(&theme)
                        .height(px(36.))
                        .label(tr::editor::voiceover::finish())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.finish_voiceover(false, window, cx)
                        })),
                )
                .child(
                    ui::EditorButton::plain(&theme, "voiceover-discard")
                        .text(&theme)
                        .label(tr::editor::voiceover::discard())
                        .on_click(
                            cx.listener(|this, _, window, cx| this.close_voiceover(window, cx)),
                        ),
                ),
            Phase::Ready => actions.child(
                ui::EditorButton::plain(&theme, "voiceover-attach")
                    .filled(&theme)
                    .height(px(36.))
                    .label(tr::editor::voiceover::add_to_timeline())
                    .on_click(cx.listener(|this, _, window, cx| this.attach_voiceover(window, cx))),
            ),
            Phase::Attached => actions.child(
                ui::EditorButton::plain(&theme, "voiceover-play")
                    .label(if self.playing {
                        tr::editor::voiceover::stop_playback()
                    } else {
                        tr::editor::voiceover::play_here()
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        if !this.playing
                            && let Some(start) = this
                                .voiceover
                                .panel
                                .as_ref()
                                .map(|panel| panel.placement.start)
                        {
                            this.seek_to_time(start, cx);
                        }
                        this.toggle_play(window, cx);
                    })),
            ),
            Phase::Starting => actions.child(
                ui::EditorButton::plain(&theme, "voiceover-cancel-start")
                    .label(tr::common::cancel())
                    .on_click(cx.listener(|this, _, window, cx| this.close_voiceover(window, cx))),
            ),
            _ => actions,
        });
        if matches!(panel.phase, Phase::Ready | Phase::Attached) && panel.ready.is_some() {
            let auditioning = self
                .library_audition
                .as_ref()
                .is_some_and(|audition| audition.id == "voiceover-take");
            content = content.child(
                ui::EditorButton::plain(&theme, "voiceover-audition")
                    .label(if auditioning {
                        tr::editor::voiceover::stop_playback()
                    } else {
                        tr::editor::voiceover::audition_take()
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        let path = this
                            .voiceover
                            .panel
                            .as_ref()
                            .and_then(|panel| panel.ready.as_ref())
                            .map(|take| take.path().to_path_buf());
                        if let Some(path) = path {
                            let gain = this
                                .voiceover
                                .panel
                                .as_ref()
                                .filter(|panel| panel.normalize)
                                .and_then(|panel| panel.ready.as_ref())
                                .and_then(|take| take.audio().suggested_gain_db())
                                .unwrap_or(0.0);
                            this.toggle_audio_audition_with_gain("voiceover-take", path, gain, cx);
                        }
                    })),
            );
        }
        if !takes.is_empty() && !busy {
            content = content.child(
                ui::EditorButton::plain(&theme, "voiceover-recovery-toggle")
                    .text(&theme)
                    .label(tr::editor::voiceover::recover_count(takes.len()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(panel) = this.voiceover.panel.as_mut() {
                            panel.recovery_open = !panel.recovery_open;
                        }
                        cx.notify();
                    })),
            );
            if panel.recovery_open {
                content = content.child(div().flex().flex_col().gap(px(8.)).children(
                    takes.into_iter().enumerate().map(|(index, take)| {
                        let label = take.manifest.as_ref().map_or_else(
                            || tr::editor::voiceover::unknown_take().into(),
                            |manifest| {
                                tr::editor::voiceover::asset_name(clock(manifest.output_start))
                            },
                        );
                        let id = take.manifest.as_ref().map(|manifest| manifest.id);
                        let recoverable = matches!(
                            take.availability,
                            TakeAvailability::Ready | TakeAvailability::Interrupted
                        );
                        let state = match take.availability {
                            TakeAvailability::Active => tr::editor::voiceover::take_active(),
                            TakeAvailability::Missing => tr::editor::voiceover::take_missing(),
                            TakeAvailability::Empty => tr::editor::voiceover::take_empty(),
                            TakeAvailability::Invalid => tr::editor::voiceover::take_invalid(),
                            TakeAvailability::Cancelled => tr::editor::voiceover::take_cancelled(),
                            _ => tr::editor::voiceover::recover(),
                        };
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.))
                            .child(div().text_size(px(12.)).child(label))
                            .child(
                                ui::EditorButton::plain(
                                    &theme,
                                    SharedString::from(format!("voiceover-recover-{index}")),
                                )
                                .label(state)
                                .disabled(!recoverable)
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        if let Some(id) = id {
                                            this.recover_voiceover(id, window, cx);
                                        }
                                    },
                                )),
                            )
                    }),
                ));
            }
        }
        Some(
            div()
                .id("voiceover-overlay")
                .occlude()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::black().opacity(0.24))
                .child(content)
                .into_any_element(),
        )
    }
}

fn clock(seconds: f64) -> String {
    let value = if seconds.is_finite() {
        seconds.max(0.)
    } else {
        0.
    };
    let minutes = (value / 60.).floor() as u64;
    format!("{minutes:02}:{:05.2}", value % 60.)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voiceover_placement_uses_output_seconds_and_reserves_an_unoccupied_lane() {
        let project: cap_project::ProjectConfiguration = serde_json::from_value(serde_json::json!({
            "timeline": { "segments": [{"recordingSegment":0,"start":0.,"end":20.,"timescale":1.25}], "zoomSegments": [],
                "audioSegments": [{"start":8.,"end":12.,"track":1,"path":"assets/audio/music.mp3"}] }
        })).unwrap();
        let plan = placement(&project, 5.25, 16., 1, 2).unwrap();
        assert_eq!(
            plan,
            Placement {
                start: 5.25,
                lane: 2,
            }
        );
        assert_eq!(placement(&project, 16., 16., 1, 2).unwrap().start, 16.);
        assert!(placement(&project, f64::NAN, 16., 1, 2).is_none());
        assert_eq!(placement(&project, 12., 16., 1, 2).unwrap().lane, 1);
    }

    #[test]
    fn voiceover_finalization_and_recovery_keep_the_project_locked() {
        for phase in [
            Phase::Starting,
            Phase::Recording,
            Phase::Paused,
            Phase::Finalizing,
            Phase::Recovering,
        ] {
            assert!(phase.busy());
        }
        for phase in [Phase::Idle, Phase::Ready, Phase::Attached, Phase::Error] {
            assert!(!phase.busy());
        }
    }
    #[test]
    fn voiceover_source_finalization_remains_locked_and_observed_while_user_stop_owns_its_reply() {
        let mut phase = Phase::Recording;
        assert_eq!(
            observe_recording_phase(&mut phase, TakeState::Finalizing),
            StatusAction::Continue
        );
        assert_eq!(phase, Phase::SourceFinalizing);
        assert!(phase.busy());
        assert_eq!(
            observe_recording_phase(&mut phase, TakeState::Ready),
            StatusAction::Recover
        );
        assert_eq!(phase, Phase::Ready);
        let mut phase = Phase::Paused;
        assert_eq!(
            observe_recording_phase(&mut phase, TakeState::Finalizing),
            StatusAction::Continue
        );
        assert_eq!(
            observe_recording_phase(&mut phase, TakeState::Failed),
            StatusAction::Failed
        );
        assert_eq!(phase, Phase::Error);
        let mut phase = Phase::Finalizing;
        assert_eq!(
            observe_recording_phase(&mut phase, TakeState::Ready),
            StatusAction::Ignore
        );
        assert_eq!(phase, Phase::Finalizing);
        assert!(phase.busy());
    }

    #[test]
    fn voiceover_recovery_moves_off_occupied_lane_and_save_retry_keeps_existing_clip() {
        let mut project: cap_project::ProjectConfiguration = serde_json::from_value(serde_json::json!({
            "timeline": { "segments": [{"recordingSegment":0,"start":0.,"end":20.,"timescale":1.25}], "zoomSegments": [],
                "audioSegments": [{"start":0.,"end":16.,"track":1,"path":"assets/audio/music.mp3"}] }
        })).unwrap();
        let path = "content/voiceovers/recovery/capture.m4a";
        let (lane, range) = audio_destination(&project, path, 1, 2, 5.25, 16., 2.).unwrap();
        assert_eq!((lane, range.clone()), (2, 5.25..7.25));
        project.timeline.as_mut().unwrap().audio_segments.push(
            crate::editor_edits::default_audio_segment(
                range.start,
                range.end,
                lane,
                path.into(),
                "Voiceover".into(),
                Some(2.),
            ),
        );
        let segment = project
            .timeline
            .as_mut()
            .unwrap()
            .audio_segments
            .last_mut()
            .unwrap();
        segment.track = 4;
        segment.start = 8.;
        segment.end = 10.;
        let snapshot = serde_json::to_value(&project).unwrap();
        assert_eq!(
            audio_destination(&project, path, 1, 5, 5.25, 16., 2.).unwrap(),
            (4, 8.0..10.0)
        );
        assert_eq!(serde_json::to_value(&project).unwrap(), snapshot);
        assert_eq!(project.timeline.as_ref().unwrap().audio_segments.len(), 2);
    }
}
