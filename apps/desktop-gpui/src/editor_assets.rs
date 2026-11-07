use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};

use gpui::{
    AnyElement, AppContext as _, Bounds, Context, CursorStyle, Entity, FontWeight, Hsla,
    InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, Pixels, Point, RenderImage, StatefulInteractiveElement as _, Styled,
    StyledImage as _, Window, div, img, point, prelude::FluentBuilder, px, size, svg,
};

use crate::{
    asset_library::{self, AssetKind, AssetRecord, StagedMedia},
    editor_timeline::{self as timeline, TrackKind, TrackRow, Transform},
    editor_window::EditorWindow,
    i18n::tr,
    stock_media::{
        Asset, Credentials, DownloadOutcome, DownloadedMedia, MediaKind, Orientation, Provider,
        SearchRequest, StockError, StockMediaClient,
    },
    ui,
};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum AssetSource {
    #[default]
    Project,
    Local,
    Online,
}

#[derive(Clone)]
struct ImportIntent {
    project: PathBuf,
    time: f64,
    timeline: String,
    metadata: std::fs::Metadata,
    lane: Option<AssetLaneIntent>,
}

#[derive(Clone)]
struct AssetLaneIntent {
    lane: u32,
    identity: String,
}

struct AssetDrag {
    record: AssetRecord,
    intent: ImportIntent,
    generation: u64,
    press: Point<Pixels>,
    pointer: Point<Pixels>,
    active: bool,
    duration: Option<f64>,
    probe_started: bool,
    error: Option<String>,
}

pub(crate) struct AssetDropGeometry<'a> {
    pub bounds: Bounds<Pixels>,
    pub scroll_y: Pixels,
    pub row_height: f32,
    pub rows: &'a [TrackRow],
    pub viewport_width: f32,
    pub transform: Transform,
    pub total: f64,
}

#[derive(Clone, Copy)]
struct AssetDropLocation {
    lane: u32,
    time: f64,
    bounds: Bounds<Pixels>,
}

fn asset_track(kind: AssetKind) -> Option<TrackKind> {
    match kind {
        AssetKind::Photo => Some(TrackKind::Image),
        AssetKind::Audio => Some(TrackKind::Audio),
        AssetKind::Video => None,
    }
}

fn asset_drag_threshold(press: Point<Pixels>, pointer: Point<Pixels>) -> bool {
    let x = f32::from(pointer.x - press.x);
    let y = f32::from(pointer.y - press.y);
    x.is_finite() && y.is_finite() && x * x + y * y >= 25.
}

fn asset_drop_location(
    geometry: &AssetDropGeometry<'_>,
    kind: AssetKind,
    pointer: Point<Pixels>,
) -> Option<AssetDropLocation> {
    let (x, y) = (f32::from(pointer.x), f32::from(pointer.y));
    let bounds = geometry.bounds;
    if !x.is_finite()
        || !y.is_finite()
        || x < f32::from(bounds.left()).max(timeline::content_left())
        || x >= f32::from(bounds.right())
            .min(timeline::content_left() + timeline::content_width(geometry.viewport_width))
        || y < f32::from(bounds.top())
        || y >= f32::from(bounds.bottom())
        || !geometry.row_height.is_finite()
        || geometry.row_height <= 0.
        || !geometry.total.is_finite()
        || geometry.total <= 0.
        || !geometry.transform.zoom.is_finite()
        || geometry.transform.zoom <= 0.
        || !geometry.transform.position.is_finite()
    {
        return None;
    }
    let content_y = y - f32::from(bounds.top()) - f32::from(geometry.scroll_y);
    if content_y < 0. {
        return None;
    }
    let stride = geometry.row_height + timeline::TRACK_ROW_GAP;
    let index = (content_y / stride).floor() as usize;
    if content_y - index as f32 * stride >= geometry.row_height {
        return None;
    }
    let row = geometry.rows.get(index)?;
    if !asset_track(kind).is_some_and(|track| track.same_family(row.kind)) {
        return None;
    }
    let time = timeline::preview_time_from_x(x, geometry.viewport_width, geometry.transform)?;
    if !time.is_finite() || time < 0. {
        return None;
    }
    let top = (f32::from(bounds.top()) + f32::from(geometry.scroll_y) + index as f32 * stride)
        .max(f32::from(bounds.top()));
    let bottom = (f32::from(bounds.top())
        + f32::from(geometry.scroll_y)
        + index as f32 * stride
        + geometry.row_height)
        .min(f32::from(bounds.bottom()));
    Some(AssetDropLocation {
        lane: row.lane,
        time,
        bounds: Bounds {
            origin: point(px(timeline::content_left()), px(top)),
            size: size(
                px(timeline::content_width(geometry.viewport_width)),
                px(bottom - top),
            ),
        },
    })
}

fn exact_asset_drop_range(
    project: &cap_project::ProjectConfiguration,
    kind: AssetKind,
    lane: u32,
    time: f64,
    total: f64,
    duration: f64,
) -> Option<std::ops::Range<f64>> {
    let timeline = project.timeline.as_ref()?;
    let (_, range) = match kind {
        AssetKind::Photo => {
            let (placed, start, end) = asset_library::image_placement(timeline, lane, time, total)?;
            (placed, start..end)
        }
        AssetKind::Audio => crate::editor_audio::audio_import_placement(
            &timeline.audio_segments,
            lane,
            lane.saturating_add(1),
            time,
            total,
            duration,
        )?,
        AssetKind::Video => return None,
    };
    Some(range)
}

fn asset_lane_identity(
    project: &cap_project::ProjectConfiguration,
    kind: AssetKind,
    lane: u32,
    minimum: u32,
) -> String {
    let content = project.timeline.as_ref().map(|timeline| match kind {
        AssetKind::Photo => serde_json::to_value(
            timeline
                .overlay_spans()
                .filter(|(_, _, track, _, _)| *track == lane)
                .map(|(_, index, _, start, end)| (index, start, end))
                .collect::<Vec<_>>(),
        ),
        AssetKind::Audio => serde_json::to_value(
            timeline
                .audio_segments
                .iter()
                .filter(|segment| segment.track == lane)
                .collect::<Vec<_>>(),
        ),
        AssetKind::Video => Ok(serde_json::Value::Null),
    });
    serde_json::to_string(&(minimum, content.and_then(Result::ok))).unwrap_or_default()
}

pub(crate) struct AssetBrowserSnapshot {
    open: bool,
    source: AssetSource,
    provider: Provider,
    kind: MediaKind,
    draft: String,
    submitted_query: String,
    searched: bool,
    page: u32,
    next_page: Option<u32>,
    results: Vec<Asset>,
    thumbnails: HashMap<String, Arc<RenderImage>>,
    thumbnail_errors: HashMap<String, (String, Option<Instant>)>,
    retry_after: HashMap<Provider, Instant>,
    selected: Option<usize>,
    rendition: usize,
}

pub(crate) struct AssetLibraryState {
    pub source: AssetSource,
    provider: Provider,
    kind: MediaKind,
    input: Option<Entity<ui::TextInputState>>,
    submitted_query: String,
    client: Option<Arc<StockMediaClient>>,
    generation: u64,
    search_cancel: Option<tokio::sync::oneshot::Sender<()>>,
    searching: bool,
    searched: bool,
    page: u32,
    next_page: Option<u32>,
    results: Vec<Asset>,
    thumbnails: HashMap<String, Arc<RenderImage>>,
    thumbnail_errors: HashMap<String, (String, Option<Instant>)>,
    thumbnail_cancel: HashMap<String, tokio::sync::oneshot::Sender<()>>,
    retry_after: HashMap<Provider, Instant>,
    retry_ticking: bool,
    selected: Option<usize>,
    rendition: usize,
    download_generation: u64,
    download_cancel: Option<tokio::sync::oneshot::Sender<()>>,
    importing: bool,
    downloading: bool,
    progress: (u64, Option<u64>),
    tracking: Option<(DownloadedMedia, ImportIntent, usize)>,
    error: Option<String>,
    records: Vec<AssetRecord>,
    records_generation: u64,
    invalid_record_metadata: bool,
    pending_restore: Option<AssetBrowserSnapshot>,
    drag: Option<AssetDrag>,
}

impl Default for AssetLibraryState {
    fn default() -> Self {
        Self {
            source: AssetSource::Project,
            provider: Provider::Unsplash,
            kind: MediaKind::Photo,
            input: None,
            submitted_query: String::new(),
            client: None,
            generation: 0,
            search_cancel: None,
            searching: false,
            searched: false,
            page: 1,
            next_page: None,
            results: Vec::new(),
            thumbnails: HashMap::new(),
            thumbnail_errors: HashMap::new(),
            thumbnail_cancel: HashMap::new(),
            retry_after: HashMap::new(),
            retry_ticking: false,
            selected: None,
            rendition: 0,
            download_generation: 0,
            download_cancel: None,
            importing: false,
            downloading: false,
            progress: (0, None),
            tracking: None,
            error: None,
            records: Vec::new(),
            records_generation: 0,
            invalid_record_metadata: false,
            pending_restore: None,
            drag: None,
        }
    }
}

impl AssetLibraryState {
    fn retry_seconds(&self, provider: Provider, now: Instant) -> u64 {
        seconds_remaining(self.retry_after.get(&provider).copied(), now)
    }

    fn wait_before_retry(&mut self, provider: Provider, seconds: u64, now: Instant) {
        if seconds > 0 {
            let until = now + Duration::from_secs(seconds.min(86_400));
            self.retry_after
                .entry(provider)
                .and_modify(|old| *old = (*old).max(until))
                .or_insert(until);
        }
    }

    fn thumbnail_matches(&self, generation: u64, asset: &Asset) -> bool {
        self.source == AssetSource::Online
            && self.generation == generation
            && self.results.iter().any(|current| {
                current.provider == asset.provider
                    && current.kind == asset.kind
                    && current.id == asset.id
                    && current.thumbnail_url == asset.thumbnail_url
            })
    }

    fn finish_thumbnail(
        &mut self,
        generation: u64,
        asset: &Asset,
        result: Result<Arc<RenderImage>, StockError>,
        now: Instant,
    ) -> bool {
        if !self.thumbnail_matches(generation, asset) {
            return false;
        }
        let key = asset_key(asset);
        let _ = self.thumbnail_cancel.remove(&key);
        match result {
            Ok(image) => {
                let _ = self.thumbnail_errors.remove(&key);
                let _ = self.thumbnails.insert(key, image);
            }
            Err(error) => {
                let until = error
                    .retry_wait_seconds()
                    .map(|seconds| now + Duration::from_secs(seconds));
                let _ = self
                    .thumbnail_errors
                    .insert(key, (stock_error(&error), until));
            }
        }
        true
    }

    fn complete_drag_probe(&mut self, generation: u64, result: Result<f64, String>) -> bool {
        if self.download_generation != generation {
            return false;
        }
        let Some(drag) = self
            .drag
            .as_mut()
            .filter(|drag| drag.generation == generation)
        else {
            return false;
        };
        match result {
            Ok(duration) => drag.duration = Some(duration),
            Err(error) => drag.error = Some(error),
        }
        true
    }

    fn browser_snapshot(&mut self, open: bool, draft: String) -> AssetBrowserSnapshot {
        self.cancel();
        self.records_generation = self.records_generation.wrapping_add(1);
        AssetBrowserSnapshot {
            open,
            source: self.source,
            provider: self.provider,
            kind: self.kind,
            draft,
            submitted_query: self.submitted_query.clone(),
            searched: self.searched,
            page: self.page,
            next_page: self.next_page,
            results: self.results.clone(),
            thumbnails: self.thumbnails.clone(),
            thumbnail_errors: self.thumbnail_errors.clone(),
            retry_after: self.retry_after.clone(),
            selected: self.selected,
            rendition: self.rendition,
        }
    }

