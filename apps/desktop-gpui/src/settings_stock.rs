use gpui::{
    AppContext as _, Context, Entity, FontWeight, IntoElement, ParentElement, SharedString, Styled,
    Window, div, px,
};

use crate::{ai_store, i18n::tr, settings_window::SettingsWindow, stock_media::Provider, ui};

pub(crate) struct StockSettings {
    draft: Entity<ui::TextInputState>,
    editing: Option<Provider>,
    busy: bool,
    notice: Option<(Provider, Notice)>,
}

#[derive(Clone, Copy)]
enum Notice {
    Saved,
    Removed,
    Invalid,
    SaveFailed,
    RemoveFailed,
}

impl Notice {
    fn text(self) -> &'static str {
        match self {
            Self::Saved => tr::settings::stock::saved(),
            Self::Removed => tr::settings::stock::removed(),
            Self::Invalid => tr::settings::stock::invalid_key(),
            Self::SaveFailed => tr::settings::stock::save_failed(),
            Self::RemoveFailed => tr::settings::stock::remove_failed(),
        }
    }

    fn is_error(self) -> bool {
        matches!(self, Self::Invalid | Self::SaveFailed | Self::RemoveFailed)
    }
}

impl StockSettings {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<SettingsWindow>) -> Self {
        Self {
            draft: cx.new(|cx| {
                let mut input = ui::TextInputState::single_line(window, cx);
                input.set_secret(true, cx);
                input
            }),
            editing: None,
            busy: false,
            notice: None,
        }
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

impl SettingsWindow {
    fn edit_stock_key(&mut self, provider: Provider, window: &mut Window, cx: &mut Context<Self>) {
        if self.pages.stock.busy {
            return;
        }
        let state = &mut self.pages.stock;
        state.editing = Some(provider);
        state.notice = None;
        state.draft.update(cx, |input, cx| input.set_text("", cx));
        window.focus(&state.draft.read(cx).focus_handle(), cx);
        cx.notify();
    }

    fn cancel_stock_key(&mut self, cx: &mut Context<Self>) {
        if self.pages.stock.busy {
            return;
        }
        self.pages.stock.editing = None;
        self.pages.stock.notice = None;
        self.pages
            .stock
            .draft
            .update(cx, |input, cx| input.set_text("", cx));
        cx.notify();
    }

    fn save_stock_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(provider) = self.pages.stock.editing.filter(|_| !self.pages.stock.busy) else {
            return;
        };
        let draft = self.pages.stock.draft.read(cx).text().to_string();
        self.pages
            .stock
            .draft
            .update(cx, |input, cx| input.set_text("", cx));
        if !ai_store::stock_key_is_valid(&draft) {
            self.pages.stock.notice = Some((provider, Notice::Invalid));
            cx.notify();
            return;
        }
        self.pages.stock.busy = true;
        self.pages.stock.notice = None;
        self.pages
            .stock
            .draft
            .update(cx, |input, cx| input.set_disabled(true, cx));
        self.spawn_tokio(
            window,
            cx,
            async move {
                tokio::task::spawn_blocking(move || {
                    ai_store::set_key(ai_store::Kind::Stock, provider_id(provider), &draft)
                })
                .await
                .is_ok_and(|result| result.is_ok())
            },
            move |this, saved, _, cx| {
                this.pages.stock.busy = false;
                this.pages.stock.draft.update(cx, |input, cx| {
                    input.set_text("", cx);
                    input.set_disabled(false, cx);
                });
                if saved {
                    this.pages.stock.editing = None;
                }
                this.pages.stock.notice = Some((
                    provider,
                    if saved {
                        Notice::Saved
                    } else {
                        Notice::SaveFailed
                    },
                ));
            },
        );
        cx.notify();
    }

    fn remove_stock_key(
        &mut self,
        provider: Provider,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pages.stock.busy {
            return;
        }
        self.pages.stock.busy = true;
        self.pages.stock.notice = None;
        self.pages.stock.draft.update(cx, |input, cx| {
            input.set_text("", cx);
            input.set_disabled(true, cx);
        });
        self.spawn_tokio(
            window,
            cx,
            async move {
                tokio::task::spawn_blocking(move || {
                    ai_store::remove_checked(ai_store::Kind::Stock, provider_id(provider))
                })
                .await
                .is_ok_and(|result| result.is_ok())
            },
            move |this, removed, _, cx| {
                this.pages.stock.busy = false;
                this.pages
                    .stock
                    .draft
                    .update(cx, |input, cx| input.set_disabled(false, cx));
                if removed {
                    this.pages.stock.editing = None;
                }
                this.pages.stock.notice = Some((
                    provider,
                    if removed {
                        Notice::Removed
                    } else {
                        Notice::RemoveFailed
                    },
                ));
            },
        );
        cx.notify();
    }

