use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    io::Write as _,
    path::{Path, PathBuf},
    sync::Arc,
};

use cap_project::{
    ProjectConfiguration, TitleCardBackground, TitleCardImageFit, TitleCardTemplate, XY,
};
use gpui::{
    AnyElement, AppContext as _, Context, Entity, Hsla, IntoElement, ParentElement, RenderImage,
    Styled, StyledImage, Window, div, img, px,
};

use crate::{
    ai_store::{self, Kind},
    asset_library::{self, AssetKind, AssetRecord, PendingGeneratedSource},
    cover_generation::{
        self, ArkBackgroundClient, BackgroundRequest, Dimensions, GeneratedBackground,
        GeneratedOrigin, GenerationContext, GenerationError, GenerationScope, TextArea,
    },
    editor_timeline::TrackKind,
    editor_window::EditorWindow,
    i18n::tr,
    settings_cover, ui,
};

#[derive(Default)]
pub(crate) struct CoverUi {
    open: bool,
    target: Option<String>,
    input: Option<Entity<ui::TextInputState>>,
    _input_observer: Option<gpui::Subscription>,
    scope: GenerationScope,
    generation: u64,
    context: Option<GenerationContext>,
    client: Option<Arc<ArkBackgroundClient>>,
    task: Option<gpui::Task<()>>,
    busy: bool,
    applying: bool,
    result: Option<Arc<GeneratedBackground>>,
    preview: Option<Arc<RenderImage>>,
    save_pending: Option<String>,
    error: Option<String>,
    saved: bool,
}

fn context_for(
    path: &Path,
    project: &ProjectConfiguration,
    target: &str,
) -> Option<GenerationContext> {
    project
        .timeline
        .as_ref()?
        .text_segments
        .iter()
        .find(|segment| {
            segment
                .title_card
                .as_ref()
                .is_some_and(|card| card.id == target)
        })?;
    let mut hash = DefaultHasher::new();
    serde_json::to_vec(project).ok()?.hash(&mut hash);
    Some(GenerationContext {
        project_id: path.to_string_lossy().into_owned(),
        target_id: target.into(),
        revision: hash.finish(),
    })
}

fn error_message(error: GenerationError) -> String {
    match error {
        GenerationError::Authentication
        | GenerationError::Permission
        | GenerationError::InvalidCredential => tr::editor::cover::authentication().into(),
        GenerationError::InvalidDescription => tr::editor::cover::description_required().into(),
        GenerationError::InvalidModel => tr::settings::cover::invalid_model().into(),
        GenerationError::UnsupportedDimensions => tr::editor::cover::unsupported_size().into(),
        GenerationError::Cancelled => tr::editor::cover::changed().into(),
        GenerationError::Timeout => tr::editor::cover::timeout().into(),
        GenerationError::RateLimited { .. } => tr::editor::cover::rate_limited().into(),
        GenerationError::ModelNotActivated => tr::editor::cover::model_not_activated().into(),
        GenerationError::HttpStatus(status) => tr::editor::cover::http_error(status),
        GenerationError::Rejected => tr::editor::cover::rejected().into(),
        GenerationError::Network => tr::editor::cover::network_error().into(),
        _ => tr::editor::cover::invalid_image().into(),
    }
}

fn thumbnail(bytes: &[u8]) -> Result<Arc<RenderImage>, GenerationError> {
    let mut rgba = image::load_from_memory(bytes)
        .map_err(|_| GenerationError::InvalidImage)?
        .thumbnail(640, 640)
        .to_rgba8();
    for pixel in rgba.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new(smallvec::smallvec![
        image::Frame::new(rgba)
    ])))
}

struct PreparedBackground {
    root: PathBuf,
    directory: fs::Metadata,
    media: PathBuf,
    identity: fs::Metadata,
    record: AssetRecord,
    source: Option<PendingGeneratedSource>,
    keep: bool,
}

