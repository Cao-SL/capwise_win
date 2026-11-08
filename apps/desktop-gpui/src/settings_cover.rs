use gpui::{AppContext as _, Context, Entity, IntoElement, ParentElement, Styled, Window, div, px};

use crate::{
    ai_store::{self, Endpoint, Kind},
    cover_generation::{self, GenerationError},
    i18n::tr,
    settings_window::{MenuKind, SettingsWindow},
    store, ui,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageProvider {
    Ark,
    OpenAi,
    Custom,
}

impl ImageProvider {
    const ALL: [Self; 3] = [Self::Ark, Self::OpenAi, Self::Custom];

    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Ark => "ark",
            Self::OpenAi => "openai",
            Self::Custom => "custom",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Ark => tr::settings::cover::provider_ark(),
            Self::OpenAi => "OpenAI",
            Self::Custom => tr::settings::cover::provider_custom(),
        }
    }

    fn base_url(self) -> &'static str {
        match self {
            Self::Ark => cover_generation::BASE_URL,
            Self::OpenAi => "https://api.openai.com/v1",
            Self::Custom => "",
        }
    }

    pub(crate) fn is_ark(self) -> bool {
        self == Self::Ark
    }
}

fn active_provider() -> ImageProvider {
    let section = store::store_section("capwise_ai");
    let id = section
        .get("imageDefault")
        .and_then(serde_json::Value::as_str);
    ImageProvider::ALL
        .into_iter()
        .find(|provider| Some(provider.id()) == id)
        .unwrap_or(ImageProvider::Ark)
}

pub(crate) fn configured_endpoint() -> Option<(ImageProvider, Endpoint)> {
    let provider = active_provider();
    ai_store::endpoint(Kind::Image, provider.id())
        .filter(|endpoint| {
            cover_generation::normalize_image_base_url(&endpoint.base_url).is_ok()
                && cover_generation::model_valid(&endpoint.model)
        })
        .map(|endpoint| (provider, endpoint))
}

pub(crate) fn configured_model() -> Option<String> {
    configured_endpoint().map(|(_, endpoint)| endpoint.model)
}

fn discovery_error(error: GenerationError) -> String {
    match error {
        GenerationError::InvalidCredential | GenerationError::Authentication => {
            tr::settings::cover::auth_failed().into()
        }
        GenerationError::Permission => tr::settings::cover::permission_failed().into(),
        GenerationError::Network | GenerationError::Timeout => {
            tr::settings::cover::network_failed().into()
        }
        GenerationError::HttpStatus(404 | 405) => tr::settings::cover::listing_unavailable().into(),
        GenerationError::RateLimited { .. } => tr::settings::cover::rate_limited().into(),
        GenerationError::Cancelled => tr::common::cancel().into(),
        _ => tr::settings::cover::models_failed().into(),
    }
}

pub(crate) struct CoverSettings {
    key: Entity<ui::TextInputState>,
    address: Entity<ui::TextInputState>,
    model: Entity<ui::TextInputState>,
    provider: ImageProvider,
    manual_model: bool,
    open: bool,
    busy: bool,
    notice: Option<String>,
    discovery: u64,
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
    models: Vec<String>,
    _subscriptions: Vec<gpui::Subscription>,
}

impl CoverSettings {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<SettingsWindow>) -> Self {
        let key = cx.new(|cx| {
            let mut input = ui::TextInputState::single_line(window, cx);
            input.set_secret(true, cx);
            input
        });
        let address = cx.new(|cx| ui::TextInputState::single_line(window, cx));
        let subscriptions = [&key, &address]
            .into_iter()
            .map(|input| {
                cx.subscribe(input, |this, _, event, cx| {
                    if *event == ui::TextInputEvent::Changed {
                        this.pages.cover.invalidate();
                        cx.notify();
                    }
                })
            })
            .collect();
        Self {
            key,
            address,
            model: cx.new(|cx| ui::TextInputState::single_line(window, cx)),
            provider: active_provider(),
            manual_model: false,
            open: false,
            busy: false,
            notice: None,
            discovery: 0,
            cancel: None,
            models: Vec::new(),
            _subscriptions: subscriptions,
        }
    }

    fn invalidate(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
            self.busy = false;
        }
        self.discovery = self.discovery.wrapping_add(1);
        self.models.clear();
        self.notice = None;
    }
}

