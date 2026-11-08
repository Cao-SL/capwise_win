use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use cap_project::{
    ProjectConfiguration, TextSegment, TitleCardBackground, TitleCardError, TitleCardImageFit,
    TitleCardRole, TitleCardTemplate, XY,
};
use gpui::{
    AnyElement, AsyncWindowContext, Context, Hsla, IntoElement, ParentElement, Styled, Window, div,
    px,
};

use crate::{
    asset_library::{self, AssetKind, AssetRecord},
    editor_edits::{self, Selection},
    editor_panels::{FieldKey, PanelSlider},
    editor_sidebar::{ColorTarget, SliderKey},
    editor_timeline::TrackKind,
    editor_window::EditorWindow,
    i18n::tr,
    ui,
};

#[derive(Default)]
pub(crate) struct TitleCardUi {
    pub(crate) cover: crate::editor_cover::CoverUi,
    pub(crate) library: crate::editor_title_card_presets::TitleCardLibraryUi,
    pub(crate) error: Option<String>,
    pub(crate) picking_image: bool,
    pub(crate) applying: bool,
    export_request: Option<Arc<()>>,
    export_task: Option<gpui::Task<()>>,
    exported: Option<String>,
}

impl TitleCardUi {
    fn exports(&self, request: &Arc<()>) -> bool {
        self.export_request
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, request))
    }

    fn cancel_export(&mut self) {
        self.export_request = None;
        self.export_task = None;
    }
}

fn card_export_time(segment: &TextSegment) -> Option<f64> {
    if !segment.enabled
        || !segment.start.is_finite()
        || !segment.end.is_finite()
        || segment.start < 0.0
        || segment.end <= segment.start
    {
        return None;
    }
    let frame = ((segment.start + (segment.end - segment.start) * 0.5) * 30.0).floor();
    if frame > f64::from(u32::MAX) {
        return None;
    }
    let time = (frame + 0.25) / 30.0;
    let rendered_time = (time * 30.0).floor() / 30.0;
    (time >= segment.start
        && time < segment.end
        && rendered_time >= segment.start
        && rendered_time < segment.end)
        .then_some(time)
}

pub(crate) fn title_card_lane(project: &ProjectConfiguration) -> u32 {
    let Some(timeline) = project.timeline.as_ref() else {
        return 0;
    };
    timeline
        .text_segments
        .iter()
        .find(|segment| {
            segment.title_card.is_some()
                && timeline.overlay_spans().all(|(kind, index, layer, _, _)| {
                    layer != segment.track
                        || (kind == cap_project::OverlayTrackKind::Text
                            && timeline.text_segments[index].title_card.is_some())
                })
        })
        .map_or_else(|| timeline.overlay_layer_count(), |segment| segment.track)
}

fn protected_image_paths(project: &Path, values: &[serde_json::Value]) -> Vec<PathBuf> {
    fn collect(root: &Path, value: &serde_json::Value, paths: &mut Vec<PathBuf>) {
        match value {
            serde_json::Value::Array(values) => {
                for value in values {
                    collect(root, value, paths);
                }
            }
            serde_json::Value::Object(object) => {
                for (key, value) in object {
                    if key == "path"
                        && let Some(path) = value.as_str()
                        && let Ok(path) = root.join(path).canonicalize()
                    {
                        paths.push(path);
                    } else {
                        collect(root, value, paths);
                    }
                }
            }
            _ => {}
        }
    }
    let mut paths = Vec::new();
    for value in values {
        collect(project, value, &mut paths);
    }
    paths
}

#[derive(Debug, PartialEq)]
struct PngFileVersion {
    length: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64),
}

fn png_file_version(path: &Path) -> Result<Option<PngFileVersion>, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => {
            #[cfg(unix)]
            use std::os::unix::fs::MetadataExt as _;
            Ok(Some(PngFileVersion {
                length: metadata.len(),
                modified: metadata.modified().ok(),
                created: metadata.created().ok(),
                #[cfg(unix)]
                identity: (metadata.dev(), metadata.ino()),
            }))
        }
        Ok(_) => Err(tr::editor::title_cards::export_invalid_path().into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

struct PngDestination {
    path: PathBuf,
    project_root: PathBuf,
    previous: Option<PngFileVersion>,
}

impl PngDestination {
    fn capture(path: &Path, project: &Path, protected: &[PathBuf]) -> Result<Self, String> {
        if !path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
        {
            return Err(tr::editor::title_cards::export_invalid_path().into());
        }
        let parent = path
            .parent()
            .ok_or_else(|| tr::editor::title_cards::export_invalid_path().to_string())?
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let name = path
            .file_name()
            .ok_or_else(|| tr::editor::title_cards::export_invalid_path().to_string())?;
        let mut path = parent.join(name);
        let project_root = project.canonicalize().map_err(|error| error.to_string())?;
        let previous = png_file_version(&path)?;
        if previous.is_some() {
            path = path.canonicalize().map_err(|error| error.to_string())?;
        }
        let destination = Self {
            path,
            project_root,
            previous,
        };
        destination.ensure_unprotected(protected)?;
        Ok(destination)
    }

    fn ensure_unprotected(&self, protected: &[PathBuf]) -> Result<(), String> {
        if protected.contains(&self.path)
            || (self.previous.is_some() && self.path.starts_with(self.project_root.join("content")))
        {
            return Err(tr::editor::title_cards::export_protected().into());
        }
        Ok(())
    }

    fn publish(&self, staged: &StagedPng, protected: &[PathBuf]) -> Result<(), String> {
        self.ensure_unprotected(protected)?;
        if png_file_version(&self.path)? != self.previous {
            return Err(tr::editor::title_cards::export_target_changed().into());
        }
        if self.previous.is_some() {
            fs::rename(&staged.path, &self.path).map_err(|error| error.to_string())
        } else {
            fs::hard_link(&staged.path, &self.path).map_err(|error| error.to_string())
        }
    }
}

struct StagedPng {
    path: PathBuf,
}

impl StagedPng {
    fn write(destination: &PngDestination, bytes: &[u8]) -> Result<Self, String> {
        let path = destination
            .path
            .with_file_name(format!(".capwise-cover-{}.tmp", uuid::Uuid::new_v4()));
        let mut file = fs::File::create_new(&path).map_err(|error| error.to_string())?;
        let staged = Self { path };
        let result = file
            .write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| error.to_string());
        drop(file);
        result?;
        Ok(staged)
    }
}