impl PreparedBackground {
    fn write(
        project: &Path,
        bytes: &[u8],
        extension: &str,
        origin: GeneratedOrigin,
    ) -> Result<Self, String> {
        if !matches!(extension, "png" | "jpg") || !origin.is_valid() {
            return Err(tr::editor::cover::invalid_image().into());
        }
        let root = project
            .canonicalize()
            .map_err(|_| tr::editor::cover::save_failed().to_string())?;
        let directory = fs::symlink_metadata(&root)
            .map_err(|_| tr::editor::cover::save_failed().to_string())?;
        let mut parent = root.clone();
        for child in ["content", "images"] {
            parent.push(child);
            match fs::create_dir(&parent) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err(tr::editor::cover::save_failed().into()),
            }
            let metadata = fs::symlink_metadata(&parent)
                .map_err(|_| tr::editor::cover::save_failed().to_string())?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(tr::editor::cover::save_failed().into());
            }
        }
        let name = format!("generated-{}.{}", uuid::Uuid::new_v4(), extension);
        let media = parent.join(&name);
        let mut file = fs::File::create_new(&media)
            .map_err(|_| tr::editor::cover::save_failed().to_string())?;
        let identity = file
            .metadata()
            .map_err(|_| tr::editor::cover::save_failed().to_string())?;
        let mut staged = Self {
            root,
            directory,
            media,
            identity,
            record: AssetRecord {
                path: format!("content/images/{name}"),
                name: tr::editor::cover::asset_name().into(),
                kind: AssetKind::Photo,
                source: None,
                rendition: None,
            },
            source: None,
            keep: false,
        };
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| tr::editor::cover::save_failed().to_string())?;
        drop(file);
        staged.source = Some(PendingGeneratedSource::register(
            &staged.root,
            staged.record.clone(),
            origin,
        )?);
        Ok(staged)
    }

    fn retain(mut self) -> AssetRecord {
        if let Some(source) = self.source.take() {
            source.commit();
        }
        self.keep = true;
        self.record.clone()
    }
}

impl Drop for PreparedBackground {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        if let Some(mut source) = self.source.take()
            && source.rollback().is_err()
        {
            return;
        }
        if fs::symlink_metadata(&self.root)
            .is_ok_and(|current| asset_library::same_file_identity(&self.directory, &current))
            && fs::symlink_metadata(&self.media)
                .is_ok_and(|current| asset_library::same_file_identity(&self.identity, &current))
        {
            let _ = fs::remove_file(&self.media);
        }
    }
}

impl EditorWindow {
    fn cover_context(&self, target: &str) -> Option<GenerationContext> {
        if self.assistant.cover_active
            && self.sidebar.title_cards.cover.target.as_deref() == Some(target)
        {
            return context_for(&self.project_path, &self.project, target);
        }
        let index = self
            .selection
            .as_ref()
            .filter(|selection| selection.track == TrackKind::Text)?
            .indices
            .last()?;
        let selected = self
            .project
            .timeline
            .as_ref()?
            .text_segments
            .get(*index)?
            .title_card
            .as_ref()?;
        if selected.id != target {
            return None;
        }
        context_for(&self.project_path, &self.project, target)
    }

    pub(crate) fn open_assistant_cover(
        &mut self,
        description: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.project.timeline.as_ref().and_then(|timeline| {
            timeline.text_segments.iter().position(|segment| {
                segment
                    .title_card
                    .as_ref()
                    .is_some_and(|card| card.role == cap_project::TitleCardRole::Intro)
            })
        }) else {
            return;
        };
        self.focus_title_card(cap_project::TitleCardRole::Intro, cx);
        self.open_cover_generator(index, window, cx);
        if let Some(input) = &self.sidebar.title_cards.cover.input {
            input.update(cx, |input, cx| input.set_text(description, cx));
        }
        cx.notify();
    }

