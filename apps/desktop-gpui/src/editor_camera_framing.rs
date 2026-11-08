use cap_project::{CameraShape, XY};
use gpui::{
    AnyElement, AppContext, Context, Entity, FontWeight, Hsla, InteractiveElement, IntoElement,
    MouseButton, ParentElement, SharedString, StatefulInteractiveElement, Styled, Subscription,
    Window, div, px,
};

use crate::{
    editor_sidebar::SliderKey,
    editor_tabs::{CameraSlider, OpenMenu, SidebarMenu},
    editor_window::EditorWindow,
    i18n::tr,
    ui,
};

#[derive(Default)]
pub(crate) struct CameraFramingUi {
    pub crop_open: bool,
    dialog: Option<RatioDialog>,
}

struct RatioDialog {
    width: Entity<ui::TextInputState>,
    height: Entity<ui::TextInputState>,
    error: bool,
    _subscriptions: Vec<Subscription>,
}

impl CameraFramingUi {
    pub fn is_open(&self) -> bool {
        self.dialog.is_some()
    }
}

fn parse_ratio(width: &str, height: &str) -> Option<XY<u32>> {
    let x = width.trim().parse::<u32>().ok()?;
    let y = height.trim().parse::<u32>().ok()?;
    ((1..=100).contains(&x)
        && (1..=100).contains(&y)
        && (0.25..=4.0).contains(&(f64::from(x) / f64::from(y))))
    .then_some(XY::new(x, y))
}

impl EditorWindow {
    pub(crate) fn camera_ratio_label(&self) -> SharedString {
        let camera = &self.style_control_project().camera;
        let label = match camera.aspect_ratio {
            Some(XY { x: 16, y: 9 }) => tr::editor::tabs::camera::framing::wide().to_string(),
            Some(XY { x: 4, y: 3 }) => tr::editor::tabs::camera::framing::standard().to_string(),
            Some(XY { x: 3, y: 4 }) => tr::editor::tabs::camera::framing::portrait().to_string(),
            Some(XY { x: 9, y: 16 }) => tr::editor::tabs::camera::framing::vertical().to_string(),
            Some(ratio) => format!(
                "{} {}:{}",
                tr::editor::tabs::camera::framing::custom(),
                ratio.x,
                ratio.y
            ),
            None => match camera.shape {
                CameraShape::Source => tr::editor::tabs::camera::shape::source().to_string(),
                CameraShape::Square => tr::editor::tabs::camera::shape::square().to_string(),
            },
        };
        label.into()
    }

    pub(crate) fn camera_ratio_items(&self, kind: SidebarMenu) -> Vec<ui::MenuItem> {
        let camera = &self.style_control_project().camera;
        let is = |x, y| {
            camera
                .aspect_ratio
                .is_some_and(|ratio| ratio.x == x && ratio.y == y)
        };
        if kind == SidebarMenu::CameraMoreShape {
            vec![
                ui::MenuItem::new(tr::editor::tabs::camera::framing::back(), false),
                ui::MenuItem::new(tr::editor::tabs::camera::framing::portrait(), is(3, 4)),
                ui::MenuItem::new(tr::editor::tabs::camera::framing::vertical(), is(9, 16)),
                ui::MenuItem::new(tr::editor::tabs::camera::framing::custom(), false),
            ]
        } else {
            vec![
                ui::MenuItem::new(
                    tr::editor::tabs::camera::shape::source(),
                    camera.aspect_ratio.is_none() && matches!(camera.shape, CameraShape::Source),
                ),
                ui::MenuItem::new(tr::editor::tabs::camera::framing::wide(), is(16, 9)),
                ui::MenuItem::new(tr::editor::tabs::camera::framing::standard(), is(4, 3)),
                ui::MenuItem::new(
                    tr::editor::tabs::camera::shape::square(),
                    camera.aspect_ratio.is_none() && matches!(camera.shape, CameraShape::Square),
                ),
                ui::MenuItem::new(
                    tr::editor::tabs::camera::framing::more(),
                    camera.aspect_ratio.is_some() && !is(16, 9) && !is(4, 3),
                ),
            ]
        }
    }

