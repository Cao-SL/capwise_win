use gpui::{AppContext as _, Context, Entity, IntoElement, ParentElement, Styled, Window, div, px};

use crate::{
    ai_store::{self, Endpoint, Kind},
    cover_generation,
    i18n::tr,
    settings_window::SettingsWindow,
    ui,
};

pub(crate) const SERVICE: &str = "ark";

pub(crate) fn configured_model() -> Option<String> {
    ai_store::endpoint(Kind::Image, SERVICE)
        .filter(|endpoint| {
            endpoint.base_url.trim_end_matches('/') == cover_generation::BASE_URL
                && cover_generation::model_valid(&endpoint.model)
        })
        .map(|endpoint| endpoint.model)
}

pub(crate) struct CoverSettings {
    key: Entity<ui::TextInputState>,
    model: Entity<ui::TextInputState>,
    open: bool,
    busy: bool,
    notice: Option<&'static str>,
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
        let subscription = cx.subscribe(&key, |this, _, event, cx| {
            if *event == ui::TextInputEvent::Changed {
                let state = &mut this.pages.cover;
                if let Some(cancel) = state.cancel.take() {
                    let _ = cancel.send(());
                    state.busy = false;
                }
                state.discovery = state.discovery.wrapping_add(1);
                state.models.clear();
                cx.notify();
            }
        });
        Self {
            key,
            model: cx.new(|cx| ui::TextInputState::single_line(window, cx)),
            open: false,
            busy: false,
            notice: None,
            discovery: 0,
            cancel: None,
            models: Vec::new(),
            _subscriptions: vec![subscription],
        }
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
    fn discover_cover_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = &mut self.pages.cover;
        if let Some(cancel) = state.cancel.take() {
            let _ = cancel.send(());
            state.discovery = state.discovery.wrapping_add(1);
            state.busy = false;
            cx.notify();
            return;
        }
        if state.busy {
            return;
        }
        let typed = state.key.read(cx).text().trim().to_string();
        state.discovery = state.discovery.wrapping_add(1);
        let generation = state.discovery;
        let (cancel, cancelled) = tokio::sync::oneshot::channel();
        state.cancel = Some(cancel);
        state.busy = true;
        state.notice = None;
        self.spawn_tokio(window, cx, async move {
            tokio::select! {
                _ = cancelled => Err(cover_generation::GenerationError::Cancelled),
                result = async {
                    let key = if typed.is_empty() { tokio::task::spawn_blocking(|| ai_store::key(Kind::Image, SERVICE)).await.ok().flatten().ok_or(cover_generation::GenerationError::InvalidCredential)? } else { typed };
                    cover_generation::ArkBackgroundClient::new()?.models(&key).await
                } => result,
            }
        }, move |this, result, _, _| {
            let state = &mut this.pages.cover;
            if state.discovery != generation { return; }
            state.cancel = None;
            state.busy = false;
            match result {
                Ok(models) => { state.notice = Some(if models.is_empty() { tr::settings::cover::models_empty() } else { tr::settings::cover::models_found() }); state.models = models; },
                Err(_) => state.notice = Some(tr::settings::cover::models_failed()),
            }
        });
        cx.notify();
    }
    fn edit_cover_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pages.cover.busy {
            return;
        }
        let model = configured_model().unwrap_or_else(|| cover_generation::DEFAULT_MODEL.into());
        let state = &mut self.pages.cover;
        state.open = true;
        state.notice = None;
        state.key.update(cx, |input, cx| input.set_text("", cx));
        state
            .model
            .update(cx, |input, cx| input.set_text(model, cx));
        window.focus(&state.key.read(cx).focus_handle(), cx);
        cx.notify();
    }

    fn save_cover_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pages.cover.busy {
            return;
        }
        let model = self.pages.cover.model.read(cx).text().trim().to_string();
        if !cover_generation::model_valid(&model) {
            self.pages.cover.notice = Some(tr::settings::cover::invalid_model());
            cx.notify();
            return;
        }
        let key = self.pages.cover.key.read(cx).text().trim().to_string();
        self.pages
            .cover
            .key
            .update(cx, |input, cx| input.set_text("", cx));
        self.pages.cover.busy = true;
        self.pages.cover.notice = None;
        self.spawn_tokio(
            window,
            cx,
            async move {
                tokio::task::spawn_blocking(move || {
                    if key.is_empty() {
                        if ai_store::key(Kind::Image, SERVICE).is_none() {
                            return false;
                        }
                    } else if ai_store::set_key(Kind::Image, SERVICE, &key).is_err() {
                        return false;
                    }
                    ai_store::set_endpoint(
                        Kind::Image,
                        SERVICE,
                        &Endpoint {
                            base_url: cover_generation::BASE_URL.into(),
                            model,
                        },
                    )
                })
                .await
                .unwrap_or(false)
            },
            |this, saved, _, _| {
                this.pages.cover.busy = false;
                this.pages.cover.open = !saved;
                this.pages.cover.notice = Some(if saved {
                    tr::settings::cover::saved()
                } else {
                    tr::settings::cover::save_failed()
                });
            },
        );
        cx.notify();
    }

    fn remove_cover_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pages.cover.busy {
            return;
        }
        self.pages.cover.busy = true;
        self.spawn_tokio(
            window,
            cx,
            async {
                tokio::task::spawn_blocking(|| {
                    ai_store::remove_checked(Kind::Image, SERVICE).is_ok()
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
                this.pages.cover.notice = Some(if removed {
                    tr::settings::cover::removed()
                } else {
                    tr::settings::cover::save_failed()
                });
            },
        );
        cx.notify();
    }

    pub(crate) fn render_cover_settings(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let state = &self.pages.cover;
        let configured = configured_model();
        let mut body = self.card(true).flex().flex_col().gap(px(10.)).child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(12.))
                        .text_color(theme.settings_muted())
                        .child(
                            configured
                                .as_deref()
                                .unwrap_or(tr::settings::cover::not_configured())
                                .to_string(),
                        ),
                )
                .child(self.button(
                    "cover-settings-edit",
                    (ui::ButtonVariant::Gray, None),
                    tr::settings::ai::configure(),
                    state.busy || state.open,
                    cx,
                    |this, window, cx| this.edit_cover_settings(window, cx),
                )),
        );
        if state.open {
            state.key.update(cx, |input, _| {
                input.set_placeholder(tr::settings::cover::key_hint())
            });
            body =
                body.child(div().text_size(px(12.)).child(tr::settings::cover::model()))
                    .child(self.pages_input("cover-image-model", &state.model))
                    .child(self.button(
                        "cover-image-discover",
                        (ui::ButtonVariant::Gray, None),
                        if state.cancel.is_some() {
                            tr::common::cancel()
                        } else {
                            tr::settings::cover::fetch_models()
                        },
                        state.busy && state.cancel.is_none(),
                        cx,
                        |this, window, cx| this.discover_cover_models(window, cx),
                    ))
                    .children(
                        state
                            .models
                            .iter()
                            .take(12)
                            .enumerate()
                            .map(|(index, model)| {
                                let value = model.clone();
                                self.button(
                                    gpui::SharedString::from(format!("cover-model-choice-{index}")),
                                    (ui::ButtonVariant::Gray, None),
                                    gpui::SharedString::from(model.clone()),
                                    state.busy,
                                    cx,
                                    move |this, _, cx| {
                                        this.pages.cover.model.update(cx, |input, cx| {
                                            input.set_text(value.clone(), cx)
                                        })
                                    },
                                )
                            }),
                    )
                    .child(div().text_size(px(12.)).child(tr::settings::cover::key()))
                    .child(self.pages_input("cover-image-key", &state.key))
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
                            .children(configured.is_some().then(|| {
                                self.button(
                                    "cover-settings-remove",
                                    (ui::ButtonVariant::Gray, None),
                                    tr::settings::cover::remove(),
                                    state.busy,
                                    cx,
                                    |this, window, cx| this.remove_cover_settings(window, cx),
                                )
                            }))
                            .child(self.button(
                                "cover-model-docs",
                                (ui::ButtonVariant::Gray, None),
                                tr::settings::cover::models_link(),
                                false,
                                cx,
                                |_, _, cx| {
                                    cx.open_url(
                                    "https://ark.volcengine.com/region:cn-beijing/docs/model-list",
                                )
                                },
                            )),
                    );
        }
        if let Some(notice) = state.notice {
            body = body.child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.settings_muted())
                    .child(notice),
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