impl Drop for StagedPng {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[derive(Debug)]
pub(crate) enum CardEditError {
    DedicatedControl,
    Invalid(TitleCardError),
}

pub(crate) fn normalize_edit(
    project: &mut ProjectConfiguration,
    previous: &ProjectConfiguration,
    explicit: bool,
) -> Result<(), CardEditError> {
    let result = (|| {
        if !explicit {
            for role in [TitleCardRole::Intro, TitleCardRole::Outro] {
                if let Some(before) = previous.title_card(role) {
                    let Some(after) = project.title_card(role) else {
                        return Err(CardEditError::DedicatedControl);
                    };
                    let before_card = before.title_card.as_ref().unwrap();
                    let after_card = after.title_card.as_ref().unwrap();
                    if before_card.id != after_card.id
                        || before.enabled != after.enabled
                        || ((before.end - before.start) - (after.end - after.start)).abs() > 1e-8
                        || (role == TitleCardRole::Intro && after.start != 0.0)
                    {
                        return Err(CardEditError::DedicatedControl);
                    }
                }
            }
        }
        let moved_outro = !explicit
            && previous
                .title_card(TitleCardRole::Outro)
                .zip(project.title_card(TitleCardRole::Outro))
                .is_some_and(|(before, after)| (before.start - after.start).abs() > 1e-8);
        project
            .normalize_title_cards()
            .map_err(CardEditError::Invalid)?;
        if moved_outro
            && previous
                .title_card(TitleCardRole::Outro)
                .zip(project.title_card(TitleCardRole::Outro))
                .is_some_and(|(before, after)| (before.start - after.start).abs() <= 1e-8)
        {
            return Err(CardEditError::DedicatedControl);
        }
        Ok(())
    })();
    if result.is_err() {
        *project = previous.clone();
    }
    result
}

pub(crate) fn edit_error_message(error: &CardEditError) -> String {
    match error {
        CardEditError::DedicatedControl => tr::editor::title_cards::dedicated_controls().into(),
        CardEditError::Invalid(error) => card_error_message(error),
    }
}

fn card_preview_time(segment: &TextSegment, fps: u32) -> Option<f64> {
    if fps == 0
        || !segment.start.is_finite()
        || !segment.end.is_finite()
        || segment.start < 0.0
        || segment.end <= segment.start
    {
        return None;
    }
    let frame = (segment.start * f64::from(fps)).ceil();
    if frame > f64::from(u32::MAX) {
        return None;
    }
    let time = frame / f64::from(fps);
    (time >= segment.start && time < segment.end).then_some(time)
}

pub(crate) fn card_error_message(error: &TitleCardError) -> String {
    match error {
        TitleCardError::ContentBeforeIntro => tr::editor::title_cards::content_before_intro(),
        TitleCardError::MissingTimeline | TitleCardError::MissingFootage => {
            tr::editor::title_cards::missing_footage()
        }
        TitleCardError::MissingCard => tr::editor::title_cards::changed(),
        TitleCardError::InvalidDuration => tr::editor::title_cards::invalid_duration(),
        TitleCardError::DuplicateRole | TitleCardError::DuplicateId => {
            tr::editor::title_cards::duplicate_card()
        }
        TitleCardError::UnanchoredIntro => tr::editor::title_cards::dedicated_controls(),
        TitleCardError::InvalidTimeline => tr::editor::title_cards::invalid_timeline(),
        TitleCardError::UnsupportedTemplateVersion => {
            tr::editor::title_cards::unsupported_template()
        }
        TitleCardError::InvalidMetadata(_) => tr::editor::title_cards::invalid_content(),
    }
    .into()
}

fn image_target(
    project: &ProjectConfiguration,
    original: &cap_project::TitleCard,
) -> Option<usize> {
    project
        .timeline
        .as_ref()?
        .text_segments
        .iter()
        .position(|segment| {
            segment.title_card.as_ref().is_some_and(|current| {
                current.id == original.id && current.background == original.background
            })
        })
}

impl EditorWindow {
    fn title_card_export_current(
        &self,
        request: &Arc<()>,
        project_path: &Path,
        instance: &std::sync::Weak<cap_editor::EditorInstance>,
    ) -> bool {
        self.sidebar.title_cards.exports(request)
            && self.project_path == project_path
            && self
                .instance
                .as_ref()
                .is_some_and(|current| std::sync::Weak::ptr_eq(&Arc::downgrade(current), instance))
    }