    fn open_cover_generator(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(card) = self
            .project
            .timeline
            .as_ref()
            .and_then(|timeline| timeline.text_segments.get(index))
            .and_then(|segment| segment.title_card.clone())
        else {
            return;
        };
        self.open_assistant(window, cx);
        self.assistant.cover_active = true;
        self.end_field_edit(cx);
        self.close_color_picker(cx);
        if self.sidebar.title_cards.cover.target.as_deref() != Some(&card.id) {
            self.sidebar.title_cards.cover = CoverUi::default();
        }
        if self.sidebar.title_cards.cover.input.is_none() {
            let input = cx.new(|cx| ui::TextInputState::multi_line(window, cx));
            input.update(cx, |input, _| {
                input.set_placeholder(tr::editor::cover::description_hint())
            });
            self.sidebar.title_cards.cover._input_observer =
                Some(cx.observe(&input, |_, _, cx| cx.notify()));
            self.sidebar.title_cards.cover.input = Some(input);
        }
        let state = &mut self.sidebar.title_cards.cover;
        state.target = Some(card.id);
        state.open = true;
        if let Some(input) = &state.input {
            window.focus(&input.read(cx).focus_handle(), cx);
        }
        cx.notify();
    }

    pub(crate) fn cancel_cover_generation(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.sidebar.title_cards.cover;
        if state.applying {
            return;
        }
        state.scope.cancel();
        state.generation = state.generation.wrapping_add(1);
        state.task = None;
        state.busy = false;
        state.open = false;
        if state.save_pending.is_none() {
            state.result = None;
            state.preview = None;
            state.context = None;
            state.error = None;
        }
        cx.notify();
    }

    pub(crate) fn generate_cover_background(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sidebar.title_cards.cover.busy
            || self.sidebar.title_cards.cover.applying
            || self.sidebar.title_cards.cover.save_pending.is_some()
            || !self.project_ready()
        {
            return;
        }
        let Some((provider, endpoint)) = settings_cover::configured_endpoint() else {
            self.sidebar.title_cards.cover.error = Some(tr::editor::cover::configure().into());
            cx.notify();
            return;
        };
        let model = endpoint.model;
        let Some(target) = self.sidebar.title_cards.cover.target.clone() else {
            return;
        };
        let Some(context) = self.cover_context(&target) else {
            return;
        };
        let Some(instance) = &self.instance else {
            return;
        };
        let size = cap_rendering::ProjectUniforms::get_output_size(
            &instance.render_constants.options,
            &self.project,
            XY::new(1920, 1920),
        );
        let description = self
            .sidebar
            .title_cards
            .cover
            .input
            .as_ref()
            .map(|input| input.read(cx).text().to_string())
            .unwrap_or_default();
        if description.trim().is_empty() || description.len() > 8_000 {
            self.sidebar.title_cards.cover.error =
                Some(tr::editor::cover::description_required().into());
            cx.notify();
            return;
        }
        let template = self
            .project
            .timeline
            .as_ref()
            .and_then(|timeline| {
                timeline.text_segments.iter().find_map(|segment| {
                    segment.title_card.as_ref().filter(|card| card.id == target)
                })
            })
            .map(|card| card.template);
        let text_area = match template {
            Some(TitleCardTemplate::Hero) => TextArea::Center,
            Some(TitleCardTemplate::Editorial) => TextArea::Left,
            _ => TextArea::Left,
        };
        let state = &mut self.sidebar.title_cards.cover;
        let client = match ArkBackgroundClient::for_endpoint(&endpoint.base_url, provider.is_ark())
            .map(Arc::new)
        {
            Ok(client) => client,
            Err(error) => {
                state.error = Some(error_message(error));
                cx.notify();
                return;
            }
        };
        state.client = Some(client.clone());
        state.generation = state.generation.wrapping_add(1);
        let generation = state.generation;
        let ticket = state.scope.begin(context.clone());
        state.context = Some(context.clone());
        state.busy = true;
        state.error = None;
        state.result = None;
        state.preview = None;
        state.saved = false;
        let request = BackgroundRequest {
            profile: cover_generation::profile_for_model(&model),
            model,
            target: Dimensions {
                width: size.0,
                height: size.1,
            },
            description,
            text_area,
        };
        state.task = Some(cx.spawn_in(window, async move |this, cx| {
            let Ok(task) = cx.update(|_, cx| {
                gpui_tokio::Tokio::spawn(cx, async move {
                    let key = tokio::task::spawn_blocking(move || {
                        ai_store::key(Kind::Image, provider.id())
                    })
                    .await
                    .map_err(|_| GenerationError::InvalidCredential)?
                    .ok_or(GenerationError::InvalidCredential)?;
                    let result = client.generate(&key, request, ticket).await?;
                    let permit = cover_generation::image_decode_permit().await?;
                    tokio::task::spawn_blocking(move || {
                        let _permit = permit;
                        let preview = thumbnail(&result.bytes)?;
                        Ok::<_, GenerationError>((Arc::new(result), preview))
                    })
                    .await
                    .map_err(|_| GenerationError::InvalidImage)?
                })
            }) else {
                return;
            };
            let result = task.await.unwrap_or(Err(GenerationError::Cancelled));
            this.update_in(cx, |this, window, cx| {
                if this.sidebar.title_cards.cover.generation != generation {
                    return;
                }
                let current = this.cover_context(&target);
                let state = &mut this.sidebar.title_cards.cover;
                state.busy = false;
                state.task = None;
                if current.as_ref() != Some(&context) {
                    state.scope.cancel();
                    state.error = Some(tr::editor::cover::changed().into());
                } else {
                    match result {
                        Ok((result, preview)) if state.scope.accepts(&result, &context) => {
                            state.result = Some(result);
                            state.preview = Some(preview);
                        }
                        Ok(_) => {}
                        Err(error) => state.error = Some(error_message(error)),
                    }
                }
                cx.notify();
                window.refresh();
            })
            .ok();
        }));
        cx.notify();
    }

