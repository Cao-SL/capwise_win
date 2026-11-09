use std::path::PathBuf;

use cap_project::{OverlayImage, OverlayRange, OverlayTrackKind};
use gpui::{AnyElement, ClipboardEntry, CursorStyle};

use super::*;

#[derive(Default)]
pub(super) enum RedactionTool {
    #[default]
    Idle,
    Armed,
    Drawing {
        bounds: Bounds<Pixels>,
        anchor: Point<Pixels>,
        pointer: Point<Pixels>,
    },
}

impl RedactionTool {
    pub(super) fn active(&self) -> bool {
        !matches!(self, Self::Idle)
    }

    pub(super) fn drawing(&self) -> bool {
        matches!(self, Self::Drawing { .. })
    }
}

struct PendingPastedImage {
    path: Option<PathBuf>,
    imported: Option<crate::import::ImportedEditorImage>,
}

impl Drop for PendingPastedImage {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl EditorWindow {
    pub(super) fn overlay_shortcut_available(&self) -> bool {
        self.project_ready()
            && self.crop.is_none()
            && !self.has_open_menu()
            && self.transition_popover.is_none()
            && !self.shape_tool_menu
            && self.sticker_picker.is_none()
            && self.canvas_drag.is_none()
            && self.shape_drag.is_none()
            && self.lane_reorder.is_none()
    }

    pub(crate) fn redaction_tool_active(&self) -> bool {
        self.redaction_tool.active()
    }

    pub(crate) fn clear_redaction_tool(&mut self) {
        self.redaction_tool = RedactionTool::Idle;
    }

    pub(super) fn add_title_from_shortcut(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_redaction_tool();
        self.arm_shape_tool(None, cx);
        self.stop_playback(cx);
        let time = self.playhead_time();
        let total = self.total_duration();
        if !edits::ensure_timeline(&mut self.project, &self.clip_display_durations) {
            return;
        }
        let Some(range) = self.project.timeline.as_ref().and_then(|timeline| {
            OverlayRange::at_playhead(timeline, OverlayTrackKind::Text, time, 3., total).ok()
        }) else {
            self.show_notice(tr::editor::assets::move_playhead(), window, cx);
            return;
        };
        let title =
            crate::style_presets::new_placeholder_text_segment(range.start, range.end, range.track);
        self.history.pause();
        let mut index = 0;
        if self.edit(
            |timeline| {
                index = edits::insert_text_segment(timeline, title);
                true
            },
            window,
            cx,
        ) {
            self.assistant.open = false;
            self.sidebar.tab = crate::editor_sidebar::SidebarTab::Captions;
            self.set_selection(Some(Selection::single(TrackKind::Text, index)), cx);
            self.canvas_selection = Some(crate::editor_canvas::CanvasSelection::Text(index));
            self.view.preview_time = None;
            self.begin_canvas_text_edit(index, window, cx);
        }
        self.history.resume(&self.project);
    }

    pub(super) fn paste_editor_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(clipboard) = cx.read_from_clipboard() else {
            return false;
        };
        if self
            .shape_clipboard_token
            .as_ref()
            .is_some_and(|token| clipboard.metadata() == Some(token))
        {
            return self.paste_shapes(window, cx);
        }
        let image = clipboard.into_entries().find_map(|entry| match entry {
            ClipboardEntry::Image(image) => Some(Ok(image.bytes)),
            ClipboardEntry::ExternalPaths(paths) if paths.0.len() == 1 => paths
                .0
                .into_iter()
                .next()
                .filter(|path| crate::import::is_supported_overlay_image_path(path))
                .map(Err),
            _ => None,
        });
        let Some(image) = image else {
            return false;
        };
        if self.asset_import_busy() {
            self.show_notice(tr::editor::assets::busy(), window, cx);
            return true;
        }
        self.clear_redaction_tool();
        self.arm_shape_tool(None, cx);
        self.stop_playback(cx);
        let time = self.playhead_time();
        let total = self.total_duration();
        if !edits::ensure_timeline(&mut self.project, &self.clip_display_durations)
            || self.project.timeline.as_ref().is_none_or(|timeline| {
                OverlayRange::at_playhead(timeline, OverlayTrackKind::Image, time, 3., total)
                    .is_err()
            })
        {
            self.show_notice(tr::editor::assets::move_playhead(), window, cx);
            return true;
        }
        let snapshot = self
            .project
            .timeline
            .as_ref()
            .and_then(|timeline| serde_json::to_vec(timeline).ok());
        self.sidebar.picking_image = true;
        self.sidebar.image_import_error = None;
        let project = self.project_path.clone();
        let name = tr::editor::window::overlay::pasted_image();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let imported = match image {
                        Ok(bytes) => OverlayImage::from_clipboard(bytes, name.into())
                            .and_then(|image| crate::import::save_editor_image(&project, image)),
                        Err(path) => crate::import::import_editor_image(&project, &path),
                    }?;
                    Ok::<_, String>(PendingPastedImage {
                        path: Some(project.join(&imported.path)),
                        imported: Some(imported),
                    })
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.sidebar.picking_image = false;
                match result {
                    Ok(mut pending) => {
                        let current = this
                            .project
                            .timeline
                            .as_ref()
                            .and_then(|timeline| serde_json::to_vec(timeline).ok());
                        if snapshot.is_none() || snapshot != current {
                            this.show_notice(tr::editor::window::image::changed(), window, cx);
                            return;
                        }
                        if let Some(imported) = pending.imported.take()
                            && this.commit_image_import_inner(None, time, imported, window, cx)
                        {
                            pending.path = None;
                        }
                    }
                    Err(error) => {
                        this.sidebar.image_import_error = Some(error);
                        this.show_notice(tr::editor::window::overlay::paste_failed(), window, cx);
                    }
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
        true
    }