    fn restore_browser(&mut self, snapshot: AssetBrowserSnapshot) -> (bool, String) {
        self.cancel();
        self.source = snapshot.source;
        self.provider = snapshot.provider;
        self.kind = snapshot.kind;
        self.submitted_query = snapshot.submitted_query;
        self.searched = snapshot.searched;
        self.page = snapshot.page;
        self.next_page = snapshot.next_page;
        self.results = snapshot.results;
        self.thumbnails = snapshot.thumbnails;
        self.thumbnail_errors = snapshot.thumbnail_errors;
        self.retry_after = snapshot.retry_after;
        self.selected = snapshot
            .selected
            .filter(|index| *index < self.results.len());
        self.rendition = self
            .selected
            .and_then(|index| self.results.get(index))
            .filter(|asset| snapshot.rendition < asset.renditions.len())
            .map_or(0, |_| snapshot.rendition);
        self.client = None;
        self.progress = (0, None);
        self.error = None;
        self.records.clear();
        self.records_generation = self.records_generation.wrapping_add(1);
        self.invalid_record_metadata = false;
        self.pending_restore = None;
        (snapshot.open, snapshot.draft)
    }

    fn cancel_search(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if let Some(cancel) = self.search_cancel.take() {
            let _ = cancel.send(());
        }
        for (_, cancel) in self.thumbnail_cancel.drain() {
            let _ = cancel.send(());
        }
        self.searching = false;
    }

    pub(crate) fn cancel(&mut self) {
        self.cancel_search();
        self.cancel_import();
    }

    fn cancel_import(&mut self) {
        self.download_generation = self.download_generation.wrapping_add(1);
        if let Some(cancel) = self.download_cancel.take() {
            let _ = cancel.send(());
        }
        self.importing = false;
        self.downloading = false;
        self.tracking = None;
        self.drag = None;
    }
}

impl Drop for AssetLibraryState {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn provider_id(provider: Provider) -> &'static str {
    match provider {
        Provider::Unsplash => "unsplash",
        Provider::Pixabay => "pixabay",
    }
}

fn provider_name(provider: Provider) -> &'static str {
    match provider {
        Provider::Unsplash => "Unsplash",
        Provider::Pixabay => "Pixabay",
    }
}

fn asset_key(asset: &Asset) -> String {
    format!("{}:{}", provider_id(asset.provider), asset.id)
}

fn seconds_remaining(until: Option<Instant>, now: Instant) -> u64 {
    until.map_or(0, |until| {
        until.saturating_duration_since(now).as_secs_f64().ceil() as u64
    })
}

fn page_retry_seconds(page: &crate::stock_media::SearchPage) -> Option<u64> {
    (!page.from_cache && page.provider == Provider::Pixabay && page.rate_limit.remaining == Some(0))
        .then_some(page.rate_limit.reset_seconds)
        .flatten()
}

static STOCK_KEYCHAIN_READ: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(1)));

fn cached_credentials(provider: Provider) -> Option<bool> {
    crate::ai_store::key_available_cached(crate::ai_store::Kind::Stock, provider_id(provider))
}

fn credentials_for(provider: Provider, key: &str) -> Credentials<'_> {
    match provider {
        Provider::Unsplash => Credentials {
            unsplash: Some(key),
            pixabay: None,
        },
        Provider::Pixabay => Credentials {
            unsplash: None,
            pixabay: Some(key),
        },
    }
}

async fn stock_key(provider: Provider) -> Result<String, StockError> {
    read_stock_key_with(provider, STOCK_KEYCHAIN_READ.clone(), |provider| {
        crate::ai_store::key(crate::ai_store::Kind::Stock, provider_id(provider))
    })
    .await
}

async fn read_stock_key_with(
    provider: Provider,
    gate: Arc<tokio::sync::Semaphore>,
    read: impl FnOnce(Provider) -> Option<String> + Send + 'static,
) -> Result<String, StockError> {
    let permit = gate
        .acquire_owned()
        .await
        .map_err(|_| StockError::Credentials)?;
    tokio::task::spawn_blocking(move || {
        // A cancelled request cannot interrupt the system authorization prompt.
        let _permit = permit;
        read(provider).ok_or(StockError::Credentials)
    })
    .await
    .map_err(|_| StockError::Network)?
}

async fn stock_request<T, F, R>(
    cancelled: &mut tokio::sync::oneshot::Receiver<()>,
    key: impl std::future::Future<Output = Result<String, StockError>>,
    request: F,
) -> Option<Result<T, StockError>>
where
    F: FnOnce(String) -> R,
    R: std::future::Future<Output = Result<T, StockError>>,
{
    tokio::select! {
        biased;
        _ = cancelled => None,
        result = async { request(key.await?).await } => Some(result),
    }
}

fn stock_error(error: &StockError) -> String {
    match error {
        StockError::Credentials => tr::editor::assets::credentials().into(),
        StockError::Http {
            status: 401 | 403, ..
        } => tr::editor::assets::credentials().into(),
        StockError::Http { status: 429, .. } => tr::editor::assets::rate_limit().into(),
        StockError::TooLarge => tr::editor::assets::too_large().into(),
        StockError::Tracking(cause) => format!(
            "{}\n{}",
            tr::editor::assets::tracking_failed(),
            stock_error(cause)
        ),
        StockError::Network | StockError::Timeout => tr::editor::assets::network_error().into(),
        StockError::Cache => tr::editor::assets::cache_error().into(),
        _ => tr::editor::assets::invalid_media().into(),
    }
}

fn timeline_identity(project: &cap_project::ProjectConfiguration) -> String {
    serde_json::to_string(
        &project
            .timeline
            .as_ref()
            .map(|timeline| (&timeline.segments, timeline.hold_windows())),
    )
    .unwrap_or_default()
}

fn same_recording(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != after.dev() || before.ino() != after.ino() {
            return false;
        }
    }
    before.len() == after.len() && before.modified().ok() == after.modified().ok()
}

impl EditorWindow {
    pub(crate) fn asset_dragging(&self) -> bool {
        self.assets.drag.is_some()
    }

    pub(crate) fn cancel_asset_drag(&mut self, cx: &mut Context<Self>) {
        if self.assets.drag.is_some() {
            self.assets.cancel_import();
            cx.notify();
        }
    }

    fn start_asset_drag(
        &mut self,
        record: AssetRecord,
        pointer: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.asset_import_busy() || asset_track(record.kind).is_none() || !self.project_ready() {
            return;
        }
        let Some(intent) = self.asset_intent() else {
            return;
        };
        self.assets.cancel_import();
        self.assets.error = None;
        let duration = (record.kind == AssetKind::Photo).then_some(3.);
        self.assets.drag = Some(AssetDrag {
            record,
            intent,
            generation: self.assets.download_generation,
            press: pointer,
            pointer,
            active: false,
            duration,
            probe_started: false,
            error: None,
        });
        window.focus(&self.focus_handle_for_menu(), cx);
        cx.notify();
    }

