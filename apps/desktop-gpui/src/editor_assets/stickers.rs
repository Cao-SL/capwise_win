//! The sticker picker: emoji and line icons, searchable in Chinese, placed
//! at the playhead as a picture layer. Icons take a colour and a line
//! weight before they go in; afterwards they move and scale on the canvas
//! like any picture.

use std::{collections::HashMap, sync::Arc};

use gpui::{
    AnyElement, AppContext as _, Context, Entity, Hsla, InteractiveElement, IntoElement,
    MouseButton, ParentElement, RenderImage, SharedString, StatefulInteractiveElement as _,
    Styled, StyledImage as _, Window, div, img, prelude::FluentBuilder, px, rgb,
};

use crate::{
    editor_window::EditorWindow,
    i18n::tr,
    stickers::{self, StickerKind},
    ui,
};

/// Colours that read on both light and dark footage.
const COLORS: &[&str] = &[
    "#111111", "#FFFFFF", "#EF4444", "#F97316", "#FACC15", "#22C55E", "#3B82F6", "#A855F7",
];
const WEIGHTS: &[f32] = &[1.5, 2., 2.75];
/// Enough to scroll through without drawing all 1,900 icons at once.
const SHOWN: usize = 210;
const COLUMNS: usize = 7;

pub(crate) struct StickerPicker {
    kind: StickerKind,
    query: Entity<ui::TextInputState>,
    _observer: gpui::Subscription,
    color: &'static str,
    weight: f32,
    thumbs: HashMap<(String, &'static str, u32), Arc<RenderImage>>,
    drawing: bool,
    left: gpui::Pixels,
    top: gpui::Pixels,
}

impl EditorWindow {
    pub(crate) fn open_sticker_picker(
        &mut self,
        left: gpui::Pixels,
        top: gpui::Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let query = cx.new(|cx| ui::TextInputState::single_line(window, cx));
        query.update(cx, |input, _| {
            input.set_placeholder(tr::editor::stickers::search())
        });
        let observer = cx.observe(&query, |this, _, cx| {
            this.draw_sticker_thumbs(cx);
            cx.notify();
        });
        query.update(cx, |input, cx| input.focus_and_select_all(window, cx));
        self.sticker_picker = Some(StickerPicker {
            kind: StickerKind::Emoji,
            query,
            _observer: observer,
            color: COLORS[0],
            weight: WEIGHTS[1],
            thumbs: HashMap::new(),
            drawing: false,
            left,
            top,
        });
        self.draw_sticker_thumbs(cx);
        cx.notify();
    }

    fn sticker_results(&self, cx: &gpui::App) -> Vec<&'static stickers::Sticker> {
        let Some(picker) = &self.sticker_picker else {
            return Vec::new();
        };
        let query = picker.query.read(cx).text().to_string();
        stickers::search(picker.kind, &query)
            .into_iter()
            .take(SHOWN)
            .collect()
    }

    fn sticker_key(picker: &StickerPicker, id: &str) -> (String, &'static str, u32) {
        match picker.kind {
            StickerKind::Emoji => (id.to_owned(), "", 0),
            StickerKind::Icon => (id.to_owned(), picker.color, (picker.weight * 100.) as u32),
        }
    }

