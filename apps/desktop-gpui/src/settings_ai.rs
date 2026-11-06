//! Settings → AI models: the default caption recogniser, the local models,
//! the cloud recognition services and the large language models. Laid out
//! after qiaomu-clipper's model settings, one row per way of recognising,
//! each with a dot that turns green once it is ready.

use std::{path::Path, time::Duration};

use gpui::{
    AppContext as _, Context, Entity, FontWeight, Hsla, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder as _, px,
};
use serde_json::Value;

use crate::{
    ai_store::{self, Endpoint, Kind},
    asr_cloud,
    editor_tabs::{self, CaptionModel},
    i18n::tr,
    llm,
    settings_window::{MenuKind, SettingsWindow},
    store, transcription, ui,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    kind: Kind,
    id: &'static str,
}

#[derive(Clone, PartialEq)]
enum Status {
    Idle,
    Testing,
    Failed(String),
}

pub(crate) struct AiState {
    key: Entity<ui::TextInputState>,
    address: Entity<ui::TextInputState>,
    model: Entity<ui::TextInputState>,
    editing: Option<Target>,
    status: Status,
    /// The round trip of the last successful test, shown on its row.
    tested: Option<(Target, u128)>,
    download_poll: Option<gpui::Task<()>>,
}

impl AiState {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<SettingsWindow>) -> Self {
        Self {
            key: cx.new(|cx| ui::TextInputState::single_line(window, cx)),
            address: cx.new(|cx| ui::TextInputState::single_line(window, cx)),
            model: cx.new(|cx| ui::TextInputState::single_line(window, cx)),
            editing: None,
            status: Status::Idle,
            tested: None,
            download_poll: None,
        }
    }
}

const READY: u32 = 0x22c55e;
const ERROR: u32 = 0xef4444;

fn defaults(target: Target) -> (Endpoint, Option<&'static str>, &'static [&'static str], bool) {
    match target.kind {
        Kind::Asr => {
            let service = asr_cloud::service(target.id).expect("known recognition service");
            (service.endpoint(), service.key_url, service.models, service.needs_key)
        }
        Kind::Llm => {
            let provider = llm::provider(target.id).expect("known model provider");
            (provider.endpoint(), provider.key_url, provider.models, provider.needs_key())
        }
    }
}

/// The recognisers the editor can use right now, in menu order: the local
/// models this Mac runs, then the cloud services that are set up.
pub(crate) fn recognizer_choices() -> Vec<&'static CaptionModel> {
    editor_tabs::available_caption_models()
}

fn selected_recognizer() -> &'static str {
    let saved = store::store_section(transcription::GPUI_STORE_SECTION)
        .get(transcription::SELECTED_MODEL_KEY)
        .and_then(Value::as_str)
        .map(str::to_string);
    let choices = recognizer_choices();
    choices
        .iter()
        .find(|model| Some(model.name) == saved.as_deref())
        .or(choices.first())
        .map_or("", |model| model.name)
}

fn monogram(name: &str) -> String {
    name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default()
}

impl SettingsWindow {
    // ---- menus -----------------------------------------------------------