    fn export_title_card_image(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sidebar.title_cards.export_request.is_some() || !self.project_ready() {
            return;
        }
        self.stop_canvas_text_edit(cx);
        self.end_field_edit(cx);
        self.close_color_picker(cx);
        let Some(segment) = self
            .project
            .timeline
            .as_ref()
            .and_then(|timeline| timeline.text_segments.get(index))
        else {
            return;
        };
        let Some(card) = &segment.title_card else {
            return;
        };
        let Some(time) = card_export_time(segment) else {
            self.title_card_error(
                tr::editor::title_cards::export_invalid_card().into(),
                window,
                cx,
            );
            return;
        };
        let filename = match card.role {
            TitleCardRole::Intro => "cover.png",
            TitleCardRole::Outro => "outro.png",
        };
        let instance = self.instance.as_ref().unwrap();
        let metadata = instance.meta().clone();
        let instance = Arc::downgrade(instance);
        let project_path = self.project_path.clone();
        let snapshot = self.project.clone();
        let force = cfg!(target_os = "windows")
            || crate::store::load()
                .export
                .is_some_and(|prefs| prefs.force_ffmpeg_decoder);
        self.stop_playback(cx);
        let request = Arc::new(());
        self.sidebar.title_cards.error = None;
        self.sidebar.title_cards.exported = None;
        self.sidebar.title_cards.export_request = Some(request.clone());
        self.sidebar.title_cards.export_task = Some(cx.spawn_in(
            window,
            async move |this, cx: &mut AsyncWindowContext| {
                let result: Result<Option<(PngDestination, StagedPng, u32, u32)>, String> = async {
                    if !this
                        .update_in(cx, |this, _, _| {
                            this.title_card_export_current(&request, &project_path, &instance)
                        })
                        .unwrap_or(false)
                    {
                        return Ok(None);
                    }
                    #[cfg(target_os = "macos")]
                    let chosen = crate::platform::try_save_file_panel(filename, &["png"])?;
                    #[cfg(not(target_os = "macos"))]
                    let chosen =
                        crate::platform::save_file_panel_async(filename, &["png"], cx).await;
                    let Some(chosen) = chosen else {
                        return Ok(None);
                    };
                    let destination = PngDestination::capture(&chosen, &project_path, &[])?;
                    if !this
                        .update_in(cx, |this, _, _| {
                            this.title_card_export_current(&request, &project_path, &instance)
                        })
                        .unwrap_or(false)
                    {
                        return Ok(None);
                    }
                    let root = project_path.clone();
                    let (destination, snapshot) = cx
                        .background_executor()
                        .spawn(async move {
                            let protected_values = [
                                serde_json::to_value(&snapshot)
                                    .map_err(|error| error.to_string())?,
                                serde_json::to_value(&metadata)
                                    .map_err(|error| error.to_string())?,
                            ];
                            let protected = protected_image_paths(&root, &protected_values);
                            destination.ensure_unprotected(&protected)?;
                            Ok::<_, String>((destination, snapshot))
                        })
                        .await?;
                    let root = project_path.clone();
                    let image = gpui_tokio::Tokio::spawn(cx, async move {
                        cap_export::preview::render_still_with_config(
                            root,
                            snapshot,
                            time,
                            XY::new(1920, 1920),
                            force,
                        )
                        .await
                    })
                    .await
                    .map_err(|error| error.to_string())?
                    .map_err(|error| error.to_string())?;
                    if !this
                        .update_in(cx, |this, _, _| {
                            this.title_card_export_current(&request, &project_path, &instance)
                        })
                        .unwrap_or(false)
                    {
                        return Ok(None);
                    }
                    cx.background_executor()
                        .spawn(async move {
                            let staged = StagedPng::write(&destination, &image.png)?;
                            Ok(Some((destination, staged, image.width, image.height)))
                        })
                        .await
                }
                .await;
                this.update_in(cx, |this, window, cx| {
                    if !this.title_card_export_current(&request, &project_path, &instance) {
                        if this.sidebar.title_cards.exports(&request) {
                            this.sidebar.title_cards.cancel_export();
                            cx.notify();
                        }
                        return;
                    }
                    this.sidebar.title_cards.cancel_export();
                    let saved = result.and_then(|result| {
                        let Some((destination, staged, width, height)) = result else {
                            return Ok(None);
                        };
                        let values = [serde_json::to_value(&this.project)
                            .map_err(|error| error.to_string())?];
                        let protected = protected_image_paths(&project_path, &values);
                        destination.publish(&staged, &protected)?;
                        Ok(Some(tr::editor::title_cards::export_saved(
                            format!("{width} × {height}"),
                            destination.path.display(),
                        )))
                    });
                    match saved {
                        Ok(Some(message)) => {
                            this.sidebar.title_cards.exported = Some(message.clone());
                            this.show_notice(message, window, cx);
                        }
                        Ok(None) => cx.notify(),
                        Err(error) => this.title_card_error(error, window, cx),
                    }
                    cx.notify();
                })
                .ok();
            },
        ));
        cx.notify();
    }

    pub(crate) fn title_card_edit(
        &mut self,
        reason: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut ProjectConfiguration) -> Result<bool, TitleCardError>,
    ) -> bool {
        if !self.project_ready() || self.voiceover.is_busy() {
            return false;
        }
        let mut next = self.project.clone();
        let changed = match change(&mut next) {
            Ok(changed) => changed,
            Err(error) => {
                self.title_card_error(card_error_message(&error), window, cx);
                return false;
            }
        };
        if !changed {
            return false;
        }
        self.sidebar.title_cards.error = None;
        self.sidebar.title_cards.applying = true;
        self.edit_project_global(reason, window, cx, move |project| {
            *project = next;
            true
        });
        self.sidebar.title_cards.applying = false;
        true
    }

    pub(crate) fn title_card_error(
        &mut self,
        message: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar.title_cards.error = Some(message.clone());
        self.show_notice(message, window, cx);
    }

    pub(crate) fn open_title_card(
        &mut self,
        role: TitleCardRole,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.end_field_edit(cx);
        self.close_color_picker(cx);
        self.sidebar.title_cards.error = None;
        if self.project.title_card(role).is_none() {
            let title = match role {
                TitleCardRole::Intro => tr::editor::title_cards::default_intro(),
                TitleCardRole::Outro => tr::editor::title_cards::default_outro(),
            };
            let mut segment = TextSegment::new_title_card(role, title);
            segment.track = title_card_lane(&self.project);
            if !self.title_card_edit("title-card-create", window, cx, move |project| {
                project
                    .upsert_title_card(segment)
                    .map(|change| change.changed)
            }) {
                return;
            }
        }
        self.focus_title_card(role, cx);
    }