    /// Draws the thumbnails the grid is about to show, off the main thread.
    fn draw_sticker_thumbs(&mut self, cx: &mut Context<Self>) {
        let results = self.sticker_results(cx);
        let Some(picker) = &mut self.sticker_picker else {
            return;
        };
        if picker.drawing {
            return;
        }
        let missing: Vec<_> = results
            .into_iter()
            .filter(|sticker| !picker.thumbs.contains_key(&Self::sticker_key(picker, &sticker.id)))
            .map(|sticker| (sticker, Self::sticker_key(picker, &sticker.id)))
            .collect();
        if missing.is_empty() {
            return;
        }
        picker.drawing = true;
        let (color, weight) = (picker.color, picker.weight);
        cx.spawn(async move |this, cx| {
            let drawn = cx
                .background_executor()
                .spawn(async move {
                    missing
                        .into_iter()
                        .filter_map(|(sticker, key)| {
                            stickers::thumbnail(sticker, color, weight).map(|image| (key, image))
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(picker) = &mut this.sticker_picker {
                    picker.drawing = false;
                    picker.thumbs.extend(drawn);
                }
                // The query may have moved on while these were drawn.
                this.draw_sticker_thumbs(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Harness: switch the open picker to icons.
    pub(crate) fn auto_sticker_icons(&mut self, cx: &mut Context<Self>) {
        if let Some(picker) = &mut self.sticker_picker {
            picker.kind = StickerKind::Icon;
            picker.color = COLORS[2];
        }
        self.draw_sticker_thumbs(cx);
    }

    pub(crate) fn auto_insert_sticker(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let sticker = stickers::STICKERS
            .emoji
            .iter()
            .chain(stickers::STICKERS.icons.iter())
            .find(|sticker| sticker.id == id);
        if let Some(sticker) = sticker {
            if sticker.kind == StickerKind::Icon
                && let Some(picker) = &mut self.sticker_picker
            {
                picker.kind = StickerKind::Icon;
                picker.color = COLORS[2];
            }
            self.insert_sticker(sticker, window, cx);
        }
    }

    fn insert_sticker(
        &mut self,
        sticker: &'static stickers::Sticker,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(picker) = self.sticker_picker.take() else {
            return;
        };
        if self.asset_import_busy() {
            return;
        }
        let Some(intent) = self.asset_intent() else {
            return;
        };
        let (color, weight) = (picker.color, picker.weight);
        self.assets.cancel_import();
        self.assets.pending_motion = Some(stickers::default_motion(sticker));
        let generation = self.assets.download_generation;
        self.assets.importing = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let png = cx
                .background_executor()
                .spawn(async move { stickers::write_png(sticker, color, weight) })
                .await;
            this.update_in(cx, |this, window, cx| match png {
                Ok(path) => {
                    this.import_asset_image(path, None, intent, generation, window, cx);
                }
                Err(error) => {
                    this.assets.importing = false;
                    this.assets.error = Some(error);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn render_sticker_picker(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let picker = self.sticker_picker.as_ref()?;
        let theme = self.theme;
        let results = self.sticker_results(cx);
        let kinds = [
            (StickerKind::Emoji, tr::editor::stickers::emoji()),
            (StickerKind::Icon, tr::editor::stickers::icons()),
        ];
        let mut panel = div()
            .id("sticker-picker")
            .absolute()
            .left(picker.left)
            .top(picker.top)
            .w(px(372.))
            .occlude()
            .flex()
            .flex_col()
            .rounded(px(12.))
            .border_1()
            .border_color(Hsla::from(theme.editor.line))
            .bg(Hsla::from(theme.editor.card))
            .shadow(theme.editor.pop_shadow())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .p(px(10.))
                    .child(
                        div().flex_1().child(
                            ui::SegmentedControl::editor(
                                &theme,
                                "sticker-kind",
                                kinds
                                    .iter()
                                    .map(|(kind, label)| {
                                        ui::SegmentOption::new(*label, picker.kind == *kind)
                                    })
                                    .collect(),
                            )
                            .stretch()
                            .item_height(px(26.))
                            .on_select(cx.listener(move |this, index: &usize, _, cx| {
                                if let (Some(picker), Some((kind, _))) =
                                    (&mut this.sticker_picker, kinds.get(*index))
                                {
                                    picker.kind = *kind;
                                }
                                this.draw_sticker_thumbs(cx);
                                cx.notify();
                            })),
                        ),
                    )
                    .child(
                        ui::EditorButton::plain(&theme, "sticker-close")
                            .left_icon("icons/x.svg")
                            .icon_size(px(14.))
                            .tooltip(&theme, tr::common::close())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.sticker_picker = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div().px(px(10.)).child(
                    ui::TextInput::search(&theme, "sticker-query", &picker.query).height(px(32.)),
                ),
            );
        if picker.kind == StickerKind::Icon {
            panel = panel.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(10.))
                    .pt(px(10.))
                    .children(COLORS.iter().enumerate().map(|(index, color)| {
                        let color = *color;
                        let selected = picker.color == color;
                        let hex = u32::from_str_radix(color.trim_start_matches('#'), 16)
                            .unwrap_or(0);
                        div()
                            .id(("sticker-color", index))
                            .size(px(20.))
                            .rounded_full()
                            .bg(rgb(hex))
                            .border_1()
                            .border_color(Hsla::from(theme.editor.line_strong))
                            .cursor_pointer()
                            .when(selected, |swatch| {
                                swatch.border_2().border_color(Hsla::from(theme.editor.accent))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(picker) = &mut this.sticker_picker {
                                    picker.color = color;
                                }
                                this.draw_sticker_thumbs(cx);
                                cx.notify();
                            }))
                    }))
                    .child(div().flex_1())
                    .children(WEIGHTS.iter().enumerate().map(|(index, weight)| {
                        let weight = *weight;
                        ui::EditorButton::plain(&theme, ("sticker-weight", index))
                            .label(match index {
                                0 => tr::editor::stickers::thin(),
                                1 => tr::editor::stickers::regular(),
                                _ => tr::editor::stickers::bold(),
                            })
                            .text_size(px(11.))
                            .height(px(24.))
                            .padding_x(px(6.))
                            .pressed(picker.weight == weight)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(picker) = &mut this.sticker_picker {
                                    picker.weight = weight;
                                }
                                this.draw_sticker_thumbs(cx);
                                cx.notify();
                            }))
                    })),
            );
        }
        let mut grid = div()
            .id("sticker-grid")
            .h(px(300.))
            .overflow_y_scroll()
            .p(px(8.))
            .flex()
            .flex_col()
            .gap(px(2.));
        if results.is_empty() {
            grid = grid.child(
                div()
                    .py(px(40.))
                    .text_center()
                    .text_size(px(12.))
                    .text_color(Hsla::from(theme.editor.text_2))
                    .child(tr::editor::stickers::empty()),
            );
        }
        for (row, chunk) in results.chunks(COLUMNS).enumerate() {
            let mut line = div().flex().gap(px(2.));
            for (column, sticker) in chunk.iter().enumerate() {
                let sticker: &'static stickers::Sticker = sticker;
                let index = row * COLUMNS + column;
                let thumb = picker
                    .thumbs
                    .get(&Self::sticker_key(picker, &sticker.id))
                    .cloned();
                let name = SharedString::from(sticker.name.clone());
                line = line.child(
                    div()
                        .id(("sticker", index))
                        .size(px(48.))
                        .flex_none()
                        .rounded(px(8.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|cell| cell.bg(Hsla::from(theme.editor.ctl_hover)))
                        // White icons need a dark well to be seen.
                        .when(
                            sticker.kind == StickerKind::Icon && picker.color == "#FFFFFF",
                            |cell| cell.bg(gpui::hsla(0., 0., 0.25, 1.)),
                        )
                        .children(thumb.map(|thumb| {
                            img(thumb).size(px(30.)).object_fit(gpui::ObjectFit::Contain)
                        }))
                        .tooltip(move |_, cx| ui::Tooltip::new(&theme, name.clone()).view(cx))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.insert_sticker(sticker, window, cx)
                        })),
                );
            }
            grid = grid.child(line);
        }
        panel = panel.child(grid).child(
            div()
                .px(px(12.))
                .py(px(8.))
                .border_t_1()
                .border_color(Hsla::from(theme.editor.line))
                .text_size(px(11.))
                .text_color(Hsla::from(theme.editor.text_3))
                .child(tr::editor::stickers::hint()),
        );
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .child(
                    div()
                        .id("sticker-backdrop")
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .occlude()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.sticker_picker = None;
                            cx.notify();
                        })),
                )
                .child(panel)
                .into_any_element(),
        )
    }
}
