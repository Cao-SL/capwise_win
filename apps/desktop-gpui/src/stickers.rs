//! Offline stickers: about 1,200 Fluent Emoji Flat (MIT, Microsoft) with
//! Chinese names and tags, and about 1,900 Lucide line icons (ISC). The pack
//! is the one the Qiaomu cover-design plugin ships (`assets/stickers`),
//! baked into the binary, so inserting never needs a network.
//!
//! A sticker goes into a video as a picture layer: its SVG is rendered to a
//! PNG at the colour and line weight picked, then imported like any image.

use std::{io::Read, path::PathBuf, sync::LazyLock};

use serde::Deserialize;

const PACK: &[u8] = include_bytes!("../assets/stickers/pack.json.gz");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum StickerKind {
    Emoji,
    Icon,
}

#[derive(Debug)]
pub(crate) struct Sticker {
    pub id: String,
    pub kind: StickerKind,
    /// The name shown on hover: Chinese for emoji, the icon's words for
    /// icons.
    pub name: String,
    /// Lower-cased searchable text, Chinese and English.
    hay: String,
    body: String,
}

#[derive(Deserialize)]
struct Pack {
    emoji: Vec<(String, String, String, String, String)>,
    line: Vec<(String, String)>,
}

pub(crate) struct Stickers {
    pub emoji: Vec<Sticker>,
    pub icons: Vec<Sticker>,
}

pub(crate) static STICKERS: LazyLock<Stickers> = LazyLock::new(|| {
    let mut text = String::new();
    let pack: Option<Pack> = flate2::read::GzDecoder::new(PACK)
        .read_to_string(&mut text)
        .ok()
        .and_then(|_| serde_json::from_str(&text).ok());
    let Some(pack) = pack else {
        tracing::error!("sticker pack could not be read");
        return Stickers {
            emoji: Vec::new(),
            icons: Vec::new(),
        };
    };
    Stickers {
        emoji: pack
            .emoji
            .into_iter()
            .map(|(id, zh, tags, en, body)| Sticker {
                hay: format!("{zh} {tags} {en} {}", id.replace('-', " ")).to_lowercase(),
                name: zh,
                id,
                kind: StickerKind::Emoji,
                body,
            })
            .collect(),
        icons: pack
            .line
            .into_iter()
            .map(|(id, body)| {
                let words = id.replace('-', " ");
                Sticker {
                    hay: format!("{words} {}", icon_chinese(&id)).to_lowercase(),
                    name: words,
                    id,
                    kind: StickerKind::Icon,
                    body,
                }
            })
            .collect(),
    }
});

/// Chinese words for the icons people ask for most; icon names are English.
const ICON_ZH: &[(&str, &str)] = &[
    ("箭头", "arrow"),
    ("对勾", "check"),
    ("勾", "check"),
    ("叉", "x"),
    ("关闭", "x"),
    ("星", "star"),
    ("爱心", "heart"),
    ("心", "heart"),
    ("火", "flame"),
    ("灯泡", "lightbulb"),
    ("皇冠", "crown"),
    ("奖杯", "trophy"),
    ("书", "book"),
    ("电脑", "laptop"),
    ("手机", "smartphone"),
    ("相机", "camera"),
    ("时间", "clock"),
    ("时钟", "clock"),
    ("搜索", "search"),
    ("设置", "settings"),
    ("用户", "user"),
    ("邮件", "mail"),
    ("位置", "map-pin"),
    ("日历", "calendar"),
    ("文件", "file"),
    ("链接", "link"),
    ("下载", "download"),
    ("上传", "upload"),
    ("播放", "play"),
    ("音乐", "music"),
    ("图片", "image"),
    ("锁", "lock"),
    ("钱", "wallet"),
    ("购物", "shopping"),
    ("礼物", "gift"),
    ("云", "cloud"),
    ("太阳", "sun"),
    ("月亮", "moon"),
    ("闪电", "zap"),
    ("盾牌", "shield"),
    ("点赞", "thumbs-up"),
    ("评论", "message"),
    ("分享", "share"),
    ("书签", "bookmark"),
    ("标签", "tag"),
    ("旗帜", "flag"),
    ("警告", "alert"),
    ("注意", "alert"),
    ("提示", "info"),
    ("问号", "help"),
    ("加号", "plus"),
    ("减号", "minus"),
    ("圆", "circle"),
    ("方框", "square"),
    ("鼠标", "mouse"),
    ("点击", "pointer"),
    ("键盘", "keyboard"),
    ("代码", "code"),
    ("终端", "terminal"),
    ("数据", "database"),
    ("图表", "chart"),
    ("火箭", "rocket"),
    ("目标", "target"),
];

