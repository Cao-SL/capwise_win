use crate::{
    i18n::tr,
    local_agents::{self, AgentConnection},
    settings_window::SettingsWindow,
    ui,
};
use gpui::{
    AppContext as _, Context, Entity, IntoElement, ParentElement, SharedString, Styled, Window,
    div, px,
};

pub(crate) struct AgentsSettings {
    detected: Vec<AgentConnection>,
    name: Entity<ui::TextInputState>,
    command: Entity<ui::TextInputState>,
    args: Entity<ui::TextInputState>,
    custom: bool,
    busy: Option<String>,
    notice: Option<String>,
    task: Option<gpui::Task<()>>,
}

impl AgentsSettings {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<SettingsWindow>) -> Self {
        let args = cx.new(|cx| ui::TextInputState::single_line(window, cx));
        args.update(cx, |input, cx| input.set_text("[]", cx));
        Self {
            detected: local_agents::PRESETS
                .iter()
                .filter_map(local_agents::detect)
                .collect(),
            name: cx.new(|cx| ui::TextInputState::single_line(window, cx)),
            command: cx.new(|cx| ui::TextInputState::single_line(window, cx)),
            args,
            custom: false,
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
        self.pages.agents.busy = Some(connection.id.clone());
        self.pages.agents.notice = None;
        self.pages.agents.task = Some(cx.spawn_in(window, async move |this, cx| {
            let Ok(task) = cx.update(|_, cx| {
                gpui_tokio::Tokio::spawn(cx, async move {
                    connection.models = local_agents::probe(&connection).await?;
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
                this.pages.agents.notice = Some(
                    result
                        .err()
                        .unwrap_or_else(|| tr::settings::agents::connected().into()),
                );
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub(crate) fn render_agent_settings(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let state = &self.pages.agents;
        let saved = local_agents::connections();
        let mut body = self.card(true).flex().flex_col().gap(px(12.));
        for preset in local_agents::PRESETS {
            let connected = saved.iter().find(|c| c.id == preset.id);
            let detected = connected
                .or_else(|| state.detected.iter().find(|c| c.id == preset.id))
                .cloned();
            let id = preset.id.to_owned();
            let label = if state.busy.as_deref() == Some(preset.id) {
                tr::settings::agents::connecting()
            } else if connected.is_some() {
                tr::settings::agents::refresh()
            } else {
                tr::settings::agents::connect()
            };
            let mut row = div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().flex_1().child(preset.name));
            if let Some(connection) = detected {
                row = row.child(
                    ui::Button::plain(
                        &theme,
                        SharedString::from(format!("agent-connect-{id}")),
                        ui::ButtonVariant::Gray,
                        ui::ButtonSize::Sm,
                    )
                    .label(label)
                    .disabled(state.busy.is_some())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.connect_agent(connection.clone(), window, cx)
                    })),
                );
            } else {
                row = row.child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme.settings_muted())
                        .child(tr::settings::agents::not_found()),
                );
            }
            if connected.is_some() {
                row = row.child(
                    ui::Button::plain(
                        &theme,
                        SharedString::from(format!("agent-remove-{id}")),
                        ui::ButtonVariant::Gray,
                        ui::ButtonSize::Sm,
                    )
                    .label(tr::settings::agents::remove())
                    .disabled(state.busy.is_some())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.pages.agents.notice = local_agents::remove(&id).err();
                        cx.notify();
                    })),
                );
            }
            body = body.child(row);
        }
        for connection in saved
            .iter()
            .filter(|c| !local_agents::PRESETS.iter().any(|p| p.id == c.id))
        {
            let refresh = connection.clone();
            let id = connection.id.clone();
            body = body.child(
                div()
                    .flex()
                    .gap(px(8.))
                    .items_center()
                    .child(div().flex_1().child(connection.name.clone()))
                    .child(
                        ui::Button::plain(
                            &theme,
                            SharedString::from(format!("agent-refresh-{id}")),
                            ui::ButtonVariant::Gray,
                            ui::ButtonSize::Sm,
                        )
                        .label(tr::settings::agents::refresh())
                        .disabled(state.busy.is_some())
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.connect_agent(refresh.clone(), window, cx)
                            },
                        )),
                    )
                    .child(
                        ui::Button::plain(
                            &theme,
                            SharedString::from(format!("agent-remove-{id}")),
                            ui::ButtonVariant::Gray,
                            ui::ButtonSize::Sm,
                        )
                        .label(tr::settings::agents::remove())
                        .disabled(state.busy.is_some())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.pages.agents.notice = local_agents::remove(&id).err();
                            cx.notify();
                        })),
                    ),
            );
        }
        body = body.child(
            div()
                .flex()
                .gap(px(8.))
                .child(
                    ui::Button::plain(
                        &theme,
                        "agents-detect",
                        ui::ButtonVariant::Gray,
                        ui::ButtonSize::Sm,
                    )
                    .label(tr::settings::agents::detect())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.pages.agents.detected = local_agents::PRESETS
                            .iter()
                            .filter_map(local_agents::detect)
                            .collect();
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
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.pages.agents.custom = !this.pages.agents.custom;
                        cx.notify();
                    })),
                ),
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
                    if name.is_empty() || name.len() > 80 || args.is_err() {
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
        if let Some(notice) = &state.notice {
            body = body.child(div().text_size(px(12.)).child(notice.clone()));
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
