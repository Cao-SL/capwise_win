//! Settings → AI models: the default caption recogniser, the local models,
//! the cloud recognition services and the large language models. Laid out
//! after qiaomu-clipper's model settings, one card per way of recognising,
//! each with a dot that turns green once it is ready.

use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use gpui::{
    AppContext as _, Context, Entity, FontWeight, Hsla, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder as _, px, relative, svg,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ModelKind {
    Asr,
    Llm,
}

impl ModelKind {
    fn store_kind(self) -> Kind {
        match self {
            Self::Asr => Kind::Asr,
            Self::Llm => Kind::Llm,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Target {
    kind: ModelKind,
    id: &'static str,
}

/// The open editor: what its buttons are doing, and the last answer.
#[derive(Clone, PartialEq)]
enum Status {
    Idle,
    Testing,
    Saving,
    SigningIn,
    Passed(u128),
    Failed(String),
}

impl Status {
    fn busy(&self) -> bool {
        matches!(self, Self::Testing | Self::Saving | Self::SigningIn)
    }
}

/// A saved service checked from its row.
#[derive(Clone, PartialEq)]
enum Check {
    Running,
    Passed(u128),
    Failed(String),
}

/// Ends a browser sign-in's wait for its return address, so closing the
/// window or the editor frees the port instead of holding it ten minutes.
struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Tone {
    Ready,
    Error,
    Idle,
}

#[derive(Default)]
struct ModelDiscovery {
    generation: u64,
    target: Option<Target>,
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
    loading: bool,
    models: Vec<llm::DiscoveredModel>,
    notice: Option<String>,
}

impl ModelDiscovery {
    fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        self.target = None;
        self.loading = false;
        self.models.clear();
        self.notice = None;
    }

    fn accepts(&self, target: Target, generation: u64) -> bool {
        self.target == Some(target) && self.generation == generation && self.loading
    }
}

impl Drop for ModelDiscovery {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
    }
}

pub(crate) struct AiState {
    key: Entity<ui::TextInputState>,
    address: Entity<ui::TextInputState>,
    model: Entity<ui::TextInputState>,
    editing: Option<Target>,
    status: Status,
    advanced_open: bool,
    sign_in: Option<StopOnDrop>,
    checks: HashMap<Target, Check>,
    download_poll: Option<gpui::Task<()>>,
    review_prompt: Entity<ui::TextInputState>,
    review_open: bool,
    review_notice: Option<String>,
    more_asr_open: bool,
    discovery: ModelDiscovery,
    _discovery_subscriptions: Vec<gpui::Subscription>,
}

impl AiState {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<SettingsWindow>) -> Self {
        let key = cx.new(|cx| {
            let mut input = ui::TextInputState::single_line(window, cx);
            input.set_secret(true, cx);
            input
        });
        let address = cx.new(|cx| ui::TextInputState::single_line(window, cx));
        let model = cx.new(|cx| ui::TextInputState::single_line(window, cx));
        let mut subscriptions: Vec<gpui::Subscription> = [&key, &address]
            .into_iter()
            .map(|input| {
                cx.subscribe(input, |this, _input, event, cx| {
                    if *event == ui::TextInputEvent::Changed {
                        this.pages.ai.discovery.invalidate();
                        this.ai_draft_changed(cx);
                    }
                })
            })
            .collect();
        subscriptions.push(cx.subscribe(&model, |this, _input, event, cx| {
            if *event == ui::TextInputEvent::Changed {
                this.ai_draft_changed(cx);
            }
        }));
        Self {
            key,
            address,
            model,
            editing: None,
            status: Status::Idle,
            advanced_open: false,
            sign_in: None,
            checks: HashMap::new(),
            download_poll: None,
            review_prompt: cx.new(|cx| {
                let mut input = ui::TextInputState::multi_line(window, cx);
                input.set_text(crate::caption_review::ReviewSettings::load().prompt, cx);
                input
            }),
            review_open: false,
            review_notice: None,
            more_asr_open: false,
            discovery: ModelDiscovery::default(),
            _discovery_subscriptions: subscriptions,
        }
    }
}

const READY: u32 = 0x22c55e;
const ERROR: u32 = 0xef4444;

fn tint(color: u32) -> Hsla {
    let mut color = Hsla::from(gpui::rgb(color));
    color.a = 0.12;
    color
}

fn defaults(
    target: Target,
) -> (
    Endpoint,
    Option<&'static str>,
    &'static [&'static str],
    bool,
) {
    match target.kind {
        ModelKind::Asr => {
            let service = asr_cloud::service(target.id).expect("known recognition service");
            (
                service.endpoint(),
                service.key_url,
                service.models,
                service.needs_key,
            )
        }
        ModelKind::Llm => {
            let provider = llm::provider(target.id).expect("known model provider");
            (
                provider.endpoint(),
                provider.key_url,
                provider.models,
                provider.needs_key(),
            )
        }
    }
}

/// The address a service ships with, before anyone edits it.
fn built_in_address(target: Target) -> &'static str {
    match target.kind {
        ModelKind::Asr => asr_cloud::service(target.id).map_or("", |service| service.base_url),
        ModelKind::Llm => llm::provider(target.id).map_or("", |provider| provider.base_url),
    }
}

/// The address field only earns a place in the editor when the service has
/// no address of its own, or someone already changed it.
fn address_shown(built_in: &str, current: &str, advanced_open: bool) -> bool {
    advanced_open || built_in.is_empty() || current.trim() != built_in
}

/// A gateway people run themselves: where it lives is the first thing to set.
fn address_always_shown(target: Target) -> bool {
    target.kind == ModelKind::Llm && target.id == "magpie"
}

fn account_target(target: Target) -> bool {
    target.kind == ModelKind::Llm
        && llm::provider(target.id).is_some_and(|provider| provider.group == llm::Group::Account)
}

/// The signed-in account's "email · plan", from the cached tokens.
fn account_label(target: Target) -> Option<String> {
    ai_store::key_cached(Kind::Llm, target.id)
        .and_then(|stored| llm::signed_in_account(&stored))
        .map(|credentials| credentials.label())
}

/// Disk sizes the way Finder shows them: decimal units, one decimal for GB.
fn format_bytes(bytes: u64) -> String {
    let gb = bytes as f64 / 1e9;
    if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{:.0} MB", (bytes as f64 / 1e6).max(1.0))
    }
}

