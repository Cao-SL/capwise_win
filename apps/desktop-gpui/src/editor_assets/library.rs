//! The Library tab of the Assets panel: what this project already uses on
//! top, the shared library (`global_library`) below, both as thumbnail
//! grids. Click a tile to put it in the video; the star on a project tile
//! keeps it for later videos.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

use gpui::{
    AnyElement, Context, CursorStyle, FontWeight, Hsla, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, ParentElement, RenderImage, SharedString,
    StatefulInteractiveElement as _, Styled, StyledImage as _, Window, div, img,
    prelude::FluentBuilder, px, svg,
};

use crate::{
    asset_library::{self, AssetKind},
    editor_window::EditorWindow,
    global_library::{self, LibraryItem},
    i18n::tr,
    ui,
};

#[derive(Default)]
pub(crate) struct LibraryUi {
    items: Vec<LibraryItem>,
    loaded: bool,
    generation: u64,
    filter: Option<AssetKind>,
    /// First frames of videos, decoded off the main thread.
    frames: HashMap<PathBuf, Arc<RenderImage>>,
    pending: HashSet<PathBuf>,
    /// Pictures with see-through edges (stickers, cut-outs), drawn whole
    /// instead of cropped to fill the tile.
    see_through: HashMap<PathBuf, bool>,
    /// Project asset paths whose bytes are already in the library.
    saved: HashSet<String>,
    working: bool,
}

const TILE_HEIGHT: f32 = 76.;

fn has_see_through_edges(path: &std::path::Path) -> bool {
    use image::{ImageDecoder, ImageFormat, ImageReader};

    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
    {
        return true;
    }
    let Ok(reader) = ImageReader::open(path).and_then(ImageReader::with_guessed_format) else {
        return false;
    };
    if matches!(reader.format(), Some(ImageFormat::Jpeg) | None) {
        return false;
    }
    let Ok(decoder) = reader.into_decoder() else {
        return false;
    };
    if !decoder.color_type().has_alpha() {
        return false;
    }
    let Ok(image) = image::DynamicImage::from_decoder(decoder) else {
        return false;
    };
    let image = image.to_rgba8();
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return false;
    }
    let clear = |x: u32, y: u32| image.get_pixel(x, y)[3] < 250;
    (0..width).any(|x| clear(x, 0) || clear(x, height - 1))
        || (0..height).any(|y| clear(0, y) || clear(width - 1, y))
}