    pub(crate) fn render_stock_services(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let rows = [Provider::Unsplash, Provider::Pixabay]
            .into_iter()
            .map(|provider| self.stock_service_row(provider, cx))
            .collect();
        vec![
            self.section(
                tr::settings::stock::title(),
                Some(tr::settings::stock::description()),
                None,
                vec![self.rows(rows).into_any_element()],
            )
            .into_any_element(),
        ]
    }

    fn stock_service_row(&self, provider: Provider, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let state = &self.pages.stock;
        let id = provider_id(provider);
        let saved_tail = ai_store::cached_key_tail(ai_store::Kind::Stock, id);
        let description = match provider {
            Provider::Unsplash => tr::settings::stock::unsplash_description(),
            Provider::Pixabay => tr::settings::stock::pixabay_description(),
        };
        let status = match &saved_tail {
            Some(Some(tail)) => format!("{} · {tail}", tr::settings::stock::configured()),
            Some(None) => tr::settings::stock::not_configured().to_string(),
            None => tr::settings::ai::key_unverified().to_string(),
        };
        let mut column = div().flex().flex_col().child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .p(px(14.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .text_size(px(13.))
                                .text_color(theme.settings_text())
                                .child(provider_name(provider)),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme.settings_muted())
                                .child(description),
                        ),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(12.))
                        .text_color(theme.settings_muted())
                        .child(status),
                )
                .child(self.button(
                    SharedString::from(format!("stock-edit-{id}")),
                    (ui::ButtonVariant::Gray, None),
                    if saved_tail.as_ref().is_some_and(Option::is_some) {
                        tr::settings::stock::edit()
                    } else {
                        tr::settings::stock::configure()
                    },
                    state.busy || state.editing == Some(provider),
                    cx,
                    move |this, window, cx| this.edit_stock_key(provider, window, cx),
                )),
        );
        if state.editing == Some(provider) {
            let placeholder = match provider {
                Provider::Unsplash => tr::settings::stock::unsplash_placeholder(),
                Provider::Pixabay => tr::settings::stock::pixabay_placeholder(),
            };
            state
                .draft
                .update(cx, |input, _| input.set_placeholder(placeholder));
            let mut buttons = div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(self.button(
                    "stock-save",
                    (ui::ButtonVariant::Primary, None),
                    tr::settings::stock::save(),
                    state.busy,
                    cx,
                    |this, window, cx| this.save_stock_key(window, cx),
                ))
                .child(self.button(
                    "stock-cancel",
                    (ui::ButtonVariant::Gray, None),
                    tr::common::cancel(),
                    state.busy,
                    cx,
                    |this, _, cx| this.cancel_stock_key(cx),
                ));
            if !matches!(saved_tail, Some(None)) {
                buttons = buttons.child(self.button(
                    "stock-remove",
                    (ui::ButtonVariant::Gray, None),
                    tr::settings::stock::remove(),
                    state.busy,
                    cx,
                    move |this, window, cx| this.remove_stock_key(provider, window, cx),
                ));
            }
            let key_url = match provider {
                Provider::Unsplash => "https://unsplash.com/oauth/applications",
                Provider::Pixabay => "https://pixabay.com/api/docs/",
            };
            buttons = buttons.child(div().flex_1()).child(self.button(
                "stock-get-key",
                (ui::ButtonVariant::Gray, None),
                tr::settings::stock::get_key(),
                state.busy,
                cx,
                move |_, _, cx| cx.open_url(key_url),
            ));
            column = column.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .px(px(14.))
                    .pb(px(14.))
                    .child(self.pages_input("stock-key", &state.draft))
                    .child(buttons),
            );
        }
        if let Some((notice_provider, notice)) = state.notice
            && notice_provider == provider
        {
            column = column.child(
                div()
                    .px(px(14.))
                    .pb(px(12.))
                    .text_size(px(12.))
                    .text_color(if notice.is_error() {
                        gpui::rgb(0xdc2626).into()
                    } else {
                        theme.settings_muted()
                    })
                    .child(notice.text()),
            );
        }
        column.into_any_element()
    }
}