/// Runs one connection check on a blocking thread. An empty `typed` key
/// falls back to the one in the keychain.
fn run_check(target: Target, typed: String, endpoint: Endpoint) -> Result<u128, String> {
    let key = if typed.is_empty() {
        ai_store::key(target.kind.store_kind(), target.id).unwrap_or_default()
    } else {
        typed
    };
    match target.kind {
        ModelKind::Asr => asr_cloud::test(target.id, &key, &endpoint),
        ModelKind::Llm => llm::test(target.id, &key, &endpoint),
    }
}

/// The recognisers the editor can use right now, in menu order: the local
/// models this Mac runs, then the cloud services that are set up.
pub(crate) fn recognizer_choices() -> Vec<&'static CaptionModel> {
    let mut models = editor_tabs::local_caption_models();
    models.extend(editor_tabs::CAPTION_MODELS.iter().filter(|model| {
        asr_cloud::service(model.name).is_some_and(|service| {
            configured_cached(Target {
                kind: ModelKind::Asr,
                id: service.id,
            })
        })
    }));
    models
}

fn configured_cached(target: Target) -> bool {
    let kind = target.kind.store_kind();
    let (_, _, _, needs_key) = defaults(target);
    ai_store::endpoint(kind, target.id).is_some_and(|endpoint| {
        ai_store::address_allowed(&endpoint.base_url) && !endpoint.model.trim().is_empty()
    }) && (!needs_key || ai_store::key_available_cached(kind, target.id) != Some(false))
}

fn ready_cached(target: Target) -> bool {
    let (_, _, _, needs_key) = defaults(target);
    configured_cached(target)
        && (!needs_key
            || ai_store::key_available_cached(target.kind.store_kind(), target.id) == Some(true))
}

fn default_provider_cached() -> Option<&'static llm::LlmProvider> {
    let available: Vec<_> = llm::configured()
        .into_iter()
        .filter(|provider| {
            configured_cached(Target {
                kind: ModelKind::Llm,
                id: provider.id,
            })
        })
        .collect();
    ai_store::llm_default()
        .and_then(|id| available.iter().copied().find(|provider| provider.id == id))
        .or_else(|| available.first().copied())
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
    name.chars()
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default()
}

fn asr_service_visible(
    id: &str,
    configured: &[String],
    selected: Option<&str>,
    editing: Option<Target>,
) -> bool {
    id == "doubao"
        || configured.iter().any(|saved| saved == id)
        || selected == Some(id)
        || editing.is_some_and(|target| target.kind == ModelKind::Asr && target.id == id)
}

