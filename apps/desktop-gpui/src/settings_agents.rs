use crate::{
    i18n::tr,
    llm,
    local_agents::{self, AgentConnection},
    settings_ai::monogram,
    settings_window::SettingsWindow,
    ui,
};
use gpui::{
    AppContext as _, Context, Entity, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};

pub(crate) struct AgentsSettings {
    detected: Vec<AgentConnection>,
    name: Entity<ui::TextInputState>,
    command: Entity<ui::TextInputState>,
    args: Entity<ui::TextInputState>,
    custom: bool,
    show_unavailable: bool,
    model_picker: Option<String>,
    model_query: Entity<ui::TextInputState>,
    _model_query_subscription: gpui::Subscription,
    busy: Option<String>,
    notice: Option<String>,
    task: Option<gpui::Task<()>>,
}

impl AgentsSettings {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<SettingsWindow>) -> Self {
        let model_query = cx.new(|cx| {
            let mut input = ui::TextInputState::single_line(window, cx);
            input.set_placeholder(tr::settings::agents::search_models());
            input
        });
        let model_query_subscription = cx.subscribe(&model_query, |_, _, event, cx| {
            if *event == ui::TextInputEvent::Changed {
                cx.notify();
            }
        });
        let args = cx.new(|cx| ui::TextInputState::single_line(window, cx));
        args.update(cx, |input, cx| input.set_text("[]", cx));
        Self {
            detected: local_agents::detect_all(),
            name: cx.new(|cx| ui::TextInputState::single_line(window, cx)),
            command: cx.new(|cx| ui::TextInputState::single_line(window, cx)),
            args,
            custom: false,
            show_unavailable: false,
            model_picker: None,
            model_query,
            _model_query_subscription: model_query_subscription,
            busy: None,
            notice: None,
            task: None,
        }
    }
}

