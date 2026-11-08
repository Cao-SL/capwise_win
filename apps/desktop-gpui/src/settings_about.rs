//! The About page: who made Capwise, where to get help, and what stays local.

use chrono::Datelike as _;
use gpui::{
    Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, img, prelude::FluentBuilder as _, px,
};

use crate::{i18n::tr, settings_window::SettingsWindow, ui};

const REPO: &str = "https://github.com/joeseesun/capwise";
const MONO_FONT: &str = "Menlo";

impl SettingsWindow {
    pub(crate) fn render_about(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        vec![
            self.render_about_identity(cx).into_any_element(),
            self.render_about_qr_cards().into_any_element(),
            self.render_about_footer().into_any_element(),
            self.render_about_advanced(cx).into_any_element(),
        ]
    }

    /// `showAdvancedSettings`: off by default, so the sidebar and the General
    /// page stay short until someone asks for more.
    fn render_about_advanced(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.rows(vec![
            self.setting_row(
                tr::settings::about::advanced(),
                Some(tr::settings::about::advanced_hint()),
                self.toggle(
                    "show-advanced-settings",
                    tr::settings::about::advanced(),
                    self.settings.show_advanced_settings,
                    cx,
                    |this, cx| {
                        let value = !this.settings.show_advanced_settings;
                        this.settings.show_advanced_settings = value;
                        this.write_bool(crate::store::SHOW_ADVANCED_SETTINGS_KEY, value, cx);
                    },
                )
                .into_any_element(),
            ),
        ])
    }

    fn render_about_identity(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let link_button = |this: &Self,
                           id: &'static str,
                           icon: &'static str,
                           label: &'static str,
                           url: String,
                           cx: &mut Context<Self>| {
            this.button(
                id,
                (ui::ButtonVariant::Gray, Some(icon)),
                label,
                false,
                cx,
                move |_, _, cx| cx.open_url(&url),
            )
        };

        div()
            .flex()
            .flex_col()
            .items_center()
            .pt(px(28.))
            .child(img("images/app-icon.png").size(px(76.)))
            .child(
                div()
                    .mt(px(14.))
                    .text_size(px(22.))
                    .line_height(px(28.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.settings_text())
                    .child(tr::brand::name()),
            )
            .child(
                div()
                    .mt(px(4.))
                    .font_family(MONO_FONT)
                    .text_size(px(11.))
                    .text_color(theme.settings_muted())
                    .child(format!("{} {}", tr::brand::alt_name(), env!("CARGO_PKG_VERSION"))),
            )
            .child(
                div()
                    .mt(px(12.))
                    .text_size(px(13.))
                    .text_color(theme.settings_muted())
                    .child(tr::settings::about::tagline()),
            )
            .child(
                div()
                    .mt(px(22.))
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .justify_center()
                    .gap(px(8.))
                    .child(link_button(
                        self,
                        "about-updates",
                        "icons/refresh-cw.svg",
                        tr::settings::about::check_updates(),
                        format!("{REPO}/releases"),
                        cx,
                    ))
                    .child(link_button(
                        self,
                        "about-issue",
                        "icons/message-circle.svg",
                        tr::settings::about::report_issue(),
                        format!("{REPO}/issues/new"),
                        cx,
                    ))
                    .child(link_button(
                        self,
                        "about-guide",
                        "icons/book-open.svg",
                        tr::settings::about::guide(),
                        format!("{REPO}#readme"),
                        cx,
                    ))
                    .child(link_button(
                        self,
                        "about-source",
                        "icons/code.svg",
                        tr::settings::about::source(),
                        REPO.to_string(),
                        cx,
                    )),
            )
    }

    fn render_about_qr_cards(&self) -> impl IntoElement {
        let theme = self.theme;
        let qr_card = |image: &'static str, title: &'static str, hint: &'static str| {
            div()
                .flex()
                .flex_1()
                .flex_col()
                .items_center()
                .gap(px(10.))
                .py(px(18.))
                .rounded(px(12.))
                .bg(theme.settings_card_bg())
                .child(
                    div()
                        .p(px(6.))
                        .rounded(px(8.))
                        .bg(gpui::white())
                        .child(img(image).size(px(112.))),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(theme.settings_text())
                                .child(title),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(theme.settings_muted())
                                .child(hint),
                        ),
                )
        };

        div()
            .flex()
            .flex_row()
            .gap(px(12.))
            .child(qr_card(
                "images/about-wechat-qr.jpg",
                tr::settings::about::wechat_title(),
                tr::settings::about::wechat_hint(),
            ))
            .child(qr_card(
                "images/about-reward-qr.png",
                tr::settings::about::reward_title(),
                tr::settings::about::reward_hint(),
            ))
    }

    fn render_about_footer(&self) -> impl IntoElement {
        let theme = self.theme;
        let links: [(&'static str, SharedString, &'static str); 5] = [
            ("about-site", "qiaomu.ai".into(), "https://qiaomu.ai"),
            (
                "about-blog",
                tr::settings::about::link_blog().into(),
                "https://blog.qiaomu.ai",
            ),
            (
                "about-picks",
                tr::settings::about::link_picks().into(),
                "https://tuijian.qiaomu.ai",
            ),
            ("about-x", "X".into(), "https://x.com/vista8"),
            ("about-github", "GitHub".into(), "https://github.com/joeseesun"),
        ];
        let last = links.len() - 1;

        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(10.))
            .pb(px(8.))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .justify_center()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(12.))
                    .children(links.into_iter().enumerate().map(|(index, (id, label, url))| {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .id(id)
                                    .cursor_pointer()
                                    .text_color(theme.settings_text())
                                    .hover(move |style| style.text_color(theme.settings_muted()))
                                    .child(label)
                                    .on_click(move |_, _, cx| cx.open_url(url)),
                            )
                            .when(index < last, |row| {
                                row.child(
                                    div().text_color(theme.settings_muted()).child("·"),
                                )
                            })
                    })),
            )
            .child(
                div()
                    .max_w(px(480.))
                    .text_center()
                    .text_size(px(11.))
                    .line_height(px(17.))
                    .text_color(theme.settings_muted())
                    .child(tr::settings::about::privacy()),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(theme.settings_muted())
                    .child(tr::settings::about::license(chrono::Local::now().year())),
            )
    }
}