impl EditorWindow {
    /// Loads the library and works out which project assets are already in
    /// it. Cheap enough to run each time the tab opens.
    pub(crate) fn refresh_library(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.assets.library.generation = self.assets.library.generation.wrapping_add(1);
        let generation = self.assets.library.generation;
        let project = self.project_path.clone();
        let records: Vec<_> = self
            .assets
            .records
            .iter()
            .map(|record| record.path.clone())
            .collect();
        cx.spawn_in(window, async move |this, cx| {
            let (items, saved) = cx
                .background_executor()
                .spawn(async move {
                    let items = global_library::items();
                    let hashes: HashSet<_> = items.iter().map(|item| item.hash.clone()).collect();
                    let saved = records
                        .into_iter()
                        .filter(|path| {
                            asset_library::project_asset_path(&project, path).is_some_and(|file| {
                                !hashes.is_empty() && global_library::contains(&file)
                            })
                        })
                        .collect::<HashSet<_>>();
                    (items, saved)
                })
                .await;
            this.update(cx, |this, cx| {
                let library = &mut this.assets.library;
                if library.generation != generation {
                    return;
                }
                library.items = items;
                library.saved = saved;
                library.loaded = true;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// A video tile's first frame; asks for it the first time it is drawn.
    fn library_frame(
        &mut self,
        path: &PathBuf,
        cx: &mut Context<Self>,
    ) -> Option<Arc<RenderImage>> {
        if let Some(frame) = self.assets.library.frames.get(path) {
            return Some(frame.clone());
        }
        if self.assets.library.pending.insert(path.clone()) {
            let source = path.clone();
            cx.spawn(async move |this, cx| {
                let frame = cx
                    .background_executor()
                    .spawn({
                        let source = source.clone();
                        async move { crate::editor_clips::decode_thumbnail_frame(&source, 0.5) }
                    })
                    .await;
                this.update(cx, |this, cx| {
                    if let Ok(frame) = frame {
                        this.assets.library.frames.insert(source, frame);
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
        None
    }

    fn library_see_through(&mut self, path: &PathBuf, cx: &mut Context<Self>) -> bool {
        if let Some(see_through) = self.assets.library.see_through.get(path) {
            return *see_through;
        }
        self.assets.library.see_through.insert(path.clone(), false);
        let source = path.clone();
        cx.spawn(async move |this, cx| {
            let see_through = cx
                .background_executor()
                .spawn({
                    let source = source.clone();
                    async move { has_see_through_edges(&source) }
                })
                .await;
            if see_through {
                this.update(cx, |this, cx| {
                    this.assets.library.see_through.insert(source, true);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
        false
    }

    fn library_thumbnail(
        &mut self,
        kind: AssetKind,
        path: &PathBuf,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let frame = div()
            .relative()
            .w_full()
            .h(px(TILE_HEIGHT))
            .rounded(px(8.))
            .overflow_hidden()
            .bg(Hsla::from(theme.editor.ctl));
        let badge = |icon: &'static str| {
            div()
                .absolute()
                .left(px(5.))
                .bottom(px(5.))
                .size(px(18.))
                .rounded(px(5.))
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::hsla(0., 0., 0., 0.55))
                .child(svg().path(icon).size(px(11.)).text_color(gpui::white()))
        };
        match kind {
            AssetKind::Photo if self.library_see_through(path, cx) => frame
                .flex()
                .items_center()
                .justify_center()
                .child(
                    img(path.clone())
                        .w_full()
                        .h(px(TILE_HEIGHT - 20.))
                        .object_fit(gpui::ObjectFit::Contain),
                )
                .into_any_element(),
            AssetKind::Photo => frame
                .child(
                    img(path.clone())
                        .size_full()
                        .rounded(px(8.))
                        .object_fit(gpui::ObjectFit::Cover),
                )
                .into_any_element(),
            AssetKind::Video => {
                let still = self.library_frame(path, cx);
                frame
                    .when_some(still, |frame, still| {
                        frame.child(
                            img(still)
                                .size_full()
                                .rounded(px(8.))
                                .object_fit(gpui::ObjectFit::Cover),
                        )
                    })
                    .child(badge("icons/video.svg"))
                    .into_any_element()
            }
            AssetKind::Audio => frame
                .flex()
                .items_center()
                .justify_center()
                .bg(crate::editor_timeline::tile_bg(
                    &theme,
                    crate::editor_timeline::TrackKind::Audio.color(),
                ))
                .child(svg().path("icons/music.svg").size(px(22.)).text_color(
                    crate::editor_timeline::tile_fg(
                        &theme,
                        crate::editor_timeline::TrackKind::Audio.color(),
                    ),
                ))
                .into_any_element(),
        }
    }

    /// One tile: picture, name, and a corner action that shows on hover.
    #[allow(clippy::too_many_arguments)]
    fn library_tile(
        &mut self,
        id: SharedString,
        kind: AssetKind,
        path: PathBuf,
        name: String,
        credit: Option<(String, String)>,
        corner: Option<AnyElement>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        on_drag: Option<Box<dyn Fn(&mut Self, &MouseDownEvent, &mut Window, &mut Context<Self>)>>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let group = SharedString::from(format!("{id}-group"));
        let thumbnail = self.library_thumbnail(kind, &path, cx);
        let busy = self.asset_import_busy();
        div()
            .id(id.clone())
            .group(group.clone())
            .relative()
            .flex()
            .flex_col()
            .gap(px(5.))
            .min_w_0()
            .p(px(3.))
            .rounded(px(10.))
            .when(!busy, |tile| {
                tile.cursor_pointer()
                    .hover(|tile| tile.bg(Hsla::from(theme.editor.ctl_hover)))
            })
            .when(busy, |tile| tile.opacity(0.6))
            .child(thumbnail)
            .child(
                div()
                    .px(px(2.))
                    .truncate()
                    .text_size(px(11.))
                    .text_color(Hsla::from(theme.editor.text_2))
                    .child(name.clone()),
            )
            // Stock media keeps its credit in view, linked to the source page.
            .children(credit.map(|(text, url)| {
                div()
                    .id(SharedString::from(format!("{id}-credit")))
                    .px(px(2.))
                    .truncate()
                    .text_size(px(10.))
                    .text_color(Hsla::from(theme.editor.text_3))
                    .hover(|credit| credit.text_color(Hsla::from(theme.editor.text_1)))
                    .child(text)
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        cx.open_url(&url);
                    })
            }))
            .children(corner.map(|corner| {
                div()
                    .absolute()
                    .top(px(7.))
                    .right(px(7.))
                    .invisible()
                    .group_hover(group, |corner| corner.visible())
                    .child(corner)
            }))
            .tooltip(move |_, cx| ui::Tooltip::new(&theme, name.clone()).view(cx))
            .when_some(on_drag.filter(|_| !busy), |tile, on_drag| {
                tile.cursor(CursorStyle::OpenHand).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        on_drag(this, event, window, cx);
                    }),
                )
            })
            .when(!busy, |tile| {
                tile.on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
            })
            .into_any_element()
    }

    fn library_corner_button(
        &self,
        id: SharedString,
        icon: &'static str,
        tooltip: &'static str,
        filled: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        div()
            .id(id)
            .size(px(24.))
            .rounded(px(7.))
            .flex()
            .items_center()
            .justify_center()
            .bg(if filled {
                Hsla::from(theme.editor.accent)
            } else {
                gpui::hsla(0., 0., 0., 0.55)
            })
            .cursor_pointer()
            .child(svg().path(icon).size(px(12.)).text_color(gpui::white()))
            .tooltip(move |_, cx| ui::Tooltip::new(&theme, tooltip).view(cx))
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                on_click(this, window, cx);
            }))
            .into_any_element()
    }

    fn library_grid(tiles: Vec<AnyElement>) -> AnyElement {
        // Three columns: wide enough to recognise a picture in a 416 px
        // sidebar, narrow enough to see a dozen at once.
        let mut grid = div().flex().flex_col().gap(px(4.));
        let mut tiles = tiles.into_iter().peekable();
        while tiles.peek().is_some() {
            let mut row = div().flex().gap(px(4.));
            for _ in 0..3 {
                row = row.child(div().flex_1().min_w_0().children(tiles.next()));
            }
            grid = grid.child(row);
        }
        grid.into_any_element()
    }

    fn library_heading(&self, label: &'static str, count: usize) -> AnyElement {
        let theme = self.theme;
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(13.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(Hsla::from(theme.editor.text_1))
            .child(label)
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::NORMAL)
                    .text_color(Hsla::from(theme.editor.text_3))
                    .child(count.to_string()),
            )
            .into_any_element()
    }

    pub(crate) fn render_library(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let filter = self.assets.library.filter;
        let matches = move |kind: AssetKind| filter.is_none_or(|filter| filter == kind);

        // What this project uses.
        let records: Vec<_> = self
            .assets
            .records
            .iter()
            .enumerate()
            .filter(|(_, record)| matches(record.kind))
            .map(|(index, record)| (index, record.clone()))
            .collect();
        let mut project_tiles = Vec::new();
        for (index, record) in records {
            let Some(path) = asset_library::project_asset_path(&self.project_path, &record.path)
            else {
                continue;
            };
            let saved = self.assets.library.saved.contains(&record.path);
            let save_path = path.clone();
            let save_name = record.name.clone();
            let save_key = record.path.clone();
            let corner = self.library_corner_button(
                SharedString::from(format!("library-keep-{index}")),
                if saved {
                    "icons/check.svg"
                } else {
                    "icons/star.svg"
                },
                if saved {
                    tr::editor::assets::library_kept()
                } else {
                    tr::editor::assets::library_keep()
                },
                saved,
                move |this, window, cx| {
                    if !this.assets.library.saved.contains(&save_key) {
                        this.keep_in_library(
                            save_path.clone(),
                            save_name.clone(),
                            Some(save_key.clone()),
                            window,
                            cx,
                        );
                    }
                },
                cx,
            );
            let drag_record = record.clone();
            let draggable = super::asset_track(record.kind).is_some();
            project_tiles.push(self.library_tile(
                SharedString::from(format!("project-asset-{index}")),
                record.kind,
                path,
                record.name.clone(),
                record.source.as_ref().map(|source| {
                    (
                        format!(
                            "{} · {}",
                            super::provider_name(source.provider),
                            source.attribution.author
                        ),
                        source.attribution.source_url.clone(),
                    )
                }),
                Some(corner),
                move |this, window, cx| this.reuse_asset(index, window, cx),
                draggable.then(|| {
                    Box::new(
                        move |this: &mut Self,
                              event: &MouseDownEvent,
                              window: &mut Window,
                              cx: &mut Context<Self>| {
                            this.start_asset_drag(drag_record.clone(), event.position, window, cx);
                        },
                    )
                        as Box<dyn Fn(&mut Self, &MouseDownEvent, &mut Window, &mut Context<Self>)>
                }),
                cx,
            ));
        }

        // The shared library.
        let items: Vec<_> = self
            .assets
            .library
            .items
            .iter()
            .filter(|item| matches(item.kind))
            .cloned()
            .collect();
        let mut library_tiles = Vec::new();
        for item in items {
            let path = global_library::path_of(&item);
            let remove_id = item.id.clone();
            let corner = self.library_corner_button(
                SharedString::from(format!("library-remove-{}", item.id)),
                "icons/trash.svg",
                tr::editor::assets::library_remove(),
                false,
                move |this, window, cx| this.remove_from_library(remove_id.clone(), window, cx),
                cx,
            );
            let insert = item.clone();
            library_tiles.push(self.library_tile(
                SharedString::from(format!("library-item-{}", item.id)),
                item.kind,
                path,
                item.name.clone(),
                None,
                Some(corner),
                move |this, window, cx| this.insert_library_item(&insert, window, cx),
                None,
                cx,
            ));
        }

        let filters = [
            (None, tr::editor::assets::library_all()),
            (Some(AssetKind::Photo), tr::editor::assets::image()),
            (Some(AssetKind::Video), tr::editor::assets::video()),
            (Some(AssetKind::Audio), tr::editor::assets::library_audio()),
        ];
        let toolbar = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .children(
                filters
                    .into_iter()
                    .enumerate()
                    .map(|(index, (kind, label))| {
                        ui::EditorButton::plain(&theme, ("library-filter", index))
                            .label(label)
                            .text_size(px(12.))
                            .height(px(28.))
                            .radius(px(14.))
                            .padding_x(px(10.))
                            .pressed(filter == kind)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.assets.library.filter = kind;
                                cx.notify();
                            }))
                            .into_any_element()
                    }),
            )
            .child(div().flex_1())
            .child(
                ui::EditorButton::plain(&theme, "library-import")
                    .outlined(&theme)
                    .left_icon("icons/plus.svg")
                    .icon_size(px(13.))
                    .text_size(px(12.))
                    .height(px(28.))
                    .label(tr::editor::assets::library_import())
                    .disabled(self.assets.library.working)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.import_to_library(window, cx)),
                    ),
            );

        let status = self.asset_status(cx);
        let mut body = div()
            .id("library-scroll")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(12.))
            .pb(px(16.))
            .gap(px(18.));
        if !status.is_empty() {
            body = body.child(div().flex().flex_col().gap(px(8.)).children(status));
        }
        if !project_tiles.is_empty() {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(self.library_heading(
                        tr::editor::assets::library_project(),
                        project_tiles.len(),
                    ))
                    .child(Self::library_grid(project_tiles)),
            );
        }
        let library_count = library_tiles.len();
        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(self.library_heading(tr::editor::assets::library_shared(), library_count))
                .child(if library_tiles.is_empty() {
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(8.))
                        .py(px(28.))
                        .px(px(16.))
                        .rounded(px(10.))
                        .border_1()
                        .border_color(Hsla::from(theme.editor.line))
                        .child(
                            svg()
                                .path("icons/folder-open.svg")
                                .size(px(20.))
                                .text_color(Hsla::from(theme.editor.text_3)),
                        )
                        .child(
                            div()
                                .text_center()
                                .text_size(px(12.))
                                .line_height(px(18.))
                                .text_color(Hsla::from(theme.editor.text_2))
                                .child(tr::editor::assets::library_empty()),
                        )
                        .into_any_element()
                } else {
                    Self::library_grid(library_tiles)
                }),
        );
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap(px(12.))
            .child(div().px(px(12.)).child(toolbar))
            .child(body)
            .into_any_element()
    }

    /// Copies a file into the shared library; `project_key` marks the
    /// project tile it came from as kept.
    pub(crate) fn keep_in_library(
        &mut self,
        path: PathBuf,
        name: String,
        project_key: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.assets.library.working = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { global_library::add(&path, &name) })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.assets.library.working = false;
                match result {
                    Ok(_) => {
                        if let Some(key) = project_key {
                            this.assets.library.saved.insert(key);
                        }
                        this.refresh_library(window, cx);
                    }
                    Err(error) => this.assets.error = Some(error),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn remove_from_library(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { global_library::remove(&id) })
                .await;
            this.update_in(cx, |this, window, cx| {
                if let Err(error) = result {
                    this.assets.error = Some(error);
                }
                this.refresh_library(window, cx);
            })
            .ok();
        })
        .detach();
    }

    fn import_to_library(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        const EXTENSIONS: &[&str] = &[
            "png", "jpg", "jpeg", "webp", "gif", "bmp", "mp4", "mov", "m4v", "mp3", "m4a", "wav",
            "aac", "flac", "ogg", "aif", "aiff",
        ];
        cx.spawn_in(window, async move |this, cx| {
            #[cfg(target_os = "linux")]
            let source = crate::platform::open_file_panel_async(
                &[(tr::editor::assets::library_shared(), EXTENSIONS)],
                None,
                cx,
            )
            .await;
            #[cfg(not(target_os = "linux"))]
            let source = crate::platform::open_image_panel(EXTENSIONS);
            let Some(source) = source else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                let name = source
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or_default()
                    .to_owned();
                this.keep_in_library(source, name, None, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Puts a library item into this video the way the matching import does:
    /// pictures and sound at the playhead, video at the end of the clips.
    fn insert_library_item(
        &mut self,
        item: &LibraryItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.asset_import_busy() {
            return;
        }
        let path = global_library::path_of(item);
        if !path.is_file() {
            self.assets.error = Some(tr::editor::assets::missing_media().into());
            cx.notify();
            return;
        }
        match item.kind {
            AssetKind::Photo => {
                let Some(intent) = self.asset_intent() else {
                    return;
                };
                self.assets.cancel_import();
                let generation = self.assets.download_generation;
                self.assets.importing = true;
                self.import_asset_image(path, None, intent, generation, window, cx);
            }
            AssetKind::Video => self.import_mp4(path, window, cx),
            AssetKind::Audio => self.import_audio_path(path, window, cx),
        }
        cx.notify();
    }
}