fn icon_chinese(id: &str) -> String {
    ICON_ZH
        .iter()
        .filter(|(_, word)| id.split('-').any(|part| part == *word) || id.starts_with(word))
        .map(|(zh, _)| *zh)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Emoji people reach for most in tutorials and covers, shown first before
/// anything is typed.
const PINNED_EMOJI: &[&str] = &[
    "fire",
    "red-heart",
    "thumbs-up",
    "check-mark-button",
    "cross-mark",
    "warning",
    "light-bulb",
    "sparkles",
    "rocket",
    "star",
    "hundred-points",
    "eyes",
    "party-popper",
    "high-voltage",
    "backhand-index-pointing-right",
    "backhand-index-pointing-down",
    "red-question-mark",
    "red-exclamation-mark",
    "bullseye",
    "brain",
    "gem-stone",
    "trophy",
    "crown",
    "money-bag",
    "laptop",
    "memo",
    "magnifying-glass-tilted-left",
    "bell",
    "megaphone",
    "clapping-hands",
    "flexed-biceps",
    "face-with-tears-of-joy",
    "smiling-face-with-heart-eyes",
    "thinking-face",
    "exploding-head",
    "grinning-face",
];

/// Icons for pointing at things on a screen recording, first.
const PINNED_ICONS: &[&str] = &[
    "arrow-right",
    "arrow-left",
    "arrow-up",
    "arrow-down",
    "arrow-up-right",
    "move-right",
    "corner-down-right",
    "mouse-pointer-2",
    "mouse-pointer-click",
    "pointer",
    "circle-check",
    "circle-x",
    "check",
    "x",
    "triangle-alert",
    "info",
    "circle-help",
    "lightbulb",
    "star",
    "heart",
    "flame",
    "zap",
    "sparkles",
    "thumbs-up",
    "search",
    "settings",
    "keyboard",
    "code",
    "terminal",
    "lock",
];

/// Results for a query, pinned favourites first when it is empty.
pub(crate) fn search(kind: StickerKind, query: &str) -> Vec<&'static Sticker> {
    let pool = match kind {
        StickerKind::Emoji => &STICKERS.emoji,
        StickerKind::Icon => &STICKERS.icons,
    };
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        let pinned = match kind {
            StickerKind::Emoji => PINNED_EMOJI,
            StickerKind::Icon => PINNED_ICONS,
        };
        let mut first: Vec<&Sticker> = pinned
            .iter()
            .filter_map(|id| pool.iter().find(|sticker| sticker.id == *id))
            .collect();
        first.extend(pool.iter().filter(|sticker| !pinned.contains(&sticker.id.as_str())));
        return first;
    }
    let dashed = query.replace(char::is_whitespace, "-");
    let mut scored: Vec<(u8, &Sticker)> = pool
        .iter()
        .filter_map(|sticker| {
            let score = if sticker.name.to_lowercase() == query || sticker.id == dashed {
                0
            } else if sticker.name.to_lowercase().contains(&query) {
                1
            } else if sticker.id.contains(&dashed) {
                2
            } else if sticker.hay.contains(&query) {
                3
            } else {
                return None;
            };
            Some((score, sticker))
        })
        .collect();
    scored.sort_by_key(|(score, _)| *score);
    scored.into_iter().map(|(_, sticker)| sticker).collect()
}