    pub(crate) fn ai_menu_items(&self, kind: MenuKind) -> Vec<ui::MenuItem> {
        match kind {
            MenuKind::AiRecognizer => {
                let selected = selected_recognizer();
                recognizer_choices()
                    .into_iter()
                    .map(|model| {
                        let place = if asr_cloud::is_cloud_model(model.name) {
                            tr::settings::ai::cloud_tag()
                        } else {
                            tr::settings::ai::local_tag()
                        };
                        ui::MenuItem::new(
                            SharedString::from(format!("{} · {place}", (model.label)())),
                            model.name == selected,
                        )
                    })
                    .collect()
            }
            MenuKind::AiModel => {
                let Some(target) = self.pages.ai.editing else {
                    return Vec::new();
                };
                defaults(target)
                    .2
                    .iter()
                    .map(|model| ui::MenuItem::new(*model, false))
                    .collect()
            }
            MenuKind::AiLlmAdd => {
                let saved = ai_store::saved_ids(Kind::Llm);
                llm::PROVIDERS
                    .iter()
                    .filter(|provider| !saved.iter().any(|id| id == provider.id))
                    .map(|provider| {
                        let group = match provider.group {
                            llm::Group::Relay => tr::settings::ai::group_relay(),
                            llm::Group::Provider => tr::settings::ai::group_provider(),
                            llm::Group::Local => tr::settings::ai::local_tag(),
                        };
                        ui::MenuItem::new(
                            SharedString::from(format!("{} · {group}", (provider.name)())),
                            false,
                        )
                    })
                    .collect()
            }
            MenuKind::AiLlmDefault => {
                let current = llm::default_provider().map(|p| p.id);
                llm::configured()
                    .into_iter()
                    .filter(|provider| provider.ready())
                    .map(|provider| {
                        ui::MenuItem::new(
                            SharedString::from(format!("{} · {}", (provider.name)(), provider.endpoint().model)),
                            Some(provider.id) == current,
                        )
                    })
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    pub(crate) fn ai_choose(
        &mut self,
        kind: MenuKind,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match kind {
            MenuKind::AiRecognizer => {
                if let Some(model) = recognizer_choices().get(index) {
                    store::set_store_setting(
                        transcription::GPUI_STORE_SECTION,
                        transcription::SELECTED_MODEL_KEY,
                        Value::String(model.name.to_string()),
                    );
                }
            }
            MenuKind::AiModel => {
                if let Some(target) = self.pages.ai.editing
                    && let Some(model) = defaults(target).2.get(index)
                {
                    self.pages.ai.model.update(cx, |input, cx| input.set_text(*model, cx));
                }
            }
            MenuKind::AiLlmAdd => {
                let saved = ai_store::saved_ids(Kind::Llm);
                if let Some(provider) = llm::PROVIDERS
                    .iter()
                    .filter(|provider| !saved.iter().any(|id| id == provider.id))
                    .nth(index)
                {
                    self.ai_edit(Target { kind: Kind::Llm, id: provider.id }, window, cx);
                }
            }
            MenuKind::AiLlmDefault => {
                if let Some(provider) = llm::configured()
                    .into_iter()
                    .filter(|provider| provider.ready())
                    .nth(index)
                {
                    ai_store::set_llm_default(provider.id);
                }
            }
            _ => {}
        }
        cx.notify();
    }

    // ---- editing a service -------------------------------------------------

    fn ai_edit(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        let (endpoint, ..) = defaults(target);
        let state = &mut self.pages.ai;
        state.editing = Some(target);
        state.status = Status::Idle;
        state.key.update(cx, |input, cx| input.set_text("", cx));
        state.address.update(cx, |input, cx| input.set_text(endpoint.base_url, cx));
        state.model.update(cx, |input, cx| input.set_text(endpoint.model, cx));
        let focus = state.key.read(cx).focus_handle();
        window.focus(&focus, cx);
        cx.notify();
    }

    fn ai_cancel(&mut self, cx: &mut Context<Self>) {
        self.pages.ai.editing = None;
        self.pages.ai.status = Status::Idle;
        cx.notify();
    }

    fn ai_remove(&mut self, target: Target, cx: &mut Context<Self>) {
        ai_store::remove(target.kind, target.id);
        if self.pages.ai.editing == Some(target) {
            self.pages.ai.editing = None;
        }
        cx.notify();
    }

    fn ai_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.pages.ai.editing else {
            return;
        };
        let typed = self.pages.ai.key.read(cx).text().trim().to_string();
        let key = if typed.is_empty() {
            ai_store::key(target.kind, target.id).unwrap_or_default()
        } else {
            typed.clone()
        };
        let endpoint = Endpoint {
            base_url: self.pages.ai.address.read(cx).text().trim().to_string(),
            model: self.pages.ai.model.read(cx).text().trim().to_string(),
        };
        self.pages.ai.status = Status::Testing;
        cx.notify();
        let tested_endpoint = endpoint.clone();
        self.spawn_tokio(
            window,
            cx,
            async move {
                let key_for_test = key.clone();
                tokio::task::spawn_blocking(move || match target.kind {
                    Kind::Asr => asr_cloud::test(target.id, &key_for_test, &tested_endpoint),
                    Kind::Llm => llm::test(target.id, &key_for_test, &tested_endpoint),
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
                .and_then(|ms| {
                    if !typed.is_empty() {
                        ai_store::set_key(target.kind, target.id, &typed)?;
                    }
                    ai_store::set_endpoint(target.kind, target.id, &endpoint);
                    Ok(ms)
                })
            },
            move |this, result, _window, _cx| match result {
                Ok(ms) => {
                    this.pages.ai.editing = None;
                    this.pages.ai.status = Status::Idle;
                    this.pages.ai.tested = Some((target, ms));
                }
                Err(error) => this.pages.ai.status = Status::Failed(error),
            },
        );
    }

    // ---- local models --------------------------------------------------------

    fn ai_download(&mut self, model: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if !transcription::begin_download(model) {
            return;
        }
        gpui_tokio::Tokio::spawn(cx, transcription::run_model_download(model.to_string())).detach();
        self.ai_poll_downloads(window, cx);
    }

    fn ai_poll_downloads(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pages.ai.download_poll = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(300)).await;
                let active = transcription::download_active();
                if !active {
                    transcription::refresh_downloaded_models();
                }
                let alive = this
                    .update_in(cx, |_, window, cx| {
                        cx.notify();
                        window.refresh();
                    })
                    .is_ok();
                if !active || !alive {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn ai_delete_model(&mut self, model: &'static str, cx: &mut Context<Self>) {
        if !transcription::begin_delete(model) {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move { transcription::run_model_delete(model) })
                .await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
        cx.notify();
    }

    // ---- the page ------------------------------------------------------------

    pub(crate) fn ai_page_shown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        transcription::refresh_downloaded_models();
        if transcription::download_active() {
            self.ai_poll_downloads(window, cx);
        }
    }

    fn ai_badge(&self, text: String) -> impl IntoElement {
        let theme = self.theme;
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .size(px(30.))
            .rounded(px(8.))
            .bg(theme.settings_fill())
            .text_size(px(13.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.settings_text())
            .child(text)
    }

    fn ai_status(&self, ready: bool, label: SharedString) -> impl IntoElement {
        let theme = self.theme;
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .text_size(px(12.))
            .text_color(theme.settings_muted())
            .child(
                div()
                    .size(px(7.))
                    .rounded_full()
                    .bg(if ready { gpui::rgb(READY).into() } else { theme.settings_border() }),
            )
            .child(label)
    }

    /// One row: badge, name with a note under it, and whatever sits on the right.
    fn ai_row(
        &self,
        badge: String,
        name: SharedString,
        tag: Option<&'static str>,
        note: SharedString,
        right: gpui::AnyElement,
    ) -> gpui::Div {
        let theme = self.theme;
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.))
            .px(px(14.))
            .py(px(11.))
            .child(self.ai_badge(badge))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(13.))
                            .text_color(theme.settings_text())
                            .child(name)
                            .children(tag.map(|tag| {
                                div()
                                    .px(px(6.))
                                    .rounded(px(5.))
                                    .bg(theme.settings_fill())
                                    .text_size(px(11.))
                                    .text_color(theme.settings_muted())
                                    .child(tag)
                            })),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(17.))
                            .text_color(theme.settings_muted())
                            .child(note),
                    ),
            )
            .child(div().flex_shrink_0().child(right))
    }

    fn ai_field(&self, label: &'static str, input: gpui::AnyElement) -> impl IntoElement {
        let theme = self.theme;
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.))
            .child(
                div()
                    .w(px(64.))
                    .flex_shrink_0()
                    .text_size(px(12.))
                    .text_color(theme.settings_muted())
                    .child(label),
            )
            .child(div().flex_1().min_w_0().child(input))
    }

    fn ai_editor(&self, target: Target, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let state = &self.pages.ai;
        let (_, key_url, models, needs_key) = defaults(target);
        let testing = state.status == Status::Testing;
        let saved_tail = ai_store::key_tail(target.kind, target.id);
        let saved = ai_store::endpoint(target.kind, target.id).is_some();
        let key_placeholder: SharedString = match &saved_tail {
            Some(tail) => tr::settings::ai::key_saved(tail).into(),
            None if !needs_key => tr::settings::ai::key_optional().into(),
            None => tr::settings::ai::key_placeholder().into(),
        };
        state.key.update(cx, |input, _| input.set_placeholder(key_placeholder));
        state.address.update(cx, |input, _| input.set_placeholder("https://…"));
        state.model.update(cx, |input, _| input.set_placeholder(tr::settings::ai::model_placeholder()));

        let model_input = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .child(div().flex_1().min_w_0().child(self.pages_input("ai-model", &state.model)))
            .when(!models.is_empty(), |row| {
                row.child(self.select(
                    "ai-common-models",
                    tr::settings::ai::common_models(),
                    MenuKind::AiModel,
                    cx,
                ))
            });

        let status = match &state.status {
            Status::Failed(error) => Some((error.clone(), Hsla::from(gpui::rgb(ERROR)))),
            Status::Testing => Some((tr::settings::ai::testing().to_string(), theme.settings_muted())),
            Status::Idle => None,
        };

        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .mx(px(14.))
            .mb(px(14.))
            .p(px(12.))
            .rounded(px(10.))
            .bg(theme.settings_fill())
            .child(self.ai_field(
                tr::settings::ai::field_key(),
                self.pages_input("ai-key", &state.key).into_any_element(),
            ))
            .child(self.ai_field(
                tr::settings::ai::field_address(),
                self.pages_input("ai-address", &state.address).into_any_element(),
            ))
            .child(self.ai_field(tr::settings::ai::field_model(), model_input.into_any_element()))
            .when_some(status, |editor, (text, color)| {
                editor.child(div().text_size(px(12.)).line_height(px(18.)).text_color(color).child(text))
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .child(self.button(
                        "ai-save",
                        (ui::ButtonVariant::Primary, None),
                        if testing { tr::settings::ai::testing() } else { tr::settings::ai::save() },
                        testing,
                        cx,
                        |this, window, cx| this.ai_save(window, cx),
                    ))
                    .child(self.button(
                        "ai-cancel",
                        (ui::ButtonVariant::Gray, None),
                        tr::common::cancel(),
                        testing,
                        cx,
                        |this, _window, cx| this.ai_cancel(cx),
                    ))
                    .when(saved, |row| {
                        row.child(self.button(
                            "ai-remove",
                            (ui::ButtonVariant::Gray, None),
                            tr::settings::ai::remove(),
                            testing,
                            cx,
                            move |this, _window, cx| this.ai_remove(target, cx),
                        ))
                    })
                    .child(div().flex_1())
                    .children(key_url.map(|url| {
                        div()
                            .id("ai-get-key")
                            .cursor_pointer()
                            .text_size(px(12.))
                            .text_color(theme.settings_text())
                            .hover(move |style| style.text_color(theme.settings_muted()))
                            .child(tr::settings::ai::get_key())
                            .on_click(move |_, _, cx| cx.open_url(url))
                    })),
            )
            .into_any_element()
    }

    /// The right side of a service row: its state, and the button that opens it.
    fn ai_service_right(&self, target: Target, ready: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let label: SharedString = match self.pages.ai.tested {
            Some((tested, ms)) if tested == target && ready => {
                tr::settings::ai::ready_ms(ms).into()
            }
            _ if ready => tr::settings::ai::ready().into(),
            _ => tr::settings::ai::not_set().into(),
        };
        let open = self.pages.ai.editing == Some(target);
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.))
            .child(self.ai_status(ready, label))
            .when(!open, |row| {
                row.child(self.button(
                    SharedString::from(format!("ai-open-{}-{}", matches!(target.kind, Kind::Llm), target.id)),
                    (ui::ButtonVariant::Gray, None),
                    if ready || ai_store::endpoint(target.kind, target.id).is_some() {
                        tr::settings::ai::edit()
                    } else {
                        tr::settings::ai::configure()
                    },
                    false,
                    cx,
                    move |this, window, cx| this.ai_edit(target, window, cx),
                ))
            })
            .into_any_element()
    }

    fn ai_recognizer_section(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let selected = selected_recognizer();
        let label = recognizer_choices()
            .into_iter()
            .find(|model| model.name == selected)
            .map_or(SharedString::default(), |model| (model.label)().into());
        let note = if asr_cloud::is_cloud_model(selected) {
            tr::settings::ai::default_cloud_note()
        } else {
            tr::settings::ai::default_local_note()
        };
        self.section(
            tr::settings::ai::recognizer_title(),
            Some(tr::settings::ai::recognizer_description()),
            None,
            vec![
                self.rows(vec![self.setting_row(
                    tr::settings::ai::default_label(),
                    Some(note),
                    self.select("ai-recognizer", label, MenuKind::AiRecognizer, cx)
                        .into_any_element(),
                )])
                .into_any_element(),
            ],
        )
        .into_any_element()
    }

    fn ai_local_section(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let snapshot = transcription::ui_snapshot(Path::new(""));
        let rows = editor_tabs::local_caption_models()
            .into_iter()
            .map(|model| {
                let name = model.name;
                let downloaded = snapshot.downloaded.contains(name);
                let downloading = snapshot.download.as_ref().filter(|download| {
                    download.model == name
                        && download.state == transcription::DownloadState::Downloading
                });
                let deleting = snapshot.deleting.as_deref() == Some(name);
                let right = if let Some(download) = downloading {
                    self.ai_status(
                        false,
                        tr::settings::ai::downloading(download.progress.clamp(0.0, 100.0).round() as u32).into(),
                    )
                    .into_any_element()
                } else if downloaded {
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(12.))
                        .child(self.ai_status(true, tr::settings::ai::downloaded().into()))
                        .child(self.button(
                            SharedString::from(format!("ai-delete-{name}")),
                            (ui::ButtonVariant::Gray, None),
                            tr::settings::ai::delete(),
                            deleting || snapshot.download.is_some(),
                            cx,
                            move |this, _window, cx| this.ai_delete_model(name, cx),
                        ))
                        .into_any_element()
                } else {
                    self.button(
                        SharedString::from(format!("ai-download-{name}")),
                        (ui::ButtonVariant::Gray, Some("icons/download.svg")),
                        tr::settings::ai::download(),
                        transcription::download_active(),
                        cx,
                        move |this, window, cx| this.ai_download(name, window, cx),
                    )
                    .into_any_element()
                };
                self.ai_row(
                    monogram(model.model_name),
                    (model.label)().into(),
                    None,
                    format!("{} · {}", (model.description)(), crate::i18n::decimal(model.size)).into(),
                    right,
                )
                .into_any_element()
            })
            .collect();
        let failure = snapshot
            .download
            .as_ref()
            .filter(|download| download.state == transcription::DownloadState::Failed)
            .map(|download| download.message.clone());
        let mut children = vec![self.rows(rows).into_any_element()];
        if let Some(message) = failure {
            children.push(
                div()
                    .px(px(4.))
                    .text_size(px(12.))
                    .text_color(Hsla::from(gpui::rgb(ERROR)))
                    .child(message)
                    .into_any_element(),
            );
        }
        self.section(
            tr::settings::ai::local_title(),
            Some(tr::settings::ai::local_description()),
            None,
            children,
        )
        .into_any_element()
    }

    fn ai_cloud_section(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let rows = asr_cloud::SERVICES
            .iter()
            .map(|service| {
                let target = Target { kind: Kind::Asr, id: service.id };
                let tag = (service.id == "doubao").then(tr::settings::ai::recommended);
                let row = self.ai_row(
                    monogram((service.name)()),
                    (service.name)().into(),
                    tag,
                    (service.note)().into(),
                    self.ai_service_right(target, service.ready(), cx),
                );
                div()
                    .flex()
                    .flex_col()
                    .child(row)
                    .when(self.pages.ai.editing == Some(target), |column| {
                        column.child(self.ai_editor(target, cx))
                    })
                    .into_any_element()
            })
            .collect();
        self.section(
            tr::settings::ai::cloud_title(),
            Some(tr::settings::ai::cloud_description()),
            None,
            vec![self.rows(rows).into_any_element()],
        )
        .into_any_element()
    }

    fn ai_llm_section(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let editing_new = self
            .pages
            .ai
            .editing
            .filter(|target| target.kind == Kind::Llm && ai_store::endpoint(Kind::Llm, target.id).is_none());
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        let ready: Vec<_> = llm::configured().into_iter().filter(|p| p.ready()).collect();
        if !ready.is_empty() {
            let default = llm::default_provider()
                .map_or(SharedString::default(), |p| format!("{} · {}", (p.name)(), p.endpoint().model).into());
            rows.push(self.setting_row(
                tr::settings::ai::llm_default(),
                None,
                self.select("ai-llm-default", default, MenuKind::AiLlmDefault, cx)
                    .into_any_element(),
            ));
        }
        let providers = llm::configured()
            .into_iter()
            .map(|provider| provider.id)
            .chain(editing_new.map(|target| target.id));
        for id in providers {
            let Some(provider) = llm::provider(id) else {
                continue;
            };
            let target = Target { kind: Kind::Llm, id };
            let endpoint = provider.endpoint();
            let note: SharedString = if endpoint.model.is_empty() {
                tr::settings::ai::model_placeholder().into()
            } else {
                endpoint.model.into()
            };
            rows.push(
                div()
                    .flex()
                    .flex_col()
                    .child(self.ai_row(
                        monogram((provider.name)()),
                        (provider.name)().into(),
                        None,
                        note,
                        self.ai_service_right(target, provider.ready(), cx),
                    ))
                    .when(self.pages.ai.editing == Some(target), |column| {
                        column.child(self.ai_editor(target, cx))
                    })
                    .into_any_element(),
            );
        }
        let body = if rows.is_empty() {
            self.card(true)
                .text_size(px(12.))
                .text_color(theme.settings_muted())
                .child(tr::settings::ai::llm_empty())
                .into_any_element()
        } else {
            self.rows(rows).into_any_element()
        };
        self.section(
            tr::settings::ai::llm_title(),
            Some(tr::settings::ai::llm_description()),
            Some(
                self.select("ai-llm-add", tr::settings::ai::llm_add(), MenuKind::AiLlmAdd, cx)
                    .into_any_element(),
            ),
            vec![body],
        )
        .into_any_element()
    }

    pub(crate) fn render_ai_models(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let mut sections = vec![
            self.ai_recognizer_section(cx),
            self.ai_cloud_section(cx),
            self.ai_local_section(cx),
        ];
        sections.extend(self.render_transcription(cx));
        sections.push(self.ai_llm_section(cx));
        sections
    }
}
