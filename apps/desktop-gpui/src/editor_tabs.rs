//! The five config-sidebar tabs beyond Background: Camera, Audio, Cursor,
//! Keyboard and Captions.
//!
//! | tab | source |
//! |---|---|
//! | Camera | `CameraConfig` (`ConfigSidebar.tsx:2978-3330`) |
//! | Audio | the inline `KTabs.Content value="audio"` (`:942-1016`) plus `SyncOffsetsConfig` (`:6081-6178`) |
//! | Cursor | the inline `KTabs.Content value="cursor"` (`:1017-1052`, controls at `:820-1000`) |
//! | Keyboard | `KeyboardTab.tsx` |
//! | Captions | `CaptionsTab.tsx` |
//!
//! Everything here writes a real `ProjectConfiguration` key path through
//! [`EditorWindow::edit_project`], which is the same fan-out a timeline edit or
//! a background slider takes.
//!
use std::{
    collections::HashSet,
    sync::{LazyLock, Mutex},
    time::Duration,
};

#[cfg(test)]
use cap_project::CaptionsData;
use cap_project::{
    BackgroundBlurConfig, BackgroundBlurMode, CameraXPosition, CameraYPosition, CaptionDisplayMode,
    CaptionSettings, CaptionTextStyle, CornerStyle, CursorAnimationStyle, CursorRippleConfig,
    KeyboardData, KeyboardSettings, ProjectConfiguration, RecordingMeta, ShadowConfiguration,
    StereoMode, transcription_ranges_cover, transcription_source_ranges,
};
use gpui::{
    AnyElement, Bounds, Context, EntityId, FontWeight, Hsla, InteractiveElement, IntoElement,
    ParentElement, Pixels, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder, px, relative, svg,
};
use serde_json::Value;

use crate::{
    editor_color::GradeTarget,
    editor_sidebar::{SliderKey, collapsible},
    editor_window::EditorWindow,
    i18n::tr,
    store,
    theme::Theme,
    transcription, ui,
};

// ---------------------------------------------------------------------------
// Catalogues
// ---------------------------------------------------------------------------

pub type Label = fn() -> &'static str;

pub const CAMERA_BLUR_MODES: &[(BackgroundBlurMode, Label)] = &[
    (BackgroundBlurMode::Off, tr::editor::tabs::camera::blur::off),
    (
        BackgroundBlurMode::Light,
        tr::editor::tabs::camera::blur::light,
    ),
    (
        BackgroundBlurMode::Heavy,
        tr::editor::tabs::camera::blur::heavy,
    ),
    #[cfg(target_os = "macos")]
    (
        BackgroundBlurMode::Remove,
        tr::editor::tabs::camera::blur::remove,
    ),
];

/// `CORNER_STYLE_OPTIONS` (`:399-402`).
pub const CORNER_STYLES: [(CornerStyle, Label); 2] = [
    (CornerStyle::Squircle, tr::editor::tabs::corner::squircle),
    (CornerStyle::Rounded, tr::editor::tabs::corner::rounded),
];

/// `STEREO_MODES` (`:375-379`).
pub const STEREO_MODES: [(StereoMode, Label); 3] = [
    (StereoMode::Stereo, tr::editor::tabs::audio::stereo::stereo),
    (
        StereoMode::MonoL,
        tr::editor::tabs::audio::stereo::mono_left,
    ),
    (
        StereoMode::MonoR,
        tr::editor::tabs::audio::stereo::mono_right,
    ),
];

/// `CURSOR_ANIMATION_STYLE_OPTIONS` (`:434-476`) -- the four presets plus the
/// `custom` value the physics sliders drop the picker into.
pub const CURSOR_STYLES: [(CursorAnimationStyle, Label, Label); 4] = [
    (
        CursorAnimationStyle::Slow,
        tr::editor::tabs::cursor::style::slow,
        tr::editor::tabs::cursor::style::slow_description,
    ),
    (
        CursorAnimationStyle::Smooth,
        tr::editor::tabs::cursor::style::smooth,
        tr::editor::tabs::cursor::style::smooth_description,
    ),
    (
        CursorAnimationStyle::Mellow,
        tr::editor::tabs::cursor::style::mellow,
        tr::editor::tabs::cursor::style::mellow_description,
    ),
    (
        CursorAnimationStyle::Fast,
        tr::editor::tabs::cursor::style::fast,
        tr::editor::tabs::cursor::style::fast_description,
    ),
];

/// `FONT_OPTIONS` (`text-style.tsx:12-16`).
pub const FONT_OPTIONS: [&str; 3] = ["System Sans-Serif", "System Serif", "System Monospace"];

const BUNDLED_CAPTION_FONTS: [(&str, Label); 11] = [
    ("Noto Sans SC", tr::editor::tabs::font::source_han_sans),
    ("Noto Serif SC", tr::editor::tabs::font::source_han_serif),
    ("宋骨黑体", tr::editor::tabs::font::songgu_hei),
    ("宋骨黑圆体", tr::editor::tabs::font::songgu_rounded),
    ("LXGW WenKai Lite", tr::editor::tabs::font::lxgw_wenkai),
    ("Smiley Sans", tr::editor::tabs::font::smiley_sans),
    ("Sarasa Gothic SC", tr::editor::tabs::font::sarasa_gothic),
    (
        "GenSenRounded2 TW",
        tr::editor::tabs::font::gensen_rounded_tw,
    ),
    ("Inter", tr::editor::tabs::font::inter),
    ("Montserrat", tr::editor::tabs::font::montserrat),
    ("Source Serif 4", tr::editor::tabs::font::source_serif),
];

fn chinese_font_families() -> &'static [cap_rendering::ChineseFontFamily] {
    static FAMILIES: std::sync::OnceLock<Vec<cap_rendering::ChineseFontFamily>> =
        std::sync::OnceLock::new();
    FAMILIES.get_or_init(cap_rendering::chinese_font_families)
}

pub fn caption_font_options() -> Vec<String> {
    BUNDLED_CAPTION_FONTS
        .iter()
        .filter(|(family, _)| cap_rendering::font_family_available(family))
        .map(|(family, _)| family.to_string())
        .collect()
}

pub(crate) fn is_bundled_font(font: &str) -> bool {
    BUNDLED_CAPTION_FONTS
        .iter()
        .any(|(family, _)| family.eq_ignore_ascii_case(font))
}

pub fn caption_font_available(font: &str) -> bool {
    FONT_OPTIONS.contains(&font)
        || cap_rendering::font_family_available(font)
        || (font == "LXGW WenKai Lite Medium"
            && cap_rendering::font_family_available("LXGW WenKai Lite"))
}

/// The project keeps the English family name; menus show it translated, and
/// a Chinese font by its Chinese name when the app speaks Chinese.
pub(crate) fn font_option_label(font: &str) -> SharedString {
    match font {
        "System Sans-Serif" => tr::editor::tabs::font::system_sans_serif().into(),
        "System Serif" => tr::editor::tabs::font::system_serif().into(),
        "System Monospace" => tr::editor::tabs::font::system_monospace().into(),
        other => {
            if let Some((_, label)) = BUNDLED_CAPTION_FONTS.iter().find(|(f, _)| *f == other) {
                return label().into();
            }
            let chinese_ui = crate::i18n::current() == crate::i18n::Locale::ZhCn;
            chinese_font_families()
                .iter()
                .find(|f| f.family == other && chinese_ui)
                .map_or_else(
                    || SharedString::from(other.to_string()),
                    |f| SharedString::from(f.chinese_name.clone()),
                )
        }
    }
}

/// A Chinese transcript shown in the generic sans gets 思源黑体 instead of
/// whatever the system falls back to.
pub(crate) fn chinese_caption_font(
    font: &str,
    segments: &[cap_project::CaptionSegment],
) -> Option<&'static str> {
    let chinese = segments
        .iter()
        .any(|segment| segment.text.chars().any(cap_project::is_unspaced_script));
    let bundled = BUNDLED_CAPTION_FONTS[0].0;
    (chinese
        && font == "System Sans-Serif"
        && chinese_font_families().iter().any(|f| f.family == bundled))
    .then_some(bundled)
}

/// `TEXT_WEIGHT_OPTIONS` (`text-style.tsx:36-40`).
pub const TEXT_WEIGHTS: [(u32, Label); 3] = [
    (400, tr::editor::tabs::weight::normal),
    (500, tr::editor::tabs::weight::medium),
    (700, tr::editor::tabs::weight::bold),
];

/// `KEYBOARD_POSITION_OPTIONS` (`text-style.tsx:28-35`).
pub const KEYBOARD_POSITIONS: [(&str, Label); 6] = [
    ("top-left", tr::editor::tabs::position::top_left),
    ("top-center", tr::editor::tabs::position::top_center),
    ("top-right", tr::editor::tabs::position::top_right),
    ("bottom-left", tr::editor::tabs::position::bottom_left),
    ("bottom-center", tr::editor::tabs::position::bottom_center),
    ("bottom-right", tr::editor::tabs::position::bottom_right),
];

pub const CAPTION_POSITIONS: [(&str, Label); 3] = [
    ("manual", tr::editor::tabs::position::manual),
    ("top-center", tr::editor::tabs::position::top_center),
    ("bottom-center", tr::editor::tabs::position::bottom_center),
];

/// `CAPTION_ANIMATION_OPTIONS` (`text-style.tsx:64-68`).
pub const CAPTION_ANIMATIONS: [(&str, Label); 3] = [
    ("none", tr::editor::tabs::captions::animation::none),
    ("bounce", tr::editor::tabs::captions::animation::bounce),
    ("pop", tr::editor::tabs::captions::animation::pop),
];

/// `CAPTION_HIGHLIGHT_STYLE_OPTIONS` (`text-style.tsx:70-73`).
pub const CAPTION_HIGHLIGHT_STYLES: [(&str, Label); 2] = [
    ("color", tr::editor::tabs::captions::highlight::color),
    ("pill", tr::editor::tabs::captions::highlight::pill),
];

/// One row of `MODEL_OPTIONS` (`CaptionsTab.tsx:72-78`).
pub struct CaptionModel {
    pub name: &'static str,
    pub label: Label,
    /// The engine identity the source shows in an info tooltip.
    pub model_name: &'static str,
    pub size: &'static str,
    pub description: Label,
}

/// `MODEL_OPTIONS` (`CaptionsTab.tsx:87-116`).
pub static CAPTION_MODELS: &[CaptionModel] = &[
    CaptionModel {
        name: crate::local_qwen::MODEL,
        label: tr::editor::tabs::captions::model::qwen,
        model_name: "Qwen3-ASR 0.6B (llama.cpp)",
        size: "~1.0GB",
        description: tr::editor::tabs::captions::model::qwen_description,
    },
    CaptionModel {
        name: "small",
        label: tr::editor::tabs::captions::model::small,
        model_name: "whisper.cpp small",
        size: "466MB",
        description: tr::editor::tabs::captions::model::small_description,
    },
    CaptionModel {
        name: "medium",
        label: tr::editor::tabs::captions::model::medium,
        model_name: "whisper.cpp medium",
        size: "1.5GB",
        description: tr::editor::tabs::captions::model::medium_description,
    },
    CaptionModel {
        name: "doubao",
        label: tr::ai::asr::doubao::name,
        model_name: "doubao",
        size: "",
        description: tr::ai::asr::doubao::note,
    },
    CaptionModel {
        name: "siliconflow",
        label: tr::ai::asr::siliconflow::name,
        model_name: "siliconflow",
        size: "",
        description: tr::ai::asr::siliconflow::note,
    },
    CaptionModel {
        name: "glm",
        label: tr::ai::asr::glm::name,
        model_name: "glm",
        size: "",
        description: tr::ai::asr::glm::note,
    },
    CaptionModel {
        name: "stepfun",
        label: tr::ai::asr::stepfun::name,
        model_name: "stepfun",
        size: "",
        description: tr::ai::asr::stepfun::note,
    },
    CaptionModel {
        name: "mimo",
        label: tr::ai::asr::mimo::name,
        model_name: "mimo",
        size: "",
        description: tr::ai::asr::mimo::note,
    },
    CaptionModel {
        name: "groq",
        label: tr::ai::asr::groq::name,
        model_name: "groq",
        size: "",
        description: tr::ai::asr::groq::note,
    },
    CaptionModel {
        name: "openai",
        label: tr::ai::asr::openai::name,
        model_name: "openai",
        size: "",
        description: tr::ai::asr::openai::note,
    },
    CaptionModel {
        name: "local-server",
        label: tr::ai::asr::local_server::name,
        model_name: "local-server",
        size: "",
        description: tr::ai::asr::local_server::note,
    },
    CaptionModel {
        name: "custom",
        label: tr::ai::asr::custom::name,
        model_name: "custom",
        size: "",
        description: tr::ai::asr::custom::note,
    },
];

pub fn local_caption_models() -> Vec<&'static CaptionModel> {
    local_caption_models_for(crate::local_qwen::supported())
}

fn local_caption_models_for(qwen_supported: bool) -> Vec<&'static CaptionModel> {
    CAPTION_MODELS
        .iter()
        .filter(|model| matches!(model.name, "qwen3-asr" | "small" | "medium"))
        .filter(|model| model.name != crate::local_qwen::MODEL || qwen_supported)
        .collect()
}

fn retired_model_replacement(saved: &str, qwen_supported: bool) -> Option<&'static str> {
    matches!(saved, "best" | "best-max").then_some(if qwen_supported {
        crate::local_qwen::MODEL
    } else {
        "small"
    })
}

pub(crate) fn migrate_caption_model_selection() {
    let section = store::store_section(transcription::GPUI_STORE_SECTION);
    if let Some(saved) = section
        .get(transcription::SELECTED_MODEL_KEY)
        .and_then(Value::as_str)
        && let Some(replacement) = retired_model_replacement(saved, crate::local_qwen::supported())
        && !store::set_store_setting(
            transcription::GPUI_STORE_SECTION,
            transcription::SELECTED_MODEL_KEY,
            Value::String(replacement.to_string()),
        )
    {
        tracing::warn!("The store refused the caption model migration");
    }
}

/// What the captions picker offers: the local models, then the cloud services
/// that are set up in Settings → AI models.
pub fn available_caption_models() -> Vec<&'static CaptionModel> {
    let mut models = local_caption_models();
    models.extend(CAPTION_MODELS.iter().filter(|model| {
        crate::asr_cloud::service(model.name).is_some_and(|service| service.ready())
    }));
    models
}

pub(crate) fn caption_model_label(name: &str) -> String {
    CAPTION_MODELS
        .iter()
        .find(|model| model.name == name)
        .map_or_else(|| name.to_string(), |model| (model.label)().to_string())
}

fn caption_model_for_display(
    preferred: &str,
    models: &[&'static CaptionModel],
    running: Option<&str>,
) -> &'static CaptionModel {
    running
        .and_then(|name| CAPTION_MODELS.iter().find(|model| model.name == name))
        .or_else(|| models.iter().copied().find(|model| model.name == preferred))
        .unwrap_or(models[0])
}