    pub(crate) fn choose_camera_ratio(
        &mut self,
        kind: SidebarMenu,
        index: usize,
        anchor: Option<ui::MenuState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if (kind == SidebarMenu::CameraShape && index == 4)
            || (kind == SidebarMenu::CameraMoreShape && index == 0)
        {
            if let Some(state) = anchor {
                let next = if kind == SidebarMenu::CameraShape {
                    SidebarMenu::CameraMoreShape
                } else {
                    SidebarMenu::CameraShape
                };
                let items = self.camera_ratio_items(next);
                self.sidebar.menu = Some(OpenMenu {
                    kind: next,
                    state: ui::MenuState::new(state.origin, &items),
                });
            }
            cx.notify();
            return;
        }
        if kind == SidebarMenu::CameraMoreShape && index == 3 {
            self.open_camera_ratio_dialog(window, cx);
            return;
        }
        let (shape, ratio) = match (kind, index) {
            (SidebarMenu::CameraShape, 0) => (CameraShape::Source, None),
            (SidebarMenu::CameraShape, 1) => (CameraShape::Source, Some(XY::new(16, 9))),
            (SidebarMenu::CameraShape, 2) => (CameraShape::Source, Some(XY::new(4, 3))),
            (SidebarMenu::CameraShape, 3) => (CameraShape::Square, None),
            (SidebarMenu::CameraMoreShape, 1) => (CameraShape::Source, Some(XY::new(3, 4))),
            (SidebarMenu::CameraMoreShape, 2) => (CameraShape::Source, Some(XY::new(9, 16))),
            _ => return,
        };
        self.edit_project("camera-aspect-ratio", window, cx, move |project| {
            project.camera.shape = shape;
            project.camera.aspect_ratio = ratio;
            true
        });
    }