impl Drop for CoverSettings {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
    }
}

impl SettingsWindow {
    pub(crate) fn cover_menu_items(&self, kind: MenuKind, cx: &gpui::App) -> Vec<ui::MenuItem> {
        let state = &self.pages.cover;
        match kind {
            MenuKind::CoverProvider => ImageProvider::ALL
                .iter()
                .map(|provider| ui::MenuItem::new(provider.label(), *provider == state.provider))
                .collect(),
            MenuKind::CoverModel => state
                .models
                .iter()
                .map(|model| ui::MenuItem::new(model.clone(), model == state.model.read(cx).text()))
                .collect(),
            _ => Vec::new(),
        }
    }

    pub(crate) fn choose_cover_menu(
        &mut self,
        kind: MenuKind,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pages.cover.busy {
            return;
        }
        match kind {
            MenuKind::CoverProvider => {
                if let Some(provider) = ImageProvider::ALL.get(index) {
                    self.load_cover_provider(*provider, window, cx);
                }
            }
            MenuKind::CoverModel => {
                if let Some(model) = self.pages.cover.models.get(index).cloned() {
                    self.pages
                        .cover
                        .model
                        .update(cx, |input, cx| input.set_text(model, cx));
                    self.pages.cover.manual_model = false;
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn load_cover_provider(
        &mut self,
        provider: ImageProvider,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let saved = ai_store::endpoint(Kind::Image, provider.id());
        let state = &mut self.pages.cover;
        state.invalidate();
        state.provider = provider;
        state.open = true;
        state.manual_model = false;
        state.key.update(cx, |input, cx| input.set_text("", cx));
        state.address.update(cx, |input, cx| {
            input.set_text(
                saved
                    .as_ref()
                    .map_or(provider.base_url(), |endpoint| &endpoint.base_url),
                cx,
            )
        });
        state.model.update(cx, |input, cx| {
            input.set_text(saved.as_ref().map_or("", |endpoint| &endpoint.model), cx)
        });
        window.focus(&state.key.read(cx).focus_handle(), cx);
        cx.notify();
    }

    fn discover_cover_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = &mut self.pages.cover;
        if state.cancel.is_some() {
            state.invalidate();
            cx.notify();
            return;
        }
        if state.busy {
            return;
        }
        let address =
            match cover_generation::normalize_image_base_url(state.address.read(cx).text()) {
                Ok(address) => address,
                Err(_) => {
                    state.notice = Some(tr::settings::cover::invalid_address().into());
                    cx.notify();
                    return;
                }
            };
        let typed = state.key.read(cx).text().trim().to_string();
        let provider = state.provider;
        state.invalidate();
        let generation = state.discovery;
        let (cancel, cancelled) = tokio::sync::oneshot::channel();
        state.cancel = Some(cancel);
        state.busy = true;
        self.spawn_tokio(window, cx, async move {
            tokio::select! {
                _ = cancelled => Err(GenerationError::Cancelled),
                result = async {
                    let key = if typed.is_empty() {
                        tokio::task::spawn_blocking(move || ai_store::key(Kind::Image, provider.id())).await.ok().flatten().ok_or(GenerationError::InvalidCredential)?
                    } else { typed };
                    cover_generation::ArkBackgroundClient::for_endpoint(&address, provider.is_ark())?.models(&key).await
                } => result,
            }
        }, move |this, result, _, cx| {
            let state = &mut this.pages.cover;
            if state.discovery != generation { return; }
            state.cancel = None;
            state.busy = false;
            match result {
                Ok(models) => {
                    state.notice = Some(if models.is_empty() { tr::settings::cover::models_empty() } else { tr::settings::cover::models_found() }.into());
                    let selected = state.model.read(cx).text().to_string();
                    if !models.is_empty() {
                        if !models.contains(&selected) { state.model.update(cx, |input, cx| input.set_text(models[0].clone(), cx)); }
                        state.manual_model = false;
                    }
                    state.models = models;
                }
                Err(error) => state.notice = Some(discovery_error(error)),
            }
        });
        cx.notify();
    }

    fn save_cover_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pages.cover.busy {
            return;
        }
        let state = &mut self.pages.cover;
        let model = state.model.read(cx).text().trim().to_string();
        if !cover_generation::model_valid(&model) {
            state.notice = Some(tr::settings::cover::invalid_model().into());
            cx.notify();
            return;
        }
        let address =
            match cover_generation::normalize_image_base_url(state.address.read(cx).text()) {
                Ok(address) => address,
                Err(_) => {
                    state.notice = Some(tr::settings::cover::invalid_address().into());
                    cx.notify();
                    return;
                }
            };
        let provider = state.provider;
        let key = state.key.read(cx).text().trim().to_string();
        state.busy = true;
        state.notice = None;
        self.spawn_tokio(
            window,
            cx,
            async move {
                tokio::task::spawn_blocking(move || {
                    if key.is_empty() {
                        if ai_store::key(Kind::Image, provider.id()).is_none() {
                            return false;
                        }
                    } else if ai_store::set_key(Kind::Image, provider.id(), &key).is_err() {
                        return false;
                    }
                    ai_store::set_endpoint(
                        Kind::Image,
                        provider.id(),
                        &Endpoint {
                            base_url: address,
                            model,
                        },
                    ) && store::set_store_setting(
                        "capwise_ai",
                        "imageDefault",
                        serde_json::Value::from(provider.id()),
                    )
                })
                .await
                .unwrap_or(false)
            },
            |this, saved, _, cx| {
                this.pages.cover.busy = false;
                this.pages.cover.open = !saved;
                if saved {
                    this.pages
                        .cover
                        .key
                        .update(cx, |input, cx| input.set_text("", cx));
                }
                this.pages.cover.notice = Some(
                    if saved {
                        tr::settings::cover::saved()
                    } else {
                        tr::settings::cover::save_failed()
                    }
                    .into(),
                );
            },
        );
        cx.notify();
    }

    fn remove_cover_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pages.cover.busy {
            return;
        }
        let provider = self.pages.cover.provider;
        self.pages.cover.busy = true;
        self.spawn_tokio(
            window,
            cx,
            async move {
                tokio::task::spawn_blocking(move || {
                    ai_store::remove_checked(Kind::Image, provider.id()).is_ok()
                })
                .await
                .unwrap_or(false)
            },
            |this, removed, _, cx| {
                this.pages.cover.busy = false;
                this.pages.cover.open = !removed;
                this.pages
                    .cover
                    .key
                    .update(cx, |input, cx| input.set_text("", cx));
                this.pages.cover.notice = Some(
                    if removed {
                        tr::settings::cover::removed()
                    } else {
                        tr::settings::cover::save_failed()
                    }
                    .into(),
                );
            },
        );
        cx.notify();
    }

    pub(crate) fn render_cover_settings(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let state = &self.pages.cover;
        let configured = configured_endpoint();
        let saved = ai_store::endpoint(Kind::Image, state.provider.id());
        let mut body = self.card(true).flex().flex_col().gap(px(10.)).child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(12.))
                        .text_color(self.theme.settings_muted())
                        .child(configured.map_or_else(
                            || tr::settings::cover::not_configured().to_string(),
                            |(provider, endpoint)| {
                                format!("{} · {}", provider.label(), endpoint.model)
                            },
                        )),
                )
                .child(self.button(
                    "cover-settings-edit",
                    (ui::ButtonVariant::Gray, None),
                    tr::settings::ai::configure(),
                    state.busy || state.open,
                    cx,
                    |this, window, cx| this.load_cover_provider(active_provider(), window, cx),
                )),
        );
        if state.open {
            for input in [&state.key, &state.address, &state.model] {
                input.update(cx, |input, cx| {
                    input.set_disabled(state.busy && state.cancel.is_none(), cx)
                });
            }
            state.key.update(cx, |input, _| {
                input.set_placeholder(if saved.is_some() {
                    tr::settings::cover::key_hint()
                } else {
                    tr::settings::ai::key_placeholder()
                })
            });
            state.address.update(cx, |input, _| {
                input.set_placeholder("https://api.example.com/v1")
            });
            let current_model = state.model.read(cx).text().to_string();
            body = body
                .child(
                    div()
                        .text_size(px(12.))
                        .child(tr::settings::cover::provider()),
                )
                .child(self.select(
                    "cover-provider",
                    state.provider.label(),
                    MenuKind::CoverProvider,
                    cx,
                ))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(div().text_size(px(12.)).child(tr::settings::cover::key()))
                        .children((state.provider == ImageProvider::Ark).then(|| {
                            self.button(
                                "cover-image-get-key",
                                (ui::ButtonVariant::Gray, Some("icons/arrow-up-right.svg")),
                                tr::settings::ai::get_key(),
                                false,
                                cx,
                                |_, _, cx| cx.open_url("https://ai.volcengine.com/console/apikey"),
                            )
                        })),
                )
                .child(self.pages_input("cover-image-key", &state.key))
                .child(
                    div()
                        .text_size(px(12.))
                        .child(tr::settings::cover::address()),
                )
                .child(self.pages_input("cover-image-address", &state.address))
                .child(self.button(
                    "cover-image-discover",
                    (ui::ButtonVariant::Gray, Some("icons/refresh-cw.svg")),
                    if state.cancel.is_some() {
                        tr::common::cancel()
                    } else {
                        tr::settings::cover::fetch_models()
                    },
                    (state.busy && state.cancel.is_none())
                        || (!state.busy
                            && state.key.read(cx).text().trim().is_empty()
                            && saved.is_none()),
                    cx,
                    |this, window, cx| this.discover_cover_models(window, cx),
                ))
                .child(div().text_size(px(12.)).child(tr::settings::cover::model()))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(div().flex_1().min_w_0().child(if state.manual_model {
                            self.pages_input("cover-image-model", &state.model)
                                .into_any_element()
                        } else {
                            ui::Select::plain(
                                &self.theme,
                                "cover-image-model-select",
                                if current_model.is_empty() {
                                    tr::settings::cover::choose_model().to_string()
                                } else {
                                    current_model
                                },
                            )
                            .stretch_label()
                            .disabled(state.busy || state.models.is_empty())
                            .on_open(cx.listener(
                                |this, bounds: &gpui::Bounds<gpui::Pixels>, window, cx| {
                                    this.open_menu(
                                        MenuKind::CoverModel,
                                        bounds.bottom_left(),
                                        window,
                                        cx,
                                    )
                                },
                            ))
                            .into_any_element()
                        }))
                        .child(self.button(
                            "cover-image-model-edit",
                            (ui::ButtonVariant::Gray, Some("icons/pencil.svg")),
                            if state.manual_model {
                                tr::settings::cover::choose_from_list()
                            } else {
                                tr::settings::cover::edit_model()
                            },
                            state.busy,
                            cx,
                            |this, window, cx| {
                                this.pages.cover.manual_model = !this.pages.cover.manual_model;
                                if this.pages.cover.manual_model {
                                    window
                                        .focus(&this.pages.cover.model.read(cx).focus_handle(), cx);
                                }
                                cx.notify();
                            },
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(8.))
                        .child(self.button(
                            "cover-settings-save",
                            (ui::ButtonVariant::Primary, None),
                            tr::common::save(),
                            state.busy,
                            cx,
                            |this, window, cx| this.save_cover_settings(window, cx),
                        ))
                        .child(self.button(
                            "cover-settings-cancel",
                            (ui::ButtonVariant::Gray, None),
                            tr::common::cancel(),
                            state.busy,
                            cx,
                            |this, _, cx| {
                                this.pages.cover.open = false;
                                this.pages
                                    .cover
                                    .key
                                    .update(cx, |input, cx| input.set_text("", cx));
                                cx.notify();
                            },
                        ))
                        .children(saved.is_some().then(|| {
                            self.button(
                                "cover-settings-remove",
                                (ui::ButtonVariant::Gray, None),
                                tr::settings::cover::remove(),
                                state.busy,
                                cx,
                                |this, window, cx| this.remove_cover_settings(window, cx),
                            )
                        })),
                );
        }
        if let Some(notice) = &state.notice {
            body = body.child(
                div()
                    .text_size(px(12.))
                    .text_color(self.theme.settings_muted())
                    .child(notice.clone()),
            );
        }
        self.section(
            tr::settings::cover::title(),
            Some(tr::settings::cover::description()),
            None,
            vec![body.into_any_element()],
        )
        .into_any_element()
    }
}
