use cap_project::{CameraShape, SceneMode, SceneSegment, SplitCameraFraming, XY};
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
    recording_defaults::StudioLayout,
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

const LAYOUT_CHOICES: [(StudioLayout, fn() -> &'static str, &str); 3] = [
    (
        StudioLayout::PictureInPicture,
        tr::main::studio_layout::pip,
        "icons/app-window.svg",
    ),
    (
        StudioLayout::ScreenLeft,
        tr::main::studio_layout::screen_left,
        "icons/panel-right.svg",
    ),
    (
        StudioLayout::CameraLeft,
        tr::main::studio_layout::camera_left,
        "icons/panel-left.svg",
    ),
];

const FRAMING_CHOICES: [(SplitCameraFraming, fn() -> &'static str); 3] = [
    (
        SplitCameraFraming::Fill,
        tr::main::studio_layout::framing_fill,
    ),
    (
        SplitCameraFraming::Relaxed,
        tr::main::studio_layout::framing_relaxed,
    ),
    (
        SplitCameraFraming::Fit,
        tr::main::studio_layout::framing_fit,
    ),
];

fn camera_scene_time(time: f64, end: f64) -> f64 {
    time.clamp(0., (end - 1e-9).max(0.))
}

fn camera_scene_at(scenes: &[SceneSegment], time: f64) -> Option<usize> {
    scenes
        .iter()
        .position(|scene| time >= scene.start && time < scene.end)
}

fn scene_camera_layout(scene: &SceneSegment) -> Option<StudioLayout> {
    match scene.mode {
        SceneMode::SplitScreen | SceneMode::Floating => Some(
            if scene.split_layout.is_some_and(|layout| layout.camera_first) {
                StudioLayout::CameraLeft
            } else {
                StudioLayout::ScreenLeft
            },
        ),
        SceneMode::Default => Some(StudioLayout::PictureInPicture),
        SceneMode::CameraOnly | SceneMode::HideCamera => None,
    }
}

impl EditorWindow {
    pub(crate) fn camera_scene_index_at(&self, playhead: f64) -> Option<usize> {
        let timeline = self.project.timeline.as_ref()?;
        let end = self.total_duration();
        camera_scene_at(&timeline.scene_segments, camera_scene_time(playhead, end))
    }

    fn camera_scene(&self) -> Option<&SceneSegment> {
        let index = self.camera_scene_index_at(self.playhead_time())?;
        self.project.timeline.as_ref()?.scene_segments.get(index)
    }

    pub(crate) fn camera_uses_split_layout(&self) -> bool {
        self.camera_scene()
            .is_some_and(|scene| matches!(scene.mode, SceneMode::SplitScreen | SceneMode::Floating))
    }

    pub(crate) fn render_camera_layout_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let scene = self.camera_scene();
        let active = scene.map_or(Some(StudioLayout::PictureInPicture), scene_camera_layout);
        let framing = scene
            .and_then(|scene| scene.split_layout)
            .unwrap_or_default()
            .camera_framing;
        let mut column = div().flex().flex_col().gap(px(12.)).child(
            ui::Field::stacked(&theme, tr::main::studio_layout::editor_layout()).child(
                div().flex().flex_wrap().gap(px(6.)).children(
                    LAYOUT_CHOICES.iter().copied().enumerate().map(
                        |(index, (layout, label, icon))| {
                            ui::EditorButton::plain(
                                &theme,
                                SharedString::from(format!("camera-layout-{index}")),
                            )
                            .choice(&theme)
                            .pressed(active == Some(layout))
                            .left_icon(icon)
                            .label(label())
                            .height(px(30.))
                            .text_size(px(12.))
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.set_camera_scene_layout(layout, window, cx)
                                },
                            ))
                        },
                    ),
                ),
            ),
        );
        if self.camera_uses_split_layout() {
            column = column.child(
                ui::Field::stacked(&theme, tr::main::studio_layout::framing()).child(
                    div().flex().flex_wrap().gap(px(6.)).children(
                        FRAMING_CHOICES.iter().copied().enumerate().map(
                            |(index, (choice, label))| {
                                ui::EditorButton::plain(
                                    &theme,
                                    SharedString::from(format!("split-camera-framing-{index}")),
                                )
                                .choice(&theme)
                                .pressed(framing == choice)
                                .label(label())
                                .height(px(30.))
                                .text_size(px(12.))
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        let time = this.playhead_time();
                                        this.edit_project_global_without_timeline(
                                            "split-camera-framing",
                                            window,
                                            cx,
                                            |project| {
                                                let Some(timeline) = project.timeline.as_mut()
                                                else {
                                                    return false;
                                                };
                                                let time =
                                                    camera_scene_time(time, timeline.duration());
                                                let Some(index) =
                                                    camera_scene_at(&timeline.scene_segments, time)
                                                else {
                                                    return false;
                                                };
                                                let segment = &mut timeline.scene_segments[index];
                                                if !matches!(
                                                    segment.mode,
                                                    SceneMode::SplitScreen | SceneMode::Floating
                                                ) {
                                                    return false;
                                                }
                                                let layout = segment
                                                    .split_layout
                                                    .get_or_insert_with(Default::default);
                                                if layout.camera_framing == choice {
                                                    return false;
                                                }
                                                layout.camera_framing = choice;
                                                true
                                            },
                                        );
                                    },
                                ))
                            },
                        ),
                    ),
                ),
            );
        }
        if self.project.timeline.as_ref().is_some_and(|timeline| {
            timeline
                .scene_segments
                .iter()
                .any(|scene| scene.start > 0. || scene.end + 1e-9 < timeline.duration())
        }) {
            column = column.child(
                div()
                    .text_size(px(11.))
                    .text_color(Hsla::from(theme.editor.text_2))
                    .child(tr::main::studio_layout::at_playhead()),
            );
        }
        column.into_any_element()
    }

    fn set_camera_scene_layout(
        &mut self,
        choice: StudioLayout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let time = self.playhead_time();
        self.edit_project_global("camera-scene-layout", window, cx, |project| {
            let Some(timeline) = project.timeline.as_mut() else {
                return false;
            };
            let end = timeline.duration();
            if !end.is_finite() || end <= 0. {
                return false;
            }
            let time = camera_scene_time(time, end);
            let scene = if let Some(index) = camera_scene_at(&timeline.scene_segments, time) {
                &mut timeline.scene_segments[index]
            } else {
                if choice == StudioLayout::PictureInPicture {
                    return false;
                }
                let index = timeline
                    .scene_segments
                    .partition_point(|scene| scene.end <= time);
                let start = index
                    .checked_sub(1)
                    .map_or(0., |prev| timeline.scene_segments[prev].end);
                let end = timeline
                    .scene_segments
                    .get(index)
                    .map_or(end, |next| next.start);
                timeline.scene_segments.insert(
                    index,
                    SceneSegment {
                        start,
                        end,
                        mode: SceneMode::Default,
                        split_layout: None,
                        transition_in: 0.,
                        transition_out: 0.,
                    },
                );
                &mut timeline.scene_segments[index]
            };

            if scene_camera_layout(scene) == Some(choice)
                && (choice == StudioLayout::PictureInPicture
                    || scene
                        .split_layout
                        .is_some_and(|layout| layout.force_horizontal))
            {
                return false;
            }
            if choice == StudioLayout::PictureInPicture {
                scene.mode = SceneMode::Default;
            } else {
                if !matches!(scene.mode, SceneMode::SplitScreen | SceneMode::Floating) {
                    scene.mode = SceneMode::Floating;
                }
                let layout = scene
                    .split_layout
                    .get_or_insert_with(|| cap_project::SplitLayout {
                        camera_framing: SplitCameraFraming::Relaxed,
                        ..Default::default()
                    });
                layout.force_horizontal = true;
                layout.camera_first = choice == StudioLayout::CameraLeft;
            }
            true
        });
    }

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