impl SettingsWindow {
    fn ai_model_choices(&self, target: Target) -> Vec<llm::DiscoveredModel> {
        let discovery = &self.pages.ai.discovery;
        if discovery.target == Some(target) && !discovery.models.is_empty() {
            return discovery.models.clone();
        }
        defaults(target)
            .2
            .iter()
            .map(|id| llm::DiscoveredModel {
                id: (*id).to_string(),
                display_name: None,
            })
            .collect()
    }

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
                self.ai_model_choices(target)
                    .into_iter()
                    .map(|model| {
                        ui::MenuItem::new(
                            SharedString::from(match model.display_name {
                                Some(name) => format!("{name} · {}", model.id),
                                None => model.id,
                            }),
                            false,
                        )
                    })
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
                            llm::Group::Account => tr::settings::ai::group_account(),
                        };
                        ui::MenuItem::new(
                            SharedString::from(format!("{} · {group}", (provider.name)())),
                            false,
                        )
                    })
                    .collect()
            }
            MenuKind::AiLlmDefault => {
                let current = default_provider_cached().map(|p| p.id);
                llm::configured()
                    .into_iter()
                    .filter(|provider| {
                        configured_cached(Target {
                            kind: ModelKind::Llm,
                            id: provider.id,
                        })
                    })
                    .map(|provider| {
                        ui::MenuItem::new(
                            SharedString::from(format!(
                                "{} · {}",
                                (provider.name)(),
                                provider.endpoint().model
                            )),
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
                    && let Some(model) = self.ai_model_choices(target).get(index)
                {
                    self.pages
                        .ai
                        .model
                        .update(cx, |input, cx| input.set_text(model.id.clone(), cx));
                }
            }
            MenuKind::AiLlmAdd => {
                let saved = ai_store::saved_ids(Kind::Llm);
                if let Some(provider) = llm::PROVIDERS
                    .iter()
                    .filter(|provider| !saved.iter().any(|id| id == provider.id))
                    .nth(index)
                {
                    self.ai_edit(
                        Target {
                            kind: ModelKind::Llm,
                            id: provider.id,
                        },
                        window,
                        cx,
                    );
                }
            }
            MenuKind::AiLlmDefault => {
                if let Some(provider) = llm::configured()
                    .into_iter()
                    .filter(|provider| {
                        configured_cached(Target {
                            kind: ModelKind::Llm,
                            id: provider.id,
                        })
                    })
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
        state.discovery.invalidate();
        state.sign_in = None;
        state.editing = Some(target);
        state.advanced_open = false;
        state.key.update(cx, |input, cx| input.set_text("", cx));
        state
            .address
            .update(cx, |input, cx| input.set_text(endpoint.base_url, cx));
        state
            .model
            .update(cx, |input, cx| input.set_text(endpoint.model, cx));
        state.status = Status::Idle;
        let focus = state.key.read(cx).focus_handle();
        window.focus(&focus, cx);
        cx.notify();
    }

    /// A passed or failed test no longer describes what the fields say.
    fn ai_draft_changed(&mut self, cx: &mut Context<Self>) {
        if matches!(self.pages.ai.status, Status::Passed(_) | Status::Failed(_)) {
            self.pages.ai.status = Status::Idle;
        }
        cx.notify();
    }

    fn ai_cancel(&mut self, cx: &mut Context<Self>) {
        self.pages.ai.discovery.invalidate();
        self.pages.ai.sign_in = None;
        self.pages
            .ai
            .key
            .update(cx, |input, cx| input.set_text("", cx));
        self.pages.ai.editing = None;
        self.pages.ai.status = Status::Idle;
        cx.notify();
    }

    fn ai_remove(&mut self, target: Target, cx: &mut Context<Self>) {
        self.pages.ai.discovery.invalidate();
        ai_store::remove(target.kind.store_kind(), target.id);
        self.pages.ai.checks.remove(&target);
        if self.pages.ai.editing == Some(target) {
            self.pages
                .ai
                .key
                .update(cx, |input, cx| input.set_text("", cx));
            self.pages.ai.editing = None;
            self.pages.ai.status = Status::Idle;
        }
        cx.notify();
    }

    fn ai_draft(&self, cx: &Context<Self>) -> (String, Endpoint) {
        let state = &self.pages.ai;
        let address = state.address.read(cx).text().trim().to_string();
        let base_url = match state.editing {
            Some(target) if target.kind == ModelKind::Llm => {
                llm::normalize_address(target.id, &address)
            }
            _ => address,
        };
        (
            state.key.read(cx).text().trim().to_string(),
            Endpoint {
                base_url,
                model: state.model.read(cx).text().trim().to_string(),
            },
        )
    }

    /// Opens the provider's sign-in page in the browser and waits for it to
    /// come back. An account keeps its tokens as the key; 词元跳动 and
    /// OpenRouter hand back a key, which is then tested and saved like a
    /// pasted one.
    fn ai_sign_in(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        let Some(flow) = llm::provider(target.id).and_then(|provider| provider.sign_in) else {
            return;
        };
        if self.pages.ai.status.busy() || self.pages.ai.editing != Some(target) {
            return;
        }
        let pending = match llm::begin_sign_in(flow) {
            Ok(pending) => pending,
            Err(error) => {
                self.pages.ai.status = Status::Failed(error);
                cx.notify();
                return;
            }
        };
        let stop = Arc::new(AtomicBool::new(false));
        self.pages.ai.sign_in = Some(StopOnDrop(stop.clone()));
        self.pages.ai.status = Status::SigningIn;
        cx.open_url(&pending.url);
        let draft_model = self.pages.ai.model.read(cx).text().trim().to_string();
        let (endpoint, ..) = defaults(target);
        cx.notify();
        self.spawn_tokio(
            window,
            cx,
            async move {
                let signed = pending.finish(stop).await?;
                if !flow.is_account() {
                    return Ok((signed.key, None));
                }
                let key = signed.key.clone();
                tokio::task::spawn_blocking(move || ai_store::set_key(Kind::Llm, target.id, &key))
                    .await
                    .map_err(|error| error.to_string())??;
                let mut model = draft_model;
                if model.is_empty()
                    && let Ok(models) =
                        llm::discover_models(target.id, &endpoint.base_url, &signed.key).await
                {
                    model = models
                        .first()
                        .map(|found| found.id.clone())
                        .unwrap_or_default();
                }
                ai_store::set_endpoint(
                    Kind::Llm,
                    target.id,
                    &Endpoint {
                        base_url: endpoint.base_url,
                        model: model.clone(),
                    },
                );
                Ok((String::new(), Some(model)))
            },
            move |this, result: Result<(String, Option<String>), String>, window, cx| {
                if this.pages.ai.editing != Some(target) || this.pages.ai.sign_in.is_none() {
                    return;
                }
                this.pages.ai.sign_in = None;
                this.pages.ai.status = Status::Idle;
                match result {
                    Ok((key, None)) => {
                        this.pages
                            .ai
                            .key
                            .update(cx, |input, cx| input.set_text(key, cx));
                        this.ai_save(window, cx);
                    }
                    Ok((_, Some(model))) => {
                        this.pages
                            .ai
                            .model
                            .update(cx, |input, cx| input.set_text(model, cx));
                        this.ai_save(window, cx);
                    }
                    Err(error) => this.pages.ai.status = Status::Failed(error),
                }
            },
        );
    }

    fn ai_stop_sign_in(&mut self, cx: &mut Context<Self>) {
        self.pages.ai.sign_in = None;
        self.pages.ai.status = Status::Idle;
        cx.notify();
    }

    /// "Test connection": tries what the fields say without saving it.
    fn ai_test_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.pages.ai.editing else {
            return;
        };
        if self.pages.ai.status.busy() {
            return;
        }
        let (typed, endpoint) = self.ai_draft(cx);
        self.pages.ai.status = Status::Testing;
        cx.notify();
        self.spawn_tokio(
            window,
            cx,
            async move {
                tokio::task::spawn_blocking(move || run_check(target, typed, endpoint))
                    .await
                    .unwrap_or_else(|error| Err(error.to_string()))
            },
            move |this, result, _window, _cx| {
                if this.pages.ai.editing != Some(target) {
                    return;
                }
                this.pages.ai.status = match result {
                    Ok(ms) => Status::Passed(ms),
                    Err(error) => Status::Failed(error),
                };
            },
        );
    }

    /// "Save": tests first, and keeps nothing that does not answer.
    fn ai_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.pages.ai.editing else {
            return;
        };
        if self.pages.ai.status.busy() {
            return;
        }
        self.pages.ai.discovery.invalidate();
        let (typed, endpoint) = self.ai_draft(cx);
        self.pages.ai.status = Status::Saving;
        cx.notify();
        let tested_endpoint = endpoint.clone();
        self.spawn_tokio(
            window,
            cx,
            async move {
                let typed_for_test = typed.clone();
                tokio::task::spawn_blocking(move || {
                    run_check(target, typed_for_test, tested_endpoint)
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
                .and_then(|ms| {
                    if !typed.is_empty() {
                        ai_store::set_key(target.kind.store_kind(), target.id, &typed)?;
                    }
                    ai_store::set_endpoint(target.kind.store_kind(), target.id, &endpoint);
                    Ok(ms)
                })
            },
            move |this, result, _window, cx| match result {
                Ok(ms) => {
                    this.pages
                        .ai
                        .key
                        .update(cx, |input, cx| input.set_text("", cx));
                    this.pages.ai.editing = None;
                    this.pages.ai.status = Status::Idle;
                    this.pages.ai.checks.insert(target, Check::Passed(ms));
                }
                Err(error) => this.pages.ai.status = Status::Failed(error),
            },
        );
    }

    /// The row's "Test" button: the saved key, address and model.
    fn ai_check_saved(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        if self.pages.ai.checks.get(&target) == Some(&Check::Running) {
            return;
        }
        let (endpoint, ..) = defaults(target);
        self.pages.ai.checks.insert(target, Check::Running);
        cx.notify();
        self.spawn_tokio(
            window,
            cx,
            async move {
                tokio::task::spawn_blocking(move || run_check(target, String::new(), endpoint))
                    .await
                    .unwrap_or_else(|error| Err(error.to_string()))
            },
            move |this, result, _window, _cx| {
                if this.pages.ai.checks.get(&target) != Some(&Check::Running) {
                    return;
                }
                this.pages.ai.checks.insert(
                    target,
                    match result {
                        Ok(ms) => Check::Passed(ms),
                        Err(error) => Check::Failed(error),
                    },
                );
            },
        );
    }

    fn ai_discover_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self
            .pages
            .ai
            .editing
            .filter(|target| target.kind == ModelKind::Llm)
        else {
            return;
        };
        if self.pages.ai.status.busy() {
            return;
        }
        if self.pages.ai.discovery.loading {
            self.pages.ai.discovery.invalidate();
            cx.notify();
            return;
        }
        let draft_key = self.pages.ai.key.read(cx).text().trim().to_string();
        let address = self.pages.ai.address.read(cx).text().trim().to_string();
        let discovery = &mut self.pages.ai.discovery;
        discovery.invalidate();
        let generation = discovery.generation;
        let (cancel, cancelled) = tokio::sync::oneshot::channel();
        discovery.target = Some(target);
        discovery.loading = true;
        discovery.cancel = Some(cancel);
        self.spawn_tokio(
            window,
            cx,
            async move {
                let key = if draft_key.is_empty() {
                    tokio::task::spawn_blocking(move || {
                        ai_store::key(Kind::Llm, target.id).unwrap_or_default()
                    })
                    .await
                    .unwrap_or_default()
                } else {
                    draft_key
                };
                tokio::select! {
                    _ = cancelled => None,
                    result = llm::discover_models(target.id, &address, &key) => Some(result),
                }
            },
            move |this, result, _window, _cx| {
                let discovery = &mut this.pages.ai.discovery;
                if this.pages.ai.editing != Some(target) || !discovery.accepts(target, generation) {
                    return;
                }
                discovery.cancel = None;
                discovery.loading = false;
                match result {
                    Some(Ok(models)) => {
                        discovery.notice = Some(if models.is_empty() {
                            tr::ai::discovery_empty().to_string()
                        } else {
                            tr::ai::discovery_found(models.len())
                        });
                        discovery.models = models;
                    }
                    Some(Err(error)) => discovery.notice = Some(error.message()),
                    None => discovery.invalidate(),
                }
            },
        );
        cx.notify();
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
                cx.background_executor()
                    .timer(Duration::from_millis(300))
                    .await;
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
        editor_tabs::migrate_caption_model_selection();
        transcription::refresh_downloaded_models();
        let saved: Vec<(Kind, String)> = ai_store::saved_ids(Kind::Asr)
            .into_iter()
            .map(|id| (Kind::Asr, id))
            .chain(
                ai_store::saved_ids(Kind::Llm)
                    .into_iter()
                    .map(|id| (Kind::Llm, id)),
            )
            .filter(|(kind, id)| ai_store::key_available_cached(*kind, id).is_none())
            .collect();
        if !saved.is_empty() {
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .spawn(async move {
                        for (kind, id) in saved {
                            ai_store::key(kind, &id);
                        }
                    })
                    .await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
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

    fn ai_status(&self, tone: Tone, label: SharedString) -> impl IntoElement {
        let theme = self.theme;
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .text_size(px(12.))
            .text_color(theme.settings_muted())
            .child(div().size(px(7.)).rounded_full().bg(match tone {
                Tone::Ready => gpui::rgb(READY).into(),
                Tone::Error => gpui::rgb(ERROR).into(),
                Tone::Idle => theme.settings_border(),
            }))
            .child(label)
    }

    /// A tinted line under a form or row: green for a passed test, red for
    /// what went wrong.
    fn ai_feedback(&self, ok: bool, text: SharedString) -> gpui::Div {
        let color = if ok { READY } else { ERROR };
        div()
            .flex()
            .flex_row()
            .items_start()
            .gap(px(8.))
            .px(px(10.))
            .py(px(7.))
            .rounded(px(8.))
            .bg(tint(color))
            .text_size(px(12.))
            .line_height(px(18.))
            .text_color(self.theme.settings_text())
            .child(
                div()
                    .mt(px(5.5))
                    .size(px(7.))
                    .flex_shrink_0()
                    .rounded_full()
                    .bg(gpui::rgb(color)),
            )
            .child(div().flex_1().min_w_0().child(text))
    }

    fn ai_progress(&self, percent: f64) -> impl IntoElement {
        let theme = self.theme;
        div()
            .w_full()
            .h(px(4.))
            .rounded_full()
            .bg(theme.settings_fill())
            .child(
                div()
                    .h_full()
                    .rounded_full()
                    .w(relative((percent / 100.0).clamp(0.0, 1.0) as f32))
                    .bg(gpui::rgb(READY)),
            )
    }

    fn ai_tag(&self, tag: &'static str) -> impl IntoElement {
        let theme = self.theme;
        div()
            .px(px(6.))
            .rounded(px(5.))
            .bg(theme.settings_fill())
            .text_size(px(11.))
            .text_color(theme.settings_muted())
            .child(tag)
    }

    /// One row: badge, name with a note under it, and whatever sits on the right.
    fn ai_row(
        &self,
        badge: String,
        name: SharedString,
        tags: Vec<&'static str>,
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
                            .children(tags.into_iter().map(|tag| self.ai_tag(tag))),
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

    fn ai_disclosure(&self, open: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        div()
            .id("ai-advanced")
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .cursor_pointer()
            .text_size(px(12.))
            .text_color(theme.settings_muted())
            .hover(move |style| style.text_color(theme.settings_text()))
            .child(tr::settings::ai::advanced())
            .child(
                svg()
                    .path(if open {
                        "icons/chevron-up.svg"
                    } else {
                        "icons/chevron-down.svg"
                    })
                    .size(px(12.))
                    .text_color(theme.settings_muted()),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.pages.ai.advanced_open = !this.pages.ai.advanced_open;
                cx.notify();
            }))
    }

    fn ai_editor(&self, target: Target, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let state = &self.pages.ai;
        let (_, key_url, _, needs_key) = defaults(target);
        let has_models = !self.ai_model_choices(target).is_empty();
        let discovered =
            state.discovery.target == Some(target) && !state.discovery.models.is_empty();
        let busy = state.status.busy();
        let saved_tail = ai_store::cached_key_tail(target.kind.store_kind(), target.id);
        let saved = ai_store::endpoint(target.kind.store_kind(), target.id).is_some();
        let key_placeholder: SharedString = match &saved_tail {
            Some(Some(tail)) => tr::settings::ai::key_saved(tail).into(),
            _ if !needs_key => tr::settings::ai::key_optional().into(),
            None if saved => tr::settings::ai::key_unverified().into(),
            _ => tr::settings::ai::key_placeholder().into(),
        };
        state
            .key
            .update(cx, |input, _| input.set_placeholder(key_placeholder));
        state
            .address
            .update(cx, |input, _| input.set_placeholder("https://…"));
        state.model.update(cx, |input, _| {
            input.set_placeholder(tr::settings::ai::model_placeholder())
        });

        let account = account_target(target);
        let flow = (target.kind == ModelKind::Llm)
            .then(|| llm::provider(target.id).and_then(|provider| provider.sign_in))
            .flatten();
        let signing_in = state.status == Status::SigningIn;
        let signed_in = account.then(|| account_label(target)).flatten();
        let provider_name = match target.kind {
            ModelKind::Llm => llm::provider(target.id).map_or("", |provider| (provider.name)()),
            ModelKind::Asr => asr_cloud::service(target.id).map_or("", |service| (service.name)()),
        };
        let built_in = built_in_address(target);
        let show_address = !account
            && (address_always_shown(target)
                || address_shown(built_in, state.address.read(cx).text(), state.advanced_open));
        let can_fold = !account && !address_always_shown(target) && !built_in.is_empty();

        let sign_in_button = |label: String, primary: bool, cx: &mut Context<Self>| {
            if signing_in {
                self.button(
                    "ai-sign-in-stop",
                    (ui::ButtonVariant::Gray, None),
                    tr::common::cancel(),
                    false,
                    cx,
                    |this, _window, cx| this.ai_stop_sign_in(cx),
                )
            } else {
                self.button(
                    "ai-sign-in",
                    (
                        if primary {
                            ui::ButtonVariant::Primary
                        } else {
                            ui::ButtonVariant::Gray
                        },
                        None,
                    ),
                    label,
                    busy,
                    cx,
                    move |this, window, cx| this.ai_sign_in(target, window, cx),
                )
            }
        };

        let key_row: gpui::AnyElement = if account {
            let (text, color) = match &signed_in {
                Some(label) if label.is_empty() => (
                    tr::settings::ai::signed_in().to_string(),
                    theme.settings_text(),
                ),
                Some(label) => (tr::settings::ai::signed_in_as(label), theme.settings_text()),
                None => (
                    tr::settings::ai::not_signed_in().to_string(),
                    theme.settings_muted(),
                ),
            };
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.))
                        .text_color(color)
                        .child(text),
                )
                .child(sign_in_button(
                    if signed_in.is_some() {
                        tr::settings::ai::sign_in_again().to_string()
                    } else {
                        tr::settings::ai::sign_in_with(provider_name)
                    },
                    signed_in.is_none(),
                    cx,
                ))
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(self.pages_input("ai-key", &state.key)),
                )
                .when(flow.is_some(), |row| {
                    row.child(sign_in_button(
                        tr::settings::ai::authorize().to_string(),
                        false,
                        cx,
                    ))
                })
                .into_any_element()
        };

        let model_input = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(self.pages_input("ai-model", &state.model)),
            )
            .when(has_models, |row| {
                row.child(self.select(
                    "ai-common-models",
                    if discovered {
                        tr::ai::discovery_choices()
                    } else {
                        tr::settings::ai::common_models()
                    },
                    MenuKind::AiModel,
                    cx,
                ))
            })
            .when(target.kind == ModelKind::Llm, |row| {
                row.child(self.button(
                    "ai-discover-models",
                    (ui::ButtonVariant::Gray, None),
                    if state.discovery.loading {
                        tr::common::cancel()
                    } else {
                        tr::ai::discovery_fetch()
                    },
                    busy,
                    cx,
                    |this, window, cx| this.ai_discover_models(window, cx),
                ))
            });

        let discovery_note = if state.discovery.loading {
            Some(tr::ai::discovery_loading().to_string())
        } else {
            state.discovery.notice.clone()
        };

        let feedback = match &state.status {
            Status::Passed(ms) => Some((true, tr::settings::ai::test_passed(*ms))),
            Status::Failed(error) => Some((false, error.clone())),
            _ => None,
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
                if account {
                    tr::settings::ai::field_account()
                } else {
                    tr::settings::ai::field_key()
                },
                key_row,
            ))
            .when(account && signed_in.is_none() && !signing_in, |editor| {
                editor.child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(theme.settings_muted())
                        .child(tr::settings::ai::account_note(provider_name)),
                )
            })
            .when(signing_in, |editor| {
                editor.child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(theme.settings_muted())
                        .child(tr::settings::ai::signing_in()),
                )
            })
            .child(self.ai_field(
                tr::settings::ai::field_model(),
                model_input.into_any_element(),
            ))
            .when_some(discovery_note, |editor, note| {
                editor.child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(theme.settings_muted())
                        .child(note),
                )
            })
            .when(show_address, |editor| {
                editor.child(
                    self.ai_field(
                        tr::settings::ai::field_address(),
                        self.pages_input("ai-address", &state.address)
                            .into_any_element(),
                    ),
                )
            })
            .when(
                can_fold && (!show_address || state.advanced_open),
                |editor| editor.child(self.ai_disclosure(state.advanced_open, cx)),
            )
            .when_some(feedback, |editor, (ok, text)| {
                editor.child(self.ai_feedback(ok, text.into()))
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
                        if state.status == Status::Saving {
                            tr::settings::ai::saving()
                        } else {
                            tr::settings::ai::save()
                        },
                        busy || (account && signed_in.is_none()),
                        cx,
                        |this, window, cx| this.ai_save(window, cx),
                    ))
                    .child(self.button(
                        "ai-test",
                        (ui::ButtonVariant::Gray, None),
                        if state.status == Status::Testing {
                            tr::settings::ai::testing()
                        } else {
                            tr::settings::ai::test()
                        },
                        busy || (account && signed_in.is_none()),
                        cx,
                        |this, window, cx| this.ai_test_draft(window, cx),
                    ))
                    .child(self.button(
                        "ai-cancel",
                        (ui::ButtonVariant::Gray, None),
                        tr::common::cancel(),
                        busy,
                        cx,
                        |this, _window, cx| this.ai_cancel(cx),
                    ))
                    .child(div().flex_1())
                    .when(saved, |row| {
                        row.child(self.button(
                            "ai-remove",
                            (ui::ButtonVariant::Gray, None),
                            if account {
                                tr::settings::ai::sign_out()
                            } else {
                                tr::settings::ai::remove()
                            },
                            busy,
                            cx,
                            move |this, _window, cx| this.ai_remove(target, cx),
                        ))
                    })
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

    /// The right side of a service row: its state, and the buttons that
    /// test and open it.
    fn ai_service_right(
        &self,
        target: Target,
        ready: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let (tone, label): (Tone, SharedString) = match self.pages.ai.checks.get(&target) {
            Some(Check::Running) => (Tone::Idle, tr::settings::ai::testing().into()),
            Some(Check::Passed(ms)) if ready => {
                (Tone::Ready, tr::settings::ai::ready_ms(*ms).into())
            }
            Some(Check::Failed(_)) => (Tone::Error, tr::settings::ai::test_failed().into()),
            _ if ready => (Tone::Ready, tr::settings::ai::ready().into()),
            _ if configured_cached(target) => {
                (Tone::Idle, tr::settings::ai::key_unverified().into())
            }
            _ => (Tone::Idle, tr::settings::ai::not_set().into()),
        };
        let open = self.pages.ai.editing == Some(target);
        let saved = ai_store::endpoint(target.kind.store_kind(), target.id).is_some();
        let id_tail = format!("{}-{}", matches!(target.kind, ModelKind::Llm), target.id);
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .child(div().mr(px(4.)).child(self.ai_status(tone, label)))
            .when(!open && saved && configured_cached(target), |row| {
                row.child(self.button(
                    SharedString::from(format!("ai-check-{id_tail}")),
                    (ui::ButtonVariant::Gray, None),
                    tr::settings::ai::check(),
                    self.pages.ai.checks.get(&target) == Some(&Check::Running),
                    cx,
                    move |this, window, cx| this.ai_check_saved(target, window, cx),
                ))
            })
            .when(!open, |row| {
                row.child(self.button(
                    SharedString::from(format!("ai-open-{id_tail}")),
                    (ui::ButtonVariant::Gray, None),
                    if saved {
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

    /// A service's row, its failed check spelled out, and its open editor.
    fn ai_service_block(
        &self,
        target: Target,
        row: gpui::Div,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let open = self.pages.ai.editing == Some(target);
        let failure = match self.pages.ai.checks.get(&target) {
            Some(Check::Failed(error)) if !open => Some(error.clone()),
            _ => None,
        };
        div()
            .flex()
            .flex_col()
            .child(row)
            .when_some(failure, |column, error| {
                column.child(
                    div()
                        .mx(px(14.))
                        .mb(px(12.))
                        .child(self.ai_feedback(false, error.into())),
                )
            })
            .when(open, |column| column.child(self.ai_editor(target, cx)))
            .into_any_element()
    }

    fn ai_local_model(name: &str) -> Option<&'static CaptionModel> {
        editor_tabs::local_caption_models()
            .into_iter()
            .find(|model| model.name == name)
    }

    /// Under the default picker: whether that choice works right now, and
    /// the one button that makes it work when it does not.
    fn ai_recognizer_status(&self, selected: &'static str, cx: &mut Context<Self>) -> gpui::Div {
        let snapshot = transcription::ui_snapshot(Path::new(""));
        let body = div().flex().flex_col().gap(px(8.)).px(px(14.)).py(px(11.));
        if asr_cloud::is_cloud_model(selected) {
            let ready = asr_cloud::service(selected).is_some_and(|service| {
                ready_cached(Target {
                    kind: ModelKind::Asr,
                    id: service.id,
                })
            });
            return body.child(if ready {
                self.ai_status(Tone::Ready, tr::settings::ai::default_cloud_note().into())
            } else {
                self.ai_status(Tone::Idle, tr::settings::ai::cloud_setup().into())
            });
        }
        let Some(model) = Self::ai_local_model(selected) else {
            return body;
        };
        let name = model.name;
        let download = snapshot
            .download
            .as_ref()
            .filter(|download| download.model == name);
        if let Some(download) =
            download.filter(|download| download.state == transcription::DownloadState::Downloading)
        {
            let percent = download.progress.clamp(0.0, 100.0);
            return body
                .child(self.ai_status(
                    Tone::Idle,
                    tr::settings::ai::downloading(percent.round() as u32).into(),
                ))
                .child(self.ai_progress(percent));
        }
        if snapshot.downloaded.contains(name) {
            let used = transcription::downloaded_model_bytes()
                .get(name)
                .copied()
                .unwrap_or_default();
            let mut label = tr::settings::ai::default_local_note().to_string();
            if used > 0 {
                label = format!(
                    "{label} · {}",
                    tr::settings::ai::disk_used(format_bytes(used))
                );
            }
            return body.child(self.ai_status(Tone::Ready, label.into()));
        }
        let failure = download
            .filter(|download| download.state == transcription::DownloadState::Failed)
            .map(|download| download.message.clone());
        body.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(12.))
                .child(div().flex_1().min_w_0().child(self.ai_status(
                    Tone::Idle,
                    tr::settings::ai::needs_download(model.size).into(),
                )))
                .child(self.button(
                    "ai-download-default",
                    (ui::ButtonVariant::Primary, Some("icons/download.svg")),
                    tr::settings::ai::download(),
                    transcription::download_active(),
                    cx,
                    move |this, window, cx| this.ai_download(name, window, cx),
                )),
        )
        .when_some(failure, |body, message| {
            body.child(self.ai_feedback(false, message.into()))
        })
    }

    fn ai_recognizer_section(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let selected = selected_recognizer();
        let label = recognizer_choices()
            .into_iter()
            .find(|model| model.name == selected)
            .map_or(SharedString::default(), |model| (model.label)().into());
        self.section(
            tr::settings::ai::recognizer_title(),
            Some(tr::settings::ai::recognizer_description()),
            None,
            vec![
                self.rows(vec![
                    self.setting_row(
                        tr::settings::ai::default_label(),
                        None,
                        self.select("ai-recognizer", label, MenuKind::AiRecognizer, cx)
                            .into_any_element(),
                    ),
                    self.ai_recognizer_status(selected, cx).into_any_element(),
                ])
                .into_any_element(),
            ],
        )
        .into_any_element()
    }

    fn ai_local_section(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let snapshot = transcription::ui_snapshot(Path::new(""));
        let sizes = transcription::downloaded_model_bytes();
        let selected = selected_recognizer();
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
                    let percent = download.progress.clamp(0.0, 100.0);
                    div()
                        .flex()
                        .flex_col()
                        .items_end()
                        .gap(px(6.))
                        .w(px(132.))
                        .child(self.ai_status(
                            Tone::Idle,
                            tr::settings::ai::downloading(percent.round() as u32).into(),
                        ))
                        .child(self.ai_progress(percent))
                        .into_any_element()
                } else if downloaded {
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(12.))
                        .child(self.ai_status(Tone::Ready, tr::settings::ai::downloaded().into()))
                        .child(self.button(
                            SharedString::from(format!("ai-delete-{name}")),
                            (ui::ButtonVariant::Gray, None),
                            tr::settings::ai::delete(),
                            deleting
                                || snapshot.download.as_ref().is_some_and(|download| {
                                    download.state == transcription::DownloadState::Downloading
                                }),
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
                let mut tags = Vec::new();
                if name == selected {
                    tags.push(tr::settings::ai::default_tag());
                }
                if name == crate::local_qwen::MODEL {
                    tags.push(tr::settings::ai::recommended());
                }
                let size = match sizes.get(name) {
                    Some(bytes) if downloaded && *bytes > 0 => {
                        tr::settings::ai::disk_used(format_bytes(*bytes))
                    }
                    _ => model.size.to_string(),
                };
                self.ai_row(
                    monogram(model.model_name),
                    (model.label)().into(),
                    tags,
                    format!("{} · {size}", (model.description)()).into(),
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
            children.push(self.ai_feedback(false, message.into()).into_any_element());
        }
        let total: u64 = sizes.values().sum();
        let total = (total > 0).then(|| {
            div()
                .text_size(px(12.))
                .text_color(self.theme.settings_muted())
                .child(tr::settings::ai::disk_total(format_bytes(total)))
                .into_any_element()
        });
        self.section(
            tr::settings::ai::local_title(),
            Some(tr::settings::ai::local_description()),
            total,
            children,
        )
        .into_any_element()
    }

    fn ai_cloud_section(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let configured = ai_store::saved_ids(Kind::Asr);
        let selected = store::store_section(transcription::GPUI_STORE_SECTION)
            .get(transcription::SELECTED_MODEL_KEY)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let primary =
            |id| asr_service_visible(id, &configured, selected.as_deref(), self.pages.ai.editing);
        let more_count = asr_cloud::SERVICES
            .iter()
            .filter(|service| !primary(service.id))
            .count();
        let rows = asr_cloud::SERVICES
            .iter()
            .filter(|service| self.pages.ai.more_asr_open || primary(service.id))
            .map(|service| {
                let target = Target {
                    kind: ModelKind::Asr,
                    id: service.id,
                };
                let mut tags = Vec::new();
                if selected.as_deref() == Some(service.id) {
                    tags.push(tr::settings::ai::default_tag());
                }
                if service.id == "doubao" {
                    tags.push(tr::settings::ai::recommended());
                }
                let row = self.ai_row(
                    monogram((service.name)()),
                    (service.name)().into(),
                    tags,
                    (service.note)().into(),
                    self.ai_service_right(target, ready_cached(target), cx),
                );
                self.ai_service_block(target, row, cx)
            })
            .collect();
        let mut children = vec![self.rows(rows).into_any_element()];
        if more_count > 0 {
            children.push(
                self.button(
                    "ai-more-speech-services",
                    (
                        ui::ButtonVariant::Gray,
                        Some(if self.pages.ai.more_asr_open {
                            "icons/chevron-up.svg"
                        } else {
                            "icons/chevron-down.svg"
                        }),
                    ),
                    if self.pages.ai.more_asr_open {
                        tr::ai::fewer_speech_services().to_string()
                    } else {
                        tr::ai::more_speech_services(more_count)
                    },
                    false,
                    cx,
                    |this, _, cx| {
                        this.pages.ai.more_asr_open = !this.pages.ai.more_asr_open;
                        cx.notify();
                    },
                )
                .into_any_element(),
            );
        }
        self.section(
            tr::settings::ai::cloud_title(),
            Some(tr::settings::ai::cloud_description()),
            None,
            children,
        )
        .into_any_element()
    }

    fn ai_llm_section(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let editing_new = self.pages.ai.editing.filter(|target| {
            target.kind == ModelKind::Llm && ai_store::endpoint(Kind::Llm, target.id).is_none()
        });
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        let ready: Vec<_> = llm::configured()
            .into_iter()
            .filter(|p| {
                configured_cached(Target {
                    kind: ModelKind::Llm,
                    id: p.id,
                })
            })
            .collect();
        if !ready.is_empty() {
            let default = default_provider_cached().map_or(SharedString::default(), |p| {
                format!("{} · {}", (p.name)(), p.endpoint().model).into()
            });
            rows.push(
                self.setting_row(
                    tr::settings::ai::llm_default(),
                    None,
                    self.select("ai-llm-default", default, MenuKind::AiLlmDefault, cx)
                        .into_any_element(),
                ),
            );
        }
        let providers = llm::configured()
            .into_iter()
            .map(|provider| provider.id)
            .chain(editing_new.map(|target| target.id));
        for id in providers {
            let Some(provider) = llm::provider(id) else {
                continue;
            };
            let target = Target {
                kind: ModelKind::Llm,
                id,
            };
            let endpoint = provider.endpoint();
            let mut note = if endpoint.model.is_empty() {
                tr::settings::ai::model_placeholder().to_string()
            } else {
                endpoint.model
            };
            if let Some(label) = account_label(target).filter(|label| !label.is_empty()) {
                note = format!("{label} · {note}");
            }
            let note: SharedString = note.into();
            let row = self.ai_row(
                monogram((provider.name)()),
                (provider.name)().into(),
                Vec::new(),
                note,
                self.ai_service_right(target, ready_cached(target), cx),
            );
            rows.push(self.ai_service_block(target, row, cx));
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
                self.select(
                    "ai-llm-add",
                    tr::settings::ai::llm_add(),
                    MenuKind::AiLlmAdd,
                    cx,
                )
                .into_any_element(),
            ),
            vec![body],
        )
        .into_any_element()
    }

    pub(crate) fn render_ai_models(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let mut sections = vec![
            self.ai_recognizer_section(cx),
            self.ai_local_section(cx),
            self.ai_cloud_section(cx),
            self.ai_llm_section(cx),
            self.render_cover_settings(cx),
            self.ai_review_section(cx),
        ];
        sections.extend(self.render_transcription(cx));
        sections
    }

    fn ai_review_section(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let settings = crate::caption_review::ReviewSettings::load();
        let theme = self.theme;
        let mut body = self
            .card(true)
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(tr::settings::review::enabled())
                    .child(self.toggle(
                        "ai-review-enabled",
                        tr::settings::review::enabled(),
                        settings.enabled,
                        cx,
                        |this, cx| {
                            let mut settings = crate::caption_review::ReviewSettings::load();
                            settings.enabled = !settings.enabled;
                            if !settings.save() {
                                this.pages.ai.review_notice =
                                    Some(tr::settings::review::save_failed().into());
                            }
                            cx.notify();
                        },
                    )),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.settings_muted())
                    .child(tr::settings::review::description()),
            )
            .child(
                ui::Button::plain(
                    &theme,
                    "ai-review-rules",
                    ui::ButtonVariant::Gray,
                    ui::ButtonSize::Sm,
                )
                .label(tr::settings::review::edit_rules())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.pages.ai.review_open = !this.pages.ai.review_open;
                    cx.notify();
                })),
            );
        if self.pages.ai.review_open {
            body = body
                .child(
                    ui::TextInput::plain(&theme, "ai-review-prompt", &self.pages.ai.review_prompt)
                        .height(px(180.))
                        .line_height(px(20.))
                        .padding_y(px(10.)),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(
                            ui::Button::plain(
                                &theme,
                                "ai-review-save",
                                ui::ButtonVariant::Primary,
                                ui::ButtonSize::Sm,
                            )
                            .label(tr::settings::review::save())
                            .on_click(cx.listener(|this, _, _, cx| {
                                let mut settings = crate::caption_review::ReviewSettings::load();
                                settings.prompt = this
                                    .pages
                                    .ai
                                    .review_prompt
                                    .read(cx)
                                    .text()
                                    .trim()
                                    .to_string();
                                this.pages.ai.review_notice = Some(
                                    if settings.save() {
                                        tr::settings::review::saved()
                                    } else {
                                        tr::settings::review::save_failed()
                                    }
                                    .into(),
                                );
                                cx.notify();
                            })),
                        )
                        .child(
                            ui::Button::plain(
                                &theme,
                                "ai-review-restore",
                                ui::ButtonVariant::Gray,
                                ui::ButtonSize::Sm,
                            )
                            .label(tr::settings::review::restore())
                            .on_click(cx.listener(|this, _, _, cx| {
                                let mut settings = crate::caption_review::ReviewSettings::load();
                                settings.prompt = crate::caption_review::DEFAULT_PROMPT.into();
                                if settings.save() {
                                    this.pages.ai.review_prompt.update(cx, |input, cx| {
                                        input.set_text(settings.prompt, cx)
                                    });
                                    this.pages.ai.review_notice =
                                        Some(tr::settings::review::saved().into());
                                } else {
                                    this.pages.ai.review_notice =
                                        Some(tr::settings::review::save_failed().into());
                                }
                                cx.notify();
                            })),
                        ),
                );
        }
        if let Some(notice) = &self.pages.ai.review_notice {
            body = body.child(div().text_size(px(12.)).child(notice.clone()));
        }
        self.section(
            tr::settings::review::title(),
            None,
            None,
            vec![body.into_any_element()],
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod discovery_tests {
    use super::*;

    #[test]
    fn collapsed_speech_services_keep_configured_defaults_and_open_editors_visible() {
        let configured = vec!["siliconflow".to_string()];
        let editing = Some(Target {
            kind: ModelKind::Asr,
            id: "custom",
        });
        let visible: Vec<_> = asr_cloud::SERVICES
            .iter()
            .filter(|service| asr_service_visible(service.id, &configured, Some("groq"), editing))
            .map(|service| service.id)
            .collect();
        assert_eq!(visible, ["doubao", "siliconflow", "groq", "custom"]);
        assert!(asr_service_visible("groq", &[], Some("groq"), None));
        assert!(!asr_service_visible(
            "openai",
            &[],
            None,
            Some(Target {
                kind: ModelKind::Llm,
                id: "openai"
            })
        ));
    }

    #[test]
    fn fresh_speech_settings_show_doubao_without_removing_alternative_services() {
        let visible: Vec<_> = asr_cloud::SERVICES
            .iter()
            .filter(|service| asr_service_visible(service.id, &[], None, None))
            .map(|service| service.id)
            .collect();
        assert_eq!(visible, ["doubao"]);
        assert!(asr_cloud::service("local-server").is_some());
        assert!(asr_cloud::service("custom").is_some());
    }

    #[test]
    fn edited_credentials_cancel_and_reject_late_results_even_for_same_provider() {
        let target = Target {
            kind: ModelKind::Llm,
            id: "deepseek",
        };
        let (cancel, mut cancelled) = tokio::sync::oneshot::channel();
        let mut discovery = ModelDiscovery {
            generation: 4,
            target: Some(target),
            cancel: Some(cancel),
            loading: true,
            models: vec![llm::DiscoveredModel {
                id: "previous-account-model".into(),
                display_name: None,
            }],
            notice: None,
        };
        assert!(discovery.accepts(target, 4));
        assert!(!discovery.accepts(
            Target {
                kind: ModelKind::Llm,
                id: "openai"
            },
            4
        ));
        discovery.invalidate();
        assert_eq!(cancelled.try_recv(), Ok(()));
        assert!(discovery.models.is_empty());
        discovery.target = Some(target);
        discovery.loading = true;
        assert!(!discovery.accepts(target, 4));
        assert!(discovery.accepts(target, 5));
    }

    #[test]
    fn disk_sizes_read_like_finder() {
        assert_eq!(format_bytes(1_019_141_728), "1.0 GB");
        assert_eq!(format_bytes(487_601_967), "488 MB");
        assert_eq!(format_bytes(200), "1 MB");
    }

    #[test]
    fn the_address_stays_folded_until_it_matters() {
        let built_in = "https://api.example.com/v1";
        assert!(!address_shown(built_in, built_in, false));
        assert!(address_shown(built_in, built_in, true));
        assert!(address_shown(
            built_in,
            "https://relay.example.com/v1",
            false
        ));
        assert!(address_shown("", "", false));
    }

    #[test]
    fn closing_settings_cancels_discovery_instead_of_leaving_background_work() {
        let (cancel, mut cancelled) = tokio::sync::oneshot::channel();
        let discovery = ModelDiscovery {
            generation: 0,
            target: None,
            cancel: Some(cancel),
            loading: true,
            models: Vec::new(),
            notice: None,
        };
        drop(discovery);
        assert_eq!(cancelled.try_recv(), Ok(()));
    }
}