    fn finish_cover_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.flush_pending_saves(cx) {
            Ok(()) => {
                let state = &mut self.sidebar.title_cards.cover;
                if let (Some(result), Some(context)) = (&state.result, &state.context) {
                    let _ = state.scope.acknowledge_applied(result, context);
                }
                state.save_pending = None;
                state.result = None;
                state.preview = None;
                state.saved = true;
                state.error = None;
            }
            Err(_) => {
                self.sidebar.title_cards.cover.error = Some(tr::editor::cover::save_failed().into())
            }
        }
        cx.notify();
        window.refresh();
    }

    fn apply_cover_background(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sidebar.title_cards.cover.busy
            || self.sidebar.title_cards.cover.applying
            || self.asset_import_busy()
            || self.voiceover.is_busy()
        {
            return;
        }
        let Some(target) = self.sidebar.title_cards.cover.target.clone() else {
            return;
        };
        if let Some(path) = self.sidebar.title_cards.cover.save_pending.clone() {
            let referenced = self.project.timeline.as_ref().is_some_and(|timeline| timeline.text_segments.iter().any(|segment| segment.title_card.as_ref().is_some_and(|card| card.id == target && matches!(&card.background, TitleCardBackground::ProjectImage { path: current, .. } if current == &path))));
            if referenced {
                self.finish_cover_save(window, cx);
            } else {
                let state = &mut self.sidebar.title_cards.cover;
                state.save_pending = None;
                state.result = None;
                state.preview = None;
                state.scope.cancel();
                self.sidebar.title_cards.cover.error = Some(tr::editor::cover::changed().into());
                cx.notify();
            }
            return;
        }
        let Some(context) = self.cover_context(&target) else {
            return;
        };
        let Some(result) = self.sidebar.title_cards.cover.result.clone() else {
            return;
        };
        if !self
            .sidebar
            .title_cards
            .cover
            .scope
            .accepts(&result, &context)
        {
            self.sidebar.title_cards.cover.error = Some(tr::editor::cover::changed().into());
            cx.notify();
            return;
        }
        self.sidebar.title_cards.cover.applying = true;
        self.sidebar.title_cards.cover.error = None;
        let project_path = self.project_path.clone();
        let generation = self.sidebar.title_cards.cover.generation;
        self.sidebar.title_cards.cover.task = Some(cx.spawn_in(window, async move |this, cx| {
            let prepared = cx
                .background_executor()
                .spawn(async move {
                    PreparedBackground::write(
                        &project_path,
                        &result.bytes,
                        result.extension,
                        result.origin.clone(),
                    )
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                if this.sidebar.title_cards.cover.generation != generation {
                    return;
                }
                this.sidebar.title_cards.cover.applying = false;
                this.sidebar.title_cards.cover.task = None;
                if this.cover_context(&target).as_ref() != Some(&context) {
                    this.sidebar.title_cards.cover.error =
                        Some(tr::editor::cover::changed().into());
                    cx.notify();
                    return;
                }
                match prepared {
                    Ok(prepared) => {
                        let path = prepared.record.path.clone();
                        let changed = this.title_card_edit(
                            "title-card-ai-background",
                            window,
                            cx,
                            |project| {
                                let Some(segment) =
                                    project.timeline.as_mut().and_then(|timeline| {
                                        timeline.text_segments.iter_mut().find(|segment| {
                                            segment
                                                .title_card
                                                .as_ref()
                                                .is_some_and(|card| card.id == target)
                                        })
                                    })
                                else {
                                    return Err(cap_project::TitleCardError::MissingCard);
                                };
                                segment.title_card.as_mut().unwrap().background =
                                    TitleCardBackground::ProjectImage {
                                        path: path.clone(),
                                        fit: TitleCardImageFit::Contain,
                                        focus: XY::new(0.5, 0.5),
                                        dim: 0.25,
                                    };
                                Ok(true)
                            },
                        );
                        if changed {
                            let record = prepared.retain();
                            this.remember_project_asset(record, cx);
                            this.sidebar.title_cards.cover.save_pending = Some(path);
                            this.finish_cover_save(window, cx);
                        }
                    }
                    Err(_) => {
                        this.sidebar.title_cards.cover.error =
                            Some(tr::editor::cover::save_failed().into())
                    }
                }
                cx.notify();
                window.refresh();
            })
            .ok();
        }));
        cx.notify();
    }

    pub(crate) fn assistant_cover_busy(&self) -> bool {
        let state = &self.sidebar.title_cards.cover;
        state.busy || state.applying || state.save_pending.is_some()
    }

    pub(crate) fn render_assistant_cover(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = &self.sidebar.title_cards.cover;
        if !state.open {
            return None;
        }
        let index = self
            .project
            .timeline
            .as_ref()?
            .text_segments
            .iter()
            .position(|segment| {
                segment
                    .title_card
                    .as_ref()
                    .is_some_and(|card| Some(card.id.as_str()) == state.target.as_deref())
            })?;
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(12.))
                .p(px(12.))
                .border_1()
                .border_color(Hsla::from(self.theme.editor.line))
                .rounded(px(12.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            gpui::svg()
                                .path("icons/image.svg")
                                .size(px(16.))
                                .text_color(Hsla::from(self.theme.editor.text_2)),
                        )
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(tr::editor::assistant::cover()),
                        ),
                )
                .child(
                    ui::Field::stacked(&self.theme, tr::editor::title_cards::title()).child(
                        self.render_field_input(
                            crate::editor_panels::FieldKey::TextContent(index),
                            Some(56.),
                        ),
                    ),
                )
                .child(
                    ui::Field::inline(&self.theme, tr::editor::title_cards::duration()).child(
                        self.render_number_field(
                            crate::editor_panels::FieldKey::TitleCardDuration(index),
                            tr::editor::title_cards::seconds(),
                            64.,
                        ),
                    ),
                )
                .child(self.render_cover_generator(index, cx))
                .into_any_element(),
        )
    }

    pub(crate) fn render_cover_generator(
        &self,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let state = &self.sidebar.title_cards.cover;
        let target = self
            .project
            .timeline
            .as_ref()
            .and_then(|timeline| timeline.text_segments.get(index))
            .and_then(|segment| segment.title_card.as_ref())
            .map(|card| card.id.as_str());
        let mut body = div().flex().flex_col().gap(px(10.));
        if !self.assistant.cover_active {
            body = body.child(
                ui::Button::plain(
                    &theme,
                    "cover-ai-open",
                    ui::ButtonVariant::Gray,
                    ui::ButtonSize::Sm,
                )
                .label(tr::editor::cover::open())
                .disabled(state.applying)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_cover_generator(index, window, cx)
                })),
            );
        }
        if !state.open || state.target.as_deref() != target {
            return body.into_any_element();
        }
        body = body.child(
            div()
                .text_size(px(11.))
                .text_color(Hsla::from(theme.editor.text_2))
                .child(tr::editor::cover::network_notice()),
        );
        if settings_cover::configured_model().is_none() {
            body = body.child(
                div()
                    .text_size(px(12.))
                    .child(tr::editor::cover::configure()),
            );
        }
        if let Some(input) = &state.input {
            body = body.child(
                ui::TextInput::plain(&theme, "cover-visual-description", input)
                    .height(px(88.))
                    .line_height(px(20.))
                    .padding_y(px(8.)),
            );
        }
        body =
            body.child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        ui::Button::plain(
                            &theme,
                            "cover-ai-generate",
                            ui::ButtonVariant::Primary,
                            ui::ButtonSize::Sm,
                        )
                        .label(if state.busy {
                            tr::editor::cover::generating()
                        } else {
                            tr::editor::cover::generate()
                        })
                        .disabled(
                            state.busy
                                || state.applying
                                || state.save_pending.is_some()
                                || settings_cover::configured_model().is_none(),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.generate_cover_background(window, cx)
                        })),
                    )
                    .child(
                        ui::Button::plain(
                            &theme,
                            "cover-ai-cancel",
                            ui::ButtonVariant::Gray,
                            ui::ButtonSize::Sm,
                        )
                        .label(tr::common::cancel())
                        .disabled(state.applying)
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_cover_generation(cx))),
                    ),
            );
        if let Some(preview) = &state.preview {
            body = body.child(
                img(preview.clone())
                    .w_full()
                    .h(px(170.))
                    .object_fit(gpui::ObjectFit::Contain),
            );
        }
        if let Some(result) = &state.result {
            body = body
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(Hsla::from(theme.editor.text_2))
                        .child(tr::editor::cover::dimensions(
                            result.origin.actual.width,
                            result.origin.actual.height,
                            result.origin.target.width,
                            result.origin.target.height,
                        )),
                )
                .child(
                    ui::Button::plain(
                        &theme,
                        "cover-ai-apply",
                        ui::ButtonVariant::Primary,
                        ui::ButtonSize::Sm,
                    )
                    .label(if state.save_pending.is_some() {
                        tr::editor::cover::retry_save()
                    } else if state.applying {
                        tr::editor::cover::applying()
                    } else {
                        tr::editor::cover::apply()
                    })
                    .disabled(state.busy || state.applying || self.asset_import_busy())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.apply_cover_background(window, cx)),
                    ),
                );
        }
        if let Some(error) = &state.error {
            body = body.child(
                div()
                    .text_size(px(12.))
                    .text_color(gpui::rgb(0xdc2626))
                    .child(error.clone()),
            );
        }
        if state.saved {
            body = body.child(div().text_size(px(12.)).child(tr::editor::cover::applied()));
        }
        body.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cover_generation::DimensionStatus;
    use cap_project::TitleCardRole;

    fn fixture() -> (Vec<u8>, GeneratedOrigin, ProjectConfiguration) {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            4,
            3,
            image::Rgb([15, 30, 120]),
        ))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
        let dimensions = Dimensions {
            width: 4,
            height: 3,
        };
        let origin = GeneratedOrigin {
            version: 1,
            provider: "volcengineArk".into(),
            requested_model: "synthetic-model".into(),
            reported_model: None,
            target: dimensions,
            requested: dimensions,
            actual: dimensions,
            dimension_status: DimensionStatus::RequestedSize,
            created: None,
        };
        let mut project: ProjectConfiguration = serde_json::from_value(serde_json::json!({"timeline":{"segments":[{"start":0.0,"end":5.0,"timescale":1.0}],"zoomSegments":[]}})).unwrap();
        project
            .upsert_title_card(cap_project::TextSegment::new_title_card(
                TitleCardRole::Intro,
                "Synthetic cover",
            ))
            .unwrap();
        (bytes.into_inner(), origin, project)
    }

    #[test]
    fn cover_context_changes_with_card_project_and_manual_edit() {
        let (_, _, project) = fixture();
        let target = project
            .title_card(TitleCardRole::Intro)
            .unwrap()
            .title_card
            .as_ref()
            .unwrap()
            .id
            .clone();
        let initial = context_for(Path::new("synthetic.cap"), &project, &target).unwrap();
        let other = context_for(Path::new("other.cap"), &project, &target).unwrap();
        assert_ne!(initial.project_id, other.project_id);
        assert!(context_for(Path::new("synthetic.cap"), &project, "removed-card").is_none());
        let mut edited = project.clone();
        edited.timeline.as_mut().unwrap().text_segments[0].content = "Edited locally".into();
        assert_ne!(
            initial.revision,
            context_for(Path::new("synthetic.cap"), &edited, &target)
                .unwrap()
                .revision
        );
    }

    #[test]
    fn unaccepted_cover_staging_rolls_back_only_its_own_files() {
        let directory = tempfile::tempdir().unwrap();
        let (bytes, origin, _) = fixture();
        let first =
            PreparedBackground::write(directory.path(), &bytes, "png", origin.clone()).unwrap();
        let rejected_path = first.media.clone();
        let second = PreparedBackground::write(directory.path(), &bytes, "png", origin)
            .unwrap()
            .retain();
        drop(first);
        assert!(!rejected_path.exists());
        assert!(!asset_library::record_path(&rejected_path).exists());
        assert!(directory.path().join(&second.path).is_file());
        assert!(
            asset_library::read_generated_origin(directory.path(), &second.path)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn retained_cover_survives_disk_save_failure_retry_and_one_undo() {
        let directory = tempfile::tempdir().unwrap();
        let (bytes, origin, mut project) = fixture();
        let before = project.clone();
        let mut history = crate::editor_edits::ProjectHistory::new(before.clone());
        let record = PreparedBackground::write(directory.path(), &bytes, "png", origin.clone())
            .unwrap()
            .retain();
        project.timeline.as_mut().unwrap().text_segments[0]
            .title_card
            .as_mut()
            .unwrap()
            .background = TitleCardBackground::ProjectImage {
            path: record.path.clone(),
            fit: TitleCardImageFit::Contain,
            focus: XY::new(0.5, 0.5),
            dim: 0.25,
        };
        history.record(&project);
        let mut pending = crate::project_save::PendingProjectSave::new(directory.path().into());
        pending.queue(project.clone());
        fs::create_dir(directory.path().join("project-config.json")).unwrap();
        assert!(pending.try_flush().is_err());
        assert_eq!(
            fs::read(directory.path().join(&record.path)).unwrap(),
            bytes
        );
        fs::remove_dir(directory.path().join("project-config.json")).unwrap();
        pending.try_flush().unwrap();
        let reopened: ProjectConfiguration = serde_json::from_slice(
            &fs::read(directory.path().join("project-config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&reopened).unwrap(),
            serde_json::to_value(&project).unwrap()
        );
        assert_eq!(
            asset_library::read_generated_origin(directory.path(), &record.path).unwrap(),
            Some(origin)
        );
        assert_eq!(
            serde_json::to_value(history.undo().unwrap()).unwrap(),
            serde_json::to_value(before).unwrap()
        );
        assert!(history.undo().is_none());
        assert_eq!(
            serde_json::to_value(history.redo().unwrap()).unwrap(),
            serde_json::to_value(project).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn generated_cover_refuses_symlinked_content_directory() {
        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), directory.path().join("content")).unwrap();
        let (bytes, origin, _) = fixture();
        assert!(PreparedBackground::write(directory.path(), &bytes, "png", origin).is_err());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }
}