impl SettingsWindow {
    fn connect_agent(
        &mut self,
        mut connection: AgentConnection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pages.agents.busy.is_some() {
            return;
        }
        let prepare = connection.command.is_empty();
        self.pages.agents.busy = Some(connection.id.clone());
        self.pages.agents.notice = None;
        self.pages.agents.task = Some(cx.spawn_in(window, async move |this, cx| {
            let Ok(task) = cx.update(|_, cx| {
                gpui_tokio::Tokio::spawn(cx, async move {
                    if prepare {
                        connection = local_agents::prepare(&connection.id).await?;
                    }
                    connection.models = local_agents::connect(&connection).await?;
                    if !connection.model.is_empty()
                        && !connection.models.iter().any(|m| m.id == connection.model)
                    {
                        connection.model.clear();
                    }
                    local_agents::save(connection)
                })
            }) else {
                return;
            };
            let result = task
                .await
                .unwrap_or_else(|_| Err(tr::settings::agents::launch_failed().into()));
            this.update_in(cx, |this, _, cx| {
                this.pages.agents.busy = None;
                this.pages.agents.task = None;
                this.pages.agents.detected = local_agents::detect_all();
                this.pages.agents.notice = result.err();
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn agent_row(
        &self,
        name: &str,
        id: &str,
        connection: Option<&AgentConnection>,
        connected: bool,
        default: Option<&str>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let state = &self.pages.agents;
        let theme = self.theme;
        let current_model =
            default.and_then(|default| default.strip_prefix(&llm::agent_content_id(id, "")));
        let note = if let Some(connection) = connection.filter(|_| connected) {
            let name = current_model.and_then(|current| {
                if current.is_empty() {
                    Some(tr::editor::assistant::default_agent())
                } else {
                    connection
                        .models
                        .iter()
                        .find(|model| model.id == current)
                        .map(|model| model.name.as_str())
                }
            });
            name.map_or_else(
                || tr::settings::agents::connected().into(),
                |name| format!("{} · {name}", tr::settings::agents::content_default()),
            )
        } else if connection.is_none() {
            tr::settings::agents::not_found().into()
        } else {
            String::new()
        };
        let busy = state.busy.is_some();
        let is_connecting = state.busy.as_deref() == Some(id);
        let picker_open = state.model_picker.as_deref() == Some(id);
        let mut actions = div().flex().items_center().gap(px(6.));
        if connected {
            let id = id.to_owned();
            actions = actions.child(
                ui::Button::settings(
                    &theme,
                    SharedString::from(format!("agent-pick-{id}")),
                    ui::ButtonVariant::Gray,
                    ui::ButtonSize::Sm,
                )
                .label(tr::settings::agents::use_for_content())
                .right_icon(if picker_open {
                    "icons/chevron-up.svg"
                } else {
                    "icons/chevron-down.svg"
                })
                .disabled(busy)
                .on_click(cx.listener(move |this, _, window, cx| {
                    let state = &mut this.pages.agents;
                    if state.model_picker.as_deref() == Some(id.as_str()) {
                        state.model_picker = None;
                    } else {
                        state.model_picker = Some(id.clone());
                        state
                            .model_query
                            .update(cx, |input, cx| input.set_text("", cx));
                        let focus = state.model_query.read(cx).focus_handle();
                        window.focus(&focus, cx);
                    }
                    cx.notify();
                })),
            );
        }
        if let Some(connection) = connection {
            let connection = connection.clone();
            let prepare = connection.command.is_empty();
            if connected && !is_connecting {
                actions = actions.child(
                    ui::EditorButton::plain(
                        &theme,
                        SharedString::from(format!("agent-refresh-{id}")),
                    )
                    .left_icon("icons/refresh-cw.svg")
                    .tooltip(&theme, tr::settings::agents::refresh())
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.connect_agent(connection.clone(), window, cx)
                    })),
                );
            } else {
                let label = match (is_connecting, prepare) {
                    (true, true) => tr::settings::agents::preparing(),
                    (true, false) => tr::settings::agents::connecting(),
                    (false, true) => tr::settings::agents::prepare(),
                    (false, false) => tr::settings::agents::connect(),
                };
                actions = actions.child(
                    ui::Button::settings(
                        &theme,
                        SharedString::from(format!("agent-connect-{id}")),
                        ui::ButtonVariant::Gray,
                        ui::ButtonSize::Sm,
                    )
                    .label(label)
                    .icon(if prepare {
                        "icons/download.svg"
                    } else {
                        "icons/link.svg"
                    })
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.connect_agent(connection.clone(), window, cx)
                    })),
                );
            }
        }
        if connected {
            let id = id.to_owned();
            actions = actions.child(
                ui::EditorButton::plain(&theme, SharedString::from(format!("agent-remove-{id}")))
                    .left_icon("icons/unplug.svg")
                    .tooltip(&theme, tr::settings::agents::remove())
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.pages.agents.notice = local_agents::remove(&id).err();
                        if this.pages.agents.model_picker.as_deref() == Some(&id) {
                            this.pages.agents.model_picker = None;
                        }
                        cx.notify();
                    })),
            );
        }
        div()
            .child(self.ai_row(
                monogram(name),
                name.to_owned().into(),
                Vec::new(),
                note.into(),
                actions.into_any_element(),
            ))
            .children(
                connection
                    .filter(|_| connected && picker_open)
                    .map(|connection| self.agent_model_picker(connection, current_model, cx)),
            )
            .into_any_element()
    }

    fn agent_model_picker(
        &self,
        connection: &AgentConnection,
        current: Option<&str>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme;
        let query = self
            .pages
            .agents
            .model_query
            .read(cx)
            .text()
            .trim()
            .to_lowercase();
        let models = std::iter::once(("", tr::editor::assistant::default_agent())).chain(
            connection
                .models
                .iter()
                .map(|model| (model.id.as_str(), model.name.as_str())),
        );
        let choices: Vec<_> = models
            .filter(|(id, name)| {
                id.to_lowercase().contains(&query) || name.to_lowercase().contains(&query)
            })
            .map(|(id, name)| {
                let is_current = current == Some(id);
                let content_id = llm::agent_content_id(&connection.id, id);
                let mut choice =
                    ui::EditorButton::plain(&theme, SharedString::from(content_id.clone()))
                        .label(name.to_owned())
                        .start()
                        .grow()
                        .pressed(is_current)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if crate::ai_store::set_llm_default(&content_id) {
                                this.pages.agents.model_picker = None;
                                this.pages.agents.notice = None;
                            } else {
                                this.pages.agents.notice =
                                    Some(tr::settings::agents::save_failed().into());
                            }
                            cx.notify();
                        }));
                if is_current {
                    choice = choice.left_icon("icons/check.svg");
                }
                if !id.is_empty() && id != name {
                    choice = choice.tooltip(&theme, id.to_owned());
                }
                div().flex().child(choice)
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .px(px(14.))
            .pb(px(10.))
            .child(ui::TextInput::search(
                &theme,
                "agent-model-search",
                &self.pages.agents.model_query,
            ))
            .child(if choices.is_empty() {
                div()
                    .px(px(7.))
                    .text_size(px(12.))
                    .text_color(theme.settings_muted())
                    .child(tr::settings::agents::models_no_matches())
                    .into_any_element()
            } else {
                div()
                    .id("agent-model-list")
                    .max_h(px(206.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .children(choices)
                    .into_any_element()
            })
            .into_any_element()
    }

    pub(crate) fn render_agent_settings(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let state = &self.pages.agents;
        let saved = local_agents::connections();
        let default = crate::ai_store::llm_default();
        let mut body = div().flex().flex_col().gap(px(10.));
        if let Some(notice) = &state.notice {
            body = body.child(self.card(true).text_size(px(12.)).child(notice.clone()));
        }
        let mut rows = Vec::new();
        for connection in &saved {
            rows.push(self.agent_row(
                &connection.name,
                &connection.id,
                Some(connection),
                true,
                default.as_deref(),
                cx,
            ));
        }
        let mut unavailable = false;
        for preset in local_agents::PRESETS {
            if saved.iter().any(|connection| connection.id == preset.id) {
                continue;
            }
            let connection = state
                .detected
                .iter()
                .find(|connection| connection.id == preset.id)
                .cloned()
                .or_else(|| preset.can_prepare().then(|| preset.pending_connection()));
            unavailable |= connection.is_none();
            if connection.is_some() || state.show_unavailable {
                rows.push(self.agent_row(
                    preset.name,
                    preset.id,
                    connection.as_ref(),
                    false,
                    None,
                    cx,
                ));
            }
        }
        if !rows.is_empty() {
            body = body.child(self.rows(rows));
        }
        body = body.child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(8.))
                .child(
                    ui::Button::plain(
                        &theme,
                        "agents-detect",
                        ui::ButtonVariant::Gray,
                        ui::ButtonSize::Sm,
                    )
                    .label(tr::settings::agents::detect())
                    .icon("icons/refresh-cw.svg")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.pages.agents.detected = local_agents::detect_all();
                        this.pages.agents.notice = None;
                        cx.notify();
                    })),
                )
                .child(
                    ui::Button::plain(
                        &theme,
                        "agents-custom",
                        ui::ButtonVariant::Gray,
                        ui::ButtonSize::Sm,
                    )
                    .label(tr::settings::agents::custom())
                    .icon("icons/plus.svg")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.pages.agents.custom = !this.pages.agents.custom;
                        cx.notify();
                    })),
                )
                .children(unavailable.then(|| {
                    ui::Button::plain(
                        &theme,
                        "agents-show-unavailable",
                        ui::ButtonVariant::Gray,
                        ui::ButtonSize::Sm,
                    )
                    .label(if state.show_unavailable {
                        tr::settings::agents::hide_unavailable()
                    } else {
                        tr::settings::agents::show_unavailable()
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.pages.agents.show_unavailable = !this.pages.agents.show_unavailable;
                        cx.notify();
                    }))
                })),
        );
        if state.custom {
            for (id, label, input) in [
                ("name", tr::settings::agents::name(), &state.name),
                ("command", tr::settings::agents::command(), &state.command),
                ("args", tr::settings::agents::args(), &state.args),
            ] {
                body = body.child(
                    div().flex().flex_col().gap(px(6.)).child(label).child(
                        ui::TextInput::bare(
                            &theme,
                            SharedString::from(format!("agent-{id}")),
                            input,
                        )
                        .height(px(32.)),
                    ),
                );
            }
            body = body.child(
                ui::Button::plain(
                    &theme,
                    "agent-custom-save",
                    ui::ButtonVariant::Primary,
                    ui::ButtonSize::Sm,
                )
                .label(tr::settings::agents::connect())
                .disabled(state.busy.is_some())
                .on_click(cx.listener(|this, _, window, cx| {
                    let state = &this.pages.agents;
                    let name = state.name.read(cx).text().trim().to_owned();
                    let command = state.command.read(cx).text().trim().to_owned();
                    let args = serde_json::from_str::<Vec<String>>(state.args.read(cx).text());
                    if name.is_empty() || name.len() > 80 || command.is_empty() || args.is_err() {
                        this.pages.agents.notice =
                            Some(tr::settings::agents::invalid_command().into());
                        cx.notify();
                        return;
                    }
                    if let Ok(args) = args {
                        this.connect_agent(
                            AgentConnection {
                                id: format!("custom-{}", uuid::Uuid::new_v4()),
                                name,
                                command,
                                args,
                                models: vec![],
                                model: String::new(),
                            },
                            window,
                            cx,
                        );
                    }
                })),
            );
        }
        self.section(
            tr::settings::agents::title(),
            Some(tr::settings::agents::description()),
            None,
            vec![body.into_any_element()],
        )
        .into_any_element()
    }
}