    pub(crate) fn focus_title_card(&mut self, role: TitleCardRole, cx: &mut Context<Self>) {
        let Some((index, time, end, lane)) = self.project.timeline.as_ref().and_then(|timeline| {
            timeline
                .text_segments
                .iter()
                .enumerate()
                .find_map(|(index, segment)| {
                    segment
                        .title_card
                        .as_ref()
                        .filter(|card| card.role == role)
                        .and_then(|_| {
                            card_preview_time(segment, crate::editor_window::EDITOR_PREVIEW_FPS)
                                .map(|time| (index, time, segment.end, segment.track))
                        })
                })
        }) else {
            return;
        };
        self.clips.open = false;
        self.set_selection(Some(Selection::single(TrackKind::Text, index)), cx);
        self.seek_to_time(time, cx);
        self.reveal_timeline_range(time, end, cx);
        self.reveal_timeline_row(TrackKind::Text, lane, cx);
    }

    pub(crate) fn update_title_card(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut TextSegment),
    ) {
        self.title_card_edit("title-card-update", window, cx, move |project| {
            let Some(mut segment) = project
                .timeline
                .as_ref()
                .and_then(|timeline| timeline.text_segments.get(index))
                .cloned()
            else {
                return Err(TitleCardError::MissingCard);
            };
            if segment.title_card.is_none() {
                return Err(TitleCardError::MissingCard);
            }
            change(&mut segment);
            project
                .upsert_title_card(segment)
                .map(|change| change.changed)
        });
    }

    pub(crate) fn commit_title_card_field(
        &mut self,
        index: usize,
        key: FieldKey,
        text: String,
        final_commit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(key, FieldKey::TitleCardDuration(_)) && !final_commit {
            return;
        }
        if matches!(key, FieldKey::TitleCardDuration(_)) {
            let Some(duration) = ui::parse_number(&text).filter(|value| value.is_finite()) else {
                self.title_card_error(
                    tr::editor::title_cards::invalid_duration().into(),
                    window,
                    cx,
                );
                return;
            };
            self.update_title_card(index, window, cx, move |segment| {
                segment.end = segment.start + duration;
            });
            return;
        }
        self.update_title_card(index, window, cx, move |segment| {
            let Some(card) = segment.title_card.as_mut() else {
                return;
            };
            match key {
                FieldKey::TitleCardSubtitle(_) => card.subtitle = text,
                FieldKey::TitleCardKicker(_) => card.kicker = text,
                _ => {}
            }
        });
    }

    pub(crate) fn delete_title_card_selection(
        &mut self,
        selection: &Selection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<bool> {
        if selection.track != TrackKind::Text {
            return None;
        }
        let timeline = self.project.timeline.as_ref()?;
        let roles: Vec<_> = selection
            .indices
            .iter()
            .filter_map(|&index| {
                timeline
                    .text_segments
                    .get(index)?
                    .title_card
                    .as_ref()
                    .map(|card| card.role)
            })
            .collect();
        if roles.is_empty() {
            return None;
        }
        let indices = selection.indices.clone();
        Some(
            self.title_card_edit("title-card-delete", window, cx, move |project| {
                let timeline = project
                    .timeline
                    .as_mut()
                    .ok_or(TitleCardError::MissingTimeline)?;
                let ordinary: Vec<_> = indices
                    .into_iter()
                    .filter(|&index| {
                        timeline
                            .text_segments
                            .get(index)
                            .is_some_and(|segment| segment.title_card.is_none())
                    })
                    .collect();
                editor_edits::delete_segments(timeline, TrackKind::Text, &ordinary);
                for role in roles {
                    project.remove_title_card(role)?;
                }
                Ok(true)
            }),
        )
    }

    fn pick_title_card_image(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.asset_import_busy() {
            self.show_notice(tr::editor::assets::busy(), window, cx);
            return;
        }
        let Some(card) = self
            .project
            .timeline
            .as_ref()
            .and_then(|timeline| timeline.text_segments.get(index))
            .and_then(|segment| segment.title_card.clone())
        else {
            return;
        };
        self.end_field_edit(cx);
        self.close_color_picker(cx);
        self.sidebar.title_cards.error = None;
        self.sidebar.title_cards.picking_image = true;
        let project_path = self.project_path.clone();
        cx.spawn_in(window, async move |this, cx: &mut AsyncWindowContext| {
            #[cfg(target_os = "linux")]
            let source = crate::platform::open_file_panel_async(
                &[(
                    tr::editor::window::image::filter(),
                    crate::import::OVERLAY_IMAGE_EXTENSIONS,
                )],
                None,
                cx,
            )
            .await;
            #[cfg(not(target_os = "linux"))]
            let source = crate::import::pick_import_file(
                &[(
                    tr::editor::window::image::filter(),
                    crate::import::OVERLAY_IMAGE_EXTENSIONS,
                )],
                cx,
            )
            .await;
            if this.update_in(cx, |_, _, _| ()).is_err() {
                return;
            }
            let imported = match source {
                Some(source) => Some(
                    cx.background_executor()
                        .spawn(async move {
                            let imported =
                                crate::import::import_editor_image(&project_path, &source)?;
                            asset_library::ensure_record(
                                &project_path,
                                &AssetRecord {
                                    path: imported.path,
                                    name: imported.name,
                                    kind: AssetKind::Photo,
                                    source: None,
                                    rendition: None,
                                },
                            )
                        })
                        .await,
                ),
                None => None,
            };
            this.update_in(cx, |this, window, cx| {
                this.sidebar.title_cards.picking_image = false;
                match imported {
                    Some(Ok(record)) => {
                        this.remember_project_asset(record.clone(), cx);
                        let current = image_target(&this.project, &card);
                        if let Some(index) = current {
                            this.update_title_card(index, window, cx, move |segment| {
                                segment.title_card.as_mut().unwrap().background =
                                    TitleCardBackground::ProjectImage {
                                        path: record.path,
                                        fit: TitleCardImageFit::Cover,
                                        focus: XY::new(0.5, 0.5),
                                        dim: 0.4,
                                    };
                            });
                        } else {
                            this.title_card_error(
                                tr::editor::title_cards::changed().into(),
                                window,
                                cx,
                            );
                        }
                    }
                    Some(Err(error)) => this.title_card_error(error, window, cx),
                    None => cx.notify(),
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn render_title_card_entries(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div().flex().gap(px(6.)).children(
                    [
                        (
                            TitleCardRole::Intro,
                            "create-intro",
                            tr::editor::title_cards::intro(),
                        ),
                        (
                            TitleCardRole::Outro,
                            "create-outro",
                            tr::editor::title_cards::outro(),
                        ),
                    ]
                    .into_iter()
                    .map(|(role, id, label)| {
                        div().flex_1().min_w_0().child(
                            ui::Button::plain(
                                &theme,
                                id,
                                ui::ButtonVariant::Gray,
                                ui::ButtonSize::Sm,
                            )
                            .label(label)
                            .icon("icons/plus.svg")
                            .full_width()
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.open_title_card(role, window, cx);
                                },
                            )),
                        )
                    }),
                ),
            )
            .children(self.sidebar.title_cards.error.as_ref().map(|error| {
                div()
                    .text_size(px(12.))
                    .text_color(Hsla::from(theme.editor.text_2))
                    .child(error.clone())
            }))
            .child(self.render_title_card_library(None, cx))
            .into_any_element()
    }

    pub(crate) fn render_title_card_panel(
        &self,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let Some(segment) = self
            .project
            .timeline
            .as_ref()
            .and_then(|timeline| timeline.text_segments.get(index))
        else {
            return div().into_any_element();
        };
        let Some(card) = segment.title_card.as_ref() else {
            return div().into_any_element();
        };
        let role = card.role;
        let template = card.template;
        let solid_color = match &card.background {
            TitleCardBackground::Solid { color } => Some(color.clone()),
            TitleCardBackground::ProjectImage { .. } => None,
        };
        let label = match role {
            TitleCardRole::Intro => tr::editor::title_cards::intro(),
            TitleCardRole::Outro => tr::editor::title_cards::outro(),
        };
        let exporting = self.sidebar.title_cards.export_request.is_some();
        let dimensions = self.instance.as_ref().map(|instance| {
            cap_rendering::ProjectUniforms::get_output_size(
                &instance.render_constants.options,
                &self.project,
                XY::new(1920, 1920),
            )
        });
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(
                ui::EditorButton::plain(&theme, "edit-text-on-canvas")
                    .left_icon("icons/pencil.svg")
                    .label(tr::editor::canvas::edit_text())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.begin_canvas_text_edit(index, window, cx);
                    })),
            )
            .child(
                ui::Field::stacked(&theme, tr::editor::title_cards::title())
                    .child(self.render_field_input(FieldKey::TextContent(index), Some(72.))),
            )
            .child(self.render_text_font_controls(index, true, cx))
            .child(
                ui::Field::stacked(&theme, tr::editor::panel::text_segment::color()).child(
                    self.render_color_input(ColorTarget::TextColor(index), &segment.color, cx),
                ),
            )
            .child(
                self.more_settings("title-card-text-effects", cx, move |this, cx| {
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .child(this.slider_field(
                            tr::editor::panel::text_segment::line_height(),
                            SliderKey::Panel(PanelSlider::TextLineHeight, index),
                            "",
                            cx,
                        ))
                        .child(this.slider_field(
                            tr::editor::panel::text_segment::letter_spacing(),
                            SliderKey::Panel(PanelSlider::TextLetterSpacing, index),
                            "px",
                            cx,
                        ))
                        .child(this.slider_field(
                            tr::editor::panel::text_segment::outline(),
                            SliderKey::Panel(PanelSlider::TextStroke, index),
                            "px",
                            cx,
                        ))
                        .children((segment.stroke_width > 0.).then(|| {
                            this.render_color_input(
                                ColorTarget::TextStroke(index),
                                &segment.stroke_color,
                                cx,
                            )
                        }))
                        .child(this.slider_field(
                            tr::editor::panel::text_segment::shadow(),
                            SliderKey::Panel(PanelSlider::TextShadow, index),
                            "x100%",
                            cx,
                        ))
                        .child(this.slider_field(
                            tr::editor::panel::text_segment::glow(),
                            SliderKey::Panel(PanelSlider::TextGlow, index),
                            "x100%",
                            cx,
                        ))
                        .child(this.slider_field(
                            tr::editor::panel::opacity(),
                            SliderKey::Panel(PanelSlider::TextOpacity, index),
                            "x100%",
                            cx,
                        ))
                        .into_any_element()
                }),
            )
            .child(self.render_title_card_library(Some(role), cx))
            .child(
                ui::Field::section(&theme, label).child(
                    div()
                        .text_size(px(12.))
                        .text_color(Hsla::from(theme.editor.text_2))
                        .child(match role {
                            TitleCardRole::Intro => tr::editor::title_cards::hint(),
                            TitleCardRole::Outro => tr::editor::title_cards::outro_hint(),
                        }),
                ),
            )
            .children(self.sidebar.title_cards.error.as_ref().map(|error| {
                div()
                    .text_size(px(12.))
                    .text_color(Hsla::from(theme.editor.text_2))
                    .child(error.clone())
            }))
            .child(
                ui::Field::stacked(&theme, tr::editor::title_cards::template()).child(
                    ui::SegmentedControl::editor(
                        &theme,
                        "title-card-template",
                        [
                            (TitleCardTemplate::Hero, tr::editor::title_cards::hero()),
                            (
                                TitleCardTemplate::Editorial,
                                tr::editor::title_cards::editorial(),
                            ),
                            (TitleCardTemplate::Split, tr::editor::title_cards::split()),
                        ]
                        .into_iter()
                        .map(|(value, label)| ui::SegmentOption::new(label, value == template))
                        .collect(),
                    )
                    .on_select(cx.listener(
                        move |this, selected: &usize, window, cx| {
                            let template = [
                                TitleCardTemplate::Hero,
                                TitleCardTemplate::Editorial,
                                TitleCardTemplate::Split,
                            ][*selected];
                            this.update_title_card(index, window, cx, move |segment| {
                                segment.title_card.as_mut().unwrap().template = template
                            });
                        },
                    )),
                ),
            )
            .child(
                ui::Field::stacked(&theme, tr::editor::title_cards::subtitle())
                    .child(self.render_field_input(FieldKey::TitleCardSubtitle(index), Some(56.))),
            )
            .child(
                ui::Field::stacked(&theme, tr::editor::title_cards::kicker())
                    .child(self.render_field_input(FieldKey::TitleCardKicker(index), None)),
            )
            .child(
                ui::Field::inline(&theme, tr::editor::title_cards::duration()).child(
                    self.render_number_field(
                        FieldKey::TitleCardDuration(index),
                        tr::editor::title_cards::seconds(),
                        64.,
                    ),
                ),
            )
            .child(
                ui::Field::section(&theme, tr::editor::title_cards::background()).child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex()
                                .gap(px(6.))
                                .child(
                                    ui::Button::plain(
                                        &theme,
                                        "title-card-solid",
                                        ui::ButtonVariant::Gray,
                                        ui::ButtonSize::Sm,
                                    )
                                    .label(tr::editor::title_cards::solid())
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            this.update_title_card(index, window, cx, |segment| {
                                                segment.title_card.as_mut().unwrap().background =
                                                    TitleCardBackground::default()
                                            });
                                        },
                                    )),
                                )
                                .child(
                                    ui::Button::plain(
                                        &theme,
                                        "title-card-image",
                                        ui::ButtonVariant::Gray,
                                        ui::ButtonSize::Sm,
                                    )
                                    .label(tr::editor::title_cards::image())
                                    .disabled(self.asset_import_busy())
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            this.pick_title_card_image(index, window, cx)
                                        },
                                    )),
                                ),
                        )
                        .children(solid_color.map(|color| {
                            self.render_color_input(
                                ColorTarget::TitleCardBackground(index),
                                &color,
                                cx,
                            )
                        })),
                ),
            )
            .child(self.render_cover_generator(index, cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        ui::Button::plain(
                            &theme,
                            "title-card-export-png",
                            ui::ButtonVariant::Gray,
                            ui::ButtonSize::Sm,
                        )
                        .label(if exporting {
                            tr::editor::title_cards::exporting()
                        } else {
                            match role {
                                TitleCardRole::Intro => tr::editor::title_cards::export_cover(),
                                TitleCardRole::Outro => tr::editor::title_cards::export_image(),
                            }
                        })
                        .disabled(
                            exporting
                                || self.sidebar.title_cards.picking_image
                                || !self.project_ready(),
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.export_title_card_image(index, window, cx)
                            },
                        )),
                    )
                    .children(dimensions.map(|(width, height)| {
                        div()
                            .text_size(px(12.))
                            .text_color(Hsla::from(theme.editor.text_2))
                            .child(tr::editor::title_cards::export_size(format!(
                                "{width} × {height}"
                            )))
                    }))
                    .children(exporting.then(|| {
                        ui::Button::plain(
                            &theme,
                            "title-card-cancel-png",
                            ui::ButtonVariant::Gray,
                            ui::ButtonSize::Sm,
                        )
                        .label(tr::editor::title_cards::export_cancel())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.sidebar.title_cards.cancel_export();
                            cx.notify();
                        }))
                    }))
                    .children(self.sidebar.title_cards.exported.as_ref().map(|message| {
                        div()
                            .text_size(px(12.))
                            .text_color(Hsla::from(theme.editor.text_2))
                            .child(message.clone())
                    })),
            )
            .child(
                ui::Button::plain(
                    &theme,
                    "title-card-remove",
                    ui::ButtonVariant::Gray,
                    ui::ButtonSize::Sm,
                )
                .label(tr::editor::title_cards::remove())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.end_field_edit(cx);
                    if this.title_card_edit("title-card-remove", window, cx, |project| {
                        project.remove_title_card(role)
                    }) {
                        this.set_selection(None, cx);
                    }
                })),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor_edits::ProjectHistory;

    fn fixture() -> ProjectConfiguration {
        serde_json::from_value(serde_json::json!({
            "timeline": {
                "segments": [{ "recordingSegment": 0, "timescale": 1.0, "start": 0.0, "end": 10.0 }],
                "zoomSegments": [{ "start": 1.0, "end": 3.0, "amount": 1.5, "mode": "auto" }]
            }
        })).unwrap()
    }

    fn cards() -> ProjectConfiguration {
        let mut project = fixture();
        let mut intro = TextSegment::new_title_card(TitleCardRole::Intro, "开场");
        intro.end = 2.0;
        project.upsert_title_card(intro).unwrap();
        project
            .upsert_title_card(TextSegment::new_title_card(TitleCardRole::Outro, "结束"))
            .unwrap();
        project
    }

    fn assert_same(left: &ProjectConfiguration, right: &ProjectConfiguration) {
        assert_eq!(
            serde_json::to_value(left).unwrap(),
            serde_json::to_value(right).unwrap()
        );
    }

    #[test]
    fn new_cards_share_a_dedicated_lane_but_leave_regular_text_lanes_alone() {
        let mut project = cards();
        assert_eq!(title_card_lane(&project), 0);
        let mut regular = project.timeline.as_ref().unwrap().text_segments[0].clone();
        regular.title_card = None;
        project
            .timeline
            .as_mut()
            .unwrap()
            .text_segments
            .push(regular);
        assert_eq!(title_card_lane(&project), 1);
        project.timeline.as_mut().unwrap().text_segments[1].track = 2;
        assert_eq!(title_card_lane(&project), 2);
    }

    #[test]
    fn card_preview_focus_uses_the_first_frame_inside_fractional_and_short_cards() {
        let fps = crate::editor_window::EDITOR_PREVIEW_FPS;
        for (start, duration, expected_frame) in [
            (0.0, 0.1, 0),
            (2.0, 2.0, 120),
            (15.44, 0.1, 927),
            (15.45, 0.1, 927),
        ] {
            let mut segment = TextSegment::new_title_card(TitleCardRole::Outro, "结束");
            segment.start = start;
            segment.end = start + duration;
            let time = card_preview_time(&segment, fps).unwrap();
            let actual_frame = (time * f64::from(fps)).round() as u32;
            assert_eq!(actual_frame, expected_frame);
            let rendered = f64::from(actual_frame) / f64::from(fps);
            assert!(rendered >= segment.start && rendered < segment.end);
            if actual_frame > 0 {
                assert!(f64::from(actual_frame - 1) / f64::from(fps) < segment.start);
            }
        }
    }

    #[test]
    fn card_preview_focus_rejects_invalid_or_unrepresentable_ranges() {
        let mut segment = TextSegment::new_title_card(TitleCardRole::Outro, "结束");
        for (start, end) in [
            (f64::NAN, 2.0),
            (0.0, f64::INFINITY),
            (-1.0, 1.0),
            (2.0, 2.0),
            (0.001, 0.002),
            (f64::from(u32::MAX), f64::from(u32::MAX) + 2.0),
        ] {
            segment.start = start;
            segment.end = end;
            assert!(card_preview_time(&segment, 60).is_none());
        }
        assert!(
            card_preview_time(
                &TextSegment::new_title_card(TitleCardRole::Intro, "开场"),
                0
            )
            .is_none()
        );
    }

    #[test]
    fn body_speed_and_outro_are_one_undo_transaction() {
        let previous = cards();
        let mut history = ProjectHistory::new(previous.clone());
        let mut edited = previous.clone();
        assert!(editor_edits::set_clip_segment_timescale(
            edited.timeline.as_mut().unwrap(),
            0,
            2.0
        ));
        normalize_edit(&mut edited, &previous, false).unwrap();
        assert_eq!(edited.title_card(TitleCardRole::Intro).unwrap().end, 2.0);
        assert_eq!(edited.title_card(TitleCardRole::Outro).unwrap().start, 7.0);
        assert_eq!(edited.title_card(TitleCardRole::Outro).unwrap().end, 9.0);
        history.record(&edited);
        assert_same(history.undo().unwrap(), &previous);
        assert_same(history.redo().unwrap(), &edited);
    }

    #[test]
    fn cut_body_reanchors_outro_without_trimming_card_duration() {
        let previous = cards();
        let mut edited = previous.clone();
        assert_eq!(
            editor_edits::cut_clip_range(edited.timeline.as_mut().unwrap(), 0, 3.0, 5.0),
            editor_edits::RangeCut::Applied
        );
        normalize_edit(&mut edited, &previous, false).unwrap();
        let outro = edited.title_card(TitleCardRole::Outro).unwrap();
        assert_eq!((outro.start, outro.end), (10.0, 12.0));
        assert_eq!(edited.title_card(TitleCardRole::Intro).unwrap().end, 2.0);
    }

    #[test]
    fn explicit_intro_duration_shifts_body_once_and_undo_restores_both() {
        let previous = cards();
        let mut edited = previous.clone();
        let mut history = ProjectHistory::new(previous.clone());
        edited
            .set_title_card_duration(TitleCardRole::Intro, 4.0)
            .unwrap();
        normalize_edit(&mut edited, &previous, true).unwrap();
        assert_eq!(
            edited.timeline.as_ref().unwrap().zoom_segments[0].start,
            5.0
        );
        assert_eq!(edited.title_card(TitleCardRole::Outro).unwrap().start, 14.0);
        history.record(&edited);
        assert_same(history.undo().unwrap(), &previous);
    }

    #[test]
    fn generic_trim_disable_and_delete_are_rejected_without_losing_layers() {
        let previous = cards();
        for operation in 0..4 {
            let mut edited = previous.clone();
            let segments = &mut edited.timeline.as_mut().unwrap().text_segments;
            match operation {
                0 => segments[0].end = 3.0,
                1 => segments[0].enabled = false,
                2 => {
                    segments.remove(0);
                }
                _ => {
                    segments[1].start += 1.0;
                    segments[1].end += 1.0;
                }
            }
            assert!(matches!(
                normalize_edit(&mut edited, &previous, false),
                Err(CardEditError::DedicatedControl)
            ));
            assert_same(&edited, &previous);
        }
    }

    #[test]
    fn duplicated_title_card_is_rejected_and_previous_live_edit_survives() {
        let initial = cards();
        let mut history = ProjectHistory::new(initial.clone());
        history.pause();
        let mut published = initial.clone();
        published.timeline.as_mut().unwrap().text_segments[0].content = "已编辑的标题".into();
        history.record(&published);
        let mut attempted = published.clone();
        let duplicate = attempted.timeline.as_ref().unwrap().text_segments[0].clone();
        attempted
            .timeline
            .as_mut()
            .unwrap()
            .text_segments
            .push(duplicate);
        assert!(matches!(
            normalize_edit(&mut attempted, &published, false),
            Err(CardEditError::Invalid(TitleCardError::DuplicateRole))
        ));
        assert_same(&attempted, &published);
        history.resume(&attempted);
        assert_same(history.undo().unwrap(), &initial);
        assert_same(history.redo().unwrap(), &published);
    }

    #[test]
    fn explicit_remove_intro_retimes_overlays_and_keeps_outro() {
        let previous = cards();
        let mut edited = previous.clone();
        assert!(edited.remove_title_card(TitleCardRole::Intro).unwrap());
        normalize_edit(&mut edited, &previous, true).unwrap();
        assert_eq!(
            edited.timeline.as_ref().unwrap().zoom_segments[0].start,
            1.0
        );
        assert_eq!(edited.title_card(TitleCardRole::Outro).unwrap().start, 10.0);
    }

    #[test]
    fn old_projects_are_not_given_cards_or_retimed() {
        let previous = fixture();
        let mut edited = previous.clone();
        edited.timeline.as_mut().unwrap().segments[0].end = 8.0;
        let expected = edited.clone();
        normalize_edit(&mut edited, &previous, false).unwrap();
        assert_same(&edited, &expected);
        assert!(edited.title_card(TitleCardRole::Intro).is_none());
    }

    #[test]
    fn image_picker_preserves_later_title_edits_but_refuses_replaced_background_or_card() {
        let mut project = cards();
        let original = project
            .title_card(TitleCardRole::Intro)
            .unwrap()
            .title_card
            .clone()
            .unwrap();
        project.timeline.as_mut().unwrap().text_segments[0].content = "新标题".into();
        assert_eq!(image_target(&project, &original), Some(0));
        project.timeline.as_mut().unwrap().text_segments[0]
            .title_card
            .as_mut()
            .unwrap()
            .background = TitleCardBackground::Solid {
            color: "#FFFFFF".into(),
        };
        assert_eq!(image_target(&project, &original), None);
        project.remove_title_card(TitleCardRole::Intro).unwrap();
        assert_eq!(image_target(&project, &original), None);
    }

    #[test]
    fn title_card_png_time_stays_inside_non_frame_aligned_cards_after_renderer_quantization() {
        let mut card = TextSegment::new_title_card(TitleCardRole::Outro, "Ending");
        for (start, duration) in [(15.44, 2.), (0., 0.1), (0.023, 0.1), (123.499999, 0.1)] {
            card.start = start;
            card.end = start + duration;
            let time = card_export_time(&card).unwrap();
            let rendered = (time * 30.).floor() / 30.;
            assert!((card.start..card.end).contains(&time));
            assert!((card.start..card.end).contains(&rendered));
        }
        card.enabled = false;
        assert!(card_export_time(&card).is_none());
        card.enabled = true;
        card.end = f64::NAN;
        assert!(card_export_time(&card).is_none());
    }

    #[test]
    fn title_card_png_publication_is_atomic_and_cancel_removes_only_staging() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project.cap");
        fs::create_dir(&project).unwrap();
        let output = root.path().join("cover.png");
        let destination = PngDestination::capture(&output, &project, &[]).unwrap();
        let staged = StagedPng::write(&destination, b"complete image").unwrap();
        let temporary = staged.path.clone();
        assert!(!output.exists());
        drop(staged);
        assert!(!temporary.exists());
        assert!(!output.exists());
        let staged = StagedPng::write(&destination, b"complete image").unwrap();
        destination.publish(&staged, &[]).unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"complete image");
        drop(staged);
        let replacement = PngDestination::capture(&output, &project, &[]).unwrap();
        let staged = StagedPng::write(&replacement, b"replacement image").unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"complete image");
        replacement.publish(&staged, &[]).unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"replacement image");
    }

    #[test]
    fn title_card_png_rejects_new_or_changed_targets_after_save_confirmation() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project.cap");
        fs::create_dir(&project).unwrap();
        let output = root.path().join("cover.png");
        let new = PngDestination::capture(&output, &project, &[]).unwrap();
        let staged = StagedPng::write(&new, b"new export").unwrap();
        fs::write(&output, b"created elsewhere").unwrap();
        assert!(new.publish(&staged, &[]).is_err());
        let previous = PngDestination::capture(&output, &project, &[]).unwrap();
        fs::write(&output, b"edited after confirmation").unwrap();
        assert!(previous.publish(&staged, &[]).is_err());
        assert_eq!(fs::read(&output).unwrap(), b"edited after confirmation");
    }

    #[test]
    fn title_card_png_protects_internal_and_external_project_assets() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project.cap");
        fs::create_dir_all(project.join("content/images")).unwrap();
        let internal = project.join("content/images/source.png");
        fs::write(&internal, b"source pixels").unwrap();
        assert!(PngDestination::capture(&internal, &project, &[]).is_err());
        let external = root.path().join("background.png");
        fs::write(&external, b"background pixels").unwrap();
        let protected = protected_image_paths(
            &project,
            &[serde_json::json!({
                "background": {"source": {"type": "image", "path": external}}
            })],
        );
        assert!(PngDestination::capture(&external, &project, &protected).is_err());
        assert_eq!(fs::read(internal).unwrap(), b"source pixels");
        assert_eq!(fs::read(external).unwrap(), b"background pixels");
    }

    #[test]
    fn title_card_png_cancel_invalidates_old_request_before_a_new_export() {
        let first = Arc::new(());
        let mut state = TitleCardUi {
            export_request: Some(first.clone()),
            ..Default::default()
        };
        assert!(state.exports(&first));
        state.cancel_export();
        assert!(!state.exports(&first));
        let next = Arc::new(());
        state.export_request = Some(next.clone());
        assert!(!state.exports(&first));
        assert!(state.exports(&next));
    }
}
