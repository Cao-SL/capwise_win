use gpui::{
    AnyElement, Context, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px, svg,
};

use crate::{
    editor_edits::Selection, editor_sidebar::SidebarTab, editor_timeline::TrackKind,
    editor_window::EditorWindow, i18n::tr, ui,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaptionTab {
    Content,
    Translation,
    Style,
}

impl EditorWindow {
    pub(crate) fn render_caption_navigation(&self, cx: &mut Context<Self>) -> AnyElement {
        let tabs = [
            (CaptionTab::Content, tr::editor::workbench::content()),
            (
                CaptionTab::Translation,
                tr::editor::workbench::translation(),
            ),
            (CaptionTab::Style, tr::editor::tabs::captions::style()),
        ];
        let has_captions = self
            .project
            .timeline
            .as_ref()
            .is_some_and(|timeline| !timeline.caption_segments.is_empty());
        let enabled = self
            .project
            .captions
            .as_ref()
            .is_some_and(|captions| captions.settings.enabled);
        div()
            .px(px(16.))
            .pt(px(12.))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(12.))
            .when(has_captions, |column| {
                column.child(
                    ui::Field::inline(&self.theme, tr::editor::workbench::show_captions()).value(
                        ui::Toggle::plain(&self.theme, "caption-workbench-visible", enabled)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.edit_project_global(
                                    "caption-enabled",
                                    window,
                                    cx,
                                    move |project| {
                                        project
                                            .captions
                                            .get_or_insert_with(Default::default)
                                            .settings
                                            .enabled = !enabled;
                                        true
                                    },
                                );
                            }))
                            .into_any_element(),
                    ),
                )
            })
            .child(
                ui::SegmentedControl::editor(
                    &self.theme,
                    "caption-workbench-tabs",
                    tabs.iter()
                        .map(|(tab, label)| {
                            ui::SegmentOption::new(
                                *label,
                                if has_captions {
                                    self.sidebar.caption_tab == *tab
                                } else {
                                    *tab == CaptionTab::Content
                                },
                            )
                            .disabled(!has_captions && *tab != CaptionTab::Content)
                        })
                        .collect(),
                )
                .stretch()
                .on_select(cx.listener(move |this, index: &usize, window, cx| {
                    if let Some((tab, _)) = tabs.get(*index) {
                        this.dismiss_sidebar_panels(cx);
                        this.sidebar.caption_tab = *tab;
                        this.sidebar.scroll.set_offset(gpui::point(px(0.), px(0.)));
                        this.focus_root(window, cx);
                        cx.notify();
                    }
                })),
            )
            .into_any_element()
    }

    pub(crate) fn render_caption_content_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let segments = self
            .project
            .timeline
            .as_ref()
            .map(|t| t.caption_segments.as_slice())
            .unwrap_or_default();
        let selected = self
            .selection
            .as_ref()
            .filter(|s| s.track == TrackKind::Caption)
            .and_then(|s| s.indices.first())
            .copied();
        let page = self
            .sidebar
            .caption_page
            .min(segments.len().saturating_sub(1) / CAPTIONS_PER_PAGE);
        let pages = segments.len().div_ceil(CAPTIONS_PER_PAGE).max(1);
        // A subtitle list the way CapCut and Descript show one: the start time
        // in a narrow column, the line beside it, the selected line tinted and
        // opened in place. No numbering, rules or instructions.
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(
                segments
                    .iter()
                    .enumerate()
                    .skip(page * CAPTIONS_PER_PAGE)
                    .take(CAPTIONS_PER_PAGE)
                    .map(|(index, segment)| {
                        let time = segment.start;
                        let active = selected == Some(index);
                        div()
                            .flex()
                            .flex_col()
                            .rounded(px(8.))
                            .when(active, |row| {
                                row.bg(Hsla::from(theme.editor.ctl)).pb(px(10.))
                            })
                            .child(
                                div()
                                    .id(SharedString::from(format!("caption-entry-{index}")))
                                    .flex()
                                    .items_start()
                                    .gap(px(10.))
                                    .px(px(8.))
                                    .py(px(7.))
                                    .rounded(px(8.))
                                    .cursor_pointer()
                                    .when(!active, |row| {
                                        row.hover(|s| s.bg(Hsla::from(theme.editor.ctl_hover)))
                                    })
                                    .child(
                                        div()
                                            .flex_none()
                                            .w(px(34.))
                                            .pt(px(1.))
                                            .text_size(px(11.))
                                            .line_height(px(19.))
                                            .text_color(Hsla::from(if active {
                                                theme.editor.accent
                                            } else {
                                                theme.editor.text_3
                                            }))
                                            .child(workbench_time(segment.start)),
                                    )
                                    .child(if active {
                                        div()
                                            .flex_1()
                                            .flex()
                                            .justify_end()
                                            .h(px(20.))
                                            .items_center()
                                            .child(
                                                svg()
                                                    .path("icons/chevron-up.svg")
                                                    .size(px(12.))
                                                    .text_color(Hsla::from(theme.editor.text_3)),
                                            )
                                    } else {
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_size(px(13.))
                                            .line_height(px(20.))
                                            .text_color(Hsla::from(theme.editor.text_1))
                                            .child(segment.text.clone())
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        // A second click on the open line
                                        // folds it again.
                                        if active {
                                            this.set_selection(None, cx);
                                            this.focus_root(window, cx);
                                            cx.notify();
                                            return;
                                        }
                                        this.set_selection(
                                            Some(Selection::single(TrackKind::Caption, index)),
                                            cx,
                                        );
                                        this.seek_to_time(time, cx);
                                        this.focus_root(window, cx);
                                        cx.notify();
                                    })),
                            )
                            .children(active.then(|| {
                                div().px(px(8.)).child(self.render_caption_panel(index, cx))
                            }))
                    }),
            )
            .when(pages > 1, |column| {
                column.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            ui::EditorButton::plain(&theme, "caption-page-prev")
                                .tinted(&theme)
                                .left_icon("icons/chevron-left.svg")
                                .label(tr::editor::workbench::previous())
                                .disabled(page == 0)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.set_selection(None, cx);
                                    this.sidebar.caption_page = page.saturating_sub(1);
                                    this.focus_root(window, cx);
                                    cx.notify();
                                })),
                        )
                        .child(format!("{} / {pages}", page + 1))
                        .child(
                            ui::EditorButton::plain(&theme, "caption-page-next")
                                .tinted(&theme)
                                .right_icon("icons/chevron-right.svg")
                                .label(tr::editor::workbench::next())
                                .disabled(page + 1 >= pages)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.set_selection(None, cx);
                                    this.sidebar.caption_page = (page + 1).min(pages - 1);
                                    this.focus_root(window, cx);
                                    cx.notify();
                                })),
                        ),
                )
            })
            .into_any_element()
    }

    pub(crate) fn open_text_video_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.transcript.caption_only = false;
        if !self.transcript.open {
            self.toggle_transcript(window, cx);
        }
        self.transcript.cleanup.open = false;
        window.focus(&self.transcript.focus, cx);
        cx.notify();
    }

    fn next_audio_lane(&self) -> u32 {
        (0..=self.tracks.audio)
            .find(|lane| {
                !self.project.timeline.as_ref().is_some_and(|timeline| {
                    timeline
                        .audio_segments
                        .iter()
                        .any(|segment| segment.track == *lane)
                })
            })
            .unwrap_or(self.tracks.audio)
    }

    pub(crate) fn render_audio_entrypoints(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let lane = self.next_audio_lane();
        let disabled = self.busy_reason().is_some() || self.audio_import_pending.is_some();
        div()
            .flex()
            .gap(px(8.))
            .p(px(16.))
            .flex_none()
            .child(
                ui::EditorButton::plain(&theme, "audio-workbench-record")
                    .filled(&theme)
                    .grow()
                    .height(px(34.))
                    .left_icon("icons/microphone.svg")
                    .label(tr::editor::voiceover::title())
                    .disabled(disabled)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_voiceover(lane, window, cx)
                    })),
            )
            .child(
                ui::EditorButton::plain(&theme, "audio-workbench-music")
                    .tinted(&theme)
                    .grow()
                    .height(px(34.))
                    .left_icon("icons/music.svg")
                    .label(tr::editor::window::track_menu::add_music())
                    .disabled(disabled)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_audio_picker(lane, window, cx)
                    })),
            )
            .into_any_element()
    }

    pub(crate) fn render_audio_back(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .px(px(16.))
            .pb(px(8.))
            .child(
                ui::EditorButton::plain(&self.theme, "audio-workbench-back")
                    .left_icon("icons/chevron-left.svg")
                    .label(tr::editor::workbench::back_audio())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.set_selection(None, cx);
                        this.dismiss_sidebar_panels(cx);
                        this.sidebar.tab = SidebarTab::Audio;
                        this.focus_root(window, cx);
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    pub(crate) fn render_added_audio(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let segments = self
            .project
            .timeline
            .as_ref()
            .map(|t| t.audio_segments.as_slice())
            .unwrap_or_default();
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .pt(px(12.))
            .border_t_1()
            .border_color(Hsla::from(theme.editor.line))
            .child(crate::editor_sidebar::section_title(
                &theme,
                tr::editor::workbench::added_audio(),
            ))
            .when(segments.is_empty(), |column| {
                column.child(
                    div()
                        .text_size(px(12.))
                        .text_color(Hsla::from(theme.editor.text_2))
                        .child(tr::editor::workbench::audio_empty()),
                )
            })
            .children(segments.iter().enumerate().map(|(index, segment)| {
                let name = segment
                    .name
                    .clone()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| tr::editor::panel::audio::fallback_name().to_string());
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .py(px(6.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(div().truncate().child(name))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(Hsla::from(theme.editor.text_2))
                                    .child(format!(
                                        "{} — {}",
                                        workbench_time(segment.start),
                                        workbench_time(segment.end)
                                    )),
                            ),
                    )
                    .child(
                        ui::EditorButton::plain(
                            &theme,
                            SharedString::from(format!("audio-entry-{index}")),
                        )
                        .tinted(&theme)
                        .label(tr::editor::workbench::adjust())
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.dismiss_sidebar_panels(cx);
                                this.set_selection(
                                    Some(Selection::single(TrackKind::Audio, index)),
                                    cx,
                                );
                                this.focus_root(window, cx);
                                cx.notify();
                            },
                        )),
                    )
            }))
            .into_any_element()
    }
}

/// Lines per page of the subtitle list.
pub(crate) const CAPTIONS_PER_PAGE: usize = 30;

/// `m:ss` -- enough to find a line; exact timing lives in the line's editor.
fn workbench_time(seconds: f64) -> String {
    let seconds = seconds.max(0.).floor() as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}