fn caption_model_summary(model: &CaptionModel) -> String {
    let size = crate::i18n::decimal(model.size);
    [(model.label)(), size.as_str(), (model.description)()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// `LANGUAGE_OPTIONS` (`CaptionsTab.tsx:118-148`), de-duplicated: the array
/// repeats `ar`/`hi`/`bn`/`ta` at the end, and a Kobalte listbox keyed on the
/// code renders each once.
pub const CAPTION_LANGUAGES: [(&str, Label); 26] = [
    ("auto", tr::editor::tabs::captions::language::auto),
    ("en", tr::editor::tabs::captions::language::en),
    ("es", tr::editor::tabs::captions::language::es),
    ("fr", tr::editor::tabs::captions::language::fr),
    ("de", tr::editor::tabs::captions::language::de),
    ("it", tr::editor::tabs::captions::language::it),
    ("pt", tr::editor::tabs::captions::language::pt),
    ("nl", tr::editor::tabs::captions::language::nl),
    ("pl", tr::editor::tabs::captions::language::pl),
    ("ru", tr::editor::tabs::captions::language::ru),
    ("sk", tr::editor::tabs::captions::language::sk),
    ("tr", tr::editor::tabs::captions::language::tr),
    ("ja", tr::editor::tabs::captions::language::ja),
    ("ko", tr::editor::tabs::captions::language::ko),
    ("zh", tr::editor::tabs::captions::language::zh),
    ("ar", tr::editor::tabs::captions::language::ar),
    ("hi", tr::editor::tabs::captions::language::hi),
    ("bn", tr::editor::tabs::captions::language::bn),
    ("ta", tr::editor::tabs::captions::language::ta),
    ("te", tr::editor::tabs::captions::language::te),
    ("mr", tr::editor::tabs::captions::language::mr),
    ("gu", tr::editor::tabs::captions::language::gu),
    ("pa", tr::editor::tabs::captions::language::pa),
    ("ur", tr::editor::tabs::captions::language::ur),
    ("fa", tr::editor::tabs::captions::language::fa),
    ("he", tr::editor::tabs::captions::language::he),
];

/// `CAPTION_STYLE_PRESETS` (`store/captions.ts:57-160`).
pub struct CaptionPreset {
    pub id: &'static str,
    pub label: Label,
    pub description: Label,
    pub font_weight: u32,
    pub size: u32,
    pub color: &'static str,
    pub background_color: &'static str,
    pub background_opacity: u32,
    pub outline: bool,
    pub outline_color: &'static str,
    pub highlight_color: &'static str,
    pub active_word_highlight: bool,
    pub highlight_style: &'static str,
    pub animation: &'static str,
    pub uppercase: bool,
    pub fade_duration: f32,
}

impl CaptionPreset {
    fn apply(&self, settings: &mut CaptionSettings) {
        settings.set_source_font_size(self.size);
        settings.preset = self.id.to_string();
        settings.font_weight = self.font_weight;
        settings.color = self.color.to_string();
        settings.background_color = self.background_color.to_string();
        settings.background_opacity = self.background_opacity;
        settings.outline = self.outline;
        settings.outline_color = self.outline_color.to_string();
        settings.highlight_color = self.highlight_color.to_string();
        settings.active_word_highlight = self.active_word_highlight;
        settings.highlight_style = self.highlight_style.to_string();
        settings.animation = self.animation.to_string();
        settings.uppercase = self.uppercase;
        settings.fade_duration = self.fade_duration;
        if self.id == "classic" {
            settings.shadow = false;
            settings.translation_gap = Some(10);
            settings.bottom_margin = Some(60);
        }
    }
}

pub static CAPTION_PRESETS: &[CaptionPreset] = &[
    CaptionPreset {
        id: "classic",
        label: tr::editor::tabs::captions::preset::classic,
        description: tr::editor::tabs::captions::preset::classic_description,
        font_weight: 700,
        size: 42,
        color: "#FFFFFF",
        background_color: "#000000",
        background_opacity: 70,
        outline: false,
        outline_color: "#000000",
        highlight_color: "#FFFFFF",
        active_word_highlight: false,
        highlight_style: "color",
        animation: "none",
        uppercase: false,
        fade_duration: 0.2,
    },
    CaptionPreset {
        id: "karaoke",
        label: tr::editor::tabs::captions::preset::karaoke,
        description: tr::editor::tabs::captions::preset::karaoke_description,
        font_weight: 700,
        size: 52,
        color: "#FFFFFF",
        background_color: "#000000",
        background_opacity: 35,
        outline: false,
        outline_color: "#000000",
        highlight_color: "#FFD400",
        active_word_highlight: true,
        highlight_style: "color",
        animation: "none",
        uppercase: false,
        fade_duration: 0.12,
    },
    CaptionPreset {
        id: "highlight",
        label: tr::editor::tabs::captions::preset::highlight,
        description: tr::editor::tabs::captions::preset::highlight_description,
        font_weight: 700,
        size: 54,
        color: "#FFFFFF",
        background_color: "#000000",
        background_opacity: 0,
        outline: true,
        outline_color: "#000000",
        highlight_color: "#7C3AED",
        active_word_highlight: true,
        highlight_style: "pill",
        animation: "bounce",
        uppercase: true,
        fade_duration: 0.12,
    },
    CaptionPreset {
        id: "pop",
        label: tr::editor::tabs::captions::preset::pop,
        description: tr::editor::tabs::captions::preset::pop_description,
        font_weight: 700,
        size: 56,
        color: "#FFFFFF",
        background_color: "#000000",
        background_opacity: 0,
        outline: true,
        outline_color: "#000000",
        highlight_color: "#FACC15",
        active_word_highlight: true,
        highlight_style: "color",
        animation: "pop",
        uppercase: true,
        fade_duration: 0.18,
    },
    CaptionPreset {
        id: "minimal",
        label: tr::editor::tabs::captions::preset::minimal,
        description: tr::editor::tabs::captions::preset::minimal_description,
        font_weight: 600,
        size: 46,
        color: "#FFFFFF",
        background_color: "#000000",
        background_opacity: 0,
        outline: true,
        outline_color: "#000000",
        highlight_color: "#FFFFFF",
        active_word_highlight: false,
        highlight_style: "color",
        animation: "none",
        uppercase: false,
        fade_duration: 0.25,
    },
];

struct QuickCaptionPreset {
    id: &'static str,
    label: Label,
    color: &'static str,
    outline_color: &'static str,
    outline_width: f32,
    background_color: &'static str,
    background_opacity: u32,
}

const QUICK_CAPTION_PRESETS: [QuickCaptionPreset; 4] = [
    QuickCaptionPreset {
        id: "white-outline",
        label: tr::editor::tabs::captions::preset::white_outline,
        color: "#FFFFFF",
        outline_color: "#000000",
        outline_width: 7.,
        background_color: "#000000",
        background_opacity: 0,
    },
    QuickCaptionPreset {
        id: "black-outline",
        label: tr::editor::tabs::captions::preset::black_outline,
        color: "#000000",
        outline_color: "#FFFFFF",
        outline_width: 8.,
        background_color: "#FFFFFF",
        background_opacity: 0,
    },
    QuickCaptionPreset {
        id: "dark-background",
        label: tr::editor::tabs::captions::preset::dark_background,
        color: "#FFFFFF",
        outline_color: "#000000",
        outline_width: 0.,
        background_color: "#000000",
        background_opacity: 70,
    },
    QuickCaptionPreset {
        id: "light-background",
        label: tr::editor::tabs::captions::preset::light_background,
        color: "#171717",
        outline_color: "#FFFFFF",
        outline_width: 0.,
        background_color: "#FFFFFF",
        background_opacity: 92,
    },
];

impl QuickCaptionPreset {
    fn apply(&self, settings: &mut CaptionSettings) {
        settings.preset = self.id.to_string();
        settings.font_weight = 700;
        settings.color = self.color.to_string();
        settings.background_color = self.background_color.to_string();
        settings.background_opacity = self.background_opacity;
        settings.outline = self.background_opacity == 0;
        settings.outline_color = self.outline_color.to_string();
        settings.outline_width = self.outline_width;
        settings.translation_style = Some(settings.resolved_translation_style());
        settings.translation_gap = Some(10);
        settings.bottom_margin = Some(60);
        settings.shadow = false;
        settings.shadow_color = self.outline_color.to_string();
        settings.shadow_opacity = 65;
        settings.shadow_size = 5.;
        settings.active_word_highlight = false;
        settings.animation = "none".to_string();
        settings.uppercase = false;
    }
}

struct CaptionPreviewLine {
    translation: bool,
    style: CaptionTextStyle,
    appearance: cap_project::CaptionAppearance,
    size: f32,
}

fn caption_preview_lines(settings: &CaptionSettings) -> Vec<CaptionPreviewLine> {
    let source = CaptionTextStyle {
        font: settings.font.clone(),
        size: settings.size.max(1),
    };
    let mut translation = settings.resolved_translation_style();
    translation.size = translation.size.max(1);
    let styles = match settings.display_mode {
        CaptionDisplayMode::Original => vec![(false, source)],
        CaptionDisplayMode::Translated => vec![(true, translation)],
        CaptionDisplayMode::Bilingual => vec![(false, source), (true, translation)],
    };
    let largest = styles
        .iter()
        .map(|(_, style)| style.size)
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    styles
        .into_iter()
        .map(|(translation, style)| CaptionPreviewLine {
            appearance: if translation {
                settings.resolved_translation_appearance()
            } else {
                settings.source_appearance()
            },
            translation,
            size: 18. * style.size as f32 / largest,
            style,
        })
        .collect()
}

struct CaptionPreviewLayout {
    scale: f32,
    width: f32,
    height: f32,
    padding: f32,
    radius: f32,
    gap: f32,
}

fn caption_preview_layout(
    settings: &CaptionSettings,
    lines: &[CaptionPreviewLine],
    widths: &[f32],
    width: f32,
    height: f32,
) -> CaptionPreviewLayout {
    let largest = lines
        .iter()
        .map(|line| line.style.size)
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    let preview_size = lines.iter().map(|line| line.size).fold(0., f32::max);
    let padding = preview_size * 0.5;
    let gap = if lines.len() > 1 {
        settings.translation_gap.unwrap_or(0).min(200) as f32 * preview_size / largest
    } else {
        0.
    };
    let natural_width = widths.iter().copied().fold(0., f32::max) + padding * 2.;
    let natural_height = lines.iter().map(|line| line.size * 1.2).sum::<f32>()
        + gap * lines.len().saturating_sub(1) as f32
        + padding * 2.;
    let scale = ((width - 16.).max(1.) / natural_width.max(1.))
        .min((height - 12.).max(1.) / natural_height.max(1.))
        .min(1.);
    CaptionPreviewLayout {
        scale,
        width: natural_width * scale,
        height: natural_height * scale,
        padding: padding * scale,
        radius: preview_size * 0.55 * scale,
        gap: gap * scale,
    }
}

/// `DEFAULT_CAMERA_SCALE_DURING_ZOOM` (`projectConfig.ts:27`).
pub const DEFAULT_CAMERA_SCALE_DURING_ZOOM: f32 = 0.7;

/// The camera tab's `ShadowSettings` fallback, and the background tab's -- the
/// UI's `{50, 18, 50}` rather than `ShadowConfiguration::default()`.
const UI_SHADOW_FALLBACK: ShadowConfiguration = ShadowConfiguration {
    size: 50.,
    opacity: 18.,
    blur: 50.,
};

// ---------------------------------------------------------------------------
// Sliders
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CameraSlider {
    Size,
    ZoomSize,
    Rounding,
    Shadow,
    ShadowSize,
    ShadowOpacity,
    ShadowBlur,
    SkinSmoothing,
    CropX,
    CropY,
}

impl CameraSlider {
    /// Whether this slider writes a field
    /// [`crate::editor_timeline::TimelineModel::build`] or
    /// [`crate::editor_timeline::TrackLanes::from_project`] reads. The match
    /// is exhaustive so a new slider has to make the choice explicitly.
    pub(crate) fn changes_timeline_model(self) -> bool {
        match self {
            Self::Size
            | Self::ZoomSize
            | Self::Rounding
            | Self::Shadow
            | Self::ShadowSize
            | Self::ShadowOpacity
            | Self::ShadowBlur
            | Self::SkinSmoothing
            | Self::CropX
            | Self::CropY => false,
        }
    }

    pub fn limits(self) -> (f32, f32, f32) {
        match self {
            // `minValue={20} maxValue={80} step={0.1}` (`:3204-3206`).
            Self::Size => (20., 80., 0.1),
            Self::ZoomSize => (10., 100., 0.1),
            _ => (0., 100., 0.1),
        }
    }

    pub fn read(self, project: &ProjectConfiguration) -> f32 {
        let camera = &project.camera;
        let shadow = camera.advanced_shadow.as_ref();
        match self {
            Self::Size => camera.size,
            Self::ZoomSize => camera.scale_during_zoom * 100.,
            Self::Rounding => camera.rounding,
            Self::Shadow => camera.shadow,
            Self::ShadowSize => shadow.map_or(UI_SHADOW_FALLBACK.size, |s| s.size),
            Self::ShadowOpacity => shadow.map_or(UI_SHADOW_FALLBACK.opacity, |s| s.opacity),
            Self::ShadowBlur => shadow.map_or(UI_SHADOW_FALLBACK.blur, |s| s.blur),
            Self::SkinSmoothing => camera.skin_smoothing * 100.,
            Self::CropX => camera.crop_position.map_or(50., |p| p.x as f32 * 100.),
            Self::CropY => camera.crop_position.map_or(50., |p| p.y as f32 * 100.),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AudioSlider {
    MicVolume,
    SystemVolume,
}

impl AudioSlider {
    /// Both audio sliders write levels [`TimelineModel::build`] reads; the
    /// lanes do not read audio levels. The match is exhaustive so a new
    /// slider has to make the choice explicitly.
    pub(crate) fn changes_timeline_model(self) -> bool {
        match self {
            Self::MicVolume | Self::SystemVolume => true,
        }
    }

    /// `minValue={-30} maxValue={10} step={0.1}` on both (`:787-791`,
    /// `:806-810`).
    pub fn limits(self) -> (f32, f32, f32) {
        (-30., 10., 0.1)
    }

    pub fn read(self, project: &ProjectConfiguration) -> f32 {
        match self {
            Self::MicVolume => project.audio.mic_volume_db,
            Self::SystemVolume => project.audio.system_volume_db,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CursorSlider {
    Size,
    Tilt,
    IdleDelay,
    Tension,
    Friction,
    Mass,
    RippleStrength,
    RippleSize,
    RippleDuration,
}

impl CursorSlider {
    /// Whether this slider writes a field
    /// [`crate::editor_timeline::TimelineModel::build`] or
    /// [`crate::editor_timeline::TrackLanes::from_project`] reads. The match
    /// is exhaustive so a new slider has to make the choice explicitly.
    pub(crate) fn changes_timeline_model(self) -> bool {
        match self {
            Self::Size
            | Self::Tilt
            | Self::IdleDelay
            | Self::Tension
            | Self::Friction
            | Self::Mass
            | Self::RippleStrength
            | Self::RippleSize
            | Self::RippleDuration => false,
        }
    }

    pub fn limits(self) -> (f32, f32, f32) {
        match self {
            Self::Size => (20., 300., 1.),
            Self::Tilt => (0., 1., 0.01),
            Self::IdleDelay => (0.5, 5., 0.1),
            Self::Tension => (1., 600., 1.),
            Self::Friction => (0., 200., 0.1),
            Self::Mass => (0.1, 15., 0.01),
            // Strength and size are stored as fractions and shown as
            // percentages; `CursorRippleConfig`'s own ranges, x100.
            Self::RippleStrength => (0., 100., 1.),
            Self::RippleSize => (25., 300., 1.),
            Self::RippleDuration => (
                CursorRippleConfig::DURATION_RANGE.0,
                CursorRippleConfig::DURATION_RANGE.1,
                0.05,
            ),
        }
    }

    pub fn read(self, project: &ProjectConfiguration) -> f32 {
        let cursor = &project.cursor;
        match self {
            Self::Size => cursor.size as f32,
            Self::Tilt => cursor.rotation_amount,
            Self::IdleDelay => cursor.hide_when_idle_delay,
            Self::Tension => cursor.tension,
            Self::Friction => cursor.friction,
            Self::Mass => cursor.mass,
            Self::RippleStrength => cursor.ripple.strength_clamped() * 100.,
            Self::RippleSize => cursor.ripple.size_clamped() * 100.,
            Self::RippleDuration => cursor.ripple.duration_clamped(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CaptionSlider {
    Size,
    TranslationSize,
    TranslationOutlineWidth,
    TranslationShadowOpacity,
    TranslationShadowSize,
    TranslationGap,
    BottomMargin,
    BackgroundOpacity,
    OutlineWidth,
    ShadowOpacity,
    ShadowSize,
    FadeDuration,
    ManualX,
    ManualY,
}

/// Where Manual placement starts for a preset no caption was drawn at yet, as
/// fractions of the frame (`captionPositionCenter`, `CaptionsTab.tsx:362-378`).
pub fn caption_position_center(position: &str) -> (f32, f32) {
    match position {
        "top-left" => (0.05, 0.08),
        "top-center" | "top" => (0.5, 0.08),
        "top-right" => (0.95, 0.08),
        "bottom-left" => (0.05, 0.85),
        "bottom-right" => (0.95, 0.85),
        _ => (0.5, 0.85),
    }
}

pub fn caption_manual_center(settings: &CaptionSettings) -> (f32, f32) {
    settings.manual_position.map_or_else(
        || caption_position_center(&settings.position),
        |xy| (xy.x, xy.y),
    )
}

pub fn caption_percent_text(fraction: f32) -> String {
    ui::format_number((f64::from(fraction) * 1000.).round() / 10.)
}

pub fn reset_caption_manual_center(settings: &mut CaptionSettings) {
    let (x, y) = crate::editor_canvas::CAPTION_DEFAULT_CENTER;
    settings.position = "manual".to_string();
    settings.manual_position = Some(cap_project::XY::new(x as f32, y as f32));
}

pub fn caption_at_default_center(settings: &CaptionSettings) -> bool {
    let (x, y) = crate::editor_canvas::CAPTION_DEFAULT_CENTER;
    caption_manual_center(settings) == (x as f32, y as f32)
}

/// `STYLE_PRESET_KEYS` knocks the style preset to "custom"
/// (`CaptionsTab.tsx:157-172, 326-336`). Turning captions on, the export
/// toggle, placement and showing filler words are not style, so they keep
/// the preset.
fn caption_toggle(
    theme: &Theme,
    label: &'static str,
    reason: &'static str,
    on: bool,
    cx: &mut Context<EditorWindow>,
    change: fn(&mut CaptionSettings),
) -> impl IntoElement {
    ui::Subfield::plain(theme, label).child(ui::Toggle::plain(theme, reason, on).on_click(
        cx.listener(move |this, _, window, cx| {
            this.set_caption_setting(reason, window, cx, change);
        }),
    ))
}

fn caption_reason_customises_style(reason: &str) -> bool {
    !matches!(
        reason,
        "caption-enabled" | "caption-export" | "caption-position" | "caption-placement"
    )
}

pub fn set_caption_manual_center(settings: &mut CaptionSettings, x: Option<f32>, y: Option<f32>) {
    let (current_x, current_y) = caption_manual_center(settings);
    let fraction = |value: f32| (value / 100.).clamp(0., 1.);
    settings.position = "manual".to_string();
    settings.manual_position = Some(cap_project::XY::new(
        x.map_or(current_x, fraction),
        y.map_or(current_y, fraction),
    ));
}

/// Switching the caption position. Entering Manual from a preset seeds the
/// manual centre from that preset (`updateCaptionPosition`,
/// `CaptionsTab.tsx:380-394`); without it Manual drew bottom-centre and had
/// nothing to adjust. `on_screen` is the caption on the preview frame and
/// its centre; the renderer's last drawn caption is the fallback.
pub fn set_caption_position(
    settings: &mut CaptionSettings,
    position: &str,
    on_screen: Option<(cap_rendering::CaptionFrameBox, (f32, f32))>,
) {
    set_caption_position_from(settings, position, |preset| {
        on_screen
            .filter(|(caption, _)| caption.is_at(preset))
            .map(|(_, center)| center)
            .or_else(|| cap_rendering::last_caption_center(preset))
    });
}

/// [`set_caption_position`] with the drawn caption centre supplied: left and
/// right presets align the box's edge to a margin, so only the drawn box
/// says where its centre is.
fn set_caption_position_from(
    settings: &mut CaptionSettings,
    position: &str,
    drawn_center: impl FnOnce(&str) -> Option<(f32, f32)>,
) {
    if position == "manual" && settings.position != "manual" {
        let (x, y) = drawn_center(&settings.position)
            .unwrap_or_else(|| caption_position_center(&settings.position));
        settings.manual_position = Some(cap_project::XY::new(x, y));
    }
    settings.position = position.to_string();
}

impl CaptionSlider {
    /// Whether this slider writes a field
    /// [`crate::editor_timeline::TimelineModel::build`] or
    /// [`crate::editor_timeline::TrackLanes::from_project`] reads. The match
    /// is exhaustive so a new slider has to make the choice explicitly.
    pub(crate) fn changes_timeline_model(self) -> bool {
        match self {
            Self::Size
            | Self::TranslationSize
            | Self::TranslationOutlineWidth
            | Self::TranslationShadowOpacity
            | Self::TranslationShadowSize
            | Self::TranslationGap
            | Self::BottomMargin
            | Self::BackgroundOpacity
            | Self::OutlineWidth
            | Self::ShadowOpacity
            | Self::ShadowSize
            | Self::FadeDuration => false,
            Self::ManualX | Self::ManualY => false,
        }
    }

    pub fn limits(self) -> (f32, f32, f32) {
        match self {
            Self::Size | Self::TranslationSize => (12., 100., 1.),
            Self::TranslationGap => (0., 200., 1.),
            Self::BottomMargin => (0., 400., 1.),
            Self::BackgroundOpacity => (0., 100., 1.),
            Self::OutlineWidth | Self::TranslationOutlineWidth => (0., 20., 0.5),
            Self::ShadowOpacity | Self::TranslationShadowOpacity => (0., 100., 1.),
            Self::ShadowSize | Self::TranslationShadowSize => (0., 30., 0.5),
            // `value={[getSetting("fadeDuration") * 100]}` over 0..50.
            Self::FadeDuration => (0., 50., 1.),
            Self::ManualX | Self::ManualY => (0., 100., 1.),
        }
    }

    pub fn read(self, project: &ProjectConfiguration) -> f32 {
        let settings = caption_settings(project);
        match self {
            Self::Size => settings.size as f32,
            Self::TranslationSize => settings.resolved_translation_style().size as f32,
            Self::TranslationOutlineWidth => settings
                .resolved_translation_appearance()
                .normalized_outline_width(),
            Self::TranslationShadowOpacity => settings
                .resolved_translation_appearance()
                .shadow_opacity
                .min(100) as f32,
            Self::TranslationShadowSize => settings
                .resolved_translation_appearance()
                .normalized_shadow_size(),
            Self::TranslationGap => settings.translation_gap.unwrap_or(0).min(200) as f32,
            Self::BottomMargin => settings.bottom_margin.unwrap_or(60).min(400) as f32,
            Self::BackgroundOpacity => settings.background_opacity as f32,
            Self::OutlineWidth => settings.normalized_outline_width(),
            Self::ShadowOpacity => settings.shadow_opacity.min(100) as f32,
            Self::ShadowSize => settings.normalized_shadow_size(),
            Self::FadeDuration => settings.fade_duration * 100.,
            Self::ManualX => caption_manual_center(&settings).0 * 100.,
            Self::ManualY => caption_manual_center(&settings).1 * 100.,
        }
    }

    fn write(self, settings: &mut CaptionSettings, value: f32) {
        match self {
            Self::Size => settings.set_source_font_size(value.round() as u32),
            Self::TranslationSize => settings.set_translation_font_size(value.round() as u32),
            Self::TranslationGap => {
                settings.translation_gap = Some(value.clamp(0., 200.).round() as u32)
            }
            Self::BottomMargin => {
                settings.bottom_margin = Some(value.clamp(0., 400.).round() as u32)
            }
            Self::BackgroundOpacity => settings.background_opacity = value.round() as u32,
            Self::OutlineWidth => {
                settings.edit_source_appearance(|a| a.outline_width = value.clamp(0., 20.))
            }
            Self::TranslationOutlineWidth => {
                settings.edit_translation_appearance(|a| a.outline_width = value.clamp(0., 20.))
            }
            Self::ShadowOpacity => settings.edit_source_appearance(|a| {
                a.shadow_opacity = value.clamp(0., 100.).round() as u32
            }),
            Self::TranslationShadowOpacity => settings.edit_translation_appearance(|a| {
                a.shadow_opacity = value.clamp(0., 100.).round() as u32
            }),
            Self::ShadowSize => {
                settings.edit_source_appearance(|a| a.shadow_size = value.clamp(0., 30.))
            }
            Self::TranslationShadowSize => {
                settings.edit_translation_appearance(|a| a.shadow_size = value.clamp(0., 30.))
            }
            Self::FadeDuration => settings.fade_duration = value / 100.,
            Self::ManualX => set_caption_manual_center(settings, Some(value), None),
            Self::ManualY => set_caption_manual_center(settings, None, Some(value)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyboardSlider {
    Size,
    BackgroundOpacity,
    FadeDuration,
    LingerDuration,
    GroupingThreshold,
}

impl KeyboardSlider {
    /// Whether this slider writes a field
    /// [`crate::editor_timeline::TimelineModel::build`] or
    /// [`crate::editor_timeline::TrackLanes::from_project`] reads. The match
    /// is exhaustive so a new slider has to make the choice explicitly.
    pub(crate) fn changes_timeline_model(self) -> bool {
        match self {
            Self::Size
            | Self::BackgroundOpacity
            | Self::FadeDuration
            | Self::LingerDuration
            | Self::GroupingThreshold => false,
        }
    }

    pub fn limits(self) -> (f32, f32, f32) {
        match self {
            Self::Size => (12., 100., 1.),
            Self::BackgroundOpacity => (0., 100., 1.),
            Self::FadeDuration => (0., 50., 1.),
            // `minValue={0} maxValue={300} step={5}` (`KeyboardTab.tsx:366-368`).
            Self::LingerDuration => (0., 300., 5.),
            // `minValue={50} maxValue={1000} step={10}` (`:380-382`).
            Self::GroupingThreshold => (50., 1000., 10.),
        }
    }

    pub fn read(self, project: &ProjectConfiguration) -> f32 {
        let settings = keyboard_settings(project);
        match self {
            Self::Size => settings.size as f32,
            Self::BackgroundOpacity => settings.background_opacity as f32,
            Self::FadeDuration => settings.fade_duration * 100.,
            Self::LingerDuration => settings.linger_duration * 100.,
            Self::GroupingThreshold => settings.grouping_threshold_ms as f32,
        }
    }
}

/// `project?.captions?.settings ?? defaultCaptionSettings`
/// (`CaptionsTab.tsx:315-319`).
pub fn caption_settings(project: &ProjectConfiguration) -> CaptionSettings {
    project
        .captions
        .as_ref()
        .map(|captions| captions.settings.clone())
        .unwrap_or_else(crate::style_presets::new_caption_settings)
}

/// `project?.keyboard?.settings ?? defaultKeyboardSettings`
/// (`KeyboardTab.tsx:39-48`).
pub fn keyboard_settings(project: &ProjectConfiguration) -> KeyboardSettings {
    project
        .keyboard
        .as_ref()
        .map(|keyboard| keyboard.settings.clone())
        .unwrap_or_default()
}

fn keyboard_generation_settings_fingerprint(settings: &KeyboardSettings) -> (u64, u32, bool, bool) {
    (
        settings.grouping_threshold_ms.to_bits(),
        settings.linger_duration.to_bits(),
        settings.show_modifiers,
        settings.show_special_keys,
    )
}

/// `updateCaptionSetting` (`CaptionsTab.tsx:321-338`): a settings write is a
/// no-op when the project has no captions block at all, which is what the
/// source's `if (!project?.captions) return` says.
fn with_caption_settings(
    project: &mut ProjectConfiguration,
    change: impl FnOnce(&mut CaptionSettings),
) -> bool {
    let Some(captions) = project.captions.as_mut() else {
        return false;
    };
    change(&mut captions.settings);
    true
}

/// `updateSetting` (`KeyboardTab.tsx:50-61`): unlike captions, this one
/// *creates* the block from the defaults when it is missing.
fn with_keyboard_settings(
    project: &mut ProjectConfiguration,
    change: impl FnOnce(&mut KeyboardSettings),
) -> bool {
    let keyboard = project.keyboard.get_or_insert_with(KeyboardData::default);
    change(&mut keyboard.settings);
    true
}

// ---------------------------------------------------------------------------
// Menus
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarMenu {
    BackgroundCornerStyle,
    CameraBlur,
    CameraShape,
    CameraMoreShape,
    CameraCornerStyle,
    AudioStereo,
    CaptionModel,
    CaptionLanguage,
    TranslationLanguage,
    CaptionFont,
    CaptionTranslationFont,
    CaptionHighlightStyle,
    CaptionPosition,
    CaptionAnimation,
    CaptionWeight,
    CaptionTranslationWeight,
    KeyboardFont,
    KeyboardPosition,
    KeyboardWeight,
    /// The segment panels' own selects carry their segment index.
    ///
    /// `TextFontFamily` is `FontPicker` (`FontPicker.tsx`), which is the one
    /// select in the sidebar the source draws as a **combobox** rather than a
    /// `KSelect`: it lists every installed family, so it filters as you type.
    /// See the README's font-picker deviation.
    TextFontFamily(usize),
    TextWeight(usize),
    Camera3DEasing(usize),
}

pub struct OpenMenu {
    pub kind: SidebarMenu,
    pub state: ui::MenuState,
}

impl EditorWindow {
    /// The rows a menu draws, with a check mark on the value in force.
    pub(crate) fn sidebar_menu_items(&self, kind: SidebarMenu) -> Vec<ui::MenuItem> {
        let snapshot = self.style_control_project();
        let project = &snapshot;
        let captions = caption_settings(project);
        let keyboard = keyboard_settings(project);
        match kind {
            SidebarMenu::TranslationLanguage => self.translation_language_items(),
            SidebarMenu::BackgroundCornerStyle => CORNER_STYLES
                .iter()
                .map(|(style, label)| {
                    ui::MenuItem::new(label(), *style == project.background.rounding_type)
                })
                .collect(),
            SidebarMenu::CameraBlur => CAMERA_BLUR_MODES
                .iter()
                .map(|(mode, label)| {
                    ui::MenuItem::new(label(), *mode == project.camera.background_blur.mode)
                })
                .collect(),
            SidebarMenu::CameraShape | SidebarMenu::CameraMoreShape => {
                self.camera_ratio_items(kind)
            }
            SidebarMenu::CameraCornerStyle => CORNER_STYLES
                .iter()
                .map(|(style, label)| {
                    ui::MenuItem::new(label(), *style == project.camera.rounding_type)
                })
                .collect(),
            SidebarMenu::AudioStereo => STEREO_MODES
                .iter()
                .map(|(mode, label)| {
                    ui::MenuItem::new(label(), *mode == project.audio.mic_stereo_mode)
                })
                .collect(),
            SidebarMenu::CaptionModel => {
                let selected = self.selected_caption_model().name;
                available_caption_models()
                    .into_iter()
                    .map(|model| {
                        ui::MenuItem::new(
                            SharedString::from(caption_model_summary(model)),
                            model.name == selected,
                        )
                    })
                    .chain(std::iter::once(ui::MenuItem::new(
                        tr::editor::tabs::captions::manage_models(),
                        false,
                    )))
                    .collect()
            }
            SidebarMenu::CaptionLanguage => CAPTION_LANGUAGES
                .iter()
                .map(|(code, label)| {
                    ui::MenuItem::new(label(), *code == self.sidebar.caption_language)
                })
                .collect(),
            SidebarMenu::CaptionFont | SidebarMenu::CaptionTranslationFont => {
                let selected = if kind == SidebarMenu::CaptionTranslationFont {
                    captions.resolved_translation_style().font
                } else {
                    captions.font.clone()
                };
                caption_font_options()
                    .iter()
                    .map(|font| ui::MenuItem::new(font_option_label(font), *font == selected))
                    .collect()
            }
            SidebarMenu::CaptionHighlightStyle => CAPTION_HIGHLIGHT_STYLES
                .iter()
                .map(|(value, label)| {
                    ui::MenuItem::new(label(), *value == captions.highlight_style)
                })
                .collect(),
            SidebarMenu::CaptionPosition => CAPTION_POSITIONS
                .iter()
                .map(|(value, label)| ui::MenuItem::new(label(), *value == captions.position))
                .collect(),
            SidebarMenu::CaptionAnimation => CAPTION_ANIMATIONS
                .iter()
                .map(|(value, label)| ui::MenuItem::new(label(), *value == captions.animation))
                .collect(),
            SidebarMenu::CaptionWeight | SidebarMenu::CaptionTranslationWeight => TEXT_WEIGHTS
                .iter()
                .map(|(weight, label)| {
                    ui::MenuItem::new(
                        label(),
                        *weight
                            == if kind == SidebarMenu::CaptionTranslationWeight {
                                captions.resolved_translation_appearance().font_weight
                            } else {
                                captions.font_weight
                            },
                    )
                })
                .collect(),
            SidebarMenu::KeyboardFont => FONT_OPTIONS
                .iter()
                .map(|font| ui::MenuItem::new(font_option_label(font), *font == keyboard.font))
                .collect(),
            SidebarMenu::KeyboardPosition => KEYBOARD_POSITIONS
                .iter()
                .map(|(value, label)| ui::MenuItem::new(label(), *value == keyboard.position))
                .collect(),
            SidebarMenu::KeyboardWeight => TEXT_WEIGHTS
                .iter()
                .map(|(weight, label)| ui::MenuItem::new(label(), *weight == keyboard.font_weight))
                .collect(),
            SidebarMenu::TextFontFamily(index)
            | SidebarMenu::TextWeight(index)
            | SidebarMenu::Camera3DEasing(index) => self.panel_menu_items(kind, index),
        }
    }

    pub(crate) fn open_sidebar_menu(
        &mut self,
        kind: SidebarMenu,
        trigger_bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A focused field would swallow the menu's arrows and Enter before the
        // root's `on_key_down` ever saw them -- its bindings sit deeper in the
        // dispatch path -- so opening a menu takes focus back, exactly as
        // clicking a `<button>` blurs a focused `<input>` in the webview.
        let focus = self.focus_handle_for_menu();
        window.focus(&focus, cx);
        let items = self.sidebar_menu_items(kind);
        self.sidebar.menu = Some(OpenMenu {
            kind,
            state: ui::MenuState::anchored(trigger_bounds, &items),
        });
        cx.notify();
    }

    /// Arrows / Home / End / Enter / Escape on an open menu. Returns whether
    /// the key was consumed, which is what keeps Escape from also clearing the
    /// timeline selection underneath.
    pub(crate) fn sidebar_menu_key(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(menu) = self.sidebar.menu.as_mut() else {
            return false;
        };
        let kind = menu.kind;
        match menu.state.on_key(key) {
            ui::MenuKey::Moved => {
                cx.notify();
                true
            }
            ui::MenuKey::Commit(index) => {
                self.choose_sidebar_menu(kind, index, window, cx);
                true
            }
            ui::MenuKey::Dismiss => {
                self.sidebar.menu = None;
                cx.notify();
                true
            }
            ui::MenuKey::Ignored => false,
        }
    }

    pub(crate) fn render_sidebar_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.sidebar.menu.as_ref()?;
        let kind = menu.kind;
        let items = self.sidebar_menu_items(kind);
        Some(
            ui::Menu::editor(&self.theme, "sidebar-menu", items, &menu.state)
                .on_select(cx.listener(move |this, index: &usize, window, cx| {
                    this.choose_sidebar_menu(kind, *index, window, cx);
                }))
                .on_dismiss(cx.listener(|this, _, _window, cx| {
                    this.sidebar.menu = None;
                    cx.notify();
                }))
                .into_any_element(),
        )
    }

    fn choose_sidebar_menu(
        &mut self,
        kind: SidebarMenu,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let anchor = self.sidebar.menu.as_ref().map(|menu| menu.state.clone());
        self.sidebar.menu = None;
        match kind {
            SidebarMenu::TranslationLanguage => self.choose_translation_language(index, window, cx),
            SidebarMenu::BackgroundCornerStyle => {
                let Some((style, _)) = CORNER_STYLES.get(index) else {
                    return;
                };
                let style = *style;
                self.edit_project("rounding-type", window, cx, move |project| {
                    if project.background.rounding_type == style {
                        return false;
                    }
                    project.background.rounding_type = style;
                    true
                });
            }
            SidebarMenu::CameraBlur => {
                let Some((mode, _)) = CAMERA_BLUR_MODES.get(index) else {
                    return;
                };
                let mode = *mode;
                self.edit_project("camera-blur", window, cx, move |project| {
                    if project.camera.background_blur.mode == mode {
                        return false;
                    }
                    project.camera.background_blur = BackgroundBlurConfig { mode };
                    true
                });
            }
            SidebarMenu::CameraShape | SidebarMenu::CameraMoreShape => {
                self.choose_camera_ratio(kind, index, anchor, window, cx);
            }
            SidebarMenu::CameraCornerStyle => {
                let Some((style, _)) = CORNER_STYLES.get(index) else {
                    return;
                };
                let style = *style;
                self.edit_project("camera-corner-style", window, cx, move |project| {
                    if project.camera.rounding_type == style {
                        return false;
                    }
                    project.camera.rounding_type = style;
                    true
                });
            }
            SidebarMenu::AudioStereo => {
                let Some((mode, _)) = STEREO_MODES.get(index) else {
                    return;
                };
                let mode = mode.clone();
                self.edit_project("audio-stereo", window, cx, move |project| {
                    if project.audio.mic_stereo_mode == mode {
                        return false;
                    }
                    project.audio.mic_stereo_mode = mode;
                    true
                });
            }
            // The two transcription pickers are local UI state in the source
            // too (`createSignal` persisted to `localStorage`,
            // `CaptionsTab.tsx:622-641`), not project config; the store's
            // `gpui` section is this app's `localStorage`.
            SidebarMenu::CaptionModel => {
                let models = available_caption_models();
                if index == models.len() {
                    cx.defer(|cx| {
                        crate::app_windows::open_ai_settings(
                            crate::settings_ai::AiTab::Recognition,
                            cx,
                        )
                    });
                }
                if let Some(model) = models.get(index) {
                    self.sidebar.caption_model = model.name;
                    if !store::set_store_setting(
                        transcription::GPUI_STORE_SECTION,
                        transcription::SELECTED_MODEL_KEY,
                        Value::String(model.name.to_string()),
                    ) {
                        tracing::warn!("the store refused the transcription model write");
                    }
                    cx.notify();
                }
            }
            SidebarMenu::CaptionLanguage => {
                if let Some((code, _)) = CAPTION_LANGUAGES.get(index) {
                    self.sidebar.caption_language = code;
                    if !store::set_store_setting(
                        transcription::GPUI_STORE_SECTION,
                        transcription::SELECTED_LANGUAGE_KEY,
                        Value::String((*code).to_string()),
                    ) {
                        tracing::warn!("the store refused the transcription language write");
                    }
                    cx.notify();
                }
            }
            SidebarMenu::CaptionFont | SidebarMenu::CaptionTranslationFont => {
                let Some(font) = caption_font_options().into_iter().nth(index) else {
                    return;
                };
                self.set_caption_setting("caption-font", window, cx, move |settings| {
                    if kind == SidebarMenu::CaptionTranslationFont {
                        settings.set_translation_font(font);
                    } else {
                        settings.set_source_font(font);
                    }
                });
            }
            SidebarMenu::CaptionHighlightStyle => {
                let Some((value, _)) = CAPTION_HIGHLIGHT_STYLES.get(index) else {
                    return;
                };
                let value = value.to_string();
                self.set_caption_setting("caption-highlight-style", window, cx, move |settings| {
                    settings.highlight_style = value
                });
            }
            SidebarMenu::CaptionPosition => {
                let Some((value, _)) = CAPTION_POSITIONS.get(index) else {
                    return;
                };
                let on_screen = self.frame_layout.and_then(|layout| {
                    let caption = layout.caption?;
                    let rect = crate::editor_canvas::norm_rect(caption.bounds, &layout);
                    Some((
                        caption,
                        ((rect.x + rect.w / 2.) as f32, (rect.y + rect.h / 2.) as f32),
                    ))
                });
                self.set_caption_setting("caption-position", window, cx, move |settings| {
                    set_caption_position(settings, value, on_screen)
                });
            }
            SidebarMenu::CaptionAnimation => {
                let Some((value, _)) = CAPTION_ANIMATIONS.get(index) else {
                    return;
                };
                let value = value.to_string();
                self.set_caption_setting("caption-animation", window, cx, move |settings| {
                    settings.animation = value
                });
            }
            SidebarMenu::CaptionWeight | SidebarMenu::CaptionTranslationWeight => {
                let Some((weight, _)) = TEXT_WEIGHTS.get(index) else {
                    return;
                };
                let weight = *weight;
                self.set_caption_setting("caption-weight", window, cx, move |settings| {
                    if kind == SidebarMenu::CaptionTranslationWeight {
                        settings.edit_translation_appearance(|a| a.font_weight = weight);
                    } else {
                        settings.edit_source_appearance(|a| a.font_weight = weight);
                    }
                });
            }
            SidebarMenu::KeyboardFont => {
                let Some(font) = FONT_OPTIONS.get(index) else {
                    return;
                };
                let font = font.to_string();
                self.set_keyboard_setting("keyboard-font", window, cx, move |settings| {
                    settings.font = font
                });
            }
            SidebarMenu::KeyboardPosition => {
                let Some((value, _)) = KEYBOARD_POSITIONS.get(index) else {
                    return;
                };
                let value = value.to_string();
                self.set_keyboard_setting("keyboard-position", window, cx, move |settings| {
                    settings.position = value
                });
            }
            SidebarMenu::KeyboardWeight => {
                let Some((weight, _)) = TEXT_WEIGHTS.get(index) else {
                    return;
                };
                let weight = *weight;
                self.set_keyboard_setting("keyboard-weight", window, cx, move |settings| {
                    settings.font_weight = weight
                });
            }
            SidebarMenu::TextFontFamily(segment)
            | SidebarMenu::TextWeight(segment)
            | SidebarMenu::Camera3DEasing(segment) => {
                self.choose_panel_menu(kind, segment, index, window, cx)
            }
        }
    }

    pub(crate) fn set_caption_setting(
        &mut self,
        reason: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut CaptionSettings),
    ) {
        self.set_caption_setting_inner(true, reason, window, cx, change);
    }

    fn set_caption_setting_inner(
        &mut self,
        rebuild_timeline: bool,
        reason: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut CaptionSettings),
    ) {
        let apply = |project: &mut ProjectConfiguration| {
            with_caption_settings(project, |settings| {
                change(settings);
                if caption_reason_customises_style(reason) {
                    settings.preset = "custom".to_string();
                }
            })
        };
        if rebuild_timeline {
            self.edit_project_global(reason, window, cx, apply);
        } else {
            self.edit_project_global_without_timeline(reason, window, cx, apply);
        }
    }

    pub(crate) fn set_caption_placement(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut CaptionSettings),
    ) {
        self.set_caption_setting_inner(false, "caption-placement", window, cx, change);
    }

    pub(crate) fn set_keyboard_setting(
        &mut self,
        reason: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut KeyboardSettings),
    ) {
        self.set_keyboard_setting_inner(true, reason, window, cx, change);
    }

    fn set_keyboard_setting_inner(
        &mut self,
        rebuild_timeline: bool,
        reason: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut KeyboardSettings),
    ) {
        let apply = |project: &mut ProjectConfiguration| with_keyboard_settings(project, change);
        if rebuild_timeline {
            self.edit_project(reason, window, cx, apply);
        } else {
            self.edit_project_without_timeline(reason, window, cx, apply);
        }
    }
}

// ---------------------------------------------------------------------------
// The slider arms
// ---------------------------------------------------------------------------

impl EditorWindow {
    pub(crate) fn apply_camera_slider(
        &mut self,
        slider: CameraSlider,
        value: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_project_with_rebuild(
            slider.changes_timeline_model(),
            "camera-slider",
            window,
            cx,
            move |project| {
                let camera = &mut project.camera;
                match slider {
                    CameraSlider::CropX | CameraSlider::CropY => {
                        let mut position = camera
                            .crop_position
                            .unwrap_or(cap_project::XY::new(0.5, 0.5));
                        if slider == CameraSlider::CropX {
                            position.x = f64::from(value) / 100.;
                        } else {
                            position.y = f64::from(value) / 100.;
                        }
                        camera.crop_position = Some(position);
                    }
                    CameraSlider::Size => camera.size = value,
                    CameraSlider::ZoomSize => camera.scale_during_zoom = value / 100.,
                    CameraSlider::Rounding => camera.rounding = value,
                    CameraSlider::Shadow => camera.shadow = value,
                    CameraSlider::SkinSmoothing => camera.skin_smoothing = value / 100.,
                    // `...(project.camera.advancedShadow ?? {50, 18, 50})`
                    // (`:3253-3259`) -- the same UI fallback the background tab has.
                    CameraSlider::ShadowSize
                    | CameraSlider::ShadowOpacity
                    | CameraSlider::ShadowBlur => {
                        let mut shadow =
                            camera.advanced_shadow.clone().unwrap_or(UI_SHADOW_FALLBACK);
                        match slider {
                            CameraSlider::ShadowSize => shadow.size = value,
                            CameraSlider::ShadowOpacity => shadow.opacity = value,
                            _ => shadow.blur = value,
                        }
                        camera.advanced_shadow = Some(shadow);
                    }
                }
                true
            },
        );
    }

    pub(crate) fn apply_audio_slider(
        &mut self,
        slider: AudioSlider,
        value: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_project_with_rebuild(
            slider.changes_timeline_model(),
            "audio-slider",
            window,
            cx,
            move |project| {
                match slider {
                    AudioSlider::MicVolume => project.audio.mic_volume_db = value,
                    AudioSlider::SystemVolume => project.audio.system_volume_db = value,
                }
                true
            },
        );
    }

    pub(crate) fn apply_cursor_slider(
        &mut self,
        slider: CursorSlider,
        value: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_project_with_rebuild(
            slider.changes_timeline_model(),
            "cursor-slider",
            window,
            cx,
            move |project| {
                let cursor = &mut project.cursor;
                match slider {
                    CursorSlider::Size => cursor.size = value.round() as u32,
                    CursorSlider::Tilt => cursor.rotation_amount = value,
                    // `clampIdleDelay`: `round(min(5, max(0.5, v)) * 10) / 10`
                    // (`:525-526`).
                    CursorSlider::IdleDelay => {
                        cursor.hide_when_idle_delay = (value.clamp(0.5, 5.) * 10.).round() / 10.
                    }
                    // `setCursorPhysics` (`:531-546`): write the field, then
                    // re-match the four presets and fall to `custom`.
                    CursorSlider::Tension | CursorSlider::Friction | CursorSlider::Mass => {
                        match slider {
                            CursorSlider::Tension => cursor.tension = value,
                            CursorSlider::Friction => cursor.friction = value,
                            _ => cursor.mass = value,
                        }
                        cursor.animation_style =
                            match_cursor_preset(cursor.tension, cursor.mass, cursor.friction)
                                .unwrap_or(CursorAnimationStyle::Custom);
                    }
                    CursorSlider::RippleStrength => cursor.ripple.strength = value / 100.,
                    CursorSlider::RippleSize => cursor.ripple.size = value / 100.,
                    CursorSlider::RippleDuration => cursor.ripple.duration = value,
                }
                true
            },
        );
    }

    pub(crate) fn apply_caption_slider(
        &mut self,
        slider: CaptionSlider,
        value: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let reason = match slider {
            CaptionSlider::ManualX | CaptionSlider::ManualY => "caption-placement",
            CaptionSlider::Size
            | CaptionSlider::TranslationSize
            | CaptionSlider::TranslationOutlineWidth
            | CaptionSlider::TranslationShadowOpacity
            | CaptionSlider::TranslationShadowSize
            | CaptionSlider::TranslationGap
            | CaptionSlider::BottomMargin
            | CaptionSlider::BackgroundOpacity
            | CaptionSlider::OutlineWidth
            | CaptionSlider::ShadowOpacity
            | CaptionSlider::ShadowSize
            | CaptionSlider::FadeDuration => "caption-slider",
        };
        self.set_caption_setting_inner(
            slider.changes_timeline_model(),
            reason,
            window,
            cx,
            move |settings| slider.write(settings, value),
        );
    }

    pub(crate) fn apply_keyboard_slider(
        &mut self,
        slider: KeyboardSlider,
        value: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_keyboard_setting_inner(
            slider.changes_timeline_model(),
            "keyboard-slider",
            window,
            cx,
            move |settings| match slider {
                KeyboardSlider::Size => settings.size = value.round() as u32,
                KeyboardSlider::BackgroundOpacity => {
                    settings.background_opacity = value.round() as u32
                }
                KeyboardSlider::FadeDuration => settings.fade_duration = value / 100.,
                KeyboardSlider::LingerDuration => settings.linger_duration = value / 100.,
                KeyboardSlider::GroupingThreshold => {
                    settings.grouping_threshold_ms = f64::from(value)
                }
            },
        );
    }
}

/// `findCursorPreset` (`:477-504`): the four named presets, matched on all
/// three physics values. Anything else is `custom`.
pub fn match_cursor_preset(tension: f32, mass: f32, friction: f32) -> Option<CursorAnimationStyle> {
    CURSOR_STYLES.iter().find_map(|(style, _, _)| {
        let preset = style.preset()?;
        ((preset.tension - tension).abs() < f32::EPSILON
            && (preset.mass - mass).abs() < f32::EPSILON
            && (preset.friction - friction).abs() < f32::EPSILON)
            .then_some(*style)
    })
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

impl EditorWindow {
    /// A `<span class="text-gray-11 text-sm">` label over a control -- the
    /// `flex flex-col gap-2` pair the caption and keyboard tabs stack every
    /// row in.
    fn labelled(&self, label: &'static str, control: AnyElement) -> AnyElement {
        ui::Field::stacked(&self.theme, label)
            .child(control)
            .into_any_element()
    }

    /// The same pair on one row, for a control that fits beside its label.
    fn labelled_inline(&self, label: &'static str, control: AnyElement) -> AnyElement {
        ui::Field::inline(&self.theme, label)
            .child(div().w(px(180.)).child(control))
            .into_any_element()
    }

    pub(crate) fn menu_select(
        &self,
        kind: SidebarMenu,
        id: &'static str,
        label: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        ui::Select::plain(&self.theme, id, label)
            .stretch_label()
            .on_open(
                cx.listener(move |this, bounds: &Bounds<Pixels>, window, cx| {
                    this.open_sidebar_menu(kind, *bounds, window, cx);
                }),
            )
            .into_any_element()
    }

    /// The same, for a trigger whose element id has to carry a segment index
    /// so two panels' selects cannot collide.
    pub(crate) fn menu_select_owned(
        &self,
        kind: SidebarMenu,
        id: SharedString,
        label: SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        ui::Select::plain(&self.theme, id, label)
            .stretch_label()
            .on_open(
                cx.listener(move |this, bounds: &Bounds<Pixels>, window, cx| {
                    this.open_sidebar_menu(kind, *bounds, window, cx);
                }),
            )
            .into_any_element()
    }

    // -- Camera --------------------------------------------------------------

    /// `CameraConfig` (`ConfigSidebar.tsx:2978-3330`).
    pub(crate) fn render_camera_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        if self
            .summary()
            .is_some_and(|summary| summary.display_is_camera && !summary.has_camera)
        {
            return div()
                .flex()
                .flex_col()
                .gap(px(16.))
                .child(crate::editor_sidebar::tab_header(
                    &theme,
                    crate::editor_sidebar::SidebarTab::Camera.title(),
                    None,
                ))
                .child(self.slider_field(
                    tr::editor::tabs::camera::skin_smoothing(),
                    SliderKey::Camera(CameraSlider::SkinSmoothing),
                    "%",
                    cx,
                ))
                .into_any_element();
        }
        let camera = &self.project.camera;
        let manual = camera.manual_position;
        let hidden = camera.hide;

        let blur_label = CAMERA_BLUR_MODES
            .iter()
            .find(|(mode, _)| *mode == camera.background_blur.mode)
            .map_or(tr::editor::tabs::camera::blur::off(), |(_, label)| label());
        let corner_label = CORNER_STYLES
            .iter()
            .find(|(style, _)| *style == camera.rounding_type)
            .map_or(tr::editor::tabs::corner::squircle(), |(_, label)| label());
        let mirror = camera.mirror;
        let keep_size = camera.scale_during_zoom >= 1.;

        let header = crate::editor_sidebar::tab_header(
            &theme,
            crate::editor_sidebar::SidebarTab::Camera.title(),
            Some(
                ui::Toggle::plain(&theme, "camera-show", !hidden)
                    .on_click(cx.listener(|this, _, window, cx| {
                        let next = !this.style_control_project().camera.hide;
                        this.edit_project("camera-hide", window, cx, move |p| {
                            p.camera.hide = next;
                            true
                        });
                    }))
                    .into_any_element(),
            ),
        );
        if hidden {
            return header;
        }

        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(header)
            .child(self.render_camera_framing_controls(cx))
            .child(self.render_camera_position(cx))
            .child(self.slider_field(
                tr::editor::tabs::camera::size_label(),
                SliderKey::Camera(CameraSlider::Size),
                "%",
                cx,
            ))
            .child(self.slider_field(
                tr::editor::tabs::camera::skin_smoothing(),
                SliderKey::Camera(CameraSlider::SkinSmoothing),
                "%",
                cx,
            ))
            .child(self.slider_field(
                tr::editor::tabs::camera::rounded_corners(),
                SliderKey::Camera(CameraSlider::Rounding),
                "%",
                cx,
            ))
            .child(self.corner_style_select(
                SidebarMenu::CameraCornerStyle,
                "camera-corner-style",
                corner_label,
                cx,
            ))
            .children(manual.map(|_| {
                div()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .items_center()
                    .h(px(32.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(Hsla::from(theme.editor.text_2))
                            .child(tr::editor::tabs::camera::custom_position()),
                    )
                    .child(
                        ui::EditorButton::plain(&theme, "camera-position-reset")
                            .tinted(&theme)
                            .height(px(26.))
                            .text_size(px(12.))
                            .label(tr::editor::tabs::camera::reset())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.edit_project("camera-manual-position", window, cx, |p| {
                                    p.camera.manual_position = None;
                                    true
                                });
                            })),
                    )
                    .into_any_element()
            }))
            .children(
                self.sidebar
                    .style_target
                    .is_none()
                    .then(|| self.render_color_correction(GradeTarget::Camera, cx)),
            )
            .child(self.more_settings("camera", cx, move |this, cx| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .child(
                        ui::Field::inline(&theme, tr::editor::tabs::camera::background()).child(
                            div().w(px(150.)).child(this.menu_select(
                                SidebarMenu::CameraBlur,
                                "camera-blur",
                                blur_label,
                                cx,
                            )),
                        ),
                    )
                    .child(
                        ui::Field::inline(&theme, tr::editor::tabs::camera::mirror()).value(
                            ui::Toggle::plain(&theme, "camera-mirror", mirror)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let next = !this.style_control_project().camera.mirror;
                                    this.edit_project("camera-mirror", window, cx, move |p| {
                                        p.camera.mirror = next;
                                        true
                                    });
                                }))
                                .into_any_element(),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(this.slider_field(
                                tr::editor::tabs::camera::shadow(),
                                SliderKey::Camera(CameraSlider::Shadow),
                                "%",
                                cx,
                            ))
                            .child(this.render_camera_shadow_settings(cx)),
                    )
                    .child(
                        ui::Field::inline(
                            &theme,
                            tr::editor::tabs::camera::keep_size_during_zoom(),
                        )
                        .value(
                            ui::Toggle::plain(&theme, "camera-keep-size", keep_size)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let keep =
                                        this.style_control_project().camera.scale_during_zoom >= 1.;
                                    let next = if keep {
                                        DEFAULT_CAMERA_SCALE_DURING_ZOOM
                                    } else {
                                        1.
                                    };
                                    this.edit_project("camera-scale-zoom", window, cx, move |p| {
                                        p.camera.scale_during_zoom = next;
                                        true
                                    });
                                }))
                                .into_any_element(),
                        ),
                    )
                    .when(!keep_size, |column| {
                        column.child(this.slider_field(
                            tr::editor::tabs::camera::size_during_zoom(),
                            SliderKey::Camera(CameraSlider::ZoomSize),
                            "%",
                            cx,
                        ))
                    })
                    .into_any_element()
            }))
            .into_any_element()
    }

    /// `CornerStyleSelect` (`:3331-3396`), now an inline row.
    fn corner_style_select(
        &self,
        kind: SidebarMenu,
        id: &'static str,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        ui::Field::inline(&self.theme, tr::editor::tabs::camera::corner_style())
            .child(
                div()
                    .w(px(150.))
                    .child(self.menu_select(kind, id, label, cx)),
            )
            .into_any_element()
    }

    /// The six camera spots as a small canvas-shaped pad: each third of a
    /// row is one target, with its dot pinned toward the matching edge.
    fn render_camera_position(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let camera = &self.project.camera;
        // A camera dragged on the canvas has a manual position and none of the
        // six dots match (`:2982-2987`).
        let selected = camera.manual_position.is_none();
        let x_index = match camera.position.x {
            CameraXPosition::Left => 0usize,
            CameraXPosition::Center => 1,
            CameraXPosition::Right => 2,
        };
        let y_index = match camera.position.y {
            CameraYPosition::Top => 0usize,
            CameraYPosition::Bottom => 1,
        };
        let accent = Hsla::from(theme.editor.accent);
        let idle = Hsla::from(theme.editor.text_3);
        let hover = Hsla::from(theme.editor.ctl_hover);

        let mut pad = div()
            .w(px(120.))
            .h(px(68.))
            .flex_none()
            .p(px(3.))
            .flex()
            .flex_col()
            .rounded(px(8.))
            .border_1()
            .border_color(Hsla::from(theme.editor.line))
            .bg(Hsla::from(theme.editor.ctl));
        for item_y in 0..2usize {
            let mut row = div().flex_1().flex().flex_row();
            for item_x in 0..3usize {
                let checked = selected && x_index == item_x && y_index == item_y;
                let cell = div()
                    .id(SharedString::from(format!(
                        "camera-position-{item_x}-{item_y}"
                    )))
                    .flex_1()
                    .flex()
                    .p(px(5.))
                    .rounded(px(5.))
                    .cursor_pointer()
                    .hover(move |this| this.bg(hover))
                    .map(|cell| match item_x {
                        0 => cell.justify_start(),
                        2 => cell.justify_end(),
                        _ => cell.justify_center(),
                    })
                    .map(|cell| {
                        if item_y == 0 {
                            cell.items_start()
                        } else {
                            cell.items_end()
                        }
                    })
                    .child(
                        div()
                            .size(px(10.))
                            .rounded(px(3.))
                            .when(checked, |dot| dot.bg(accent))
                            .when(!checked, |dot| dot.border_1().border_color(idle)),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.edit_project("camera-position", window, cx, move |project| {
                            project.camera.position = cap_project::CameraPosition {
                                x: match item_x {
                                    0 => CameraXPosition::Left,
                                    2 => CameraXPosition::Right,
                                    _ => CameraXPosition::Center,
                                },
                                y: if item_y == 0 {
                                    CameraYPosition::Top
                                } else {
                                    CameraYPosition::Bottom
                                },
                            };
                            // The batch clears the manual override too (`:3011-3016`).
                            project.camera.manual_position = None;
                            true
                        });
                    }));
                row = row.child(cell);
            }
            pad = pad.child(row);
        }

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(Hsla::from(theme.editor.text_1))
                    .child(tr::editor::tabs::camera::position()),
            )
            .child(pad)
            .into_any_element()
    }

    /// `ShadowSettings` on the camera tab -- the same component the background
    /// tab's shadow field uses, against `camera.advancedShadow`.
    fn render_camera_shadow_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let open = self.sidebar.camera_shadow_open.is_open();

        div()
            .flex()
            .flex_col()
            .child(crate::editor_sidebar::disclosure_row(
                &theme,
                "camera-shadow-advanced",
                tr::editor::tabs::camera::advanced_shadow(),
                open,
                cx.listener(|this, _, window, cx| {
                    this.sidebar.camera_shadow_open.toggle();
                    this.animate_collapsibles(window, cx);
                }),
            ))
            .child(collapsible(
                &self.sidebar.camera_shadow_open,
                div()
                    .flex()
                    .flex_col()
                    .pt(px(4.))
                    .child(self.slider_field(
                        tr::editor::tabs::camera::size_label(),
                        SliderKey::Camera(CameraSlider::ShadowSize),
                        "int",
                        cx,
                    ))
                    .child(self.slider_field(
                        tr::editor::tabs::camera::shadow_opacity(),
                        SliderKey::Camera(CameraSlider::ShadowOpacity),
                        "%",
                        cx,
                    ))
                    .child(self.slider_field(
                        tr::editor::tabs::camera::shadow_blur(),
                        SliderKey::Camera(CameraSlider::ShadowBlur),
                        "int",
                        cx,
                    ))
                    .into_any_element(),
            ))
            .into_any_element()
    }

    // -- Audio ---------------------------------------------------------------

    /// The audio tab (`:942-1016`) and `SyncOffsetsConfig` (`:6081-6178`).
    pub(crate) fn render_audio_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let audio = &self.project.audio;
        let summary = self.summary();
        let muted = audio.mute;
        let has_microphone = summary.is_some_and(|summary| summary.has_microphone);
        let has_system_audio = summary.is_some_and(|summary| summary.has_system_audio);
        let stereo_mic = summary.is_some_and(|summary| summary.mic_channels == Some(2));
        let stereo_label = STEREO_MODES
            .iter()
            .find(|(mode, _)| *mode == audio.mic_stereo_mode)
            .map_or(tr::editor::tabs::audio::stereo::stereo(), |(_, label)| {
                label()
            });

        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(crate::editor_sidebar::tab_header(
                &theme,
                tr::editor::workbench::original_audio(),
                Some(
                    div()
                        .id("audio-mute-row")
                        .tooltip_show_delay(ui::TOOLTIP_SHOW_DELAY)
                        .tooltip(move |_, cx| {
                            ui::Tooltip::new(&theme, tr::editor::tabs::audio::mute()).view(cx)
                        })
                        .child(ui::Toggle::plain(&theme, "audio-mute", !muted).on_click(
                            cx.listener(|this, _, window, cx| {
                                let next = !this.project.audio.mute;
                                this.edit_project("audio-mute", window, cx, move |p| {
                                    p.audio.mute = next;
                                    true
                                });
                            }),
                        ))
                        .into_any_element(),
                ),
            ))
            .children(has_microphone.then(|| self.render_studio_sound_row(cx)))
            .children(
                (has_microphone && self.project.audio.improve)
                    .then(|| self.render_studio_sound_options(cx)),
            )
            .children((!has_microphone).then(|| {
                div()
                    .text_size(px(12.))
                    .text_color(Hsla::from(theme.editor.text_2))
                    .child(tr::editor::workbench::no_microphone())
            }))
            .children(has_microphone.then(|| {
                self.slider_field_disabled(
                    tr::editor::tabs::audio::mic_volume(),
                    SliderKey::Audio(AudioSlider::MicVolume),
                    "volume",
                    muted,
                    cx,
                )
                .into_any_element()
            }))
            .children(has_system_audio.then(|| {
                self.slider_field_disabled(
                    tr::editor::tabs::audio::system_volume(),
                    SliderKey::Audio(AudioSlider::SystemVolume),
                    "volume",
                    muted,
                    cx,
                )
                .into_any_element()
            }))
            .children(self.sidebar.audio_enhancement_error.as_ref().map(|error| {
                div()
                    .text_size(px(12.))
                    .text_color(Hsla::from(theme.editor.playhead))
                    .child(error.clone())
                    .into_any_element()
            }))
            .child(self.render_added_audio(cx))
            .child(self.more_settings("audio", cx, move |this, cx| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(16.))
                    .children(stereo_mic.then(|| {
                        ui::Field::inline(&theme, tr::editor::tabs::audio::stereo_mode())
                            .child(div().w(px(150.)).child(this.menu_select(
                                SidebarMenu::AudioStereo,
                                "audio-stereo",
                                stereo_label,
                                cx,
                            )))
                            .into_any_element()
                    }))
                    .children(has_microphone.then(|| this.render_studio_sound_default(cx)))
                    .children(this.render_sync_offsets(cx))
                    .into_any_element()
            }))
            .into_any_element()
    }

    fn render_studio_sound_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let enabled = self.project.audio.improve;

        div()
            .id("studio-sound-row")
            .tooltip_show_delay(ui::TOOLTIP_SHOW_DELAY)
            .tooltip(move |_, cx| {
                ui::Tooltip::new(&theme, tr::editor::tabs::audio::studio_sound_description())
                    .view(cx)
            })
            .child(
                ui::Field::inline(&theme, tr::editor::workbench::voice_enhance()).value(
                    ui::Toggle::plain(&theme, "audio-improve", enabled)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.edit_project("audio-improve", window, cx, move |project| {
                                project.audio.improve = !enabled;
                                true
                            });
                        }))
                        .into_any_element(),
                ),
            )
            .into_any_element()
    }

    fn render_studio_sound_default(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let enabled_by_default = self.sidebar.audio_enhancement_default;
        ui::Field::inline(&theme, tr::editor::tabs::audio::use_for_new())
            .value(
                ui::Toggle::plain(&theme, "audio-improve-default", enabled_by_default)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let next = !enabled_by_default;
                        if store::set_studio_sound_defaults(next, this.project.audio.isolation) {
                            this.sidebar.audio_enhancement_default = next;
                            this.sidebar.audio_enhancement_error = None;
                        } else {
                            this.sidebar.audio_enhancement_error =
                                Some(tr::editor::tabs::audio::studio_sound_save_failed().into());
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
            )
            .into_any_element()
    }

    fn render_studio_sound_options(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let isolation_options = [
            (
                cap_project::VoiceIsolation::Light,
                tr::editor::tabs::audio::isolation_light(),
                tr::editor::tabs::audio::isolation_light_description(),
            ),
            (
                cap_project::VoiceIsolation::Balanced,
                tr::editor::tabs::audio::isolation_balanced(),
                tr::editor::tabs::audio::isolation_balanced_description(),
            ),
            (
                cap_project::VoiceIsolation::Strong,
                tr::editor::tabs::audio::isolation_strong(),
                tr::editor::tabs::audio::isolation_strong_description(),
            ),
        ];
        let enabled = self.project.audio.improve;
        let description = isolation_options
            .iter()
            .find(|(value, _, _)| *value == self.project.audio.isolation)
            .map(|(_, _, description)| *description)
            .unwrap_or_default();

        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .when(enabled, |column| {
                column.child(
                    div()
                        .id("voice-isolation-row")
                        .tooltip_show_delay(ui::TOOLTIP_SHOW_DELAY)
                        .tooltip(move |_, cx| ui::Tooltip::new(&theme, description).view(cx))
                        .child(
                            ui::SegmentedControl::editor(
                                &theme,
                                "voice-isolation",
                                isolation_options
                                    .iter()
                                    .map(|(value, label, _)| {
                                        ui::SegmentOption::new(
                                            *label,
                                            self.project.audio.isolation == *value,
                                        )
                                    })
                                    .collect(),
                            )
                            .stretch()
                            .on_select(cx.listener(
                                move |this, index: &usize, window, cx| {
                                    let Some(&(isolation, _, _)) = isolation_options.get(*index)
                                    else {
                                        return;
                                    };
                                    this.edit_project(
                                        "voice-isolation",
                                        window,
                                        cx,
                                        move |project| {
                                            if project.audio.isolation == isolation {
                                                return false;
                                            }
                                            project.audio.isolation = isolation;
                                            true
                                        },
                                    );
                                    if this.sidebar.audio_enhancement_default {
                                        this.sidebar.audio_enhancement_error =
                                            if store::set_studio_sound_isolation(isolation) {
                                                None
                                            } else {
                                                Some(
                                                tr::editor::tabs::audio::studio_sound_save_failed()
                                                    .into(),
                                            )
                                            };
                                        cx.notify();
                                    }
                                },
                            )),
                        ),
                )
            })
            .into_any_element()
    }

    // -- Cursor --------------------------------------------------------------

    /// The cursor tab (`:820-1000`).
    pub(crate) fn render_cursor_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let cursor = &self.project.cursor;
        let hidden = cursor.hide;

        let header =
            crate::editor_sidebar::tab_header(
                &theme,
                crate::editor_sidebar::SidebarTab::Cursor.title(),
                Some(
                    div()
                        .id("cursor-show-row")
                        .tooltip_show_delay(ui::TOOLTIP_SHOW_DELAY)
                        .tooltip(move |_, cx| {
                            ui::Tooltip::new(&theme, tr::editor::tabs::cursor::show()).view(cx)
                        })
                        .child(ui::Toggle::plain(&theme, "cursor-show", !hidden).on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.edit_project("cursor-hide", window, cx, move |p| {
                                    p.cursor.hide = !hidden;
                                    true
                                });
                            }),
                        ))
                        .into_any_element(),
                ),
            );
        let body = div().flex().flex_col().gap(px(16.)).child(header);

        if hidden {
            return body.into_any_element();
        }

        let style_index = CURSOR_STYLES
            .iter()
            .position(|(style, _, _)| *style == cursor.animation_style);
        let idle = cursor.hide_when_idle;

        let body = body
            .child(
                ui::Field::stacked(&theme, tr::editor::tabs::cursor::style_label())
                    .child(self.render_cursor_style_picker(cx)),
            )
            .child(self.slider_field(
                tr::editor::tabs::cursor::size_label(),
                SliderKey::Cursor(CursorSlider::Size),
                "%",
                cx,
            ))
            .child(self.render_cursor_ripple(cx));
        let mut more = div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(self.slider_field(
                tr::editor::tabs::cursor::tilt(),
                SliderKey::Cursor(CursorSlider::Tilt),
                "x100%",
                cx,
            ))
            .child(
                ui::Field::inline(&theme, tr::editor::tabs::cursor::hide_when_idle()).value(
                    ui::Toggle::plain(&theme, "cursor-idle", idle)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.edit_project("cursor-idle", window, cx, move |p| {
                                p.cursor.hide_when_idle = !idle;
                                true
                            });
                        }))
                        .into_any_element(),
                ),
            );

        if idle {
            let readout = format!("{:.1}s", cursor.hide_when_idle_delay);
            more = more.child(
                ui::Subfield::plain(&theme, tr::editor::tabs::cursor::inactivity_delay())
                    .gap(px(16.))
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(12.))
                            .items_center()
                            .child(self.slider_flex(
                                SliderKey::Cursor(CursorSlider::IdleDelay),
                                "s",
                                cx,
                            ))
                            .child(
                                div()
                                    .w(px(48.))
                                    .text_size(px(12.))
                                    .text_color(Hsla::from(theme.gray_11))
                                    .child(readout),
                            ),
                    ),
            );
        }

        let smooth = !cursor.raw;
        more = more
            .child(
                ui::Field::section(&theme, tr::editor::tabs::cursor::movement_style()).child(
                    ui::RadioCards::plain(
                        &theme,
                        "cursor-style",
                        CURSOR_STYLES
                            .iter()
                            .map(|(_, label, description)| {
                                ui::RadioCard::new(label(), Some(description()))
                            })
                            .collect(),
                        style_index,
                    )
                    .on_select(cx.listener(
                        |this, index: &usize, window, cx| {
                            let Some((style, _, _)) = CURSOR_STYLES.get(*index) else {
                                return;
                            };
                            let style = *style;
                            // `applyCursorStylePreset` (`:551-561`): the style and
                            // its three physics values, in one batch.
                            this.edit_project("cursor-style", window, cx, move |project| {
                                project.cursor.animation_style = style;
                                if let Some(preset) = style.preset() {
                                    project.cursor.tension = preset.tension;
                                    project.cursor.mass = preset.mass;
                                    project.cursor.friction = preset.friction;
                                }
                                true
                            });
                        },
                    )),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        ui::Field::inline(&theme, tr::editor::tabs::cursor::smooth_movement())
                            .value(
                                ui::Toggle::plain(&theme, "cursor-smooth", smooth)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.sidebar.cursor_physics_open.set_open(!smooth);
                                        this.animate_collapsibles(window, cx);
                                        this.edit_project("cursor-raw", window, cx, move |p| {
                                            p.cursor.raw = smooth;
                                            true
                                        });
                                    }))
                                    .into_any_element(),
                            ),
                    )
                    .child(collapsible(
                        &self.sidebar.cursor_physics_open,
                        div()
                            .flex()
                            .flex_col()
                            // `pt-4 pb-6`
                            .pt(px(4.))
                            .pb(px(8.))
                            .child(self.slider_field(
                                tr::editor::tabs::cursor::tension(),
                                SliderKey::Cursor(CursorSlider::Tension),
                                "int",
                                cx,
                            ))
                            .child(self.slider_field(
                                tr::editor::tabs::cursor::friction(),
                                SliderKey::Cursor(CursorSlider::Friction),
                                "int",
                                cx,
                            ))
                            .child(self.slider_field(
                                tr::editor::tabs::cursor::mass(),
                                SliderKey::Cursor(CursorSlider::Mass),
                                "",
                                cx,
                            ))
                            .into_any_element(),
                    )),
            );

        // An explicit family forces the SVG assets, so the toggle would be
        // showing a setting the renderer is already overriding.
        if cursor.cursor_type().family().is_none() {
            more = more.child(
                ui::Field::inline(&theme, tr::editor::tabs::cursor::svg_cursors()).value(
                    ui::Toggle::plain(&theme, "cursor-svg", cursor.use_svg)
                        .on_click(cx.listener(|this, _, window, cx| {
                            let next = !this.style_control_project().cursor.use_svg;
                            this.edit_project("cursor-svg", window, cx, move |p| {
                                p.cursor.use_svg = next;
                                true
                            });
                        }))
                        .into_any_element(),
                ),
            );
        }

        body.child(self.more_settings("cursor", cx, move |_, _| more.into_any_element()))
            .into_any_element()
    }

    // -- Keyboard ------------------------------------------------------------

    fn set_keyboard_message(
        &mut self,
        message: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_notice(message.clone(), window, cx);
        self.keyboard_generation_message = Some(message);
    }

    pub(crate) fn generate_keyboard_segments_clicked(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.generating_keyboard {
            return;
        }
        let Some(timeline) = self.project.timeline.clone() else {
            self.set_keyboard_message(
                tr::editor::tabs::keyboard::timeline_unavailable().into(),
                window,
                cx,
            );
            cx.notify();
            return;
        };
        let Ok(timeline_fingerprint) = serde_json::to_vec(&timeline) else {
            self.set_keyboard_message(
                tr::editor::tabs::keyboard::timeline_unreadable().into(),
                window,
                cx,
            );
            cx.notify();
            return;
        };
        let settings = keyboard_settings(&self.project);
        let settings_fingerprint = keyboard_generation_settings_fingerprint(&settings);
        let path = self.project_path.clone();
        self.generating_keyboard = true;
        self.keyboard_generation_message = None;
        cx.notify();
        window.refresh();

        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let meta = RecordingMeta::load_for_project(&path)
                        .map_err(tr::editor::tabs::keyboard::load_failed)?;
                    cap_project::generate_project_keyboard_segments(&meta, &timeline, &settings)
                })
                .await;

            this.update_in(cx, |this, window, cx| {
                this.generating_keyboard = false;
                let segments = match result {
                    Ok(segments) => segments,
                    Err(error) => {
                        tracing::error!("keyboard generation failed: {error}");
                        this.set_keyboard_message(error, window, cx);
                        cx.notify();
                        window.refresh();
                        return;
                    }
                };
                let timeline_unchanged = this
                    .project
                    .timeline
                    .as_ref()
                    .and_then(|timeline| serde_json::to_vec(timeline).ok())
                    .is_some_and(|fingerprint| fingerprint == timeline_fingerprint);
                let settings_unchanged =
                    keyboard_generation_settings_fingerprint(&keyboard_settings(&this.project))
                        == settings_fingerprint;
                if !timeline_unchanged || !settings_unchanged {
                    this.set_keyboard_message(
                        tr::editor::tabs::keyboard::changed_during_generation().into(),
                        window,
                        cx,
                    );
                    cx.notify();
                    window.refresh();
                    return;
                }
                let has_segments = !segments.is_empty();
                let already_empty = this
                    .project
                    .timeline
                    .as_ref()
                    .is_none_or(|timeline| timeline.keyboard_segments.is_empty());
                if !has_segments && already_empty {
                    this.set_keyboard_message(
                        tr::editor::tabs::keyboard::no_presses().into(),
                        window,
                        cx,
                    );
                    cx.notify();
                    window.refresh();
                    return;
                }

                if has_segments {
                    this.tracks.keyboard = true;
                }
                this.edit_project("keyboard-generate", window, cx, move |project| {
                    if has_segments {
                        let keyboard = project.keyboard.get_or_insert_with(KeyboardData::default);
                        keyboard.settings.enabled = true;
                    }
                    let Some(timeline) = project.timeline.as_mut() else {
                        return false;
                    };
                    timeline.keyboard_segments = segments;
                    true
                });
                if has_segments {
                    this.keyboard_generation_message = None;
                } else {
                    if this.selection.as_ref().is_some_and(|selection| {
                        selection.track == crate::editor_timeline::TrackKind::Keyboard
                    }) {
                        this.set_selection(None, cx);
                    }
                    this.set_keyboard_message(
                        tr::editor::tabs::keyboard::no_presses().into(),
                        window,
                        cx,
                    );
                    cx.notify();
                    window.refresh();
                }
            })
            .ok();
        })
        .detach();
    }

    /// `KeyboardTab` (`KeyboardTab.tsx:128-553`).
    pub(crate) fn render_keyboard_settings(
        &self,
        segment: Option<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let settings = keyboard_settings(&self.project);
        let font_label = font_option_label(&settings.font);
        let position_label = KEYBOARD_POSITIONS
            .iter()
            .find(|(value, _)| *value == settings.position)
            .map_or_else(
                || match settings.position.as_str() {
                    "top-left" => tr::editor::tabs::position::top_left(),
                    "top-right" => tr::editor::tabs::position::top_right(),
                    "bottom-left" => tr::editor::tabs::position::bottom_left(),
                    "bottom-right" => tr::editor::tabs::position::bottom_right(),
                    "top" => tr::editor::tabs::position::top_center(),
                    _ => tr::editor::tabs::position::bottom_center(),
                },
                |(_, label)| label(),
            );
        let weight_label = TEXT_WEIGHTS
            .iter()
            .find(|(weight, _)| *weight == settings.font_weight)
            .map_or(tr::editor::tabs::weight::normal(), |(_, label)| label());

        let generating = self.generating_keyboard;
        let can_generate = !generating && self.project.timeline.is_some();
        let message = self.keyboard_generation_message.as_ref().map(|message| {
            div()
                .text_size(px(12.))
                .text_color(Hsla::from(theme.editor.text_2))
                .child(message.clone())
                .into_any_element()
        });
        let timing = segment.and_then(|index| {
            self.project
                .timeline
                .as_ref()
                .and_then(|timeline| timeline.keyboard_segments.get(index))
                .map(|segment| (index, segment.start, segment.end))
        });

        let font_color = settings.color.clone();
        let background_color = settings.background_color.clone();
        let fade = format!("{:.0}ms", settings.fade_duration * 1000.);
        let linger = format!("{:.1}s", settings.linger_duration);
        let grouping = format!("{:.0}ms", settings.grouping_threshold_ms);
        let show_modifiers = settings.show_modifiers;
        let show_special_keys = settings.show_special_keys;
        let uppercase = settings.uppercase;
        let toggle_row = |id: &'static str,
                          label: &'static str,
                          value: bool,
                          apply: fn(&mut cap_project::KeyboardSettings, bool),
                          cx: &mut Context<Self>| {
            ui::Field::inline(&theme, label)
                .value(
                    ui::Toggle::plain(&theme, id, value)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.set_keyboard_setting(id, window, cx, move |settings| {
                                apply(settings, !value)
                            });
                        }))
                        .into_any_element(),
                )
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(16.))
                    .child(
                        ui::Field::inline(&theme, tr::editor::tabs::keyboard::position()).child(
                            div().w(px(150.)).child(self.menu_select(
                                SidebarMenu::KeyboardPosition,
                                "keyboard-position",
                                position_label,
                                cx,
                            )),
                        ),
                    )
                    .child(self.slider_field(
                        tr::editor::tabs::keyboard::size_label(),
                        SliderKey::Keyboard(KeyboardSlider::Size),
                        "",
                        cx,
                    ))
                    .child(self.more_settings("keyboard", cx, move |this, cx| {
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(18.))
                            .children(timing.map(|(index, start, end)| {
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(12.))
                                    .child(this.timing_field(
                                        crate::editor_panels::FieldKey::KeyboardStart(index),
                                        crate::editor_panels::FieldKey::KeyboardEnd(index),
                                        start,
                                        end,
                                        cx,
                                    ))
                                    .child(this.slider_field(
                                        tr::editor::panel::fade_duration(),
                                        SliderKey::Panel(
                                            crate::editor_panels::PanelSlider::KeyboardFade,
                                            index,
                                        ),
                                        "",
                                        cx,
                                    ))
                            }))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(12.))
                                    .child(crate::editor_sidebar::section_title(
                                        &theme,
                                        tr::editor::tabs::keyboard::font_settings(),
                                    ))
                                    .child(this.labelled_inline(
                                        tr::editor::tabs::keyboard::font_family(),
                                        this.menu_select(
                                            SidebarMenu::KeyboardFont,
                                            "keyboard-font",
                                            font_label,
                                            cx,
                                        ),
                                    ))
                                    .child(
                                        ui::Field::inline(
                                            &theme,
                                            tr::editor::tabs::keyboard::font_weight(),
                                        )
                                        .child(
                                            div().w(px(150.)).child(this.menu_select(
                                                SidebarMenu::KeyboardWeight,
                                                "keyboard-weight",
                                                weight_label,
                                                cx,
                                            )),
                                        ),
                                    )
                                    .child(this.labelled(
                                        tr::editor::tabs::keyboard::text_color(),
                                        this.render_hex_field(
                                            crate::editor_panels::FieldKey::KeyboardColor,
                                            &font_color,
                                            cx,
                                        ),
                                    )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(12.))
                                    .child(crate::editor_sidebar::section_title(
                                        &theme,
                                        tr::editor::tabs::keyboard::background_settings(),
                                    ))
                                    .child(this.labelled(
                                        tr::editor::tabs::keyboard::background_color(),
                                        this.render_hex_field(
                                            crate::editor_panels::FieldKey::KeyboardBackground,
                                            &background_color,
                                            cx,
                                        ),
                                    ))
                                    .child(this.slider_field(
                                        tr::editor::tabs::keyboard::background_opacity(),
                                        SliderKey::Keyboard(KeyboardSlider::BackgroundOpacity),
                                        "%",
                                        cx,
                                    )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(12.))
                                    .child(crate::editor_sidebar::section_title(
                                        &theme,
                                        tr::editor::tabs::keyboard::animation(),
                                    ))
                                    .child(this.labelled_readout(
                                        tr::editor::tabs::keyboard::fade_duration(),
                                        SliderKey::Keyboard(KeyboardSlider::FadeDuration),
                                        fade,
                                        cx,
                                    ))
                                    .child(this.labelled_readout(
                                        tr::editor::tabs::keyboard::linger_duration(),
                                        SliderKey::Keyboard(KeyboardSlider::LingerDuration),
                                        linger,
                                        cx,
                                    ))
                                    .child(this.labelled_readout(
                                        tr::editor::tabs::keyboard::grouping_threshold(),
                                        SliderKey::Keyboard(KeyboardSlider::GroupingThreshold),
                                        grouping,
                                        cx,
                                    )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(12.))
                                    .child(crate::editor_sidebar::section_title(
                                        &theme,
                                        tr::editor::tabs::keyboard::behavior(),
                                    ))
                                    .child(toggle_row(
                                        "keyboard-modifiers",
                                        tr::editor::tabs::keyboard::show_modifiers(),
                                        show_modifiers,
                                        |settings, value| settings.show_modifiers = value,
                                        cx,
                                    ))
                                    .child(toggle_row(
                                        "keyboard-special",
                                        tr::editor::tabs::keyboard::show_special_keys(),
                                        show_special_keys,
                                        |settings, value| settings.show_special_keys = value,
                                        cx,
                                    ))
                                    .child(toggle_row(
                                        "keyboard-uppercase",
                                        tr::editor::tabs::keyboard::uppercase(),
                                        uppercase,
                                        |settings, value| settings.uppercase = value,
                                        cx,
                                    )),
                            )
                            .child(
                                div().flex().child(
                                    ui::EditorButton::plain(&theme, "keyboard-generate")
                                        .tinted(&theme)
                                        .left_icon("icons/refresh-cw.svg")
                                        .icon_size(px(13.))
                                        .text_size(px(12.))
                                        .label(if generating {
                                            tr::editor::tabs::keyboard::generating()
                                        } else {
                                            tr::editor::tabs::keyboard::regenerate()
                                        })
                                        .disabled(!can_generate)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.generate_keyboard_segments_clicked(window, cx);
                                        })),
                                ),
                            )
                            .into_any_element()
                    })),
            )
            .children(message)
            .into_any_element()
    }

    /// A slider with a `text-xs text-right` readout underneath -- the shape the
    /// keyboard and caption tabs use for every duration.
    fn labelled_readout(
        &self,
        label: &'static str,
        slider: SliderKey,
        readout: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        ui::Field::inline(&self.theme, label)
            .value_text(readout)
            .child(self.slider_flex(slider, "", cx))
            .into_any_element()
    }

    // -- Captions ------------------------------------------------------------

    /// `CaptionsTab` (`CaptionsTab.tsx:765-1561`).
    pub(crate) fn render_captions_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let settings = caption_settings(&self.project);
        let has_captions = self
            .project
            .timeline
            .as_ref()
            .is_some_and(|timeline| !timeline.caption_segments.is_empty())
            || self
                .project
                .captions
                .as_ref()
                .is_some_and(|captions| !captions.segments.is_empty());

        self.ensure_captions_init(cx);
        let snapshot = transcription::ui_snapshot(&self.project_path);
        let running_model =
            crate::recording_autocaptions::running_model(&self.project_path, snapshot.generating);
        let models = if running_model.is_some() {
            local_caption_models()
        } else {
            available_caption_models()
        };
        let model = caption_model_for_display(
            &self.sidebar.caption_model,
            &models,
            running_model.as_deref(),
        );

        let language_label = CAPTION_LANGUAGES
            .iter()
            .find(|(code, _)| *code == self.sidebar.caption_language)
            .map_or(
                tr::editor::tabs::captions::language::auto(),
                |(_, label)| label(),
            );
        let position_label = CAPTION_POSITIONS
            .iter()
            .find(|(value, _)| *value == settings.position)
            .map_or_else(
                || match settings.position.as_str() {
                    "top-left" => tr::editor::tabs::position::top_left(),
                    "top-right" => tr::editor::tabs::position::top_right(),
                    "bottom-left" => tr::editor::tabs::position::bottom_left(),
                    "bottom-right" => tr::editor::tabs::position::bottom_right(),
                    "top" => tr::editor::tabs::position::top_center(),
                    _ => tr::editor::tabs::position::bottom_center(),
                },
                |(_, label)| label(),
            );

        let model_downloaded = snapshot.downloaded.contains(model.name);
        let cloud = crate::asr_cloud::is_cloud_model(model.name);
        let active_download = snapshot
            .download
            .as_ref()
            .filter(|download| download.state == transcription::DownloadState::Downloading);
        let is_downloading = active_download.is_some();
        let download_percent = active_download.map_or(0_u32, |download| {
            download.progress.clamp(0.0, 100.0).round() as u32
        });
        let downloading_label = active_download
            .and_then(|download| models.iter().find(|entry| entry.name == download.model))
            .map_or((model.label)(), |entry| (entry.label)());
        let download_message = active_download
            .map(|download| download.message.clone())
            .filter(|message| !message.is_empty());
        let download_failure = snapshot
            .download
            .as_ref()
            .filter(|download| {
                download.state == transcription::DownloadState::Failed
                    && download.model == model.name
            })
            .map(|download| download.message.clone());
        let deleting = snapshot.deleting.as_deref() == Some(model.name);
        let any_deleting = snapshot.deleting.is_some();
        let generating = snapshot.generating;
        // `hasAudio` (`CaptionsTab.tsx:600-605`).
        let has_audio = self
            .summary()
            .is_some_and(|summary| summary.has_microphone || summary.has_system_audio);
        let error_color = Hsla::from(gpui::rgb(0xef4444));

        // The `KSelect.Trigger` with label, description and size
        // (`CaptionsTab.tsx:824-860`); the info tooltip carries the engine
        // identity the source puts behind the ⓘ button.
        let model_trigger = div()
            .id("caption-model")
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .py(px(8.))
            .rounded(px(8.))
            .bg(self.panel_bg())
            .border_1()
            .border_color(Hsla::from(theme.editor.line))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(Hsla::from(theme.gray_12))
                            .truncate()
                            .child((model.label)()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(Hsla::from(theme.gray_11))
                            .truncate()
                            .child((model.description)()),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(10.))
                    .text_color(Hsla::from(theme.gray_10))
                    .child(crate::i18n::decimal(model.size)),
            )
            .child(
                svg()
                    .path("icons/chevron-down.svg")
                    .size(px(16.))
                    .flex_shrink_0()
                    .text_color(Hsla::from(theme.gray_11)),
            )
            .tooltip({
                let model_name = SharedString::new_static(model.model_name);
                move |_window, cx| ui::Tooltip::new(&theme, model_name.clone()).view(cx)
            });
        let model_trigger = if generating {
            model_trigger.into_any_element()
        } else {
            ui::Menu::trigger(
                model_trigger,
                cx.listener(|this, bounds: &Bounds<Pixels>, window, cx| {
                    this.open_sidebar_menu(SidebarMenu::CaptionModel, *bounds, window, cx);
                }),
            )
            .into_any_element()
        };

        let wants_resplit =
            has_captions
                && self.project.captions.as_ref().is_some_and(|captions| {
                    crate::caption_breaks::wants_resplit(&captions.segments)
                });
        let resplitting = self.sidebar.caption_resplit.running;
        let status_line: Option<AnyElement> = if let Some(error) = snapshot.generation_error.clone()
        {
            Some(
                div()
                    .text_size(px(12.))
                    .text_color(error_color)
                    .child(error)
                    .into_any_element(),
            )
        } else if snapshot.refining {
            Some(
                div()
                    .id("caption-refining")
                    .text_size(px(12.))
                    .text_color(Hsla::from(theme.editor.text_2))
                    .child(SharedString::from(
                        tr::editor::tabs::captions::refining_word_timings(),
                    ))
                    .into_any_element(),
            )
        } else {
            None
        };

        let setup: Option<AnyElement> = if model_downloaded {
            (has_audio && !has_captions).then(|| {
                ui::EditorButton::plain(&theme, "caption-generate")
                    .filled(&theme)
                    .height(px(34.))
                    .left_icon("icons/captions.svg")
                    .label(if generating {
                        tr::editor::tabs::captions::generating()
                    } else {
                        tr::editor::tabs::captions::generate()
                    })
                    .disabled(generating || any_deleting)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.generate_transcript(true, window, cx);
                    }))
                    .into_any_element()
            })
        } else if cloud {
            Some(
                ui::EditorButton::plain(&theme, "caption-cloud-setup")
                    .filled(&theme)
                    .height(px(34.))
                    .left_icon("icons/settings.svg")
                    .label(tr::editor::tabs::captions::cloud_setup())
                    .on_click(|_, _, cx| {
                        cx.defer(|cx| {
                            crate::app_windows::open_ai_settings(
                                crate::settings_ai::AiTab::Recognition,
                                cx,
                            )
                        });
                    })
                    .into_any_element(),
            )
        } else {
            let download_button = ui::EditorButton::plain(&theme, "caption-download")
                .filled(&theme)
                .height(px(34.))
                .disabled(is_downloading)
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.start_caption_model_download(cx);
                }));
            let download_button = if is_downloading {
                download_button.label(tr::editor::tabs::captions::downloading(
                    downloading_label,
                    download_percent,
                ))
            } else {
                download_button
                    .left_icon("icons/download.svg")
                    .label(tr::editor::tabs::captions::download((model.label)()))
            };
            let mut column = div().flex().flex_col().gap(px(8.)).child(download_button);
            if is_downloading {
                column = column
                    .child(
                        div()
                            .w_full()
                            .h(px(6.))
                            .rounded_full()
                            .overflow_hidden()
                            .bg(Hsla::from(theme.editor.ctl))
                            .child(
                                div()
                                    .h_full()
                                    .rounded_full()
                                    .bg(Hsla::from(theme.editor.accent))
                                    .w(relative(download_percent as f32 / 100.)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(Hsla::from(theme.editor.text_2))
                            .child(download_message.unwrap_or_else(|| {
                                tr::editor::tabs::captions::keep_open().to_string()
                            })),
                    );
            } else {
                column = column.child(
                    div()
                        .text_size(px(12.))
                        .text_color(Hsla::from(theme.editor.text_2))
                        .child(tr::editor::tabs::captions::download_note()),
                );
            }
            if let Some(failure) = download_failure {
                column = column.child(
                    div()
                        .text_size(px(12.))
                        .text_color(error_color)
                        .child(failure),
                );
            }
            Some(column.into_any_element())
        };

        // One toolbar: find and replace reads like a search field, the AI
        // check sits beside it. Editing is the list itself -- click a line.
        let quick_actions = has_captions.then(|| {
            div()
                .flex()
                .gap(px(8.))
                .child(
                    ui::EditorButton::plain(&theme, "caption-find-replace")
                        .outlined(&theme)
                        .grow()
                        .start()
                        .height(px(32.))
                        .left_icon("icons/search.svg")
                        .icon_size(px(14.))
                        .label(tr::editor::tabs::captions::find_replace())
                        .disabled(generating)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_transcript_search(window, cx)
                        })),
                )
                .child(self.render_caption_review_button(cx))
                .into_any_element()
        });
        let regenerate = (has_captions && model_downloaded && has_audio).then(|| {
            ui::EditorButton::plain(&theme, "caption-generate")
                .tinted(&theme)
                .height(px(32.))
                .left_icon("icons/refresh-cw.svg")
                .label(if generating {
                    tr::editor::tabs::captions::generating()
                } else {
                    tr::editor::workbench::recognize_again()
                })
                .disabled(generating || any_deleting)
                .on_click(
                    cx.listener(|this, _, window, cx| this.generate_transcript(true, window, cx)),
                )
                .into_any_element()
        });
        let resplit = wants_resplit.then(|| {
            ui::EditorButton::plain(&theme, "caption-resplit")
                .tinted(&theme)
                .height(px(32.))
                .label(if resplitting {
                    tr::editor::tabs::captions::resplitting()
                } else {
                    tr::editor::tabs::captions::resplit()
                })
                .disabled(generating || resplitting)
                .on_click(
                    cx.listener(|this, _, window, cx| this.resplit_captions(false, window, cx)),
                )
                .into_any_element()
        });
        let model_status = model_downloaded.then(|| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(8.))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.))
                        .min_w_0()
                        .child(
                            svg()
                                .path("icons/check.svg")
                                .size(px(14.))
                                .flex_shrink_0()
                                .text_color(Hsla::from(theme.editor.text_3)),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(Hsla::from(theme.editor.text_2))
                                .truncate()
                                .child(SharedString::from(if cloud {
                                    tr::editor::tabs::captions::cloud_ready().to_string()
                                } else {
                                    tr::editor::tabs::captions::model_downloaded((model.label)())
                                })),
                        ),
                )
                .child(if cloud {
                    ui::EditorButton::plain(&theme, "caption-cloud-settings")
                        .tinted(&theme)
                        .height(px(26.))
                        .text_size(px(12.))
                        .left_icon("icons/settings.svg")
                        .icon_size(px(13.))
                        .label(tr::editor::tabs::captions::cloud_settings())
                        .on_click(|_, _, cx| {
                            cx.defer(|cx| {
                                crate::app_windows::open_ai_settings(
                                    crate::settings_ai::AiTab::Recognition,
                                    cx,
                                )
                            });
                        })
                        .into_any_element()
                } else {
                    ui::EditorButton::plain(&theme, "caption-delete")
                        .tinted(&theme)
                        .height(px(26.))
                        .text_size(px(12.))
                        .left_icon("icons/trash.svg")
                        .icon_size(px(13.))
                        .label(if deleting {
                            tr::editor::tabs::captions::deleting()
                        } else {
                            tr::editor::tabs::captions::delete()
                        })
                        .disabled(generating || is_downloading || deleting)
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.delete_caption_model(cx);
                        }))
                        .into_any_element()
                })
                .into_any_element()
        });

        let (setup_core, setup_more) = if has_captions {
            (None, setup)
        } else {
            (setup, None)
        };

        let transcription = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                ui::Field::inline(&theme, tr::editor::tabs::captions::model_label())
                    .child(div().w(px(190.)).child(model_trigger)),
            )
            .children(cloud.then(|| {
                div()
                    .text_size(px(12.))
                    .text_color(Hsla::from(theme.editor.text_2))
                    .child(tr::settings::ai::default_cloud_note())
            }))
            .child(
                ui::Field::inline(&theme, tr::editor::tabs::captions::language_label()).child(
                    div().w(px(150.)).child(self.menu_select(
                        SidebarMenu::CaptionLanguage,
                        "caption-language",
                        language_label,
                        cx,
                    )),
                ),
            )
            .children(model_status)
            .children(setup_more);

        let position = has_captions.then(|| {
            ui::Field::inline(&theme, tr::editor::tabs::captions::position())
                .child(div().w(px(150.)).child(self.menu_select(
                    SidebarMenu::CaptionPosition,
                    "caption-position",
                    position_label,
                    cx,
                )))
                .into_any_element()
        });
        if has_captions && self.sidebar.caption_tab == crate::editor_workbench::CaptionTab::Style {
            return div()
                .flex()
                .flex_col()
                .gap(px(16.))
                .child(self.render_caption_style_choices(&settings, cx))
                .children(position)
                .into_any_element();
        }
        if has_captions
            && self.sidebar.caption_tab == crate::editor_workbench::CaptionTab::Translation
        {
            return self.render_caption_translation(cx).into_any_element();
        }
        let body = if has_captions {
            div()
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .children(quick_actions)
                        .child(self.render_caption_review(cx)),
                )
                .child(self.render_caption_content_list(cx))
                .child(self.more_settings("captions", cx, move |_, cx| {
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(12.))
                        .children(regenerate)
                        .children(resplit)
                        .child(
                            ui::EditorButton::plain(&theme, "caption-text-video-editor")
                                .tinted(&theme)
                                .left_icon("icons/scissors.svg")
                                .label(tr::editor::workbench::text_video_edit())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_text_video_editor(window, cx)
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(Hsla::from(theme.editor.text_2))
                                .child(tr::editor::workbench::video_edit_hint()),
                        )
                        .child(transcription)
                        .into_any_element()
                }))
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(Hsla::from(theme.editor.text_2))
                        .child(tr::editor::workbench::caption_empty()),
                )
                .child(transcription)
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .children(self.render_recording_setup(cx))
            .children(self.render_recording_captions(cx))
            .children(setup_core)
            .children(status_line)
            .child(body)
            .into_any_element()
    }

    pub(crate) fn render_caption_style_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let settings = caption_settings(&self.project);
        let font_label = font_option_label(&settings.font);
        let animation_label = CAPTION_ANIMATIONS
            .iter()
            .find(|(value, _)| *value == settings.animation)
            .map_or(
                tr::editor::tabs::captions::animation::bounce(),
                |(_, label)| label(),
            );
        let highlight_label = CAPTION_HIGHLIGHT_STYLES
            .iter()
            .find(|(value, _)| *value == settings.highlight_style)
            .map_or(
                tr::editor::tabs::captions::highlight::color(),
                |(_, label)| label(),
            );

        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(
                ui::Field::section(&theme, tr::editor::tabs::captions::font_settings()).child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(12.))
                        .child(self.caption_font_controls(
                            tr::editor::tabs::captions::source_style(),
                            SidebarMenu::CaptionFont,
                            "caption-font",
                            font_label,
                            CaptionSlider::Size,
                            cx,
                        ))
                        .child(self.caption_font_controls(
                            tr::editor::tabs::captions::translation_style(),
                            SidebarMenu::CaptionTranslationFont,
                            "caption-translation-font",
                            font_option_label(&settings.resolved_translation_style().font),
                            CaptionSlider::TranslationSize,
                            cx,
                        ))
                        .children(
                            (settings.display_mode == CaptionDisplayMode::Bilingual).then(|| {
                                self.slider_field(
                                    tr::editor::tabs::captions::translation_gap(),
                                    SliderKey::Caption(CaptionSlider::TranslationGap),
                                    "px",
                                    cx,
                                )
                            }),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(8.))
                                .child(
                                    ui::Subfield::plain(
                                        &theme,
                                        tr::editor::tabs::captions::active_word_highlight(),
                                    )
                                    .child(
                                        ui::Toggle::plain(
                                            &theme,
                                            "caption-active-word",
                                            settings.active_word_highlight,
                                        )
                                        .on_click(
                                            cx.listener(|this, _, window, cx| {
                                                let next = !caption_settings(&this.project)
                                                    .active_word_highlight;
                                                this.set_caption_setting(
                                                    "caption-active-word",
                                                    window,
                                                    cx,
                                                    move |settings| {
                                                        settings.active_word_highlight = next
                                                    },
                                                );
                                            }),
                                        ),
                                    ),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(Hsla::from(theme.gray_10))
                                        .child(tr::editor::tabs::captions::highlight_note()),
                                ),
                        )
                        .children(settings.active_word_highlight.then(|| {
                            self.labelled_inline(
                                tr::editor::tabs::captions::highlight_style(),
                                self.menu_select(
                                    SidebarMenu::CaptionHighlightStyle,
                                    "caption-highlight-style",
                                    highlight_label,
                                    cx,
                                ),
                            )
                        })),
                ),
            )
            .child(
                ui::Field::section(&theme, tr::editor::tabs::captions::background_settings())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(12.))
                            .child(caption_toggle(
                                &theme,
                                tr::editor::tabs::captions::show_background(),
                                "caption-background",
                                settings.background_opacity > 0,
                                cx,
                                |settings| {
                                    settings.background_opacity = if settings.background_opacity > 0
                                    {
                                        0
                                    } else {
                                        90
                                    };
                                },
                            ))
                            .when(settings.background_opacity > 0, |this| {
                                this.child(self.labelled(
                                    tr::editor::tabs::captions::background_color(),
                                    self.render_hex_field(
                                        crate::editor_panels::FieldKey::CaptionBackground,
                                        &settings.background_color,
                                        cx,
                                    ),
                                ))
                                .child(
                                    self.slider_field(
                                        tr::editor::tabs::captions::background_opacity(),
                                        SliderKey::Caption(CaptionSlider::BackgroundOpacity),
                                        "%",
                                        cx,
                                    )
                                    .into_any_element(),
                                )
                            }),
                    ),
            )
            .children(
                (settings.position != "manual" && !settings.position.starts_with("top")).then(
                    || {
                        let control = if settings.bottom_margin.is_some() {
                            self.slider_field(
                                tr::editor::tabs::captions::bottom_margin(),
                                SliderKey::Caption(CaptionSlider::BottomMargin),
                                "px",
                                cx,
                            )
                            .into_any_element()
                        } else {
                            ui::Field::inline(&theme, tr::editor::tabs::captions::bottom_margin())
                                .child(
                                    ui::Button::plain(
                                        &theme,
                                        "caption-adjust-bottom-margin",
                                        ui::ButtonVariant::Gray,
                                        ui::ButtonSize::Sm,
                                    )
                                    .label(tr::editor::tabs::captions::adjust_bottom_margin())
                                    .on_click(cx.listener(
                                        |this, _, window, cx| {
                                            this.apply_caption_slider(
                                                CaptionSlider::BottomMargin,
                                                60.,
                                                window,
                                                cx,
                                            );
                                        },
                                    )),
                                )
                                .into_any_element()
                        };
                        div().flex().flex_col().gap(px(8.)).child(control).child(
                            div()
                                .text_size(px(12.))
                                .text_color(Hsla::from(theme.gray_10))
                                .child(tr::editor::tabs::captions::spacing_scale_note()),
                        )
                    },
                ),
            )
            .children((settings.position == "manual").then(|| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(self.caption_position_row(
                        tr::editor::tabs::captions::horizontal_position(),
                        CaptionSlider::ManualX,
                        crate::editor_panels::FieldKey::CaptionManualX,
                        cx,
                    ))
                    .child(self.caption_position_row(
                        tr::editor::tabs::captions::vertical_position(),
                        CaptionSlider::ManualY,
                        crate::editor_panels::FieldKey::CaptionManualY,
                        cx,
                    ))
                    .child(
                        div().flex().flex_row().justify_end().child(
                            ui::Button::plain(
                                &theme,
                                "caption-reset-position",
                                ui::ButtonVariant::Gray,
                                ui::ButtonSize::Sm,
                            )
                            .label(tr::editor::tabs::captions::reset_position())
                            .disabled(caption_at_default_center(&settings))
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.set_caption_placement(
                                        window,
                                        cx,
                                        reset_caption_manual_center,
                                    );
                                },
                            )),
                        ),
                    )
            }))
            .child(
                ui::Field::section(&theme, tr::editor::tabs::captions::animation_section()).child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(12.))
                        .child(self.labelled_inline(
                            tr::editor::tabs::captions::animation_style(),
                            self.menu_select(
                                SidebarMenu::CaptionAnimation,
                                "caption-animation",
                                animation_label,
                                cx,
                            ),
                        ))
                        .child(self.labelled(
                            tr::editor::tabs::captions::highlight_color(),
                            self.render_hex_field(
                                crate::editor_panels::FieldKey::CaptionHighlight,
                                &settings.highlight_color,
                                cx,
                            ),
                        ))
                        .child(self.labelled_readout(
                            tr::editor::tabs::captions::fade_duration(),
                            SliderKey::Caption(CaptionSlider::FadeDuration),
                            format!("{:.0}ms", settings.fade_duration * 1000.),
                            cx,
                        )),
                ),
            )
            .into_any_element()
    }

    fn caption_font_controls(
        &self,
        title: &'static str,
        menu: SidebarMenu,
        id: &'static str,
        font_label: SharedString,
        size: CaptionSlider,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        ui::Field::section(&self.theme, title)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(self.labelled_inline(
                        tr::editor::tabs::captions::font_family(),
                        self.menu_select(menu, id, font_label, cx),
                    ))
                    .child(self.slider_field(
                        tr::editor::tabs::captions::size_label(),
                        SliderKey::Caption(size),
                        "",
                        cx,
                    ))
                    .child(self.caption_appearance_controls(
                        menu == SidebarMenu::CaptionTranslationFont,
                        cx,
                    )),
            )
            .into_any_element()
    }

    fn caption_appearance_controls(&self, translation: bool, cx: &mut Context<Self>) -> AnyElement {
        use crate::editor_panels::FieldKey;
        let settings = caption_settings(&self.project);
        let appearance = if translation {
            settings.resolved_translation_appearance()
        } else {
            settings.source_appearance()
        };
        let (color, outline, shadow) = if translation {
            (
                FieldKey::CaptionTranslationColor,
                FieldKey::CaptionTranslationOutline,
                FieldKey::CaptionTranslationShadow,
            )
        } else {
            (
                FieldKey::CaptionColor,
                FieldKey::CaptionOutline,
                FieldKey::CaptionShadow,
            )
        };
        let weight = TEXT_WEIGHTS
            .iter()
            .find(|(value, _)| *value == appearance.font_weight)
            .map_or(tr::editor::tabs::weight::bold(), |(_, label)| label());
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(self.labelled_inline(
                tr::editor::tabs::captions::font_weight(),
                self.menu_select(
                    if translation {
                        SidebarMenu::CaptionTranslationWeight
                    } else {
                        SidebarMenu::CaptionWeight
                    },
                    if translation {
                        "caption-translation-weight"
                    } else {
                        "caption-weight"
                    },
                    weight,
                    cx,
                ),
            ))
            .child(self.labelled(
                tr::editor::tabs::captions::text_color(),
                self.render_hex_field(color, &appearance.color, cx),
            ))
            .child(caption_toggle(
                &self.theme,
                tr::editor::tabs::captions::uppercase(),
                if translation {
                    "caption-translation-uppercase"
                } else {
                    "caption-uppercase"
                },
                appearance.uppercase,
                cx,
                if translation {
                    |s| s.edit_translation_appearance(|a| a.uppercase = !a.uppercase)
                } else {
                    |s| s.edit_source_appearance(|a| a.uppercase = !a.uppercase)
                },
            ))
            .child(caption_toggle(
                &self.theme,
                tr::editor::tabs::captions::outline(),
                if translation {
                    "caption-translation-outline"
                } else {
                    "caption-outline"
                },
                appearance.outline,
                cx,
                if translation {
                    |s| s.edit_translation_appearance(|a| a.outline = !a.outline)
                } else {
                    |s| s.edit_source_appearance(|a| a.outline = !a.outline)
                },
            ))
            .children(appearance.outline.then(|| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(self.labelled(
                        tr::editor::tabs::captions::outline_color(),
                        self.render_hex_field(outline, &appearance.outline_color, cx),
                    ))
                    .child(self.slider_field(
                        tr::editor::tabs::captions::outline_width(),
                        SliderKey::Caption(if translation {
                            CaptionSlider::TranslationOutlineWidth
                        } else {
                            CaptionSlider::OutlineWidth
                        }),
                        "%",
                        cx,
                    ))
            }))
            .child(caption_toggle(
                &self.theme,
                tr::editor::tabs::captions::shadow(),
                if translation {
                    "caption-translation-shadow"
                } else {
                    "caption-shadow"
                },
                appearance.shadow,
                cx,
                if translation {
                    |s| s.edit_translation_appearance(|a| a.shadow = !a.shadow)
                } else {
                    |s| s.edit_source_appearance(|a| a.shadow = !a.shadow)
                },
            ))
            .children(appearance.shadow.then(|| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(self.labelled(
                        tr::editor::tabs::captions::shadow_color(),
                        self.render_hex_field(shadow, &appearance.shadow_color, cx),
                    ))
                    .child(self.slider_field(
                        tr::editor::tabs::captions::shadow_opacity(),
                        SliderKey::Caption(if translation {
                            CaptionSlider::TranslationShadowOpacity
                        } else {
                            CaptionSlider::ShadowOpacity
                        }),
                        "%",
                        cx,
                    ))
                    .child(self.slider_field(
                        tr::editor::tabs::captions::shadow_size(),
                        SliderKey::Caption(if translation {
                            CaptionSlider::TranslationShadowSize
                        } else {
                            CaptionSlider::ShadowSize
                        }),
                        "%",
                        cx,
                    ))
            }))
            .into_any_element()
    }

    fn caption_position_row(
        &self,
        name: impl Into<SharedString>,
        slider: CaptionSlider,
        field: crate::editor_panels::FieldKey,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        ui::Field::inline(&self.theme, name)
            .child(self.slider_flex(SliderKey::Caption(slider), "%", cx))
            .value(self.render_number_field(field, "%", 56.))
            .into_any_element()
    }

    fn render_caption_style_choices(
        &self,
        settings: &CaptionSettings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let choices = div().grid().grid_cols(2).gap(px(8.)).children(
            QUICK_CAPTION_PRESETS
                .iter()
                .enumerate()
                .map(|(index, preset)| {
                    let selected = settings.preset == preset.id;
                    div()
                        .id(SharedString::from(format!("caption-style-{}", preset.id)))
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .p(px(8.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(Hsla::from(if selected {
                            theme.blue_9
                        } else {
                            theme.gray_4
                        }))
                        .bg(self.panel_bg())
                        .cursor_pointer()
                        .tab_index(0)
                        .hover(|style| style.border_color(Hsla::from(theme.blue_9)))
                        .child(self.caption_outline_preview(preset))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(4.))
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .child(div().flex_1().child((preset.label)()))
                                .children(selected.then(|| {
                                    svg()
                                        .path("icons/check.svg")
                                        .size(px(14.))
                                        .text_color(Hsla::from(theme.blue_9))
                                })),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.edit_project_global(
                                "caption-outline-preset",
                                window,
                                cx,
                                move |project| {
                                    let settings = &mut project
                                        .captions
                                        .get_or_insert_with(Default::default)
                                        .settings;
                                    QUICK_CAPTION_PRESETS[index].apply(settings);
                                    true
                                },
                            );
                        }))
                }),
        );
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(choices)
            .child(crate::editor_sidebar::disclosure_row(
                &theme,
                "caption-more-styles",
                tr::editor::tabs::captions::more_styles(),
                self.sidebar.caption_more_styles,
                cx.listener(|this, _, _, cx| {
                    this.sidebar.caption_more_styles = !this.sidebar.caption_more_styles;
                    this.invalidate_sidebar(cx);
                }),
            ))
            .children(
                self.sidebar
                    .caption_more_styles
                    .then(|| self.render_caption_presets(settings, cx)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(self.render_style_presets_bar(
                                crate::style_presets::PresetKind::Captions,
                                &[0],
                                cx,
                            )),
                    )
                    .child(
                        ui::EditorButton::plain(&theme, "caption-style-edit")
                            .tinted(&theme)
                            .height(px(32.))
                            .left_icon("icons/pencil.svg")
                            .label(tr::editor::workbench::adjust_style())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_caption_style_editor(None, window, cx)
                            })),
                    ),
            )
            .into_any_element()
    }

    pub(crate) fn apply_quick_caption_preset(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(preset) = QUICK_CAPTION_PRESETS.get(index) else {
            return;
        };
        if caption_settings(&self.project).preset == preset.id {
            return;
        }
        self.edit_project_global("caption-outline-preset", window, cx, move |project| {
            preset.apply(
                &mut project
                    .captions
                    .get_or_insert_with(Default::default)
                    .settings,
            );
            true
        });
    }

    fn caption_outline_preview(&self, preset: &QuickCaptionPreset) -> AnyElement {
        let mut settings = caption_settings(&self.project);
        preset.apply(&mut settings);
        self.caption_style_preview(&settings)
    }

    pub(crate) fn caption_style_preview(&self, settings: &CaptionSettings) -> AnyElement {
        let settings = settings.clone();
        div()
            .h(px(64.))
            .w_full()
            .rounded(px(5.))
            .overflow_hidden()
            .bg(gpui::rgb(0x9299a2))
            .child(
                gpui::canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        let parse_color = |value: &str, fallback: Hsla| {
                            crate::editor_sidebar::hex_to_rgb(value)
                                .map(|rgba| Hsla::from(gpui::rgba(u32::from_be_bytes(rgba))))
                                .unwrap_or(fallback)
                        };
                        let color = parse_color(&settings.color, gpui::white());
                        let mut background = parse_color(&settings.background_color, gpui::black());
                        background.a = settings.background_opacity.min(100) as f32 / 100.;
                        let lines = caption_preview_lines(&settings);
                        let samples = lines
                            .iter()
                            .map(|line| {
                                let sample = if line.translation {
                                    tr::editor::tabs::captions::preview_translation()
                                } else {
                                    tr::editor::tabs::captions::preview_original()
                                };
                                if line.appearance.uppercase {
                                    sample.to_uppercase()
                                } else {
                                    sample.to_string()
                                }
                            })
                            .collect::<Vec<_>>();
                        let shape = |index: usize,
                                     scale: f32,
                                     color: Hsla,
                                     effects: bool,
                                     window: &Window| {
                            let line = &lines[index];
                            let text = &samples[index];
                            let mut font = gpui::font(SharedString::from(line.style.font.clone()));
                            font.weight = FontWeight(line.appearance.font_weight as f32);
                            let mut runs = vec![gpui::TextRun {
                                len: text.len(),
                                font,
                                color,
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            }];
                            if effects && !line.translation && settings.active_word_highlight {
                                let start =
                                    text.rfind(char::is_whitespace).map_or(0, |index| index + 1);
                                let mut active = runs[0].clone();
                                active.len -= start;
                                let highlight = parse_color(&settings.highlight_color, color);
                                if settings.highlight_style == "pill" {
                                    active.background_color = Some(highlight);
                                } else {
                                    active.color = highlight;
                                }
                                runs[0].len = start;
                                if start == 0 {
                                    runs.clear();
                                }
                                runs.push(active);
                            }
                            window.text_system().shape_line(
                                text.clone().into(),
                                px(line.size * scale),
                                &runs,
                                None,
                            )
                        };
                        let widths = (0..lines.len())
                            .map(|index| f32::from(shape(index, 1., color, false, window).width))
                            .collect::<Vec<_>>();
                        let layout = caption_preview_layout(
                            &settings,
                            &lines,
                            &widths,
                            f32::from(bounds.size.width),
                            f32::from(bounds.size.height),
                        );
                        let origin = bounds.origin
                            + gpui::point(
                                px((f32::from(bounds.size.width) - layout.width) / 2.),
                                px((f32::from(bounds.size.height) - layout.height) / 2.),
                            );
                        window.paint_quad(
                            gpui::fill(
                                Bounds::new(
                                    origin,
                                    gpui::size(px(layout.width), px(layout.height)),
                                ),
                                background,
                            )
                            .corner_radii(px(layout.radius)),
                        );
                        let mut top = origin.y + px(layout.padding);
                        for (index, line) in lines.iter().enumerate() {
                            let appearance = &line.appearance;
                            let color = parse_color(&appearance.color, gpui::white());
                            let outline = parse_color(&appearance.outline_color, gpui::black());
                            let glyphs = shape(index, layout.scale, color, true, window);
                            let line_height = px(line.size * 1.2 * layout.scale);
                            let position =
                                gpui::point(origin.x + (px(layout.width) - glyphs.width) / 2., top);
                            if appearance.shadow {
                                let mut shadow_color =
                                    parse_color(&appearance.shadow_color, gpui::black());
                                shadow_color.a *= appearance.shadow_opacity.min(100) as f32 / 100.;
                                let shadow =
                                    shape(index, layout.scale, shadow_color, false, window);
                                let offset = px(line.size
                                    * layout.scale
                                    * appearance.normalized_shadow_size()
                                    / 100.);
                                let _ = shadow.paint(
                                    position + gpui::point(offset, offset),
                                    line_height,
                                    gpui::TextAlign::Left,
                                    None,
                                    window,
                                    cx,
                                );
                            }
                            if appearance.outline {
                                let stroke = line.size
                                    * layout.scale
                                    * appearance.normalized_outline_width()
                                    / 100.;
                                let stroked = shape(index, layout.scale, outline, false, window);
                                for (x, y) in [
                                    (-1., -1.),
                                    (0., -1.),
                                    (1., -1.),
                                    (-1., 0.),
                                    (1., 0.),
                                    (-1., 1.),
                                    (0., 1.),
                                    (1., 1.),
                                ] {
                                    let _ = stroked.paint(
                                        position + gpui::point(px(x * stroke), px(y * stroke)),
                                        line_height,
                                        gpui::TextAlign::Left,
                                        None,
                                        window,
                                        cx,
                                    );
                                }
                            }
                            let _ = glyphs.paint(
                                position,
                                line_height,
                                gpui::TextAlign::Left,
                                None,
                                window,
                                cx,
                            );
                            top += line_height + px(layout.gap);
                        }
                    },
                )
                .size_full(),
            )
            .into_any_element()
    }

    fn render_caption_presets(
        &self,
        settings: &CaptionSettings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let active = settings.preset.clone();

        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap(px(8.))
            .children(CAPTION_PRESETS.iter().enumerate().map(|(index, preset)| {
                let selected = active == preset.id;
                div()
                    .id(SharedString::from(format!("caption-preset-{}", preset.id)))
                    // Two across the 382px column with `gap-2`.
                    .w(px(187.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .p(px(6.))
                    .rounded(px(8.))
                    .bg(self.panel_bg())
                    .border_1()
                    .border_color(if selected {
                        Hsla::from(theme.blue_9)
                    } else {
                        Hsla::from(theme.gray_3)
                    })
                    // `ring-1 ring-blue-9`, painted behind an opaque card.
                    .when(selected, |this| {
                        this.shadow(vec![gpui::BoxShadow {
                            color: Hsla::from(theme.blue_9),
                            offset: gpui::point(px(0.), px(0.)),
                            blur_radius: px(0.),
                            spread_radius: px(1.),
                            inset: false,
                        }])
                    })
                    .child(self.caption_preset_preview(preset))
                    .child(
                        div()
                            .px(px(2.))
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(Hsla::from(theme.gray_12))
                            .child((preset.label)()),
                    )
                    .tooltip({
                        let description = SharedString::from((preset.description)());
                        move |_window, cx| ui::Tooltip::new(&theme, description.clone()).view(cx)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.apply_caption_preset(index, window, cx);
                    }))
            }))
            .into_any_element()
    }

    fn caption_preset_preview(&self, preset: &CaptionPreset) -> AnyElement {
        let mut settings = caption_settings(&self.project);
        preset.apply(&mut settings);
        self.caption_style_preview(&settings)
    }

    fn apply_caption_preset(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(preset) = CAPTION_PRESETS.get(index) else {
            return;
        };
        self.edit_project_global("caption-preset", window, cx, move |project| {
            // Unlike the keyboard tab, the source's caption setters bail when
            // there is no captions block. `applyCaptionPreset` goes through the
            // same setter, so it does too.
            let Some(captions) = project.captions.as_mut() else {
                return false;
            };
            let settings = &mut captions.settings;
            preset.apply(settings);
            true
        });
    }

    // -- Transcription glue ---------------------------------------------------

    pub(crate) fn selected_caption_model(&self) -> &'static CaptionModel {
        let models = available_caption_models();
        models
            .iter()
            .copied()
            .find(|model| model.name == self.sidebar.caption_model)
            .unwrap_or(models[0])
    }

    /// The Captions tab's `onMount` (`CaptionsTab.tsx:557-615`): scan the
    /// model folder, restore the persisted model/language selection, and --
    /// when a download started by an earlier editor window is still streaming
    /// -- reattach the progress poller. Deferred out of render because it
    /// mutates the sidebar state, and keyed by entity so it runs once per
    /// window.
    pub(crate) fn ensure_captions_init(&self, cx: &mut Context<Self>) {
        static INITIALISED: LazyLock<Mutex<HashSet<EntityId>>> =
            LazyLock::new(|| Mutex::new(HashSet::new()));
        if !INITIALISED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(cx.entity().entity_id())
        {
            return;
        }

        let entity = cx.entity();
        cx.defer(move |cx: &mut gpui::App| {
            entity.update(cx, |this, cx| {
                transcription::refresh_downloaded_models();
                migrate_caption_model_selection();

                let section = store::store_section(transcription::GPUI_STORE_SECTION);
                if let Some(saved) = section
                    .get(transcription::SELECTED_MODEL_KEY)
                    .and_then(Value::as_str)
                    && let Some(saved_model) = available_caption_models()
                        .iter()
                        .find(|model| model.name == saved)
                {
                    this.sidebar.caption_model = saved_model.name;
                }
                if let Some(saved) = section
                    .get(transcription::SELECTED_LANGUAGE_KEY)
                    .and_then(Value::as_str)
                    && let Some((code, _)) =
                        CAPTION_LANGUAGES.iter().find(|(code, _)| *code == saved)
                {
                    this.sidebar.caption_language = code;
                }

                if transcription::download_active() {
                    this.spawn_caption_download_poll(cx);
                }
                cx.notify();
            });
        });
    }

    /// Repaints the tab while a download streams -- the stand-in for Tauri's
    /// `DownloadProgress` events plus the 1s status poll
    /// (`CaptionsTab.tsx:550-555`); the hub already carries the numbers.
    fn spawn_caption_download_poll(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                let active = transcription::download_active();
                if this
                    .update(cx, |this, cx| this.repaint_caption_download(cx))
                    .is_err()
                {
                    break;
                }
                if !active {
                    break;
                }
            }
        })
        .detach();
    }

    /// One tick of [`Self::spawn_caption_download_poll`]. Only the captions
    /// tab reads the download, so only the sidebar is rebuilt.
    pub(crate) fn repaint_caption_download(&mut self, cx: &mut Context<Self>) {
        self.invalidate_sidebar(cx);
        if self.transcript.open {
            self.invalidate_transcript(cx);
        }
    }

    /// `downloadModel` (`CaptionsTab.tsx:643-690`). The download runs on the
    /// tokio runtime and survives this window; the poll task repaints.
    pub(crate) fn start_caption_model_download(&mut self, cx: &mut Context<Self>) {
        let model = self.selected_caption_model().name;
        if !transcription::begin_download(model) {
            return;
        }
        gpui_tokio::Tokio::spawn(cx, transcription::run_model_download(model.to_string())).detach();
        self.spawn_caption_download_poll(cx);
        cx.notify();
    }

    /// `deleteModel` (`CaptionsTab.tsx:692-713`).
    fn delete_caption_model(&mut self, cx: &mut Context<Self>) {
        let model = self.selected_caption_model().name;
        if !transcription::begin_delete(model) {
            return;
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move { transcription::run_model_delete(model) })
                .await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    /// `generateCaptions` (`CaptionsTab.tsx:715-757`): transcribe on the
    /// tokio blocking pool, then apply the result through one `edit_project`
    /// write so a regenerate is a single undo entry. The transcript panel
    /// passes `show_captions: false`: transcribing to edit by text must not
    /// switch subtitles on.
    pub(crate) fn generate_transcript(
        &mut self,
        show_captions: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_caption_export_wait(cx);
        self.generate_transcript_with_model(
            show_captions,
            self.selected_caption_model().name,
            self.sidebar.caption_language,
            false,
            window,
            cx,
        );
    }

    pub(crate) fn generate_transcript_with_model(
        &mut self,
        show_captions: bool,
        model: &'static str,
        language: &'static str,
        automatic: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(summary) = self.summary() else {
            return;
        };
        let recording_durations = summary.clip_display_durations.clone();
        let recording_duration = summary.recording_duration;
        let timeline = self.project.timeline.clone();
        let transcribed_ranges =
            transcription_source_ranges(timeline.as_ref(), &recording_durations);
        let project_path = self.project_path.clone();
        if !transcription::begin_generation(&project_path) {
            return;
        }
        self.cancel_caption_processing(cx);
        let initial_content = crate::caption_review::caption_content_snapshot(&self.project);
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let task_path = project_path.clone();
            let task_durations = recording_durations.clone();
            let task = gpui_tokio::Tokio::spawn(cx, async move {
                tokio::task::spawn_blocking(move || {
                    transcription::transcribe_blocking(
                        &task_path,
                        model,
                        language,
                        timeline.as_ref(),
                        &task_durations,
                    )
                })
                .await
                .map_err(|error| format!("Transcription task panicked: {error}"))?
            });
            let result = match task.await {
                Ok(result) => result,
                Err(error) => Err(format!("Transcription task failed: {error}")),
            };

            if automatic && !crate::store::GeneralSettings::load().auto_captions_after_recording {
                let skipped = crate::recording_autocaptions::skip_request(&project_path);
                if let Err(error) = skipped {
                    let _ = this.update(cx, |this, cx| {
                        this.caption_processing_save_failed(error, cx);
                    });
                }
            }
            let result = crate::recording_autocaptions::guard_generation_outcome(
                &project_path,
                automatic,
                false,
                result,
            );
            let result = match result {
                Ok(Some(transcript)) => this
                    .update_in(cx, |this, window, cx| {
                        let Some(mut transcript) =
                            crate::recording_autocaptions::guard_generation_outcome(
                                &project_path,
                                automatic,
                                false,
                                Ok(transcript),
                            )?
                        else {
                            return Ok(false);
                        };
                        if automatic && crate::recording_autocaptions::has_captions(&this.project) {
                            return Ok(false);
                        }
                        if crate::caption_review::caption_content_snapshot(&this.project)
                            != initial_content
                        {
                            return Err(tr::editor::review::changed().to_string());
                        }
                        let current_ranges = transcription_source_ranges(
                            this.project.timeline.as_ref(),
                            &recording_durations,
                        );
                        if !transcription_ranges_cover(&transcribed_ranges, &current_ranges) {
                            return Err(
                                "The timeline changed during generation. Generate captions again."
                                    .to_string(),
                            );
                        }
                        transcript.refinement.requested_manually = !automatic;
                        this.apply_generated_captions(
                            transcript,
                            &recording_durations,
                            recording_duration,
                            show_captions,
                            window,
                            cx,
                        )?;
                        this.flush_pending_saves(cx)?;
                        this.activate_caption_processing(cx)?;
                        if !automatic {
                            crate::recording_autocaptions::accept_manual_generation(&project_path);
                        }
                        Ok(true)
                    })
                    .unwrap_or_else(|_| {
                        Err(tr::editor::tabs::captions::auto_draft::interrupted().to_string())
                    }),
                Ok(None) => Ok(false),
                Err(error) => Err(error),
            };
            let outcome = crate::recording_autocaptions::finish_generation_outcome(
                &project_path,
                automatic,
                result,
            );
            let _ = this.update_in(cx, |this, window, cx| {
                if matches!(outcome, Ok(true)) {
                    match crate::recording_autocaptions::guard_generation_outcome(
                        &project_path,
                        automatic,
                        true,
                        Ok(()),
                    ) {
                        Ok(Some(())) => this.refine_caption_timings(window, cx),
                        Ok(None) => {}
                        Err(error) => this.caption_processing_save_failed(error, cx),
                    }
                }
                this.invalidate_sidebar(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Runs a pending word-timing refinement for this project in the
    /// background, then swaps the refined times in. Started after a
    /// transcript is shown and again whenever the project opens, so an
    /// interrupted refinement resumes; a finished one leaves no request.
    pub(crate) fn refine_caption_timings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.project_ready() {
            return;
        }
        if self.activate_caption_processing(cx).is_err() {
            return;
        }
        let project_path = self.project_path.clone();
        let request = match transcription::refinement_request_checked(&project_path) {
            Ok(Some(request)) => request,
            Ok(None) => {
                self.continue_caption_processing(window, cx);
                return;
            }
            Err(error) => {
                self.caption_processing_save_failed(error, cx);
                return;
            }
        };
        match crate::recording_autocaptions::refinement_allowed(&project_path, &request) {
            Ok(true) => {}
            Ok(false) => return,
            Err(error) => {
                self.caption_processing_save_failed(error, cx);
                return;
            }
        }
        // Cloud services and Qwen3-ASR have no model here to refine word times with.
        if crate::asr_cloud::is_cloud_model(&request.model)
            || request.model == crate::local_qwen::MODEL
        {
            transcription::clear_refinement_request(&project_path);
            self.continue_caption_processing(window, cx);
            return;
        }
        let Some(input) = self
            .project
            .captions
            .as_ref()
            .filter(|captions| captions.source_timed && !captions.segments.is_empty())
            .map(|captions| captions.segments.clone())
        else {
            transcription::clear_refinement_request(&project_path);
            self.continue_caption_processing(window, cx);
            return;
        };
        let recording_durations = self.clip_display_durations.clone();
        let Some(stop) = transcription::begin_refinement(&project_path) else {
            return;
        };
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let task_path = project_path.clone();
            let task_request = request.clone();
            let task_stop = stop.clone();
            let task = gpui_tokio::Tokio::spawn(cx, async move {
                let runtime = tokio::runtime::Handle::current();
                tokio::task::spawn_blocking(move || {
                    std::thread::Builder::new()
                        .name("caption-refinement".to_string())
                        .spawn(move || {
                            transcription::lower_thread_priority();
                            transcription::refine_blocking(
                                &task_path,
                                &task_request,
                                &input,
                                &recording_durations,
                                Some(runtime),
                                &task_stop,
                            )
                        })
                        .map_err(|error| format!("Failed to start caption refinement: {error}"))?
                        .join()
                        .map_err(|_| "Caption refinement panicked".to_string())?
                })
                .await
                .map_err(|error| format!("Caption refinement task failed: {error}"))?
            });
            let result = match task.await {
                Ok(result) => result,
                Err(error) => Err(format!("Caption refinement failed: {error}")),
            };
            let stopped = stop.load(std::sync::atomic::Ordering::Relaxed);
            match result {
                Ok(Some(refined)) if !stopped => {
                    let _ = this.update_in(cx, |this, window, cx| {
                        match crate::recording_autocaptions::refinement_allowed(
                            &project_path,
                            &request,
                        ) {
                            Ok(true) => {}
                            Ok(false) => return,
                            Err(error) => {
                                this.caption_processing_save_failed(error, cx);
                                return;
                            }
                        }
                        if transcription::refinement_request(&project_path).as_ref()
                            == Some(&request)
                        {
                            let before =
                                crate::caption_review::caption_content_snapshot(&this.project);
                            let mut next = this.project.clone();
                            transcription::apply_refined_captions(
                                &mut next,
                                &refined,
                                &this.clip_display_durations,
                            );
                            let saved = this
                                .checkpoint_caption_refinement(&before, &next, cx)
                                .and_then(|()| {
                                    this.land_caption_refinement(&refined, window, cx);
                                    this.resplit_captions(true, window, cx);
                                    this.flush_pending_saves(cx)
                                })
                                .and_then(|()| this.activate_caption_processing(cx));
                            match saved {
                                Ok(()) => transcription::clear_refinement_request(&project_path),
                                Err(error) => this.caption_processing_save_failed(error, cx),
                            }
                        }
                    });
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!("{error}");
                    if !stopped {
                        let _ = this.update_in(cx, |this, _, cx| {
                            match crate::recording_autocaptions::refinement_allowed(
                                &project_path,
                                &request,
                            ) {
                                Ok(true)
                                    if transcription::refinement_request(&project_path)
                                        .as_ref()
                                        == Some(&request) =>
                                {
                                    transcription::clear_refinement_request(&project_path);
                                }
                                Ok(_) => {}
                                Err(error) => this.caption_processing_save_failed(error, cx),
                            }
                        });
                    }
                }
            }
            transcription::finish_refinement(&project_path, &stop);
            let _ = this.update_in(cx, |this, window, cx| {
                if !stopped {
                    match crate::recording_autocaptions::refinement_allowed(&project_path, &request)
                    {
                        Ok(true) => this.continue_caption_processing(window, cx),
                        Ok(false) => {}
                        Err(error) => this.caption_processing_save_failed(error, cx),
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The success half of `generateCaptions` (`CaptionsTab.tsx:730-744`):
    /// `applyCaptionResultToProject` inside the project-write fan-out, then
    /// the caption track lane switched on.
    fn apply_generated_captions(
        &mut self,
        transcript: transcription::Transcript,
        recording_durations: &[f64],
        recording_duration: f64,
        show_captions: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let transcription::Transcript {
            segments,
            refinement,
        } = transcript;
        let mut next = self.project.clone();
        let shown = next
            .captions
            .as_ref()
            .is_some_and(|captions| captions.settings.enabled);
        crate::style_presets::seed_default_captions(&mut next);
        transcription::apply_caption_result(
            &mut next,
            segments,
            &refinement.language,
            recording_durations,
            recording_duration,
        );
        if !show_captions && let Some(captions) = next.captions.as_mut() {
            captions.settings.enabled = shown;
        }
        if let Some(captions) = next.captions.as_mut()
            && let Some(font) = chinese_caption_font(&captions.settings.font, &captions.segments)
        {
            captions.settings.set_source_font(font.to_string());
        }
        self.queue_caption_processing(&next, refinement, cx)?;
        self.edit_project_global("caption-generate", window, cx, move |project| {
            *project = next;
            true
        });
        if show_captions {
            self.tracks.caption = true;
        }
        self.resplit_captions(true, window, cx);
        cx.notify();
        Ok(())
    }

    // -- Sync offsets --------------------------------------------------------

    /// `SyncOffsetsConfig` (`:6081-6178`): per **recording** clip, not per
    /// timeline segment, which is why it lives with the audio settings rather
    /// than in a segment panel.
    fn render_sync_offsets(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = self.theme;
        let summary = self.summary()?;
        if !(summary.has_system_audio || summary.has_microphone || summary.has_camera) {
            return None;
        }
        let clips = summary.recording_clips.max(1);

        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(Hsla::from(theme.editor.text_2))
                                .child(tr::editor::tabs::sync::title()),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(Hsla::from(theme.editor.text_3))
                                .child(tr::editor::tabs::sync::description()),
                        ),
                )
                .children((0..clips).map(|clip| {
                    let auto = self
                        .project
                        .clips
                        .iter()
                        .find(|item| item.index as usize == clip)
                        .is_some_and(|item| item.offsets_auto_calculated);
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(14.))
                        .children((clips > 1).then(|| {
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(Hsla::from(theme.editor.text_2))
                                .child(SharedString::from(tr::editor::tabs::sync::clip(clip)))
                                .into_any_element()
                        }))
                        .children(auto.then(|| {
                            div()
                                .text_size(px(12.))
                                .text_color(Hsla::from(theme.editor.text_3))
                                .child(tr::editor::tabs::sync::auto_description())
                                .into_any_element()
                        }))
                        .children(summary.has_system_audio.then(|| {
                            self.render_offset_field(
                                clip,
                                OffsetKind::SystemAudio,
                                tr::editor::tabs::sync::system_audio(),
                                auto,
                                cx,
                            )
                        }))
                        .children(summary.has_microphone.then(|| {
                            self.render_offset_field(
                                clip,
                                OffsetKind::Mic,
                                tr::editor::tabs::sync::microphone(),
                                auto,
                                cx,
                            )
                        }))
                        .children(summary.has_camera.then(|| {
                            self.render_offset_field(
                                clip,
                                OffsetKind::Camera,
                                tr::editor::tabs::sync::camera(),
                                auto,
                                cx,
                            )
                        }))
                        .into_any_element()
                }))
                .into_any_element(),
        )
    }
}

/// Which of `ClipOffsets`' three a `SourceOffsetField` edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OffsetKind {
    SystemAudio,
    Mic,
    Camera,
}

impl OffsetKind {
    pub fn read(self, offsets: &cap_project::ClipOffsets) -> f32 {
        match self {
            Self::SystemAudio => offsets.system_audio,
            Self::Mic => offsets.mic,
            Self::Camera => offsets.camera,
        }
    }

    pub fn write(self, offsets: &mut cap_project::ClipOffsets, value: f32) {
        match self {
            Self::SystemAudio => offsets.system_audio = value,
            Self::Mic => offsets.mic = value,
            Self::Camera => offsets.camera = value,
        }
    }
}

/// The default caption block a project without one would need. Kept next to the
/// setters so the two stories -- "captions bail, keyboard creates" -- sit
/// together, and used by the test below that pins the asymmetry.
#[cfg(test)]
fn default_captions() -> CaptionsData {
    CaptionsData::default()
}

impl EditorWindow {
    /// `SourceOffsetField` (`ConfigSidebar.tsx:6179-6243`): a `NumberField`
    /// reading milliseconds, an `ms` label, and four nudge buttons.
    fn render_offset_field(
        &self,
        clip: usize,
        kind: OffsetKind,
        name: &'static str,
        auto: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let key = crate::editor_panels::FieldKey::SyncOffset(clip, kind);
        let current = self
            .project
            .clips
            .iter()
            .find(|item| item.index as usize == clip)
            .map_or(0., |item| f64::from(kind.read(&item.offsets)));

        let mut field = ui::Field::section(&theme, name);
        if auto {
            field = field.badge(tr::editor::tabs::sync::auto_synced());
        }

        field
            .child(
                // `flex flex-row justify-between items-center -mt-2 w-full`
                div()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .items_center()
                    .mt(px(-8.))
                    .w_full()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_end()
                            .gap(px(4.))
                            .child(self.render_number_field(key, "ms", 80.)),
                    )
                    .child(div().flex().flex_row().gap(px(4.)).children(
                        [-100_i32, -10, 10, 100].map(|delta| {
                            div()
                                .id(SharedString::from(format!(
                                    "offset-{clip}-{kind:?}-{delta}"
                                )))
                                .px(px(4.))
                                .py(px(2.))
                                .rounded(px(2.))
                                .border_1()
                                .border_color(Hsla::from(theme.editor.line))
                                .bg(Hsla::from(theme.gray_1))
                                .text_size(px(12.))
                                .text_color(Hsla::from(theme.gray_11))
                                .child(SharedString::from(format!(
                                    "{}{}ms",
                                    if delta > 0 { "+" } else { "-" },
                                    delta.abs()
                                )))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    // `rawValue() + v`, in milliseconds.
                                    let next = (current * 1000.).round() + f64::from(delta);
                                    this.set_clip_offset(clip, kind, next, window, cx);
                                }))
                        }),
                    )),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A left-aligned caption sits a margin in from the edge, its centre a
    /// half box width further in. Manual starts at that drawn centre, so
    /// switching does not move it.
    #[test]
    fn manual_caption_position_starts_at_the_drawn_caption() {
        let mut settings = CaptionSettings {
            position: "bottom-left".into(),
            ..CaptionSettings::default()
        };
        set_caption_position_from(&mut settings, "manual", |position| {
            (position == "bottom-left").then_some((0.16, 0.86))
        });
        assert_eq!(
            settings.manual_position.map(|xy| (xy.x, xy.y)),
            Some((0.16, 0.86))
        );

        // Nothing drawn yet: the preset's table entry.
        let mut settings = CaptionSettings {
            position: "top-right".into(),
            ..CaptionSettings::default()
        };
        set_caption_position_from(&mut settings, "manual", |_| None);
        assert_eq!(
            settings.manual_position.map(|xy| (xy.x, xy.y)),
            Some((0.95, 0.08))
        );
    }

    /// The caption on the preview frame wins over the renderer's last drawn
    /// one when it was drawn for the preset being left.
    #[test]
    fn manual_caption_position_starts_at_the_caption_on_screen() {
        let mut settings = CaptionSettings {
            position: "bottom-left".into(),
            ..CaptionSettings::default()
        };
        let on_screen = cap_rendering::CaptionFrameBox::new([0.; 4], "bottom-left");
        set_caption_position(&mut settings, "manual", Some((on_screen, (0.21, 0.84))));
        assert_eq!(
            settings.manual_position.map(|xy| (xy.x, xy.y)),
            Some((0.21, 0.84))
        );
    }

    /// A 0.6.1 report: with the caption position set to Manual, the captions
    /// stayed bottom-centre and nothing could move them. Manual now starts
    /// where the preset drew them and the sliders move it.
    #[test]
    fn manual_caption_position_starts_at_the_preset_and_moves() {
        let mut settings = CaptionSettings {
            position: "top-center".into(),
            ..CaptionSettings::default()
        };
        set_caption_position_from(&mut settings, "manual", |_| None);
        assert_eq!(settings.position, "manual");
        assert_eq!(
            settings.manual_position.map(|xy| (xy.x, xy.y)),
            Some((0.5, 0.08))
        );

        set_caption_manual_center(&mut settings, Some(20.), None);
        set_caption_manual_center(&mut settings, None, Some(130.));
        assert_eq!(
            settings.manual_position.map(|xy| (xy.x, xy.y)),
            Some((0.2, 1.0))
        );
        let mut project = ProjectConfiguration::default();
        project.captions = Some(CaptionsData {
            settings: settings.clone(),
            ..CaptionsData::default()
        });
        assert_eq!(CaptionSlider::ManualX.read(&project), 20.);
        assert_eq!(CaptionSlider::ManualY.read(&project), 100.);

        // Picking Manual again keeps the adjusted spot.
        set_caption_position(&mut settings, "manual", None);
        assert_eq!(
            settings.manual_position.map(|xy| (xy.x, xy.y)),
            Some((0.2, 1.0))
        );

        // A slider on a preset switches it to Manual from the preset's spot.
        let mut preset = CaptionSettings {
            position: "bottom-right".into(),
            ..CaptionSettings::default()
        };
        set_caption_manual_center(&mut preset, None, Some(50.));
        assert_eq!(preset.position, "manual");
        assert_eq!(
            preset.manual_position.map(|xy| (xy.x, xy.y)),
            Some((0.95, 0.5))
        );
    }

    #[test]
    fn moving_the_captions_keeps_the_style_preset() {
        for reason in ["caption-position", "caption-placement"] {
            assert!(!caption_reason_customises_style(reason));
        }
        for reason in ["caption-slider", "caption-font", "caption-animation"] {
            assert!(caption_reason_customises_style(reason));
        }
    }

    #[test]
    fn reset_position_returns_manual_captions_to_bottom_centre() {
        let mut settings = CaptionSettings {
            position: "manual".into(),
            manual_position: Some(cap_project::XY::new(0.2, 0.3)),
            ..CaptionSettings::default()
        };
        assert!(!caption_at_default_center(&settings));
        reset_caption_manual_center(&mut settings);
        assert_eq!(settings.position, "manual");
        assert_eq!(
            settings.manual_position.map(|xy| (xy.x, xy.y)),
            Some((0.5, 0.85))
        );
        assert!(caption_at_default_center(&settings));

        let unset = CaptionSettings {
            position: "manual".into(),
            ..CaptionSettings::default()
        };
        assert!(caption_at_default_center(&unset));
    }

    /// The typed X and Y read back the way they were stored, to a tenth of
    /// a percent, including centres a drag left between whole numbers.
    #[test]
    fn manual_position_fields_read_a_tenth_of_a_percent() {
        assert_eq!(caption_percent_text(0.5), "50");
        assert_eq!(caption_percent_text(0.423_7), "42.4");
        assert_eq!(caption_percent_text(0.0), "0");
        assert_eq!(caption_percent_text(1.0), "100");
        assert_eq!(caption_percent_text(0.333_333_34), "33.3");

        let mut settings = CaptionSettings {
            position: "manual".into(),
            ..CaptionSettings::default()
        };
        set_caption_manual_center(&mut settings, Some(33.3), Some(150.));
        assert_eq!(
            caption_percent_text(caption_manual_center(&settings).0),
            "33.3"
        );
        assert_eq!(
            caption_percent_text(caption_manual_center(&settings).1),
            "100"
        );
    }

    #[test]
    fn outlined_caption_styles_keep_content_font_size_and_placement() {
        let mut original = CaptionSettings {
            font: "Noto Serif SC".into(),
            size: 64,
            position: "top-center".into(),
            display_mode: cap_project::CaptionDisplayMode::Bilingual,
            ..Default::default()
        };
        original.set_translation("intro", "开始录制", "Start recording".into(), "en".into());
        for preset in &QUICK_CAPTION_PRESETS {
            let mut settings = original.clone();
            preset.apply(&mut settings);
            assert_eq!(settings.font, original.font);
            assert_eq!(settings.size, original.size);
            assert_eq!(settings.position, original.position);
            assert_eq!(settings.display_mode, original.display_mode);
            assert_eq!(settings.translations, original.translations);
            assert_eq!(settings.preset, preset.id);
            assert_eq!(settings.color, preset.color);
            assert_eq!(settings.outline_color, preset.outline_color);
            assert_eq!(settings.outline_width, preset.outline_width);
            assert_eq!(settings.background_opacity, preset.background_opacity);
            assert_eq!(settings.outline, preset.background_opacity == 0);
            assert!(!settings.shadow);
            assert!(!settings.active_word_highlight);
        }
        assert_ne!(
            QUICK_CAPTION_PRESETS[0].color,
            QUICK_CAPTION_PRESETS[1].color
        );
        assert_ne!(
            QUICK_CAPTION_PRESETS[0].outline_color,
            QUICK_CAPTION_PRESETS[1].outline_color
        );
    }

    #[test]
    fn caption_translation_controls_are_independent_and_reversible_without_changing_content() {
        let mut project = slider_model_project(4);
        let settings = &mut project.captions.as_mut().unwrap().settings;
        settings.font = "Noto Sans SC".into();
        settings.size = 48;
        settings.display_mode = CaptionDisplayMode::Bilingual;
        settings.set_translation("caption", "hi", "你好".into(), "zh".into());
        let translations = settings.translations.clone();
        let original = serde_json::to_value(&project).unwrap();
        let timeline = serde_json::to_value(&project.timeline).unwrap();
        let captions = serde_json::to_value(&project.captions.as_ref().unwrap().segments).unwrap();
        let mut history = crate::editor_edits::ProjectHistory::new(project.clone());
        assert!(with_caption_settings(&mut project, |settings| {
            CaptionSlider::Size.write(settings, 64.)
        }));
        history.record(&project);
        let settings = &project.captions.as_ref().unwrap().settings;
        assert_eq!(
            (settings.size, settings.resolved_translation_style().size),
            (64, 48)
        );
        assert_eq!(settings.translations, translations);
        assert_eq!(serde_json::to_value(&project.timeline).unwrap(), timeline);
        assert_eq!(
            serde_json::to_value(&project.captions.as_ref().unwrap().segments).unwrap(),
            captions
        );
        assert_eq!(
            serde_json::to_value(history.undo().unwrap()).unwrap(),
            original
        );
        assert!(!history.can_undo());
        project = history.redo().unwrap().clone();
        assert!(with_caption_settings(&mut project, |settings| {
            CaptionSlider::TranslationSize.write(settings, 32.)
        }));
        history.record(&project);
        let settings = &project.captions.as_ref().unwrap().settings;
        assert_eq!(
            (settings.size, settings.resolved_translation_style().size),
            (64, 32)
        );
        assert_eq!(CaptionSlider::TranslationSize.read(&project), 32.);
        assert_eq!(CaptionSlider::Size.read(&project), 64.);
        assert_eq!(
            history
                .undo()
                .unwrap()
                .captions
                .as_ref()
                .unwrap()
                .settings
                .resolved_translation_style()
                .size,
            48
        );
        assert_eq!(serde_json::to_value(&project.timeline).unwrap(), timeline);
    }

    #[test]
    fn caption_spacing_edits_preserve_legacy_fonts_content_and_undo() {
        let mut project = slider_model_project(2);
        let original = serde_json::to_value(&project).unwrap();
        let old_settings = caption_settings(&project);
        let mut history = crate::editor_edits::ProjectHistory::new(project.clone());
        assert_eq!(CaptionSlider::TranslationGap.read(&project), 0.);
        assert_eq!(serde_json::to_value(&project).unwrap(), original);
        for (slider, value) in [
            (CaptionSlider::TranslationGap, 10.),
            (CaptionSlider::BottomMargin, 60.),
        ] {
            assert!(with_caption_settings(&mut project, |settings| slider.write(settings, value)));
            history.record(&project);
        }
        let settings = caption_settings(&project);
        assert_eq!(settings.translation_gap, Some(10));
        assert_eq!(settings.bottom_margin, Some(60));
        assert_eq!(settings.translation_style, old_settings.translation_style);
        assert_eq!(
            settings.resolved_translation_style(),
            old_settings.resolved_translation_style()
        );
        assert_eq!(settings.position, old_settings.position);
        assert_eq!(settings.translations, old_settings.translations);
        assert_eq!(
            serde_json::to_value(&project.timeline).unwrap(),
            original["timeline"]
        );
        let saved = serde_json::to_value(&project).unwrap();
        assert_eq!(
            history
                .undo()
                .unwrap()
                .captions
                .as_ref()
                .unwrap()
                .settings
                .bottom_margin,
            None
        );
        assert_eq!(
            serde_json::to_value(history.undo().unwrap()).unwrap(),
            original
        );
        assert!(history.redo().is_some());
        assert_eq!(
            serde_json::to_value(history.redo().unwrap()).unwrap(),
            saved
        );
        let reopened: ProjectConfiguration = serde_json::from_value(saved).unwrap();
        assert_eq!(CaptionSlider::TranslationGap.read(&reopened), 10.);
        assert_eq!(CaptionSlider::BottomMargin.read(&reopened), 60.);
    }

    #[test]
    fn caption_style_previews_resolve_both_fonts_and_sizes_in_the_selected_display_mode() {
        let mut settings = CaptionSettings {
            font: "Noto Sans SC".into(),
            size: 48,
            display_mode: CaptionDisplayMode::Bilingual,
            ..Default::default()
        };
        let before = serde_json::to_value(&settings).unwrap();
        let legacy = caption_preview_lines(&settings);
        assert_eq!(legacy.len(), 2);
        assert_eq!((legacy[0].size, legacy[1].size), (18., 18.));
        assert_eq!(legacy[0].style.font, legacy[1].style.font);
        assert_eq!(serde_json::to_value(&settings).unwrap(), before);
        settings.set_translation_font("Inter".into());
        settings.set_translation_font_size(40);
        let bilingual = caption_preview_lines(&settings);
        assert_eq!((bilingual[0].size, bilingual[1].size), (18., 15.));
        assert_eq!(bilingual[0].style.font, "Noto Sans SC");
        assert_eq!(bilingual[1].style.font, "Inter");
        for (mode, translated, font) in [
            (CaptionDisplayMode::Original, false, "Noto Sans SC"),
            (CaptionDisplayMode::Translated, true, "Inter"),
        ] {
            settings.display_mode = mode;
            let lines = caption_preview_lines(&settings);
            assert_eq!(lines.len(), 1);
            assert_eq!(lines[0].translation, translated);
            assert_eq!(lines[0].style.font, font);
            assert_eq!(lines[0].size, 18.);
        }
        settings.display_mode = CaptionDisplayMode::Bilingual;
        let translation = settings.translation_style.clone();
        for preset in CAPTION_PRESETS {
            let mut applied = settings.clone();
            preset.apply(&mut applied);
            assert_eq!(applied.translation_style, translation);
            let lines = caption_preview_lines(&applied);
            assert!((lines[0].size / lines[1].size - preset.size as f32 / 40.).abs() < 0.001);
        }
    }

    #[test]
    fn caption_preview_uses_one_fitted_box_for_both_languages() {
        let settings = CaptionSettings {
            size: 48,
            translation_style: Some(CaptionTextStyle {
                font: "Inter".into(),
                size: 32,
            }),
            display_mode: CaptionDisplayMode::Bilingual,
            translation_gap: Some(12),
            ..Default::default()
        };
        let lines = caption_preview_lines(&settings);
        let wide = caption_preview_layout(&settings, &lines, &[120., 180.], 300., 92.);
        assert_eq!(wide.width, 198.);
        assert_eq!(wide.padding, 9.);
        assert!((wide.height - 58.5).abs() < 0.001);
        assert!((wide.gap - 4.5).abs() < 0.001);
        assert!(wide.radius > 0.);
        let narrow = caption_preview_layout(&settings, &lines, &[120., 180.], 140., 92.);
        assert!(narrow.width <= 124.001);
        assert!(narrow.height < wide.height);
        assert!((narrow.gap / narrow.scale - wide.gap).abs() < 0.001);
    }

    #[test]
    fn caption_preview_single_language_has_no_bilingual_gap() {
        let settings = CaptionSettings {
            display_mode: CaptionDisplayMode::Translated,
            translation_gap: Some(200),
            ..Default::default()
        };
        let lines = caption_preview_lines(&settings);
        let layout = caption_preview_layout(&settings, &lines, &[100.], 200., 92.);
        assert_eq!(layout.gap, 0.);
        assert!((layout.height - 39.6).abs() < 0.001);
    }

    #[test]
    fn every_tab_slider_has_the_range_its_call_site_declares() {
        assert_eq!(CaptionSlider::OutlineWidth.limits(), (0., 20., 0.5));
        assert_eq!(CaptionSlider::ShadowOpacity.limits(), (0., 100., 1.));
        assert_eq!(CaptionSlider::ShadowSize.limits(), (0., 30., 0.5));
        assert_eq!(CameraSlider::Size.limits(), (20., 80., 0.1));
        assert_eq!(CameraSlider::ZoomSize.limits(), (10., 100., 0.1));
        assert_eq!(CameraSlider::ShadowBlur.limits(), (0., 100., 0.1));
        assert_eq!(AudioSlider::MicVolume.limits(), (-30., 10., 0.1));
        assert_eq!(CursorSlider::Size.limits(), (20., 300., 1.));
        assert_eq!(CursorSlider::IdleDelay.limits(), (0.5, 5., 0.1));
        assert_eq!(CursorSlider::Mass.limits(), (0.1, 15., 0.01));
        assert_eq!(KeyboardSlider::LingerDuration.limits(), (0., 300., 5.));
        assert_eq!(
            KeyboardSlider::GroupingThreshold.limits(),
            (50., 1000., 10.)
        );
        assert_eq!(CaptionSlider::FadeDuration.limits(), (0., 50., 1.));
    }

    #[test]
    fn camera_zoom_size_slider_shows_the_rendered_scale() {
        let mut project = ProjectConfiguration::default();
        assert_eq!(CameraSlider::ZoomSize.read(&project), 70.);
        project.camera.scale_during_zoom = 0.4;
        assert_eq!(CameraSlider::ZoomSize.read(&project), 40.);
        project.camera.scale_during_zoom = 1.;
        assert_eq!(CameraSlider::ZoomSize.read(&project), 100.);
    }

    #[test]
    fn the_percent_sliders_read_and_write_the_same_number() {
        // `value={[getSetting("fadeDuration") * 100]}` / `v[0] / 100`.
        let mut project = ProjectConfiguration {
            keyboard: Some(KeyboardData::default()),
            ..Default::default()
        };
        project.keyboard.as_mut().unwrap().settings.fade_duration = 0.15;
        assert!((KeyboardSlider::FadeDuration.read(&project) - 15.).abs() < 1e-4);
        project.keyboard.as_mut().unwrap().settings.linger_duration = 0.8;
        assert!((KeyboardSlider::LingerDuration.read(&project) - 80.).abs() < 1e-4);
    }

    #[test]
    fn the_physics_sliders_fall_back_to_custom_off_a_preset() {
        // Mellow's preset is `{470, 3, 70}`.
        assert_eq!(
            match_cursor_preset(470., 3., 70.),
            Some(CursorAnimationStyle::Mellow)
        );
        assert_eq!(match_cursor_preset(471., 3., 70.), None);
        // Every named style in the picker has a preset behind it.
        for (style, ..) in CURSOR_STYLES {
            assert!(style.preset().is_some(), "{style:?} has no preset");
        }
    }

    #[test]
    fn the_catalogues_match_the_source() {
        assert_eq!(CAPTION_PRESETS.len(), 5);
        assert_eq!(CAPTION_PRESETS[0].id, "classic");
        assert_eq!(CAPTION_PRESETS[2].highlight_style, "pill");
        assert_eq!(CAPTION_POSITIONS[0].0, "manual");
        assert_eq!(KEYBOARD_POSITIONS.len(), 6);
        assert_eq!(FONT_OPTIONS.len(), 3);
        for font in FONT_OPTIONS {
            assert_eq!(font_option_label(font).as_ref(), font);
        }
        assert_eq!(font_option_label("Inter").as_ref(), "Inter");
        assert_eq!(TEXT_WEIGHTS.len(), 3);
        assert_eq!(
            CAMERA_BLUR_MODES.len(),
            if cfg!(target_os = "macos") { 4 } else { 3 }
        );
        assert_eq!(CAPTION_MODELS.len(), 3 + crate::asr_cloud::SERVICES.len());
        assert_eq!(CAPTION_MODELS[0].name, crate::local_qwen::MODEL);
        assert_eq!(CAPTION_MODELS[1].name, "small");
        assert_eq!(CAPTION_MODELS[2].name, "medium");
        for (model, service) in CAPTION_MODELS[3..].iter().zip(crate::asr_cloud::SERVICES) {
            assert_eq!(model.name, service.id);
        }
        assert!(
            CAPTION_MODELS
                .iter()
                .all(|model| !transcription::is_parakeet_model(model.name))
        );
    }

    #[test]
    fn local_catalog_only_offers_multilingual_models_supported_by_the_platform() {
        for (supported, expected) in [
            (true, vec![crate::local_qwen::MODEL, "small", "medium"]),
            (false, vec!["small", "medium"]),
        ] {
            assert_eq!(
                local_caption_models_for(supported)
                    .iter()
                    .map(|model| model.name)
                    .collect::<Vec<_>>(),
                expected
            );
        }
        assert_eq!(
            local_caption_models()
                .iter()
                .map(|model| model.name)
                .collect::<Vec<_>>(),
            local_caption_models_for(crate::local_qwen::supported())
                .iter()
                .map(|model| model.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn automatic_model_display_uses_the_running_engine_and_restores_the_manual_preference() {
        let models: Vec<_> = CAPTION_MODELS.iter().collect();
        let preferred = "doubao";
        let running = caption_model_for_display(preferred, &models, Some(crate::local_qwen::MODEL));
        assert_eq!(running.name, crate::local_qwen::MODEL);
        assert!(!crate::asr_cloud::is_cloud_model(running.name));
        assert!(caption_model_label(running.name).contains("0.6B"));
        assert_eq!(
            caption_model_for_display(preferred, &models, None).name,
            "doubao"
        );
        assert_eq!(
            caption_model_for_display(preferred, &models, Some("medium")).name,
            "medium"
        );
    }

    #[test]
    fn retired_model_preferences_migrate_to_local_defaults_only() {
        for saved in ["best", "best-max"] {
            assert_eq!(
                retired_model_replacement(saved, true),
                Some(crate::local_qwen::MODEL)
            );
            assert_eq!(retired_model_replacement(saved, false), Some("small"));
        }
        for saved in [
            crate::local_qwen::MODEL,
            "small",
            "medium",
            "doubao",
            "unknown",
        ] {
            assert_eq!(retired_model_replacement(saved, true), None);
            assert_eq!(retired_model_replacement(saved, false), None);
        }
    }

    #[test]
    fn a_project_without_a_captions_block_refuses_a_settings_write() {
        // `if (!project?.captions) return` (`CaptionsTab.tsx:325`).
        let mut project = ProjectConfiguration::default();
        assert!(!with_caption_settings(&mut project, |settings| {
            settings.size = 99
        }));
        project.captions = Some(default_captions());
        assert!(with_caption_settings(&mut project, |settings| {
            settings.size = 99
        }));
        assert_eq!(caption_settings(&project).size, 99);
    }

    #[test]
    fn a_keyboard_settings_write_creates_the_block_it_needs() {
        // `if (!project?.keyboard) { setProject("keyboard", { settings: {...} }) }`
        let mut project = ProjectConfiguration::default();
        assert!(with_keyboard_settings(&mut project, |settings| {
            settings.size = 72
        }));
        assert_eq!(keyboard_settings(&project).size, 72);
        // The rest of the defaults come along, not just the written key.
        assert_eq!(keyboard_settings(&project).position, "bottom-center");
    }

    fn slider_model_project(timeline_segments: usize) -> ProjectConfiguration {
        let segments: Vec<serde_json::Value> = (0..timeline_segments)
            .map(|index| {
                serde_json::json!({
                    "recordingSegment": 0,
                    "timescale": 1.0,
                    "start": index as f64,
                    "end": index as f64 + 1.0,
                })
            })
            .collect();
        let config = serde_json::json!({
            "timeline": {
                "segments": segments,
                "transitions": [
                    {"segmentIndex": 1, "type": "cross-fade", "duration": 0.4}
                ],
                "zoomSegments": [
                    {"start": 0.0, "end": 1.0, "amount": 1.5, "mode": "auto"}
                ],
                "sceneSegments": [{"start": 0.0, "end": 1.0}],
                "maskSegments": [{
                    "start": 0.0,
                    "end": 1.0,
                    "track": 0,
                    "maskType": "sensitive",
                    "center": {"x": 0.5, "y": 0.5},
                    "size": {"x": 0.2, "y": 0.2}
                }],
                "textSegments": [{
                    "start": 0.0,
                    "end": 1.0,
                    "track": 0,
                    "content": "title"
                }],
                "captionSegments": [{
                    "id": "caption",
                    "start": 0.0,
                    "end": 1.0,
                    "text": "hi"
                }],
                "keyboardSegments": [{
                    "id": "keyboard",
                    "start": 0.0,
                    "end": 1.0,
                    "displayText": "A"
                }],
                "audioSegments": [{
                    "start": 0.0,
                    "end": 1.0,
                    "track": 0,
                    "path": "/tmp/none.mp3"
                }],
                "styleSegments": [{
                    "start": 0.0,
                    "end": 1.0,
                    "track": 0,
                    "enabled": true,
                    "name": "Style",
                    "overrides": {}
                }],
                "imageSegments": [{
                    "start": 0.0,
                    "end": 1.0,
                    "track": 0,
                    "enabled": true,
                    "path": "/tmp/none.png"
                }],
                "camera3dSegments": [{"start": 0.0, "end": 1.0}]
            },
            "captions": {"settings": {"enabled": true}, "segments": []},
            "keyboard": {"settings": {"enabled": true}}
        });
        serde_json::from_value(config).expect("the slider fixture parses")
    }

    fn animated_gradient_project() -> ProjectConfiguration {
        let mut project = slider_model_project(64);
        project.background.source = cap_project::BackgroundSource::AnimatedGradient {
            config: cap_project::AnimatedGradientConfig::default(),
        };
        project
    }

    fn gradient_source() -> cap_project::BackgroundSource {
        cap_project::BackgroundSource::Gradient {
            from: [0, 0, 0],
            to: [255, 255, 255],
            angle: 90,
            noise_intensity: Some(0.2),
            noise_scale: Some(1.0),
            animated: Some(false),
            animation_speed: Some(1.0),
        }
    }

    macro_rules! slider_case {
        ($name:literal, |$project:ident| $body:block) => {
            (
                $name,
                (|$project: &mut ProjectConfiguration| $body) as fn(&mut ProjectConfiguration),
            )
        };
        ($name:literal, |$project:ident| $body:expr) => {
            (
                $name,
                (|$project: &mut ProjectConfiguration| $body) as fn(&mut ProjectConfiguration),
            )
        };
    }

    fn slider_change_cases() -> Vec<(&'static str, fn(&mut ProjectConfiguration))> {
        vec![
            slider_case!("background.blur", |project| project.background.blur = 42.),
            slider_case!("background.padding", |project| {
                project.background.padding = 12.;
                project.background.source = cap_project::BackgroundSource::Color {
                    value: [255, 255, 255],
                    alpha: 255,
                };
            }),
            slider_case!("background.rounding", |project| {
                project.background.rounding = 18.;
                project.background.source = cap_project::BackgroundSource::Color {
                    value: [0, 0, 0],
                    alpha: 255,
                };
            }),
            slider_case!("background.motion_blur", |project| {
                project.cursor.motion_blur = 0.5;
                project.screen_motion_blur = 0.5;
            }),
            slider_case!("background.border.width", |project| {
                let mut border = project.background.border.clone().unwrap_or_default();
                border.width = 8.;
                project.background.border = Some(border);
            }),
            slider_case!("background.border.opacity", |project| {
                let mut border = project.background.border.clone().unwrap_or_default();
                border.opacity = 60.;
                project.background.border = Some(border);
            }),
            slider_case!("background.notch.width", |project| {
                let mut notch = project.background.notch.unwrap_or_default();
                notch.enabled = true;
                notch.width = Some(0.2);
                project.background.notch = Some(notch);
            }),
            slider_case!("background.notch.height", |project| {
                let mut notch = project.background.notch.unwrap_or_default();
                notch.enabled = true;
                notch.height = Some(0.05);
                project.background.notch = Some(notch);
            }),
            slider_case!("background.notch.x", |project| {
                let mut notch = project.background.notch.unwrap_or_default();
                notch.enabled = true;
                notch.x = Some(0.25);
                project.background.notch = Some(notch);
            }),
            slider_case!("background.shadow", |project| project.background.shadow =
                22.),
            slider_case!("background.advanced_shadow.size", |project| {
                project.background.advanced_shadow = Some(ShadowConfiguration {
                    size: 22.,
                    opacity: 50.,
                    blur: 4.,
                });
            }),
            slider_case!("background.advanced_shadow.opacity", |project| {
                project.background.advanced_shadow = Some(ShadowConfiguration {
                    size: 20.,
                    opacity: 80.,
                    blur: 4.,
                });
            }),
            slider_case!("background.advanced_shadow.blur", |project| {
                project.background.advanced_shadow = Some(ShadowConfiguration {
                    size: 20.,
                    opacity: 50.,
                    blur: 12.,
                });
            }),
            slider_case!("background.gradient.angle", |project| {
                project.background.source = gradient_source();
                if let cap_project::BackgroundSource::Gradient { angle, .. } =
                    &mut project.background.source
                {
                    *angle = 45;
                }
            }),
            slider_case!("background.gradient.noise_intensity", |project| {
                project.background.source = gradient_source();
                if let cap_project::BackgroundSource::Gradient {
                    noise_intensity, ..
                } = &mut project.background.source
                {
                    *noise_intensity = Some(0.8);
                }
            }),
            slider_case!("background.gradient.noise_scale", |project| {
                project.background.source = gradient_source();
                if let cap_project::BackgroundSource::Gradient { noise_scale, .. } =
                    &mut project.background.source
                {
                    *noise_scale = Some(2.0);
                }
            }),
            slider_case!("camera.size", |project| project.camera.size = 55.),
            slider_case!("camera.zoom_size", |project| project
                .camera
                .scale_during_zoom = 1.5),
            slider_case!("camera.rounding", |project| project.camera.rounding = 18.),
            slider_case!("camera.shadow", |project| project.camera.shadow = 30.),
            slider_case!("camera.advanced_shadow.size", |project| {
                project.camera.advanced_shadow = Some(ShadowConfiguration {
                    size: 22.,
                    opacity: 50.,
                    blur: 4.,
                });
            }),
            slider_case!("camera.advanced_shadow.opacity", |project| {
                project.camera.advanced_shadow = Some(ShadowConfiguration {
                    size: 20.,
                    opacity: 80.,
                    blur: 4.,
                });
            }),
            slider_case!("camera.advanced_shadow.blur", |project| {
                project.camera.advanced_shadow = Some(ShadowConfiguration {
                    size: 20.,
                    opacity: 50.,
                    blur: 12.,
                });
            }),
            slider_case!("cursor.size", |project| project.cursor.size = 2),
            slider_case!("cursor.rotation_amount", |project| project
                .cursor
                .rotation_amount =
                15.),
            slider_case!("cursor.hide_when_idle_delay", |project| project
                .cursor
                .hide_when_idle_delay =
                2.5),
            slider_case!("cursor.tension", |project| project.cursor.tension = 45.),
            slider_case!("cursor.friction", |project| project.cursor.friction = 45.),
            slider_case!("cursor.mass", |project| project.cursor.mass = 1.5),
            slider_case!("cursor.animation_style", |project| project
                .cursor
                .animation_style =
                CursorAnimationStyle::Custom),
            slider_case!("cursor.ripple.strength", |project| project
                .cursor
                .ripple
                .strength = 45.),
            slider_case!("cursor.ripple.size", |project| project.cursor.ripple.size =
                45.),
            slider_case!("cursor.ripple.duration", |project| project
                .cursor
                .ripple
                .duration = 900.),
            slider_case!("caption.size", |project| project
                .captions
                .as_mut()
                .unwrap()
                .settings
                .size = 72),
            slider_case!("caption.translation_size", |project| {
                CaptionSlider::TranslationSize
                    .write(&mut project.captions.as_mut().unwrap().settings, 32.)
            }),
            slider_case!("caption.translation_gap", |project| {
                CaptionSlider::TranslationGap
                    .write(&mut project.captions.as_mut().unwrap().settings, 12.)
            }),
            slider_case!("caption.bottom_margin", |project| {
                CaptionSlider::BottomMargin
                    .write(&mut project.captions.as_mut().unwrap().settings, 80.)
            }),
            slider_case!("caption.background_opacity", |project| project
                .captions
                .as_mut()
                .unwrap()
                .settings
                .background_opacity =
                80),
            slider_case!("caption.fade_duration", |project| project
                .captions
                .as_mut()
                .unwrap()
                .settings
                .fade_duration = 0.5),
            slider_case!("caption.preset", |project| project
                .captions
                .as_mut()
                .unwrap()
                .settings
                .preset = "custom".into()),
            slider_case!("keyboard.size", |project| project
                .keyboard
                .as_mut()
                .unwrap()
                .settings
                .size = 72),
            slider_case!("keyboard.background_opacity", |project| project
                .keyboard
                .as_mut()
                .unwrap()
                .settings
                .background_opacity =
                80),
            slider_case!("keyboard.fade_duration", |project| project
                .keyboard
                .as_mut()
                .unwrap()
                .settings
                .fade_duration = 0.5),
            slider_case!("keyboard.linger_duration", |project| project
                .keyboard
                .as_mut()
                .unwrap()
                .settings
                .linger_duration =
                1.5),
            slider_case!("keyboard.grouping_threshold_ms", |project| project
                .keyboard
                .as_mut()
                .unwrap()
                .settings
                .grouping_threshold_ms =
                750.),
        ]
    }
    fn assert_timeline_model_unchanged_in(
        mut project: ProjectConfiguration,
        name: &str,
        change: impl FnOnce(&mut ProjectConfiguration),
    ) {
        use crate::editor_timeline::{TimelineModel, TrackLanes};

        let lanes_before = TrackLanes::from_project(&project, true);
        let before = TimelineModel::build_with_lanes(&project, true, true, &lanes_before);
        change(&mut project);
        let lanes_after = TrackLanes::from_project(&project, true);
        assert_eq!(
            lanes_before, lanes_after,
            "{name} must not change the track lanes"
        );
        let after = TimelineModel::build_with_lanes(&project, true, true, &lanes_after);
        assert_eq!(
            format!("{before:?}"),
            format!("{after:?}"),
            "{name} must not change the timeline model"
        );
    }

    fn assert_timeline_model_unchanged(name: &str, change: impl FnOnce(&mut ProjectConfiguration)) {
        assert_timeline_model_unchanged_in(slider_model_project(64), name, change);
    }

    fn assert_timeline_model_changes_in(
        mut project: ProjectConfiguration,
        name: &str,
        change: impl FnOnce(&mut ProjectConfiguration),
    ) {
        use crate::editor_timeline::TimelineModel;

        let before = TimelineModel::build(&project, true, true);
        change(&mut project);
        let after = TimelineModel::build(&project, true, true);
        assert_ne!(
            format!("{before:?}"),
            format!("{after:?}"),
            "{name} must change the timeline model"
        );
    }

    fn assert_timeline_model_changes(name: &str, change: impl FnOnce(&mut ProjectConfiguration)) {
        assert_timeline_model_changes_in(slider_model_project(64), name, change);
    }

    #[test]
    fn slider_edits_do_not_change_the_timeline_model() {
        for (name, change) in slider_change_cases() {
            assert_timeline_model_unchanged(name, change);
        }

        for target in [GradeTarget::Screen, GradeTarget::Camera] {
            for slider in crate::editor_color::GradeSlider::ADJUST
                .iter()
                .copied()
                .chain(std::iter::once(crate::editor_color::GradeSlider::Grain))
            {
                let target_name = match target {
                    GradeTarget::Screen => "screen",
                    GradeTarget::Camera => "camera",
                };
                let name = format!("grade.{target_name}.{}", slider.label());
                assert_timeline_model_unchanged(&name, move |project| {
                    let grade = match target {
                        GradeTarget::Screen => &mut project.color_correction.screen,
                        GradeTarget::Camera => &mut project.color_correction.camera,
                    };
                    slider.write(grade, 0.5);
                    grade.preset = "custom".into();
                });
            }
        }

        for parameter in cap_project::AnimatedGradientParameter::ALL {
            let name = format!("animated_gradient.{}", parameter.control().label);
            assert_timeline_model_unchanged_in(
                animated_gradient_project(),
                &name,
                move |project| {
                    let current = {
                        let cap_project::BackgroundSource::AnimatedGradient { config } =
                            &project.background.source
                        else {
                            panic!("the fixture is an animated gradient");
                        };
                        parameter.get(config)
                    };
                    let control = parameter.control();
                    let next = if current + control.step <= control.max {
                        current + control.step
                    } else {
                        control.min + control.step
                    };
                    let cap_project::BackgroundSource::AnimatedGradient { config } =
                        &mut project.background.source
                    else {
                        panic!("the fixture is an animated gradient");
                    };
                    parameter.set(config, next);
                },
            );
        }

        assert_timeline_model_unchanged_in(
            animated_gradient_project(),
            "animated_gradient.stop_position",
            |project| {
                let cap_project::BackgroundSource::AnimatedGradient { config } =
                    &mut project.background.source
                else {
                    panic!("the fixture is an animated gradient");
                };
                config.color_stops[0].position = 75.0;
            },
        );
    }

    #[test]
    fn audio_levels_still_change_the_timeline_model() {
        assert_timeline_model_changes("audio.mic_volume_db", |project| {
            project.audio.mic_volume_db = 1.5;
        });
        assert_timeline_model_changes("audio.system_volume_db", |project| {
            project.audio.system_volume_db = 1.5;
        });
        let mut camera_project = slider_model_project(64);
        camera_project
            .timeline
            .as_mut()
            .unwrap()
            .scene_segments
            .clear();
        assert_timeline_model_changes_in(camera_project, "camera.hide", |project| {
            project.camera.hide = true;
        });
        assert_timeline_model_changes("captions.settings.enabled", |project| {
            project.captions.as_mut().unwrap().settings.enabled = false;
        });
        assert_timeline_model_changes("keyboard.settings.enabled", |project| {
            project.keyboard.as_mut().unwrap().settings.enabled = false;
        });
    }

    /// Pin every variant's classification, not just the project fields the
    /// slider happens to write. A new variant still fails to compile until it
    /// picks a side; this catches flipping an existing variant without
    /// updating the routing tests.
    #[test]
    fn slider_classification_is_pinned_per_variant() {
        use crate::editor_color::GradeSlider;
        use crate::editor_sidebar::BgSlider;

        for slider in [
            BgSlider::Blur,
            BgSlider::Padding,
            BgSlider::Rounding,
            BgSlider::MotionBlur,
            BgSlider::BorderWidth,
            BgSlider::BorderOpacity,
            BgSlider::NotchWidth,
            BgSlider::NotchHeight,
            BgSlider::NotchX,
            BgSlider::Shadow,
            BgSlider::ShadowSize,
            BgSlider::ShadowOpacity,
            BgSlider::ShadowBlur,
            BgSlider::GradientAngle,
            BgSlider::GradientNoise,
            BgSlider::GradientGrain,
        ] {
            assert!(
                !slider.changes_timeline_model(),
                "background {slider:?} must not rebuild the timeline"
            );
        }

        for slider in [
            CameraSlider::Size,
            CameraSlider::ZoomSize,
            CameraSlider::Rounding,
            CameraSlider::Shadow,
            CameraSlider::ShadowSize,
            CameraSlider::ShadowOpacity,
            CameraSlider::ShadowBlur,
            CameraSlider::SkinSmoothing,
        ] {
            assert!(
                !slider.changes_timeline_model(),
                "camera {slider:?} must not rebuild the timeline"
            );
        }

        for slider in [AudioSlider::MicVolume, AudioSlider::SystemVolume] {
            assert!(
                slider.changes_timeline_model(),
                "audio {slider:?} must rebuild the timeline"
            );
        }

        for slider in [
            CursorSlider::Size,
            CursorSlider::Tilt,
            CursorSlider::IdleDelay,
            CursorSlider::Tension,
            CursorSlider::Friction,
            CursorSlider::Mass,
            CursorSlider::RippleStrength,
            CursorSlider::RippleSize,
            CursorSlider::RippleDuration,
        ] {
            assert!(
                !slider.changes_timeline_model(),
                "cursor {slider:?} must not rebuild the timeline"
            );
        }

        for slider in [
            CaptionSlider::Size,
            CaptionSlider::TranslationSize,
            CaptionSlider::TranslationGap,
            CaptionSlider::BottomMargin,
            CaptionSlider::BackgroundOpacity,
            CaptionSlider::OutlineWidth,
            CaptionSlider::ShadowOpacity,
            CaptionSlider::ShadowSize,
            CaptionSlider::FadeDuration,
            CaptionSlider::ManualX,
            CaptionSlider::ManualY,
        ] {
            assert!(
                !slider.changes_timeline_model(),
                "caption {slider:?} must not rebuild the timeline"
            );
        }

        for slider in [
            KeyboardSlider::Size,
            KeyboardSlider::BackgroundOpacity,
            KeyboardSlider::FadeDuration,
            KeyboardSlider::LingerDuration,
            KeyboardSlider::GroupingThreshold,
        ] {
            assert!(
                !slider.changes_timeline_model(),
                "keyboard {slider:?} must not rebuild the timeline"
            );
        }

        for slider in [
            GradeSlider::Grain,
            GradeSlider::Intensity,
            GradeSlider::Exposure,
            GradeSlider::Contrast,
            GradeSlider::Saturation,
            GradeSlider::Temperature,
            GradeSlider::Tint,
            GradeSlider::Fade,
            GradeSlider::SplitTone,
            GradeSlider::Vignette,
        ] {
            assert!(
                !slider.changes_timeline_model(),
                "grade {slider:?} must not rebuild the timeline"
            );
        }
    }

    /// Times the rebuild the new route skips; the rest of `project_changed`
    /// (history, publish, save, notify) is unchanged and not measured here.
    ///
    /// The fixture is a synthetic stress case: 2,000 contiguous clips plus
    /// every track kind, not a typical project. Quote these numbers only as
    /// the cost of the skipped `TimelineModel::build_with_lanes`.
    ///
    /// `cargo test --release --manifest-path apps/desktop-gpui/Cargo.toml --bin cap-gpui -- --ignored benchmark_slider_edit_skips_timeline_rebuild --nocapture`
    #[test]
    #[ignore = "manual slider edit benchmark"]
    fn benchmark_slider_edit_skips_timeline_rebuild() {
        use std::{hint::black_box, time::Instant};

        fn median(samples: &mut [f64]) -> f64 {
            assert!(samples.len() > 1, "median needs more than one sample");
            samples.sort_by(f64::total_cmp);
            let middle = samples.len() / 2;
            if samples.len() % 2 == 0 {
                (samples[middle - 1] + samples[middle]) / 2.
            } else {
                samples[middle]
            }
        }

        use crate::editor_timeline::{TimelineModel, TrackLanes};

        let project = slider_model_project(2_000);
        let lanes = TrackLanes::from_project(&project, true);

        // Warm caches and the allocator before sampling.
        for _ in 0..3 {
            drop(black_box(TimelineModel::build_with_lanes(
                &project, true, true, &lanes,
            )));
        }

        let mut samples = Vec::new();
        for _ in 0..10 {
            let started = Instant::now();
            for _ in 0..10 {
                drop(black_box(TimelineModel::build_with_lanes(
                    &project, true, true, &lanes,
                )));
            }
            samples.push(started.elapsed().as_secs_f64() * 1_000_000. / 10.);
        }

        let skipped = median(&mut samples);
        println!(
            "slider_edit timeline_segments=2000 samples=10 iterations_per_sample=10 \
             skipped_rebuild_us={skipped:.3}"
        );
    }
    #[test]
    fn caption_preview_and_sliders_keep_each_language_appearance_independent() {
        let mut settings = CaptionSettings {
            display_mode: CaptionDisplayMode::Bilingual,
            ..Default::default()
        };
        settings.edit_source_appearance(|a| {
            a.color = "#FF0000".into();
            a.outline = true;
        });
        settings.edit_translation_appearance(|a| {
            a.color = "#0000FF".into();
            a.shadow = true;
        });
        let source = settings.source_appearance();
        CaptionSlider::TranslationOutlineWidth.write(&mut settings, 12.);
        CaptionSlider::TranslationShadowOpacity.write(&mut settings, 50.);
        CaptionSlider::TranslationShadowSize.write(&mut settings, 14.);
        assert_eq!(settings.source_appearance(), source);
        let translated = settings.resolved_translation_appearance();
        CaptionSlider::OutlineWidth.write(&mut settings, 3.);
        CaptionSlider::ShadowOpacity.write(&mut settings, 25.);
        assert_eq!(settings.resolved_translation_appearance(), translated);
        let lines = caption_preview_lines(&settings);
        assert_eq!(lines[0].appearance.color, "#FF0000");
        assert_eq!(lines[1].appearance.color, "#0000FF");
        assert_eq!(lines[1].appearance.outline_width, 12.);
        assert_eq!(lines[1].appearance.shadow_opacity, 50);
        assert_eq!(lines[1].appearance.shadow_size, 14.);
    }
}