    pub(crate) fn move_asset_drag(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.assets.drag.as_mut() else {
            return;
        };
        drag.pointer = event.position;
        let activating = !drag.active && asset_drag_threshold(drag.press, drag.pointer);
        drag.active |= activating;
        let probe = if drag.active && drag.record.kind == AssetKind::Audio && !drag.probe_started {
            drag.probe_started = true;
            Some((
                drag.generation,
                drag.intent.project.clone(),
                drag.record.path.clone(),
            ))
        } else {
            None
        };
        if activating {
            self.stop_playback(cx);
        }
        if let Some((generation, project, path)) = probe {
            cx.spawn_in(window, async move |this, cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        let source = asset_library::project_asset_path(&project, &path)
                            .ok_or_else(|| tr::editor::assets::missing_media().to_string())?;
                        crate::editor_audio::validate_audio_duration(&source)
                    })
                    .await;
                this.update_in(cx, |this, _, cx| {
                    if this.assets.complete_drag_probe(generation, result) {
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    pub(crate) fn finish_asset_drag(
        &mut self,
        event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mut drag) = self.assets.drag.take() else {
            return;
        };
        if !drag.active {
            self.assets.cancel_import();
            cx.notify();
            return;
        }
        let location = asset_drop_location(
            &self.asset_drop_geometry(window),
            drag.record.kind,
            event.position,
        );
        let valid_intent = self.accepts_asset(drag.generation, &drag.intent, drag.record.kind);
        self.assets.cancel_import();
        let Some(location) = location.filter(|_| valid_intent && drag.error.is_none()) else {
            self.assets.error = drag.error;
            cx.notify();
            return;
        };
        if drag.duration.is_some_and(|duration| {
            exact_asset_drop_range(
                &self.project,
                drag.record.kind,
                location.lane,
                location.time,
                self.total_duration(),
                duration,
            )
            .is_none()
        }) {
            cx.notify();
            return;
        }
        let Some(track) = asset_track(drag.record.kind) else {
            return;
        };
        drag.intent.time = location.time;
        drag.intent.lane = Some(AssetLaneIntent {
            lane: location.lane,
            identity: asset_lane_identity(
                &self.project,
                drag.record.kind,
                location.lane,
                self.tracks.count(track),
            ),
        });
        let generation = self.assets.download_generation;
        match drag.record.kind {
            AssetKind::Photo => {
                self.reuse_image_asset(drag.record, drag.intent, generation, window, cx)
            }
            AssetKind::Audio => {
                if let Some(source) =
                    asset_library::project_asset_path(&drag.intent.project, &drag.record.path)
                {
                    self.reuse_audio_asset(
                        drag.record,
                        source,
                        drag.intent,
                        generation,
                        window,
                        cx,
                    );
                } else {
                    self.assets.error = Some(tr::editor::assets::missing_media().into());
                }
            }
            AssetKind::Video => {}
        }
        cx.notify();
    }

    pub(crate) fn render_asset_drag_layer(
        &self,
        window: &Window,
        _cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let drag = self.assets.drag.as_ref()?;
        let mut layer = div()
            .id("asset-library-drag")
            .absolute()
            .inset_0()
            .cursor(CursorStyle::ClosedHand);
        if drag.active {
            let theme = self.theme;
            let geometry = self.asset_drop_geometry(window);
            let location =
                asset_drop_location(&geometry, drag.record.kind, drag.pointer).filter(|_| {
                    drag.error.is_none()
                        && self.accepts_asset(drag.generation, &drag.intent, drag.record.kind)
                });
            let range = location.and_then(|location| {
                drag.duration.and_then(|duration| {
                    exact_asset_drop_range(
                        &self.project,
                        drag.record.kind,
                        location.lane,
                        location.time,
                        geometry.total,
                        duration,
                    )
                })
            });
            if let (Some(location), Some(range)) = (location, range.as_ref()) {
                let width = timeline::content_width(geometry.viewport_width);
                let seconds_per_pixel = geometry.transform.secs_per_pixel(width);
                let left = (timeline::content_left()
                    + ((range.start - geometry.transform.position) / seconds_per_pixel) as f32)
                    .max(f32::from(location.bounds.left()));
                let right = (timeline::content_left()
                    + ((range.end - geometry.transform.position) / seconds_per_pixel) as f32)
                    .min(f32::from(location.bounds.right()));
                if right > left {
                    layer = layer.child(
                        div()
                            .absolute()
                            .left(px(left))
                            .top(location.bounds.top())
                            .w(px(right - left))
                            .h(location.bounds.size.height)
                            .rounded(px(5.))
                            .border_2()
                            .border_color(Hsla::from(theme.editor.text_1)),
                    );
                }
            }
            let status = if let Some(error) = &drag.error {
                error.clone()
            } else if range.is_some() {
                tr::editor::assets::drop_at(format!(
                    "{:.2}s",
                    location.map_or(0., |location| location.time)
                ))
                .to_string()
            } else if location.is_some() && drag.duration.is_none() {
                tr::editor::assets::drop_checking().to_string()
            } else {
                tr::editor::assets::drop_invalid().to_string()
            };
            let viewport = window.viewport_size();
            layer = layer.child(
                div()
                    .absolute()
                    .left(px((f32::from(drag.pointer.x) + 14.)
                        .clamp(8., (f32::from(viewport.width) - 248.).max(8.))))
                    .top(px((f32::from(drag.pointer.y) + 14.)
                        .clamp(8., (f32::from(viewport.height) - 90.).max(8.))))
                    .w(px(232.))
                    .p(px(10.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(Hsla::from(theme.editor.text_2))
                    .bg(Hsla::from(theme.editor.ctl))
                    .text_color(Hsla::from(theme.editor.text_1))
                    .text_size(px(12.))
                    .child(
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(drag.record.name.clone()),
                    )
                    .child(div().mt(px(4.)).text_size(px(11.)).child(status)),
            );
        }
        Some(layer.into_any_element())
    }

    pub(crate) fn asset_import_busy(&self) -> bool {
        self.assets.importing
            || self.assets.drag.is_some()
            || self.assets.tracking.is_some()
            || self.clips.importing
            || self.audio_import_pending.is_some()
            || self.sidebar.picking_image
            || self.sidebar.title_cards.picking_image
    }

    pub(crate) fn remember_project_asset(&mut self, record: AssetRecord, cx: &mut Context<Self>) {
        self.assets.records_generation = self.assets.records_generation.wrapping_add(1);
        if let Some(existing) = self
            .assets
            .records
            .iter_mut()
            .find(|existing| existing.path == record.path)
        {
            *existing = record;
        } else {
            self.assets.records.push(record);
        }
        cx.notify();
    }
    pub(crate) fn take_asset_browser_snapshot(
        &mut self,
        cx: &Context<Self>,
    ) -> Option<AssetBrowserSnapshot> {
        let draft = self.assets.input.as_ref()?.read(cx).text().to_owned();
        Some(self.assets.browser_snapshot(self.clips.open, draft))
    }

    pub(crate) fn restore_asset_browser(
        &mut self,
        snapshot: AssetBrowserSnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.project_ready() {
            self.assets.pending_restore = Some(snapshot);
            return;
        }
        self.apply_asset_browser_snapshot(snapshot, window, cx);
    }

    fn apply_asset_browser_snapshot(
        &mut self,
        snapshot: AssetBrowserSnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prepare_asset_search_input(window, cx);
        let (open, draft) = self.assets.restore_browser(snapshot);
        self.arm_asset_retry_timer(cx);
        if let Some(input) = &self.assets.input {
            input.update(cx, |input, cx| input.set_text(draft, cx));
        }
        self.clips.open = open;
        if open {
            self.transcript.open = false;
        }
        self.refresh_asset_records(window, cx);
        cx.notify();
    }

    pub(crate) fn restore_pending_asset_browser(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(snapshot) = self.assets.pending_restore.take() {
            self.apply_asset_browser_snapshot(snapshot, window, cx);
        }
    }

    pub(crate) fn prepare_asset_library(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prepare_asset_search_input(window, cx);
        self.refresh_asset_records(window, cx);
    }

    fn prepare_asset_search_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.assets.input.is_none() {
            let input = cx.new(|cx| ui::TextInputState::single_line(window, cx));
            input.update(cx, |input, _| {
                input.set_placeholder(tr::editor::assets::search_placeholder())
            });
            let subscription =
                cx.subscribe_in(&input, window, |this: &mut Self, _, event, window, cx| {
                    if matches!(event, ui::TextInputEvent::Confirmed) {
                        this.search_assets(1, true, window, cx);
                    }
                });
            self.push_text_subscription(subscription);
            self.assets.input = Some(input);
            if cached_credentials(Provider::Unsplash) == Some(false)
                && cached_credentials(Provider::Pixabay) == Some(true)
            {
                self.assets.provider = Provider::Pixabay;
            }
        }
    }

    fn refresh_asset_records(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.assets.records_generation = self.assets.records_generation.wrapping_add(1);
        let generation = self.assets.records_generation;
        let project = self.project_path.clone();
        let configuration = self.project.clone();
        cx.spawn_in(window, async move |this, cx| {
            let records = cx
                .background_executor()
                .spawn(
                    async move { asset_library::collect_project_records(&project, &configuration) },
                )
                .await;
            this.update(cx, |this, cx| {
                if this.assets.records_generation != generation {
                    return;
                }
                this.assets.invalid_record_metadata = records.invalid_metadata;
                this.assets.records = records.records;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn render_asset_sources(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let sources = [
            AssetSource::Project,
            AssetSource::Local,
            AssetSource::Online,
        ];
        div().flex_none().w_full().p(px(12.)).child(
            ui::SegmentedControl::editor(
                &theme,
                "asset-source",
                sources
                    .iter()
                    .map(|source| {
                        let label = match source {
                            AssetSource::Project => tr::editor::assets::project(),
                            AssetSource::Local => tr::editor::assets::local(),
                            AssetSource::Online => tr::editor::assets::online(),
                        };
                        ui::SegmentOption::new(label, self.assets.source == *source)
                    })
                    .collect(),
            )
            .stretch()
            .item_height(px(28.))
            .on_select(cx.listener(move |this, index: &usize, window, cx| {
                let Some(source) = ui::option_at(&sources, *index) else {
                    return;
                };
                if this.assets.source != source {
                    this.assets.cancel_search();
                }
                this.assets.source = source;
                this.prepare_asset_library(window, cx);
                cx.notify();
            })),
        )
    }

    pub(crate) fn render_asset_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.assets.source == AssetSource::Local {
            self.render_local_assets(cx)
        } else {
            self.render_online_assets(cx)
        }
    }

    fn render_local_assets(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let mut header = div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(Hsla::from(theme.editor.text_2))
                    .child(tr::editor::assets::local_hint()),
            )
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .gap(px(6.))
                    .child(
                        div().flex_1().min_w_0().whitespace_nowrap().child(
                            ui::Button::plain(
                                &theme,
                                "asset-image",
                                ui::ButtonVariant::Gray,
                                ui::ButtonSize::Sm,
                            )
                            .label(tr::editor::assets::import_image())
                            .icon("icons/image.svg")
                            .full_width()
                            .disabled(self.asset_import_busy())
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.pick_asset_image(window, cx)
                                }),
                            ),
                        ),
                    )
                    .child(
                        div().flex_1().min_w_0().whitespace_nowrap().child(
                            ui::Button::plain(
                                &theme,
                                "asset-video",
                                ui::ButtonVariant::Gray,
                                ui::ButtonSize::Sm,
                            )
                            .label(tr::editor::assets::import_video())
                            .icon("icons/video.svg")
                            .full_width()
                            .disabled(self.asset_import_busy())
                            .on_click(
                                cx.listener(|this, _, window, cx| this.pick_mp4_asset(window, cx)),
                            ),
                        ),
                    )
                    .child(
                        div().flex_1().min_w_0().whitespace_nowrap().child(
                            ui::Button::plain(
                                &theme,
                                "asset-audio",
                                ui::ButtonVariant::Gray,
                                ui::ButtonSize::Sm,
                            )
                            .label(tr::editor::assets::import_audio())
                            .icon("icons/music.svg")
                            .full_width()
                            .disabled(self.asset_import_busy())
                            .on_click(cx.listener(|this, _, _, cx| this.open_audio_picker(0, cx))),
                        ),
                    ),
            );
        header = header.child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .child(tr::editor::assets::reusable()),
        );
        if self.assets.invalid_record_metadata {
            header = header.child(
                div()
                    .text_size(px(12.))
                    .text_color(Hsla::from(theme.editor.text_2))
                    .child(tr::editor::assets::metadata_error()),
            );
        }
        let mut list = div()
            .id("local-assets")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_y_scroll()
            .gap(px(12.));
        if self.assets.records.is_empty() {
            list = list.child(
                div()
                    .text_size(px(12.))
                    .text_color(Hsla::from(theme.editor.text_2))
                    .child(tr::editor::assets::local_empty()),
            );
        }
        for (index, record) in self.assets.records.iter().enumerate() {
            let drag_record = record.clone();
            let row = div()
                .flex()
                .gap(px(8.))
                .items_center()
                .p(px(8.))
                .rounded(px(8.))
                .bg(Hsla::from(theme.editor.ctl))
                .child(
                    svg()
                        .path(match record.kind {
                            AssetKind::Photo => "icons/image.svg",
                            AssetKind::Video => "icons/video.svg",
                            AssetKind::Audio => "icons/music.svg",
                        })
                        .size(px(16.))
                        .flex_shrink_0()
                        .text_color(Hsla::from(theme.editor.text_2)),
                )
                .child(
                    div()
                        .id(("asset-drag-source", index))
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.))
                        .when(
                            asset_track(record.kind).is_some() && !self.asset_import_busy(),
                            |name| {
                                name.cursor(CursorStyle::OpenHand).on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                        this.start_asset_drag(
                                            drag_record.clone(),
                                            event.position,
                                            window,
                                            cx,
                                        );
                                        cx.stop_propagation();
                                    }),
                                )
                            },
                        )
                        .child(record.name.clone())
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(Hsla::from(theme.editor.text_2))
                                .child(record.source.as_ref().map_or_else(
                                    || tr::editor::assets::local_file().to_string(),
                                    |source| {
                                        format!(
                                            "{} · {}",
                                            provider_name(source.provider),
                                            source.attribution.author
                                        )
                                    },
                                )),
                        ),
                )
                .child(
                    ui::Button::plain(
                        &theme,
                        ("reuse-asset", index),
                        ui::ButtonVariant::Gray,
                        ui::ButtonSize::Sm,
                    )
                    .label(if record.kind == AssetKind::Video {
                        tr::editor::assets::append_video()
                    } else {
                        tr::editor::assets::reuse()
                    })
                    .disabled(self.asset_import_busy())
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.reuse_asset(index, window, cx)),
                    ),
                );
            list = list.child(row);
            if let Some(source) = &record.source {
                let author = source.attribution.author_url.clone();
                let origin = source.attribution.source_url.clone();
                list = list.child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(
                            ui::Button::plain(
                                &theme,
                                ("asset-author-link", index),
                                ui::ButtonVariant::Gray,
                                ui::ButtonSize::Sm,
                            )
                            .label(tr::editor::assets::author_profile())
                            .on_click(move |_, _, cx| cx.open_url(&author)),
                        )
                        .child(
                            ui::Button::plain(
                                &theme,
                                ("asset-source-link", index),
                                ui::ButtonVariant::Gray,
                                ui::ButtonSize::Sm,
                            )
                            .label(tr::editor::assets::source_page())
                            .on_click(move |_, _, cx| cx.open_url(&origin)),
                        ),
                );
            }
        }
        if !self.assets.records.is_empty() {
            list = list.child(
                div()
                    .text_size(px(11.))
                    .text_color(Hsla::from(theme.editor.text_2))
                    .child(tr::editor::assets::drag_hint()),
            );
        }
        let status = self.asset_status(cx);
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .p(px(12.))
            .gap(px(12.))
            .child(header)
            .children((!status.is_empty()).then(|| {
                div()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .gap(px(8.))
                    .children(status)
            }))
            .child(list)
            .into_any_element()
    }

    fn render_online_assets(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let waiting = self
            .assets
            .retry_seconds(self.assets.provider, Instant::now())
            > 0;
        let configured = cached_credentials(self.assets.provider);
        let mut body = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap(px(10.))
            .px(px(12.))
            .pb(px(12.));
        body = body.child(
            div()
                .flex()
                .min_w_0()
                .gap(px(4.))
                .children(
                    [
                        (Provider::Unsplash, "Unsplash"),
                        (Provider::Pixabay, "Pixabay"),
                    ]
                    .into_iter()
                    .enumerate()
                    .map(|(index, (provider, name))| {
                        div().flex_1().min_w_0().whitespace_nowrap().child(
                            ui::Button::plain(
                                &theme,
                                ("stock-provider", index),
                                ui::ButtonVariant::Gray,
                                ui::ButtonSize::Sm,
                            )
                            .label(name)
                            .full_width()
                            .when(self.assets.provider == provider, |button| {
                                button.icon("icons/check.svg")
                            })
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.assets.cancel_search();
                                    this.assets.provider = provider;
                                    if provider == Provider::Unsplash {
                                        this.assets.kind = MediaKind::Photo;
                                    }
                                    this.clear_asset_results();
                                    cx.notify();
                                },
                            )),
                        )
                    }),
                )
                .child(
                    div().flex_1().min_w_0().whitespace_nowrap().child(
                        ui::Button::plain(
                            &theme,
                            "stock-settings",
                            ui::ButtonVariant::Gray,
                            ui::ButtonSize::Sm,
                        )
                        .label(tr::editor::assets::settings())
                        .full_width()
                        .on_click(|_, _, cx| {
                            crate::app_windows::open_settings(
                                crate::settings_window::Page::Assets,
                                cx,
                            )
                        }),
                    ),
                ),
        );
        if configured == Some(false) {
            return body
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(Hsla::from(theme.editor.text_2))
                        .child(tr::editor::assets::setup_hint()),
                )
                .child(
                    ui::Button::plain(
                        &theme,
                        "stock-setup",
                        ui::ButtonVariant::Primary,
                        ui::ButtonSize::Md,
                    )
                    .label(tr::editor::assets::setup())
                    .on_click(|_, _, cx| {
                        crate::app_windows::open_settings(crate::settings_window::Page::Assets, cx)
                    }),
                )
                .children(self.asset_status(cx))
                .into_any_element();
        }
        body = body.child(
            div().flex().min_w_0().gap(px(6.)).children(
                [
                    (MediaKind::Photo, tr::editor::assets::image()),
                    (MediaKind::Video, tr::editor::assets::video()),
                ]
                .into_iter()
                .enumerate()
                .map(|(index, (kind, label))| {
                    div().flex_1().min_w_0().whitespace_nowrap().child(
                        ui::Button::plain(
                            &theme,
                            ("stock-kind", index),
                            ui::ButtonVariant::Gray,
                            ui::ButtonSize::Sm,
                        )
                        .label(label)
                        .full_width()
                        .disabled(
                            kind == MediaKind::Video && self.assets.provider == Provider::Unsplash,
                        )
                        .when(self.assets.kind == kind, |button| {
                            button.icon("icons/check.svg")
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.assets.cancel_search();
                            this.assets.kind = kind;
                            this.clear_asset_results();
                            cx.notify();
                        })),
                    )
                }),
            ),
        );
        if let Some(input) = &self.assets.input {
            body = body.child(
                div()
                    .flex()
                    .gap(px(6.))
                    .child(ui::TextInput::search(&theme, "stock-query", input))
                    .child(
                        ui::Button::plain(
                            &theme,
                            "stock-search",
                            ui::ButtonVariant::Gray,
                            ui::ButtonSize::Sm,
                        )
                        .label(if self.assets.searching {
                            tr::editor::assets::cancel()
                        } else {
                            tr::editor::assets::search()
                        })
                        .disabled(waiting && !self.assets.searching)
                        .on_click(cx.listener(|this, _, window, cx| {
                            if this.assets.searching {
                                this.assets.cancel_search();
                                this.assets.searched = false;
                                cx.notify();
                            } else {
                                this.search_assets(1, true, window, cx);
                            }
                        })),
                    ),
            );
        }
        let mut results = div()
            .id("stock-results")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(8.));
        if self.assets.results.is_empty() {
            results = results.child(
                div()
                    .py(px(12.))
                    .text_size(px(12.))
                    .text_color(Hsla::from(theme.editor.text_2))
                    .child(if self.assets.searching {
                        tr::editor::assets::searching()
                    } else if self.assets.searched && self.assets.error.is_none() {
                        tr::editor::assets::no_results()
                    } else {
                        tr::editor::assets::search_hint()
                    }),
            );
        }
        for (row, assets) in self.assets.results.chunks(2).enumerate() {
            results = results.child(div().flex().gap(px(8.)).children(
                assets.iter().enumerate().map(|(column, asset)| {
                    let index = row * 2 + column;
                    let key = asset_key(asset);
                    let mut preview = div()
                        .h(px(96.))
                        .w_full()
                        .overflow_hidden()
                        .rounded(px(6.))
                        .bg(Hsla::from(theme.editor.ctl))
                        .flex()
                        .items_center()
                        .justify_center();
                    if let Some(image) = self.assets.thumbnails.get(&key) {
                        preview = preview.child(
                            img(image.clone())
                                .size_full()
                                .object_fit(gpui::ObjectFit::Cover),
                        );
                    } else {
                        let failure = self.assets.thumbnail_errors.get(&key);
                        let seconds = failure
                            .map_or(0, |(_, until)| seconds_remaining(*until, Instant::now()));
                        let loading = self.assets.thumbnail_cancel.contains_key(&key);
                        let available = asset.thumbnail_url.is_some();
                        preview = preview.flex_col().gap(px(4.)).child(
                            div()
                                .text_size(px(11.))
                                .text_color(Hsla::from(theme.editor.text_2))
                                .child(if !available {
                                    tr::editor::assets::preview_unavailable()
                                } else if loading {
                                    tr::editor::assets::preview_loading()
                                } else {
                                    tr::editor::assets::preview_failed()
                                }),
                        );
                        if available && !loading {
                            preview = preview.child(
                                ui::Button::plain(
                                    &theme,
                                    ("stock-preview-retry", index),
                                    ui::ButtonVariant::Gray,
                                    ui::ButtonSize::Sm,
                                )
                                .label(if seconds > 0 {
                                    tr::editor::assets::retry_in(seconds)
                                } else {
                                    tr::editor::assets::retry_preview().to_string()
                                })
                                .disabled(seconds > 0)
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.load_asset_thumbnail(index, window, cx);
                                    },
                                )),
                            );
                        }
                    }
                    let source_url = asset.attribution.source_url.clone();
                    let author_url = asset.attribution.author_url.clone();
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(
                            div()
                                .id(("stock-card", index))
                                .rounded(px(7.))
                                .border_1()
                                .border_color(if self.assets.selected == Some(index) {
                                    Hsla::from(theme.editor.accent)
                                } else {
                                    Hsla::from(theme.editor.line)
                                })
                                .cursor_pointer()
                                .child(preview)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.assets.selected = Some(index);
                                    let output = this
                                        .frame_layout
                                        .map_or([1920, 1080], |layout| layout.output_size);
                                    this.assets.rendition = asset_library::recommended_rendition(
                                        &this.assets.results[index].renditions,
                                        output,
                                    );
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .id(("stock-author", index))
                                .text_size(px(11.))
                                .text_color(Hsla::from(theme.editor.text_2))
                                .cursor_pointer()
                                .child(asset.attribution.author.clone())
                                .on_click(move |_, _, cx| cx.open_url(&author_url)),
                        )
                        .child(
                            div()
                                .id(("stock-origin", index))
                                .text_size(px(10.))
                                .text_color(Hsla::from(theme.editor.text_3))
                                .cursor_pointer()
                                .child(format!(
                                    "{}{}",
                                    provider_name(asset.provider),
                                    asset
                                        .duration_seconds
                                        .map_or_else(String::new, |seconds| format!(
                                            " · {seconds}s"
                                        ))
                                ))
                                .on_click(move |_, _, cx| cx.open_url(&source_url)),
                        )
                }),
            ));
        }
        if self.assets.page > 1 || self.assets.next_page.is_some() {
            results = results.child(
                div()
                    .flex()
                    .gap(px(6.))
                    .child(
                        ui::Button::plain(
                            &theme,
                            "stock-prev",
                            ui::ButtonVariant::Gray,
                            ui::ButtonSize::Sm,
                        )
                        .label(tr::editor::assets::previous())
                        .disabled(self.assets.page <= 1 || self.assets.searching || waiting)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.search_assets(
                                this.assets.page.saturating_sub(1).max(1),
                                false,
                                window,
                                cx,
                            )
                        })),
                    )
                    .child(
                        ui::Button::plain(
                            &theme,
                            "stock-next",
                            ui::ButtonVariant::Gray,
                            ui::ButtonSize::Sm,
                        )
                        .label(tr::editor::assets::next())
                        .disabled(
                            self.assets.next_page.is_none() || self.assets.searching || waiting,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some(page) = this.assets.next_page {
                                this.search_assets(page, false, window, cx);
                            }
                        })),
                    ),
            );
        }
        body = body.child(results);
        if let Some(asset) = self
            .assets
            .selected
            .and_then(|index| self.assets.results.get(index))
        {
            body = body
                .child(
                    div().flex().flex_wrap().gap(px(4.)).children(
                        asset
                            .renditions
                            .iter()
                            .enumerate()
                            .map(|(index, rendition)| {
                                let size = match (rendition.width, rendition.height) {
                                    (Some(width), Some(height)) => format!("{width} × {height}"),
                                    _ => rendition.label.clone(),
                                };
                                let label =
                                    rendition.approximate_bytes.map_or(size.clone(), |bytes| {
                                        format!("{size} · {:.1} MB", bytes as f64 / 1_048_576.)
                                    });
                                ui::Button::plain(
                                    &theme,
                                    ("stock-size", index),
                                    ui::ButtonVariant::Gray,
                                    ui::ButtonSize::Sm,
                                )
                                .label(label)
                                .when(self.assets.rendition == index, |button| {
                                    button.icon("icons/check.svg")
                                })
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.assets.rendition = index;
                                        cx.notify();
                                    },
                                ))
                            }),
                    ),
                )
                .child(
                    ui::Button::plain(
                        &theme,
                        "stock-add",
                        ui::ButtonVariant::Primary,
                        ui::ButtonSize::Md,
                    )
                    .label(if asset.kind == MediaKind::Photo {
                        tr::editor::assets::add_image(crate::editor_timeline::format_time(
                            self.playhead,
                        ))
                    } else {
                        tr::editor::assets::append_video().to_string()
                    })
                    .full_width()
                    .disabled(
                        self.asset_import_busy()
                            || self.assets.retry_seconds(asset.provider, Instant::now()) > 0,
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.download_asset(window, cx))),
                );
        }
        body.children(self.asset_status(cx)).into_any_element()
    }

    fn asset_status(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = self.theme;
        let mut elements = Vec::new();
        if self.assets.importing || self.clips.importing {
            let (received, total) = self.assets.progress;
            let message = if self.clips.importing || !self.assets.downloading {
                tr::editor::assets::preparing().to_string()
            } else if let Some(total) = total {
                format!(
                    "{} {:.1} / {:.1} MB",
                    tr::editor::assets::downloading(),
                    received as f64 / 1_048_576.,
                    total as f64 / 1_048_576.
                )
            } else {
                format!(
                    "{} {:.1} MB",
                    tr::editor::assets::downloading(),
                    received as f64 / 1_048_576.
                )
            };
            elements.push(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().flex_1().text_size(px(12.)).child(message))
                    .child(
                        ui::Button::plain(
                            &theme,
                            "asset-cancel",
                            ui::ButtonVariant::Gray,
                            ui::ButtonSize::Sm,
                        )
                        .label(tr::editor::assets::cancel())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.assets.cancel();
                            this.assets.error = None;
                            this.cancel_asset_video_import(cx);
                            cx.notify();
                        })),
                    )
                    .into_any_element(),
            );
        }
        if self.assets.source == AssetSource::Online {
            for provider in [Provider::Unsplash, Provider::Pixabay] {
                let relevant = provider == self.assets.provider
                    || self
                        .assets
                        .tracking
                        .as_ref()
                        .is_some_and(|(media, _, _)| media.asset.provider == provider);
                let seconds = self.assets.retry_seconds(provider, Instant::now());
                if relevant && seconds > 0 {
                    elements.push(
                        div()
                            .text_size(px(12.))
                            .text_color(Hsla::from(theme.editor.text_2))
                            .child(format!(
                                "{} · {}",
                                provider_name(provider),
                                tr::editor::assets::retry_in(seconds)
                            ))
                            .into_any_element(),
                    );
                }
            }
        }
        if let Some(error) = &self.assets.error {
            elements.push(
                div()
                    .text_size(px(12.))
                    .text_color(Hsla::from(theme.editor.text_2))
                    .child(error.clone())
                    .into_any_element(),
            );
        }
        if self.assets.tracking.is_some() {
            elements.push(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        ui::Button::plain(
                            &theme,
                            "asset-track-retry",
                            ui::ButtonVariant::Gray,
                            ui::ButtonSize::Sm,
                        )
                        .label(tr::editor::assets::retry())
                        .disabled(self.assets.tracking.as_ref().is_some_and(|(media, _, _)| {
                            self.assets
                                .retry_seconds(media.asset.provider, Instant::now())
                                > 0
                        }))
                        .on_click(
                            cx.listener(|this, _, window, cx| {
                                this.retry_asset_tracking(window, cx)
                            }),
                        ),
                    )
                    .child(
                        ui::Button::plain(
                            &theme,
                            "asset-track-cancel",
                            ui::ButtonVariant::Gray,
                            ui::ButtonSize::Sm,
                        )
                        .label(tr::editor::assets::cancel())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.assets.cancel();
                            this.assets.error = None;
                            cx.notify();
                        })),
                    )
                    .into_any_element(),
            );
        }
        elements
    }

    fn clear_asset_results(&mut self) {
        self.assets.results.clear();
        self.assets.thumbnails.clear();
        self.assets.thumbnail_errors.clear();
        self.assets.selected = None;
        self.assets.searched = false;
        self.assets.error = None;
        self.assets.page = 1;
        self.assets.next_page = None;
    }

    fn stock_client(&mut self) -> Result<Arc<StockMediaClient>, StockError> {
        if let Some(client) = &self.assets.client {
            return Ok(client.clone());
        }
        let client = Arc::new(StockMediaClient::new(
            crate::store::app_data_dir().join("asset-cache"),
        )?);
        self.assets.client = Some(client.clone());
        Ok(client)
    }

    fn arm_asset_retry_timer(&mut self, cx: &mut Context<Self>) {
        if self.assets.retry_ticking {
            return;
        }
        self.assets.retry_ticking = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let keep = this
                    .update(cx, |this, cx| {
                        let now = Instant::now();
                        let active = this.assets.retry_after.values().any(|until| *until > now)
                            || this
                                .assets
                                .thumbnail_errors
                                .values()
                                .any(|(_, until)| until.is_some_and(|until| until > now));
                        this.assets.retry_ticking = active;
                        cx.notify();
                        active
                    })
                    .unwrap_or(false);
                if !keep {
                    return;
                }
            }
        })
        .detach();
    }

    fn asset_request_failed(
        &mut self,
        provider: Provider,
        error: &StockError,
        cx: &mut Context<Self>,
    ) {
        if let Some(seconds) = error.retry_wait_seconds() {
            self.assets
                .wait_before_retry(provider, seconds, Instant::now());
            self.arm_asset_retry_timer(cx);
        }
        self.assets.error = Some(stock_error(error));
    }

    fn load_asset_thumbnail(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(asset) = self.assets.results.get(index).cloned() else {
            return;
        };
        if asset.thumbnail_url.is_none() {
            return;
        }
        let key = asset_key(&asset);
        if self.assets.thumbnails.contains_key(&key)
            || self.assets.thumbnail_cancel.contains_key(&key)
            || self
                .assets
                .thumbnail_errors
                .get(&key)
                .is_some_and(|(_, until)| seconds_remaining(*until, Instant::now()) > 0)
        {
            return;
        }
        let client = match self.stock_client() {
            Ok(client) => client,
            Err(error) => {
                let _ = self
                    .assets
                    .thumbnail_errors
                    .insert(key, (stock_error(&error), None));
                cx.notify();
                return;
            }
        };
        let generation = self.assets.generation;
        let (cancel, receiver) = tokio::sync::oneshot::channel();
        let _ = self.assets.thumbnail_cancel.insert(key.clone(), cancel);
        let _ = self.assets.thumbnail_errors.remove(&key);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let worker_asset = asset.clone();
            let result = gpui_tokio::Tokio::spawn(cx, async move {
                tokio::select! { biased; _ = receiver => None, result = client.thumbnail_bytes(&worker_asset) => Some(result) }
            }).await;
            let bytes = match result {
                Ok(None) => return,
                Ok(Some(result)) => result,
                Err(_) => Err(StockError::Network),
            };
            let result = match bytes {
                Ok(bytes) => cx.background_executor().spawn(async move { decode_thumbnail(&bytes).ok_or(StockError::UnsupportedMedia) }).await,
                Err(error) => Err(error),
            };
            this.update(cx, |this, cx| {
                if this.assets.finish_thumbnail(generation, &asset, result, Instant::now()) {
                    this.arm_asset_retry_timer(cx);
                    cx.notify();
                }
            }).ok();
        }).detach();
    }

    fn search_assets(
        &mut self,
        page: u32,
        new_query: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .assets
            .retry_seconds(self.assets.provider, Instant::now())
            > 0
        {
            return;
        }
        let Some(input) = &self.assets.input else {
            return;
        };
        let query = search_query(
            input.read(cx).text(),
            &self.assets.submitted_query,
            new_query,
        );
        if query.is_empty() {
            return;
        }
        self.assets.cancel_search();
        self.clear_asset_results();
        self.assets.submitted_query = query.clone();
        let client = match self.stock_client() {
            Ok(client) => client,
            Err(error) => {
                self.assets.error = Some(stock_error(&error));
                cx.notify();
                return;
            }
        };
        let generation = self.assets.generation;
        let provider = self.assets.provider;
        let request = SearchRequest {
            provider,
            kind: self.assets.kind,
            query,
            page,
            per_page: 12,
            orientation: Orientation::Any,
        };
        let (cancel, mut receiver) = tokio::sync::oneshot::channel();
        self.assets.search_cancel = Some(cancel);
        self.assets.searching = true;
        self.assets.searched = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = gpui_tokio::Tokio::spawn(cx, async move {
                stock_request(&mut receiver, stock_key(provider), |key| async move {
                    let keys = credentials_for(provider, &key);
                    client.search(&keys, request).await
                })
                .await
            })
            .await;
            let result = match result {
                Ok(Some(result)) => result,
                Ok(None) => return,
                Err(_) => Err(StockError::Network),
            };
            this.update_in(cx, |this, window, cx| {
                if this.assets.generation != generation {
                    return;
                }
                this.assets.searching = false;
                this.assets.search_cancel = None;
                match result {
                    Ok(page) => {
                        if let Some(seconds) = page_retry_seconds(&page) {
                            this.assets
                                .wait_before_retry(provider, seconds, Instant::now());
                            this.arm_asset_retry_timer(cx);
                        }
                        this.assets.page = page.page;
                        this.assets.next_page = page.next_page;
                        this.assets.results = page.assets;
                        for index in 0..this.assets.results.len() {
                            this.load_asset_thumbnail(index, window, cx);
                        }
                    }
                    Err(error) => this.asset_request_failed(provider, &error, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn asset_intent(&self) -> Option<ImportIntent> {
        Some(ImportIntent {
            project: self.project_path.canonicalize().ok()?,
            time: self.playhead,
            timeline: timeline_identity(&self.project),
            metadata: std::fs::metadata(self.project_path.join("recording-meta.json")).ok()?,
            lane: None,
        })
    }

    fn accepts_asset(&self, generation: u64, intent: &ImportIntent, kind: AssetKind) -> bool {
        self.assets.download_generation == generation
            && self.project_ready()
            && self
                .project_path
                .canonicalize()
                .is_ok_and(|path| path == intent.project)
            && (kind == AssetKind::Video || timeline_identity(&self.project) == intent.timeline)
            && intent.lane.as_ref().is_none_or(|target| {
                asset_track(kind).is_some_and(|track| {
                    asset_lane_identity(&self.project, kind, target.lane, self.tracks.count(track))
                        == target.identity
                })
            })
            && std::fs::metadata(self.project_path.join("recording-meta.json"))
                .is_ok_and(|metadata| same_recording(&intent.metadata, &metadata))
    }

    fn download_asset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.asset_import_busy() {
            return;
        }
        let Some(asset) = self
            .assets
            .selected
            .and_then(|index| self.assets.results.get(index))
            .cloned()
        else {
            return;
        };
        let provider = asset.provider;
        if self.assets.retry_seconds(provider, Instant::now()) > 0 {
            return;
        }
        let Some(intent) = self.asset_intent() else {
            return;
        };
        let client = match self.stock_client() {
            Ok(client) => client,
            Err(error) => {
                self.assets.error = Some(stock_error(&error));
                cx.notify();
                return;
            }
        };
        self.assets.cancel_import();
        self.assets.importing = true;
        self.assets.downloading = true;
        self.assets.error = None;
        self.assets.progress = (0, None);
        let generation = self.assets.download_generation;
        let rendition = self.assets.rendition;
        let (cancel, mut receiver) = tokio::sync::oneshot::channel();
        self.assets.download_cancel = Some(cancel);
        let progress = Arc::new(Mutex::new((0, None)));
        let worker_progress = progress.clone();
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(120))
                    .await;
                let value = progress.lock().map(|value| *value).unwrap_or_default();
                if !this
                    .update(cx, |this, cx| {
                        if this.assets.download_generation != generation || !this.assets.importing {
                            return false;
                        }
                        this.assets.progress = value;
                        cx.notify();
                        true
                    })
                    .unwrap_or(false)
                {
                    return;
                }
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            let result = gpui_tokio::Tokio::spawn(cx, async move {
                stock_request(&mut receiver, stock_key(provider), |key| async move {
                    let keys = credentials_for(provider, &key);
                    client
                        .download_with_progress(
                            &keys,
                            &asset,
                            rendition,
                            128 * 1024 * 1024,
                            move |bytes, total| {
                                if let Ok(mut progress) = worker_progress.lock() {
                                    *progress = (bytes, total);
                                }
                            },
                        )
                        .await
                })
                .await
            })
            .await;
            let result = match result {
                Ok(Some(result)) => result,
                Ok(None) => return,
                Err(_) => Err(StockError::Network),
            };
            this.update_in(cx, |this, window, cx| {
                if this.assets.download_generation != generation {
                    return;
                }
                this.assets.importing = false;
                this.assets.downloading = false;
                this.assets.download_cancel = None;
                match result {
                    Ok(DownloadOutcome::Ready(media)) => this
                        .import_downloaded_asset(media, intent, rendition, generation, window, cx),
                    Ok(DownloadOutcome::NeedsTracking { media, error }) => {
                        this.assets.tracking = Some((media, intent, rendition));
                        this.asset_request_failed(provider, &error, cx);
                    }
                    Err(error) => this.asset_request_failed(provider, &error, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn retry_asset_tracking(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.assets.tracking.as_ref().is_some_and(|(media, _, _)| {
            self.assets
                .retry_seconds(media.asset.provider, Instant::now())
                > 0
        }) {
            return;
        }
        let Some((mut media, intent, rendition)) = self.assets.tracking.take() else {
            return;
        };
        let client = match self.stock_client() {
            Ok(client) => client,
            Err(error) => {
                self.assets.tracking = Some((media, intent, rendition));
                self.assets.error = Some(stock_error(&error));
                cx.notify();
                return;
            }
        };
        let generation = self.assets.download_generation;
        let provider = media.asset.provider;
        let (cancel, mut receiver) = tokio::sync::oneshot::channel();
        self.assets.download_cancel = Some(cancel);
        self.assets.importing = true;
        self.assets.error = None;
        cx.spawn_in(window, async move |this, cx| {
            let result = gpui_tokio::Tokio::spawn(cx, async move {
                let pending_media = &mut media;
                let result = stock_request(&mut receiver, stock_key(provider), |key| async move {
                    let keys = credentials_for(provider, &key);
                    client.retry_download_tracking(&keys, pending_media).await
                })
                .await;
                (media, result)
            })
            .await;
            let (media, result) = match result {
                Ok((media, Some(result))) => (media, result),
                Ok((_, None)) => return,
                Err(_) => {
                    this.update(cx, |this, cx| {
                        if this.assets.download_generation == generation {
                            this.assets.importing = false;
                            this.assets.download_cancel = None;
                            this.assets.error = Some(tr::editor::assets::network_error().into());
                            cx.notify();
                        }
                    })
                    .ok();
                    return;
                }
            };
            this.update_in(cx, |this, window, cx| {
                if this.assets.download_generation != generation {
                    return;
                }
                this.assets.importing = false;
                this.assets.downloading = false;
                this.assets.download_cancel = None;
                match result {
                    Ok(()) => this
                        .import_downloaded_asset(media, intent, rendition, generation, window, cx),
                    Err(error) => {
                        let provider = media.asset.provider;
                        this.assets.tracking = Some((media, intent, rendition));
                        this.asset_request_failed(provider, &error, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn import_downloaded_asset(
        &mut self,
        media: DownloadedMedia,
        intent: ImportIntent,
        rendition: usize,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.accepts_asset(generation, &intent, media.asset.kind.into()) {
            self.assets.error = Some(tr::editor::assets::project_changed().into());
            return;
        }
        let record = AssetRecord {
            path: String::new(),
            name: if media.asset.description.trim().is_empty() {
                format!(
                    "{} · {}",
                    provider_name(media.asset.provider),
                    media.asset.attribution.author
                )
            } else {
                media.asset.description.clone()
            },
            kind: media.asset.kind.into(),
            source: Some(media.asset),
            rendition: Some(rendition),
        };
        self.assets.importing = true;
        cx.spawn_in(window, async move |this, cx| {
            let kind = record.kind;
            let result = cx
                .background_executor()
                .spawn(async move { StagedMedia::write(&media.bytes, media.extension, &record) })
                .await;
            this.update_in(cx, |this, window, cx| {
                if this.assets.download_generation != generation {
                    return;
                }
                if !this.accepts_asset(generation, &intent, kind) {
                    this.assets.importing = false;
                    this.assets.error = Some(tr::editor::assets::project_changed().into());
                    cx.notify();
                    return;
                }
                match result {
                    Ok(staged) => {
                        if kind == AssetKind::Photo {
                            this.import_asset_image(
                                staged.path.clone(),
                                Some(staged),
                                intent,
                                generation,
                                window,
                                cx,
                            );
                        } else {
                            this.assets.importing = false;
                            this.import_asset_video(staged.path.clone(), Some(staged), window, cx);
                        }
                    }
                    Err(error) => {
                        this.assets.importing = false;
                        this.assets.error = Some(error);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn pick_asset_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.asset_import_busy() {
            return;
        }
        let Some(intent) = self.asset_intent() else {
            return;
        };
        self.assets.cancel_import();
        let generation = self.assets.download_generation;
        self.assets.importing = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            #[cfg(target_os = "linux")]
            let source = crate::platform::open_file_panel_async(
                &[(
                    tr::editor::assets::image(),
                    crate::import::OVERLAY_IMAGE_EXTENSIONS,
                )],
                None,
                cx,
            )
            .await;
            #[cfg(not(target_os = "linux"))]
            let source = crate::platform::open_image_panel(crate::import::OVERLAY_IMAGE_EXTENSIONS);
            this.update_in(cx, |this, window, cx| {
                if this.assets.download_generation != generation {
                    return;
                }
                this.assets.importing = false;
                if let Some(source) = source {
                    if this.accepts_asset(generation, &intent, AssetKind::Photo) {
                        this.assets.importing = true;
                        this.import_asset_image(source, None, intent, generation, window, cx);
                    } else {
                        this.assets.error = Some(tr::editor::assets::project_changed().into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn import_asset_image(
        &mut self,
        source: PathBuf,
        staged: Option<StagedMedia>,
        intent: ImportIntent,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = intent.project.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let source_record = asset_library::read_record(&source, staged.is_some())?;
                    if source_record
                        .as_ref()
                        .is_some_and(|record| record.kind != AssetKind::Photo)
                    {
                        return Err(tr::editor::assets::metadata_error().into());
                    }
                    let imported = crate::import::import_editor_image(&project, &source)?;
                    let mut record = source_record.unwrap_or_else(|| AssetRecord {
                        path: String::new(),
                        name: imported.name.clone(),
                        kind: AssetKind::Photo,
                        source: None,
                        rendition: None,
                    });
                    record.path = imported.path.clone();
                    if let Err(error) = asset_library::write_record(&project, &record) {
                        let _ = std::fs::remove_file(project.join(&imported.path));
                        return Err(error);
                    }
                    drop(staged);
                    Ok::<_, String>((imported, record))
                })
                .await;
            let cleanup_project = intent.project.clone();
            let mut cleanup = result
                .as_ref()
                .ok()
                .map(|(imported, record)| (cleanup_project.join(&imported.path), record.clone()));
            this.update_in(cx, |this, window, cx| {
                if this.assets.download_generation != generation {
                    return;
                }
                if !this.accepts_asset(generation, &intent, AssetKind::Photo) {
                    this.assets.importing = false;
                    this.assets.error = Some(tr::editor::assets::project_changed().into());
                    cx.notify();
                    return;
                }
                this.assets.importing = false;
                match result {
                    Ok((mut imported, record)) => {
                        imported.name = record.name.clone();
                        let path = imported.path.clone();
                        this.commit_image_import(0, intent.time, imported, window, cx);
                        if this.project.timeline.as_ref().is_some_and(|timeline| {
                            timeline
                                .image_segments
                                .iter()
                                .any(|segment| segment.path == path)
                        }) {
                            cleanup = None;
                            this.remember_project_asset(record, cx);
                            this.assets.error = None;
                        } else {
                            this.assets.error = this
                                .sidebar
                                .image_import_error
                                .clone()
                                .or_else(|| Some(tr::editor::assets::project_changed().into()));
                        }
                    }
                    Err(error) => this.assets.error = Some(error),
                }
                cx.notify();
            })
            .ok();
            if let Some((path, record)) = cleanup {
                if let Err(error) = asset_library::remove_source_if_matches(&cleanup_project, &record) {
                    tracing::warn!(%error, "Retaining unpublished media because its source record could not be removed");
                } else {
                    let _ = std::fs::remove_file(asset_library::record_path(&path));
                    let _ = std::fs::remove_file(path);
                }
            }
        })
        .detach();
    }

    fn reuse_asset(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.asset_import_busy() {
            return;
        }
        let Some(record) = self.assets.records.get(index).cloned() else {
            return;
        };
        let Some(source) = asset_library::project_asset_path(&self.project_path, &record.path)
        else {
            self.assets.error = Some(tr::editor::assets::missing_media().into());
            cx.notify();
            return;
        };
        let Some(intent) = self.asset_intent() else {
            return;
        };
        self.assets.cancel_import();
        let generation = self.assets.download_generation;
        if record.kind == AssetKind::Photo {
            self.reuse_image_asset(record, intent, generation, window, cx);
        } else if record.kind == AssetKind::Audio {
            self.reuse_audio_asset(record, source, intent, generation, window, cx);
        } else {
            match asset_library::ensure_reusable_record(&self.project_path, &record) {
                Ok(record) if self.reuse_video_asset(&record, window, cx) => {}
                Ok(_) => self.assets.error = Some(tr::editor::assets::missing_media().into()),
                Err(error) => self.assets.error = Some(error),
            }
            cx.notify();
        }
    }

    fn reuse_image_asset(
        &mut self,
        record: AssetRecord,
        intent: ImportIntent,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.assets.importing = true;
        self.assets.error = None;
        let project = intent.project.clone();
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let imported = cx
                .background_executor()
                .spawn(async move { asset_library::prepare_image_reuse(&project, &record) })
                .await;
            this.update_in(cx, |this, window, cx| {
                if this.assets.download_generation != generation {
                    return;
                }
                this.assets.importing = false;
                if !this.accepts_asset(generation, &intent, AssetKind::Photo) {
                    this.assets.error = Some(tr::editor::assets::project_changed().into());
                } else {
                    match imported {
                        Ok(imported) => {
                            let lane = intent.lane.as_ref().map_or(0, |target| target.lane);
                            if intent.lane.is_some()
                                && exact_asset_drop_range(
                                    &this.project,
                                    AssetKind::Photo,
                                    lane,
                                    intent.time,
                                    this.total_duration(),
                                    3.,
                                )
                                .is_none()
                            {
                                this.assets.error = Some(tr::editor::assets::drop_invalid().into());
                            } else {
                                this.commit_image_import(lane, intent.time, imported, window, cx);
                                this.assets.error = this.sidebar.image_import_error.clone();
                            }
                        }
                        Err(error) => this.assets.error = Some(error),
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn reuse_audio_asset(
        &mut self,
        record: AssetRecord,
        source: PathBuf,
        intent: ImportIntent,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.assets.importing = true;
        self.assets.error = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let duration = cx
                .background_executor()
                .spawn(async move { crate::editor_audio::validate_audio_duration(&source) })
                .await;
            this.update_in(cx, |this, window, cx| {
                if this.assets.download_generation != generation {
                    return;
                }
                this.assets.importing = false;
                if !this.accepts_asset(generation, &intent, AssetKind::Audio) {
                    this.assets.error = Some(tr::editor::assets::project_changed().into());
                } else {
                    match duration.and_then(|duration| {
                        this.commit_project_audio_asset(
                            record,
                            duration,
                            intent.time,
                            intent.lane.as_ref().map(|target| target.lane),
                            window,
                            cx,
                        )
                    }) {
                        Ok(()) => this.assets.error = None,
                        Err(error) => this.assets.error = Some(error),
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

fn decode_thumbnail(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().ok()?.thumbnail(400, 240).into_rgba8();
    Some(crate::library::rgba_to_render_image(image))
}

fn search_query(draft: &str, submitted: &str, new_query: bool) -> String {
    if new_query { draft.trim() } else { submitted }.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stock_credentials_cancelled_authorization_cannot_start_network_or_another_prompt() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let gate = Arc::new(tokio::sync::Semaphore::new(1));
        let requests = Arc::new(AtomicUsize::new(0));
        let (started, waiting) = tokio::sync::oneshot::channel();
        let (release, finish) = tokio::sync::oneshot::channel();
        let (cancel, mut cancelled) = tokio::sync::oneshot::channel();
        let worker_gate = gate.clone();
        let worker_requests = requests.clone();
        let first = tokio::spawn(async move {
            stock_request(
                &mut cancelled,
                read_stock_key_with(Provider::Unsplash, worker_gate, move |provider| {
                    assert_eq!(provider, Provider::Unsplash);
                    started.send(()).unwrap();
                    finish.blocking_recv().unwrap();
                    Some("synthetic-access".into())
                }),
                move |_| async move {
                    worker_requests.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                },
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(2), waiting)
            .await
            .unwrap()
            .unwrap();
        cancel.send(()).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), first)
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );
        assert_eq!(gate.available_permits(), 0);

        let second_reads = Arc::new(AtomicUsize::new(0));
        let worker_reads = second_reads.clone();
        let worker_gate = gate.clone();
        let worker_requests = requests.clone();
        let (queued, queue_started) = tokio::sync::oneshot::channel();
        let (cancel, mut cancelled) = tokio::sync::oneshot::channel();
        let second = tokio::spawn(async move {
            stock_request(
                &mut cancelled,
                async move {
                    queued.send(()).unwrap();
                    read_stock_key_with(Provider::Pixabay, worker_gate, move |_| {
                        worker_reads.fetch_add(1, Ordering::SeqCst);
                        Some("synthetic-other-access".into())
                    })
                    .await
                },
                move |_| async move {
                    worker_requests.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                },
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(2), queue_started)
            .await
            .unwrap()
            .unwrap();
        cancel.send(()).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), second)
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );
        release.send(()).unwrap();
        let permit = tokio::time::timeout(Duration::from_secs(2), gate.acquire())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second_reads.load(Ordering::SeqCst), 0);
        assert_eq!(requests.load(Ordering::SeqCst), 0);
        drop(permit);
    }

    #[tokio::test]
    async fn stock_credentials_read_only_selected_provider_and_stop_on_missing_key() {
        use std::sync::atomic::{AtomicBool, Ordering};

        for provider in [Provider::Unsplash, Provider::Pixabay] {
            let gate = Arc::new(tokio::sync::Semaphore::new(1));
            let (_cancel, mut cancelled) = tokio::sync::oneshot::channel();
            let result = stock_request(
                &mut cancelled,
                read_stock_key_with(provider, gate, move |selected| {
                    assert_eq!(selected, provider);
                    Some("synthetic-access".into())
                }),
                move |key| async move {
                    let credentials = credentials_for(provider, &key);
                    assert_eq!(
                        credentials.unsplash.is_some(),
                        provider == Provider::Unsplash
                    );
                    assert_eq!(credentials.pixabay.is_some(), provider == Provider::Pixabay);
                    Ok(())
                },
            )
            .await;
            assert!(matches!(result, Some(Ok(()))));
        }
        let request_started = Arc::new(AtomicBool::new(false));
        let worker_started = request_started.clone();
        let (_cancel, mut cancelled) = tokio::sync::oneshot::channel();
        let result = stock_request(
            &mut cancelled,
            read_stock_key_with(
                Provider::Unsplash,
                Arc::new(tokio::sync::Semaphore::new(1)),
                |_| None,
            ),
            move |_| async move {
                worker_started.store(true, Ordering::SeqCst);
                Ok(())
            },
        )
        .await;
        assert!(matches!(result, Some(Err(StockError::Credentials))));
        assert!(!request_started.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn stock_credentials_already_cancelled_request_never_reads_keychain() {
        let (cancel, mut cancelled) = tokio::sync::oneshot::channel();
        cancel.send(()).unwrap();
        let result = stock_request(
            &mut cancelled,
            read_stock_key_with(
                Provider::Unsplash,
                Arc::new(tokio::sync::Semaphore::new(1)),
                |_| {
                    panic!("cancelled request must not access keychain");
                },
            ),
            |_| async { Ok(()) },
        )
        .await;
        assert!(result.is_none());
    }

    #[test]
    fn asset_drop_geometry_obeys_zoom_scroll_kind_gaps_and_window_bounds() {
        let rows = [
            TrackRow {
                kind: TrackKind::Image,
                lane: 0,
            },
            TrackRow {
                kind: TrackKind::Audio,
                lane: 2,
            },
            TrackRow {
                kind: TrackKind::Image,
                lane: 3,
            },
        ];
        let stride = 40. + timeline::TRACK_ROW_GAP;
        let geometry = AssetDropGeometry {
            bounds: Bounds {
                origin: point(px(9.), px(300.)),
                size: size(px(1182.), px(90.)),
            },
            scroll_y: px(-stride),
            row_height: 40.,
            rows: &rows,
            viewport_width: 1200.,
            transform: Transform {
                zoom: 10.,
                position: 5.,
            },
            total: 20.,
        };
        let x = timeline::content_left() + timeline::content_width(1200.) * 0.25;
        let audio =
            asset_drop_location(&geometry, AssetKind::Audio, point(px(x), px(310.))).unwrap();
        assert_eq!(audio.lane, 2);
        assert!((audio.time - 7.5).abs() < 0.00001);
        assert_eq!(audio.bounds.top(), px(300.));
        let image =
            asset_drop_location(&geometry, AssetKind::Photo, point(px(x), px(310. + stride)))
                .unwrap();
        assert_eq!(image.lane, 3);
        for (kind, pointer) in [
            (AssetKind::Photo, point(px(x), px(310.))),
            (AssetKind::Video, point(px(x), px(310.))),
            (
                AssetKind::Audio,
                point(px(timeline::content_left() - 1.), px(310.)),
            ),
            (AssetKind::Audio, point(px(x), px(299.))),
            (AssetKind::Audio, point(px(x), px(390.))),
            (
                AssetKind::Audio,
                point(px(x), px(340. + timeline::TRACK_ROW_GAP / 2.)),
            ),
            (AssetKind::Audio, point(px(1201.), px(310.))),
            (AssetKind::Audio, point(px(f32::NAN), px(310.))),
        ] {
            assert!(asset_drop_location(&geometry, kind, pointer).is_none());
        }
        let shortened = AssetDropGeometry {
            total: 7.,
            ..geometry
        };
        assert!(
            asset_drop_location(&shortened, AssetKind::Audio, point(px(x), px(310.))).is_some()
        );
    }

    fn drag_test_project() -> cap_project::ProjectConfiguration {
        cap_project::ProjectConfiguration {
            timeline: Some(serde_json::from_value(serde_json::json!({
                "segments": [{"start":0,"end":12,"timescale":1}], "zoomSegments": [],
                "imageSegments": [{"start":2,"end":5,"track":1}],
                "audioSegments": [{"start":2,"end":5,"track":1,"path":"assets/audio/example.wav"}]
            })).unwrap()),
            ..Default::default()
        }
    }

    #[test]
    fn asset_drop_on_an_occupied_lane_still_places_the_asset() {
        let project = drag_test_project();
        for kind in [AssetKind::Photo, AssetKind::Audio] {
            assert_eq!(
                exact_asset_drop_range(&project, kind, 1, 3., 12., 3.),
                Some(3.0..6.0)
            );
            assert_eq!(
                exact_asset_drop_range(&project, kind, 1, 5., 12., 3.),
                Some(5.0..8.0)
            );
            assert_eq!(
                exact_asset_drop_range(&project, kind, 0, 3., 12., 3.),
                Some(3.0..6.0)
            );
            assert_eq!(
                exact_asset_drop_range(&project, kind, 1, 11., 12., 3.),
                Some(11.0..14.0)
            );
            assert_eq!(
                exact_asset_drop_range(&project, kind, 1, 12., 12., 3.),
                Some(12.0..15.0)
            );
        }
        assert_eq!(project.timeline.as_ref().unwrap().image_segments.len(), 1);
        assert_eq!(project.timeline.as_ref().unwrap().audio_segments.len(), 1);
    }

    #[test]
    fn asset_drop_pending_lane_guard_tracks_target_edits_and_explicit_rows() {
        let mut project = drag_test_project();
        let before = asset_lane_identity(&project, AssetKind::Photo, 1, 2);
        project.timeline.as_mut().unwrap().audio_segments[0].volume_db = -6.;
        assert_eq!(
            asset_lane_identity(&project, AssetKind::Photo, 1, 2),
            before
        );
        assert_ne!(
            asset_lane_identity(&project, AssetKind::Photo, 1, 1),
            before
        );
        project.timeline.as_mut().unwrap().image_segments[0].end = 6.;
        assert_ne!(
            asset_lane_identity(&project, AssetKind::Photo, 1, 2),
            before
        );
        project.timeline.as_mut().unwrap().image_segments[0].end = 5.;
        project
            .timeline
            .as_mut()
            .unwrap()
            .text_segments
            .push(crate::editor_edits::default_text_segment(8., 9., 1));
        assert_ne!(
            asset_lane_identity(&project, AssetKind::Photo, 1, 2),
            before
        );
        project.timeline.as_mut().unwrap().text_segments.clear();
        project.timeline.as_mut().unwrap().image_segments.clear();
        assert_ne!(
            asset_lane_identity(&project, AssetKind::Photo, 1, 2),
            before
        );
    }

    #[test]
    fn asset_drop_threshold_and_cancel_keep_late_audio_probe_from_reviving_drag() {
        let origin = point(px(10.), px(10.));
        assert!(!asset_drag_threshold(origin, point(px(12.), px(14.))));
        assert!(asset_drag_threshold(origin, point(px(13.), px(14.))));
        assert!(!asset_drag_threshold(
            origin,
            point(px(f32::INFINITY), px(14.))
        ));
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut state = AssetLibraryState::default();
        state.drag = Some(AssetDrag {
            record: AssetRecord {
                path: "assets/audio/example.wav".into(),
                name: "Example".into(),
                kind: AssetKind::Audio,
                source: None,
                rendition: None,
            },
            intent: ImportIntent {
                project: file.path().into(),
                time: 0.,
                timeline: String::new(),
                metadata: file.as_file().metadata().unwrap(),
                lane: None,
            },
            generation: state.download_generation,
            press: origin,
            pointer: origin,
            active: true,
            duration: None,
            probe_started: true,
            error: None,
        });
        let generation = state.download_generation;
        assert!(state.complete_drag_probe(generation, Ok(8.)));
        assert_eq!(state.drag.as_ref().unwrap().duration, Some(8.));
        state.cancel_import();
        assert!(!state.complete_drag_probe(generation, Ok(9.)));
        assert!(state.drag.is_none());
    }

    #[test]
    fn asset_timeline_identity_rejects_changed_holds_but_keeps_cosmetic_text_edits() {
        let mut project = cap_project::ProjectConfiguration {
            timeline: Some(
                serde_json::from_value(serde_json::json!({
                    "segments": [{"start": 0, "end": 10, "timescale": 1}],
                    "zoomSegments": [],
                    "textSegments": [{"start": 2, "end": 4, "layout": "fullscreen", "content": "Title"}]
                }))
                .unwrap(),
            ),
            ..Default::default()
        };
        let original = timeline_identity(&project);
        project.timeline.as_mut().unwrap().text_segments[0].content = "New title".into();
        assert_eq!(timeline_identity(&project), original);
        project.timeline.as_mut().unwrap().text_segments[0].end = 5.;
        assert_ne!(timeline_identity(&project), original);
        project.timeline.as_mut().unwrap().text_segments[0].end = 4.;
        project.timeline.as_mut().unwrap().text_segments[0].enabled = false;
        assert_ne!(timeline_identity(&project), original);
        project.timeline.as_mut().unwrap().text_segments[0].enabled = true;
        project.timeline.as_mut().unwrap().text_segments[0].layout =
            cap_project::TextLayout::Overlay;
        assert_ne!(timeline_identity(&project), original);
        let without_hold = timeline_identity(&project);
        project.timeline.as_mut().unwrap().text_segments[0].start = 3.;
        assert_eq!(timeline_identity(&project), without_hold);
        project.timeline.as_mut().unwrap().text_segments.push(
            serde_json::from_value(serde_json::json!({
                "start": 6, "end": 8, "layout": "fullscreen"
            }))
            .unwrap(),
        );
        assert_ne!(timeline_identity(&project), without_hold);
    }

    fn browser_test_asset() -> Asset {
        Asset {
            provider: Provider::Pixabay,
            kind: MediaKind::Video,
            id: "123".into(),
            description: "Mountain".into(),
            thumbnail_url: None,
            renditions: [480, 1080]
                .into_iter()
                .map(|height| crate::stock_media::Rendition {
                    label: format!("{height}p"),
                    url: format!("https://cdn.pixabay.com/video/2026/11/07/{height}.mp4"),
                    width: Some(height * 16 / 9),
                    height: Some(height),
                    approximate_bytes: None,
                })
                .collect(),
            duration_seconds: Some(10),
            attribution: crate::stock_media::Attribution {
                author: "Creator".into(),
                author_url: "https://pixabay.com/users/creator-123/".into(),
                source_url: "https://pixabay.com/videos/id-123/".into(),
                license_name: "Pixabay Content License".into(),
                license_url: "https://pixabay.com/service/license-summary/".into(),
                api_attribution_required: false,
                search_source_credit_required: true,
                additional_rights_may_apply: true,
            },
            download_location: None,
        }
    }

    #[test]
    fn stock_retry_deadlines_survive_search_cancel_provider_switch_and_reload() {
        let now = Instant::now();
        let mut state = AssetLibraryState::default();
        state.wait_before_retry(Provider::Pixabay, 42, now);
        state.cancel_search();
        state.provider = Provider::Unsplash;
        assert_eq!(state.retry_seconds(state.provider, now), 0);
        state.provider = Provider::Pixabay;
        assert_eq!(
            state.retry_seconds(state.provider, now + Duration::from_millis(100)),
            42
        );
        state.wait_before_retry(Provider::Pixabay, 1, now);
        let snapshot = state.browser_snapshot(true, "new query".into());
        let mut restored = AssetLibraryState::default();
        let _ = restored.restore_browser(snapshot);
        assert_eq!(
            restored.retry_seconds(Provider::Pixabay, now + Duration::from_secs(41)),
            1
        );
        assert_eq!(
            restored.retry_seconds(Provider::Pixabay, now + Duration::from_secs(42)),
            0
        );
        assert_eq!(restored.retry_seconds(Provider::Unsplash, now), 0);
    }

    #[test]
    fn cached_or_unspecified_quota_never_restarts_a_retry_deadline() {
        let mut page = crate::stock_media::SearchPage {
            provider: Provider::Pixabay,
            kind: MediaKind::Photo,
            page: 1,
            per_page: 12,
            total: 0,
            available: 0,
            next_page: None,
            fetched_at: 0,
            expires_at: 0,
            from_cache: false,
            rate_limit: crate::stock_media::RateLimit {
                limit: Some(100),
                remaining: Some(0),
                reset_seconds: Some(30),
            },
            assets: Vec::new(),
        };
        assert_eq!(page_retry_seconds(&page), Some(30));
        page.from_cache = true;
        assert_eq!(page_retry_seconds(&page), None);
        page.from_cache = false;
        page.rate_limit.reset_seconds = None;
        assert_eq!(page_retry_seconds(&page), None);
        page.rate_limit.reset_seconds = Some(30);
        page.rate_limit.remaining = Some(1);
        assert_eq!(page_retry_seconds(&page), None);
        page.rate_limit.remaining = Some(0);
        page.provider = Provider::Unsplash;
        assert_eq!(page_retry_seconds(&page), None);
    }

    #[test]
    fn thumbnail_failure_retry_and_late_callbacks_preserve_current_results() {
        let now = Instant::now();
        let mut asset = browser_test_asset();
        asset.thumbnail_url = Some("https://cdn.pixabay.com/video/preview.jpg".into());
        let key = asset_key(&asset);
        let mut state = AssetLibraryState::default();
        state.source = AssetSource::Online;
        state.results = vec![asset.clone()];
        let (cancel, mut cancelled) = tokio::sync::oneshot::channel();
        let _ = state.thumbnail_cancel.insert(key.clone(), cancel);
        let old = state.generation;
        state.cancel_search();
        assert!(cancelled.try_recv().is_ok());
        assert!(!state.finish_thumbnail(old, &asset, Err(StockError::Network), now));
        assert!(state.thumbnail_errors.is_empty());
        let mut obsolete = asset.clone();
        obsolete.thumbnail_url = Some("https://cdn.pixabay.com/video/old.jpg".into());
        assert!(!state.finish_thumbnail(
            state.generation,
            &obsolete,
            Err(StockError::Timeout),
            now
        ));
        state.source = AssetSource::Local;
        assert!(!state.finish_thumbnail(state.generation, &asset, Err(StockError::Timeout), now));
        state.source = AssetSource::Online;
        assert!(state.finish_thumbnail(
            state.generation,
            &asset,
            Err(StockError::Http {
                status: 429,
                retry_after_seconds: Some(2)
            }),
            now
        ));
        assert_eq!(seconds_remaining(state.thumbnail_errors[&key].1, now), 2);
        assert_eq!(
            seconds_remaining(state.thumbnail_errors[&key].1, now + Duration::from_secs(2)),
            0
        );
        assert!(decode_thumbnail(b"not an image").is_none());
        assert!(state.finish_thumbnail(
            state.generation,
            &asset,
            Err(StockError::UnsupportedMedia),
            now
        ));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 2)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let image = decode_thumbnail(bytes.get_ref()).unwrap();
        assert!(state.finish_thumbnail(state.generation, &asset, Ok(image), now));
        assert!(state.thumbnails.contains_key(&key));
        assert!(!state.thumbnail_errors.contains_key(&key));
    }

    #[test]
    fn browser_snapshot_restores_page_results_selection_and_separate_draft() {
        let asset = browser_test_asset();
        let mut original = AssetLibraryState::default();
        original.source = AssetSource::Online;
        original.provider = Provider::Pixabay;
        original.kind = MediaKind::Video;
        original.submitted_query = "mountain".into();
        original.searched = true;
        original.page = 2;
        original.next_page = Some(3);
        original.results = vec![asset.clone()];
        original.selected = Some(0);
        original.rendition = 1;
        original.importing = true;
        original.downloading = true;
        original.progress = (1024, Some(2048));
        original.error = Some("old failure".into());
        original.records = vec![AssetRecord {
            path: "content/old.mp4".into(),
            name: "Old".into(),
            kind: AssetKind::Video,
            source: None,
            rendition: None,
        }];
        original.invalid_record_metadata = true;
        let snapshot = original.browser_snapshot(true, "city".into());
        let mut restored = AssetLibraryState::default();
        let (open, draft) = restored.restore_browser(snapshot);
        assert!(open);
        assert!(matches!(restored.source, AssetSource::Online));
        assert_eq!(restored.provider, Provider::Pixabay);
        assert_eq!(restored.kind, MediaKind::Video);
        assert_eq!(draft, "city");
        assert_eq!(restored.submitted_query, "mountain");
        assert_eq!(
            search_query(&draft, &restored.submitted_query, false),
            "mountain"
        );
        assert!(restored.searched);
        assert_eq!(restored.page, 2);
        assert_eq!(restored.next_page, Some(3));
        assert_eq!(
            serde_json::to_value(&restored.results).unwrap(),
            serde_json::json!([asset])
        );
        assert_eq!(restored.selected, Some(0));
        assert_eq!(restored.rendition, 1);
        assert!(!restored.importing);
        assert!(!restored.downloading);
        assert!(!restored.searching);
        assert!(restored.search_cancel.is_none());
        assert!(restored.download_cancel.is_none());
        assert!(restored.tracking.is_none());
        assert!(restored.error.is_none());
        assert_eq!(restored.progress, (0, None));
        assert!(restored.records.is_empty());
        assert!(!restored.invalid_record_metadata);
    }

    #[test]
    fn browser_snapshot_cancels_old_workers_and_restore_invalidates_pending_jobs() {
        let mut original = AssetLibraryState::default();
        let (search, mut old_search) = tokio::sync::oneshot::channel();
        let (download, mut old_download) = tokio::sync::oneshot::channel();
        original.search_cancel = Some(search);
        original.download_cancel = Some(download);
        original.searching = true;
        original.importing = true;
        let old_generations = (
            original.generation,
            original.download_generation,
            original.records_generation,
        );
        let snapshot = original.browser_snapshot(true, String::new());
        assert!(old_search.try_recv().is_ok());
        assert!(old_download.try_recv().is_ok());
        assert_ne!(original.generation, old_generations.0);
        assert_ne!(original.download_generation, old_generations.1);
        assert_ne!(original.records_generation, old_generations.2);

        let mut restored = AssetLibraryState::default();
        let (search, mut new_search) = tokio::sync::oneshot::channel();
        let (download, mut new_download) = tokio::sync::oneshot::channel();
        restored.search_cancel = Some(search);
        restored.download_cancel = Some(download);
        let generations = (
            restored.generation,
            restored.download_generation,
            restored.records_generation,
        );
        let _ = restored.restore_browser(snapshot);
        assert!(new_search.try_recv().is_ok());
        assert!(new_download.try_recv().is_ok());
        assert_ne!(restored.generation, generations.0);
        assert_ne!(restored.download_generation, generations.1);
        assert_ne!(restored.records_generation, generations.2);
    }

    #[test]
    fn browser_snapshot_preserves_closed_library_and_overrides_provider_default() {
        let mut original = AssetLibraryState::default();
        original.source = AssetSource::Local;
        original.provider = Provider::Unsplash;
        original.kind = MediaKind::Photo;
        let snapshot = original.browser_snapshot(false, " saved draft ".into());
        let mut restored = AssetLibraryState::default();
        restored.provider = Provider::Pixabay;
        let (open, draft) = restored.restore_browser(snapshot);
        assert!(!open);
        assert!(matches!(restored.source, AssetSource::Local));
        assert_eq!(restored.provider, Provider::Unsplash);
        assert_eq!(restored.kind, MediaKind::Photo);
        assert_eq!(draft, " saved draft ");
        assert!(!restored.searched);
    }

    #[test]
    fn pagination_keeps_submitted_query_when_search_input_changes() {
        assert_eq!(search_query("city", "mountain", false), "mountain");
        assert_eq!(search_query(" city ", "mountain", true), "city");
    }

    #[test]
    fn cancelling_import_advances_generation_and_signals_worker() {
        let mut state = AssetLibraryState::default();
        let generation = state.download_generation;
        let (send, mut receive) = tokio::sync::oneshot::channel();
        state.download_cancel = Some(send);
        state.importing = true;
        state.cancel_import();
        assert!(receive.try_recv().is_ok());
        assert_ne!(generation, state.download_generation);
        assert!(!state.importing);
    }
}