    pub(crate) fn toggle_redaction_tool(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.redaction_tool_active() {
            self.clear_redaction_tool();
            cx.notify();
            return;
        }
        if !self.overlay_shortcut_available() {
            return;
        }
        self.stop_playback(cx);
        if !self.canvas_overlay_visible() {
            return;
        }
        self.stop_canvas_text_edit(cx);
        self.end_field_edit(cx);
        self.view.preview_time = None;
        self.scene_preview_time = None;
        self.arm_shape_tool(None, cx);
        if !edits::ensure_timeline(&mut self.project, &self.clip_display_durations) {
            return;
        }
        if self.redaction_range().is_none() {
            self.show_notice(tr::editor::window::overlay::redact_range(), window, cx);
            return;
        }
        self.redaction_tool = RedactionTool::Armed;
        self.focus_root(window, cx);
        cx.notify();
    }

    fn redaction_range(&self) -> Option<OverlayRange> {
        OverlayRange::at_playhead(
            self.project.timeline.as_ref()?,
            OverlayTrackKind::Mask,
            self.playhead_time(),
            3.,
            self.total_duration(),
        )
        .ok()
    }

    pub(crate) fn render_redaction_surface(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let accent = Hsla::from(self.theme.blue_9);
        match self.redaction_tool {
            RedactionTool::Idle => None,
            RedactionTool::Armed => Some(
                div()
                    .id("redaction-surface")
                    .absolute()
                    .inset_0()
                    .occlude()
                    .cursor(CursorStyle::Crosshair)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            if let Some(bounds) = this.canvas_bounds()
                                && bounds.contains(&event.position)
                            {
                                cx.stop_propagation();
                                this.redaction_tool = RedactionTool::Drawing {
                                    bounds,
                                    anchor: event.position,
                                    pointer: event.position,
                                };
                                cx.notify();
                            }
                        }),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(8.))
                            .top(px(8.))
                            .px(px(8.))
                            .py(px(4.))
                            .rounded(px(5.))
                            .bg(Hsla::from(self.theme.editor.card_2))
                            .text_color(Hsla::from(self.theme.editor.text_1))
                            .text_size(px(12.))
                            .child(tr::editor::window::overlay::redact_draw()),
                    )
                    .into_any_element(),
            ),
            RedactionTool::Drawing {
                bounds,
                anchor,
                pointer,
            } => {
                let (x, y, w, h) = rect(bounds, anchor, pointer);
                Some(
                    div()
                        .absolute()
                        .left(px((x * f64::from(f32::from(bounds.size.width))) as f32))
                        .top(px((y * f64::from(f32::from(bounds.size.height))) as f32))
                        .w(px((w * f64::from(f32::from(bounds.size.width))) as f32))
                        .h(px((h * f64::from(f32::from(bounds.size.height))) as f32))
                        .border_1()
                        .border_color(accent)
                        .into_any_element(),
                )
            }
        }
    }

    pub(super) fn redaction_drag_layer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        ui::Slider::drag_layer(
            "canvas-redaction-drag",
            cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if let RedactionTool::Drawing { pointer, .. } = &mut this.redaction_tool {
                    *pointer = event.position;
                    cx.notify();
                }
            }),
            cx.listener(|this, event: &MouseUpEvent, window, cx| {
                this.finish_redaction(event.position, window, cx);
            }),
        )
        .cursor(CursorStyle::Crosshair)
    }

    fn finish_redaction(
        &mut self,
        pointer: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let RedactionTool::Drawing { bounds, anchor, .. } =
            std::mem::take(&mut self.redaction_tool)
        else {
            return;
        };
        let (x, y, width, height) = rect(bounds, anchor, pointer);
        if width * f64::from(f32::from(bounds.size.width)) < 3.
            || height * f64::from(f32::from(bounds.size.height)) < 3.
        {
            self.redaction_tool = RedactionTool::Armed;
            cx.notify();
            return;
        }
        let Some(mask) = self
            .redaction_range()
            .and_then(|range| range.redact(x, y, width, height, false).ok())
        else {
            self.show_notice(tr::editor::window::overlay::redact_range(), window, cx);
            return;
        };
        let mut index = 0;
        if self.edit(
            |timeline| {
                index = edits::insert_mask_segment(timeline, mask);
                true
            },
            window,
            cx,
        ) {
            self.set_selection(Some(Selection::single(TrackKind::Mask, index)), cx);
            self.canvas_selection = Some(crate::editor_canvas::CanvasSelection::Mask(index));
        }
        cx.notify();
    }
}

fn rect(bounds: Bounds<Pixels>, a: Point<Pixels>, b: Point<Pixels>) -> (f64, f64, f64, f64) {
    let normalized = |point: Point<Pixels>| {
        (
            f64::from((point.x - bounds.origin.x) / bounds.size.width).clamp(0., 1.),
            f64::from((point.y - bounds.origin.y) / bounds.size.height).clamp(0., 1.),
        )
    };
    let (ax, ay) = normalized(a);
    let (bx, by) = normalized(b);
    (ax.min(bx), ay.min(by), (ax - bx).abs(), (ay - by).abs())
}