    pub(crate) fn render_camera_framing_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                ui::Field::inline(&theme, tr::editor::tabs::camera::shape_label())
                    .child(div().flex_1().min_w_0().child(self.menu_select_owned(
                        SidebarMenu::CameraShape,
                        "camera-aspect-ratio".into(),
                        self.camera_ratio_label(),
                        cx,
                    )))
                    .child(
                        ui::EditorButton::plain(&theme, "camera-adjust-framing")
                            .tinted(&theme)
                            .choice(&theme)
                            .left_icon("icons/crop.svg")
                            .icon_size(px(14.))
                            .text_size(px(12.))
                            .label(tr::editor::tabs::camera::framing::adjust())
                            .pressed(self.camera_framing.crop_open)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.camera_framing.crop_open = !this.camera_framing.crop_open;
                                cx.notify();
                            })),
                    ),
            )
            .children(self.camera_framing.crop_open.then(|| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(self.slider_field(
                        tr::editor::tabs::camera::framing::horizontal(),
                        SliderKey::Camera(CameraSlider::CropX),
                        "%",
                        cx,
                    ))
                    .child(self.slider_field(
                        tr::editor::tabs::camera::framing::vertical_position(),
                        SliderKey::Camera(CameraSlider::CropY),
                        "%",
                        cx,
                    ))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(Hsla::from(theme.editor.text_2))
                            .child(tr::editor::tabs::camera::framing::crop_hint()),
                    )
                    .child(
                        ui::Button::plain(
                            &theme,
                            "camera-framing-done",
                            ui::ButtonVariant::Gray,
                            ui::ButtonSize::Md,
                        )
                        .label(tr::editor::tabs::camera::framing::done())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.camera_framing.crop_open = false;
                            cx.notify();
                        })),
                    )
            }))
            .into_any_element()
    }

    fn open_camera_ratio_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ratio = self
            .style_control_project()
            .camera
            .aspect_ratio
            .unwrap_or(XY::new(4, 5));
        let inputs = [ratio.x, ratio.y].map(|value| {
            cx.new(|cx| {
                let mut input = ui::TextInputState::single_line(window, cx);
                input.set_text(value.to_string(), cx);
                input
            })
        });
        let subscriptions = inputs
            .iter()
            .map(|input| {
                cx.subscribe_in(
                    input,
                    window,
                    |this: &mut Self, _, event: &ui::TextInputEvent, window, cx| match event {
                        ui::TextInputEvent::Confirmed => this.apply_custom_camera_ratio(window, cx),
                        ui::TextInputEvent::Cancelled => this.close_camera_ratio_dialog(window, cx),
                        _ => cx.notify(),
                    },
                )
            })
            .collect();
        inputs[0].update(cx, |input, cx| input.focus_and_select_all(window, cx));
        self.camera_framing.dialog = Some(RatioDialog {
            width: inputs[0].clone(),
            height: inputs[1].clone(),
            error: false,
            _subscriptions: subscriptions,
        });
        cx.notify();
    }

    pub(crate) fn camera_ratio_dialog_tab(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dialog) = &self.camera_framing.dialog {
            let target = if dialog.width.read(cx).focus_handle().is_focused(window) {
                &dialog.height
            } else {
                &dialog.width
            };
            target.update(cx, |input, cx| input.focus_and_select_all(window, cx));
        }
    }

    pub(crate) fn close_camera_ratio_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.camera_framing.dialog = None;
        window.focus(&self.focus_handle_for_menu(), cx);
        cx.notify();
    }

    fn apply_custom_camera_ratio(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.camera_framing.dialog.as_mut() else {
            return;
        };
        let Some(ratio) = parse_ratio(dialog.width.read(cx).text(), dialog.height.read(cx).text())
        else {
            dialog.error = true;
            cx.notify();
            return;
        };
        self.close_camera_ratio_dialog(window, cx);
        self.edit_project("camera-custom-ratio", window, cx, move |project| {
            project.camera.shape = CameraShape::Source;
            project.camera.aspect_ratio = Some(ratio);
            true
        });
    }

    pub(crate) fn render_camera_ratio_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.camera_framing.dialog.as_ref()?;
        let theme = self.theme;
        Some(
            div()
                .absolute()
                .inset_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .id("camera-ratio-backdrop")
                        .absolute()
                        .inset_0()
                        .size_full()
                        .bg(gpui::hsla(0., 0., 0., 0.35))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.close_camera_ratio_dialog(window, cx)
                        })),
                )
                .child(
                    div()
                        .id("camera-ratio-dialog")
                        .occlude()
                        .w(px(360.))
                        .p(px(20.))
                        .flex()
                        .flex_col()
                        .gap(px(18.))
                        .rounded(px(12.))
                        .border_1()
                        .border_color(Hsla::from(theme.editor.line))
                        .bg(Hsla::from(theme.editor.card))
                        .shadow(theme.editor.pop_shadow())
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(tr::editor::tabs::camera::framing::custom()),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(12.))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap(px(6.))
                                        .child(tr::editor::tabs::camera::framing::width())
                                        .child(
                                            ui::TextInput::plain(
                                                &theme,
                                                "camera-ratio-width",
                                                &dialog.width,
                                            )
                                            .height(px(36.))
                                            .padding_x(px(10.))
                                            .radius(px(8.)),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap(px(6.))
                                        .child(tr::editor::tabs::camera::framing::height())
                                        .child(
                                            ui::TextInput::plain(
                                                &theme,
                                                "camera-ratio-height",
                                                &dialog.height,
                                            )
                                            .height(px(36.))
                                            .padding_x(px(10.))
                                            .radius(px(8.)),
                                        ),
                                ),
                        )
                        .children(dialog.error.then(|| {
                            div()
                                .text_size(px(12.))
                                .text_color(gpui::rgb(0xd92d20))
                                .child(tr::editor::tabs::camera::framing::invalid())
                        }))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(px(8.))
                                .child(
                                    ui::Button::plain(
                                        &theme,
                                        "camera-ratio-cancel",
                                        ui::ButtonVariant::Gray,
                                        ui::ButtonSize::Md,
                                    )
                                    .label(tr::common::cancel())
                                    .on_click(cx.listener(
                                        |this, _, window, cx| {
                                            this.close_camera_ratio_dialog(window, cx)
                                        },
                                    )),
                                )
                                .child(
                                    ui::Button::plain(
                                        &theme,
                                        "camera-ratio-apply",
                                        ui::ButtonVariant::Blue,
                                        ui::ButtonSize::Md,
                                    )
                                    .label(tr::editor::tabs::camera::framing::apply())
                                    .on_click(cx.listener(
                                        |this, _, window, cx| {
                                            this.apply_custom_camera_ratio(window, cx)
                                        },
                                    )),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_ratio_rejects_invalid_input_and_preserves_dimensions() {
        for (width, height) in [
            ("0", "1"),
            ("1", "0"),
            ("101", "50"),
            ("NaN", "3"),
            ("1", "100"),
            ("5.5", "4"),
        ] {
            assert!(parse_ratio(width, height).is_none());
        }
        let ratio = parse_ratio(" 4 ", "5").unwrap();
        assert_eq!((ratio.x, ratio.y), (4, 5));
        assert!(parse_ratio("1", "4").is_some());
        assert!(parse_ratio("4", "1").is_some());
    }
}