/// The sticker as a standalone SVG. Icons take the colour and line weight.
pub(crate) fn svg(sticker: &Sticker, color: &str, stroke: f32) -> String {
    match sticker.kind {
        StickerKind::Emoji => format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">{}</svg>"#,
            sticker.body
        ),
        StickerKind::Icon => format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">{}</svg>"#,
            sticker
                .body
                .replace("currentColor", color)
                .replace(r#"stroke-width="2""#, &format!(r#"stroke-width="{stroke}""#))
        ),
    }
}

/// Renders an SVG to straight-alpha RGBA, `size` pixels square.
pub(crate) fn rasterize(svg: &str, size: u32) -> Option<image::RgbaImage> {
    let tree = resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default()).ok()?;
    let view = tree.size();
    let scale = (size as f32 / view.width()).min(size as f32 / view.height());
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size)?;
    let transform = resvg::tiny_skia::Transform::from_translate(
        (size as f32 - view.width() * scale) / 2.,
        (size as f32 - view.height() * scale) / 2.,
    )
    .pre_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for pixel in pixmap.pixels() {
        let color = pixel.demultiply();
        rgba.extend_from_slice(&[color.red(), color.green(), color.blue(), color.alpha()]);
    }
    image::RgbaImage::from_raw(size, size, rgba)
}

/// A picker thumbnail in gpui's BGRA.
pub(crate) fn thumbnail(sticker: &Sticker, color: &str, stroke: f32) -> Option<std::sync::Arc<gpui::RenderImage>> {
    let mut image = rasterize(&svg(sticker, color, stroke), 64)?;
    for pixel in image.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Some(std::sync::Arc::new(gpui::RenderImage::new(smallvec::smallvec![
        image::Frame::new(image)
    ])))
}

/// Writes the sticker as a 512 px PNG for import and returns its path.
pub(crate) fn write_png(sticker: &Sticker, color: &str, stroke: f32) -> Result<PathBuf, String> {
    let image = rasterize(&svg(sticker, color, stroke), 512)
        .ok_or_else(|| "sticker could not be drawn".to_string())?;
    // The file name becomes the layer's name on the timeline, so it is the
    // sticker's own name; the colour picks the folder.
    let directory = std::env::temp_dir().join("capwise-stickers").join(match sticker.kind {
        StickerKind::Emoji => "emoji".to_owned(),
        StickerKind::Icon => color.trim_start_matches('#').to_ascii_lowercase(),
    });
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let name: String = sticker
        .name
        .chars()
        .map(|c| if c == '/' || c == ':' || c.is_control() { '-' } else { c })
        .collect();
    let path = directory.join(format!("{}.png", name.trim()));
    image
        .save_with_format(&path, image::ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pack_loads_and_searches_in_chinese_and_english() {
        assert!(STICKERS.emoji.len() > 1000);
        assert!(STICKERS.icons.len() > 1500);
        assert_eq!(search(StickerKind::Emoji, "")[0].id, "fire");
        assert!(search(StickerKind::Emoji, "火箭").iter().any(|s| s.id == "rocket"));
        assert!(search(StickerKind::Icon, "箭头").iter().any(|s| s.id.contains("arrow")));
        assert!(search(StickerKind::Icon, "arrow right").iter().any(|s| s.id == "arrow-right"));
    }

    #[test]
    fn icons_take_colour_and_weight_and_both_kinds_draw() {
        let icon = search(StickerKind::Icon, "check")[0];
        let drawn = svg(icon, "#ff0000", 3.);
        assert!(drawn.contains("#ff0000") && drawn.contains(r#"stroke-width="3""#));
        let image = rasterize(&drawn, 64).unwrap();
        assert!(image.pixels().any(|pixel| pixel.0[3] > 0 && pixel.0[0] > 200));
        let emoji = search(StickerKind::Emoji, "")[0];
        assert!(rasterize(&svg(emoji, "#000", 2.), 64).is_some());
    }
}
