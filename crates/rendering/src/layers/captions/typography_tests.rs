use super::{
    CaptionText, CaptionTextColors, CaptionsLayer, ShapedCaptionText,
    gpu_tests::{constants, frames, render_pixels, uniforms},
};
use cap_project::{CaptionDisplayMode, CaptionSettings, CaptionTextStyle, CaptionTrackSegment, XY};
use glyphon::{Color, Family, FontSystem, Weight};

fn fonts() -> FontSystem {
    let mut db = glyphon::fontdb::Database::new();
    for bytes in [
        include_bytes!(
            "../../../../../apps/desktop-gpui/assets/caption-fonts/NotoSansSC-Medium.otf"
        )
        .as_slice(),
        include_bytes!("../../../../../apps/desktop-gpui/assets/caption-fonts/Inter-Regular.ttf")
            .as_slice(),
        include_bytes!(
            "../../../../../apps/desktop-gpui/assets/caption-fonts/SourceSerif4-Regular.ttf"
        )
        .as_slice(),
    ] {
        db.load_font_data(bytes.to_vec());
    }
    FontSystem::new_with_locale_and_db("zh-CN".into(), db)
}

fn layer(constants: &crate::RenderVideoConstants) -> CaptionsLayer {
    let mut layer = CaptionsLayer::new(&constants.device, &constants.queue);
    layer.font_system = fonts();
    layer
}

fn segment(source: &str) -> CaptionTrackSegment {
    serde_json::from_value(serde_json::json!({
        "id": "caption", "text": source, "start": 0.0, "end": 4.0, "words": []
    }))
    .unwrap()
}

fn settings(source: &str, translated: &str) -> CaptionSettings {
    let mut settings = CaptionSettings {
        enabled: true,
        display_mode: CaptionDisplayMode::Bilingual,
        font: "Noto Sans SC".into(),
        size: 72,
        font_weight: 500,
        animation: "none".into(),
        background_opacity: 0,
        fade_duration: 0.0,
        ..Default::default()
    };
    settings.set_translation("caption", source, translated.into(), "en".into());
    settings
}

fn render(
    constants: &crate::RenderVideoConstants,
    layer: &mut CaptionsLayer,
    settings: &CaptionSettings,
    segment: &CaptionTrackSegment,
    size: XY<u32>,
) -> Vec<u8> {
    let mut uniforms = uniforms(
        constants,
        settings.clone(),
        std::slice::from_ref(segment),
        size,
    );
    uniforms.frame_number = 60;
    layer.prepare(&uniforms, &frames(size), size, constants);
    assert!(layer.has_caption);
    render_pixels(constants, layer, size)
}

fn assert_pixels_equal(actual: &[u8], expected: &[u8]) {
    assert_eq!(actual.len(), expected.len());
    let different = actual.iter().zip(expected).filter(|(a, b)| a != b).count();
    assert_eq!(different, 0, "rendered bytes differ");
}

#[test]
fn typography_cache_changes_family_and_size_without_reusing_old_glyphs() {
    let mut fonts = fonts();
    let mut text = CaptionText::new();
    let shaped = |family, size: f32| ShapedCaptionText {
        text: "A bilingual caption".into(),
        font_size: size.to_bits(),
        wrap_width: 800.0_f32.to_bits(),
        family: Family::Name(family),
        weight: Weight::NORMAL,
        colors: CaptionTextColors::Uniform(Color::rgb(255, 255, 255)),
        wrap: true,
        balance: false,
    };
    assert!(text.update(&mut fonts, shaped("Inter", 40.0)));
    let first = text
        .buffer
        .layout_runs()
        .flat_map(|run| run.glyphs)
        .map(|glyph| glyph.font_id)
        .collect::<Vec<_>>();
    assert!(!first.is_empty());
    assert!(!text.update(&mut fonts, shaped("Inter", 40.0)));
    assert!(text.update(&mut fonts, shaped("Source Serif 4", 40.0)));
    let second = text
        .buffer
        .layout_runs()
        .flat_map(|run| run.glyphs)
        .map(|glyph| glyph.font_id)
        .collect::<Vec<_>>();
    assert!(second.iter().all(|id| *id != first[0]));
    let before = text.buffer.layout_runs().next().unwrap().line_height;
    assert!(text.update(&mut fonts, shaped("Source Serif 4", 60.0)));
    assert!(text.buffer.layout_runs().next().unwrap().line_height > before);
}

#[test]
#[ignore = "requires a working wgpu adapter"]
fn independent_translation_typography_changes_pixels_and_reuses_cache_correctly() {
    let size = XY::new(640, 720);
    let constants = constants(size);
    let source = "开始录制";
    let segment = segment(source);
    let mut settings = settings(source, "Start recording");
    settings.translation_style = Some(CaptionTextStyle {
        font: "Inter".into(),
        size: 40,
    });
    let mut reused = layer(&constants);
    let first = render(&constants, &mut reused, &settings, &segment, size);
    assert!(first.chunks_exact(4).any(|pixel| pixel[3] != 0));
    assert_eq!(
        reused.text.shaped.as_ref().unwrap().family,
        Family::Name("Noto Sans SC")
    );
    assert_eq!(
        reused.translation_text.shaped.as_ref().unwrap().family,
        Family::Name("Inter")
    );
    assert!(
        reused.text.buffer.metrics().font_size > reused.translation_text.buffer.metrics().font_size
    );
    let mut previous = first;
    for next in [
        CaptionTextStyle {
            font: "Inter".into(),
            size: 64,
        },
        CaptionTextStyle {
            font: "Source Serif 4".into(),
            size: 64,
        },
        CaptionTextStyle {
            font: "Inter".into(),
            size: 40,
        },
    ] {
        settings.translation_style = Some(next);
        let pixels = render(&constants, &mut reused, &settings, &segment, size);
        assert!(pixels != previous);
        assert_pixels_equal(
            &pixels,
            &render(
                &constants,
                &mut layer(&constants),
                &settings,
                &segment,
                size,
            ),
        );
        previous = pixels;
    }
    let translation_key = reused.translation_text.shaped.as_ref().unwrap().font_size;
    settings.set_source_font_size(90);
    let changed = render(&constants, &mut reused, &settings, &segment, size);
    assert!(changed != previous);
    assert_eq!(
        translation_key,
        reused.translation_text.shaped.as_ref().unwrap().font_size
    );
    assert_pixels_equal(
        &changed,
        &render(
            &constants,
            &mut layer(&constants),
            &settings,
            &segment,
            size,
        ),
    );
    let mut latin_segment = segment.clone();
    latin_segment.text = "Read the demo".into();
    settings.translations.clear();
    settings.set_translation(
        "caption",
        &latin_segment.text,
        "Start recording".into(),
        "en".into(),
    );
    let before_font = render(&constants, &mut reused, &settings, &latin_segment, size);
    settings.set_source_font("Inter".into());
    let after_font = render(&constants, &mut reused, &settings, &latin_segment, size);
    assert!(after_font != before_font);
    assert_eq!(
        reused.translation_text.shaped.as_ref().unwrap().font_size,
        translation_key
    );
    assert_pixels_equal(
        &after_font,
        &render(
            &constants,
            &mut layer(&constants),
            &settings,
            &latin_segment,
            size,
        ),
    );
    settings.outline = true;
    settings.outline_width = 8.0;
    settings.shadow = true;
    settings.shadow_size = 8.0;
    settings.edit_translation_appearance(|a| {
        a.outline = true;
        a.outline_width = 8.;
        a.shadow = true;
        a.shadow_size = 8.;
    });
    let effects = render(&constants, &mut reused, &settings, &latin_segment, size);
    assert!(effects != after_font);
    assert_eq!(
        reused.translation_outline_text.buffer.metrics().font_size,
        reused.translation_text.buffer.metrics().font_size
    );
    assert_eq!(
        reused.translation_shadow_text.buffer.metrics().font_size,
        reused.translation_text.buffer.metrics().font_size
    );
    assert_eq!(
        reused.outline_text.buffer.metrics().font_size,
        reused.text.buffer.metrics().font_size
    );
    assert_pixels_equal(
        &effects,
        &render(
            &constants,
            &mut layer(&constants),
            &settings,
            &latin_segment,
            size,
        ),
    );
}

#[test]
#[ignore = "requires a working wgpu adapter"]
fn caption_modes_and_missing_translation_keep_the_correct_typography_pixels() {
    let size = XY::new(640, 1080);
    let constants = constants(size);
    let segment = segment("Ship today");
    let mut legacy = settings(&segment.text, "Deliver soon");
    legacy.font = "Inter".into();
    legacy.size = 48;
    let mut reused = layer(&constants);
    let legacy_pixels = render(&constants, &mut reused, &legacy, &segment, size);
    let mut null_json = serde_json::to_value(&legacy).unwrap();
    null_json["translationStyle"] = serde_json::Value::Null;
    let null: CaptionSettings = serde_json::from_value(null_json).unwrap();
    assert_pixels_equal(
        &legacy_pixels,
        &render(&constants, &mut reused, &null, &segment, size),
    );
    let mut independent = legacy.clone();
    independent.translation_style = Some(legacy.resolved_translation_style());
    assert_pixels_equal(
        &legacy_pixels,
        &render(&constants, &mut reused, &independent, &segment, size),
    );
    independent.set_translation_font("Source Serif 4".into());
    independent.set_translation_font_size(36);
    independent.display_mode = CaptionDisplayMode::Original;
    legacy.display_mode = CaptionDisplayMode::Original;
    let original = render(&constants, &mut reused, &legacy, &segment, size);
    assert_pixels_equal(
        &original,
        &render(&constants, &mut reused, &independent, &segment, size),
    );
    independent.display_mode = CaptionDisplayMode::Translated;
    let translated = render(&constants, &mut reused, &independent, &segment, size);
    let mut old_translated = legacy.clone();
    old_translated.display_mode = CaptionDisplayMode::Translated;
    old_translated.font = "Source Serif 4".into();
    old_translated.size = 36;
    assert_pixels_equal(
        &translated,
        &render(&constants, &mut reused, &old_translated, &segment, size),
    );
    assert!(!reused.has_highlight);
    for mode in [
        CaptionDisplayMode::Translated,
        CaptionDisplayMode::Bilingual,
    ] {
        independent.display_mode = mode;
        let mut missing = independent.clone();
        missing.translations.clear();
        assert_pixels_equal(
            &original,
            &render(&constants, &mut reused, &missing, &segment, size),
        );
        let mut stale = independent.clone();
        stale.translations.get_mut("caption").unwrap().source_text = "Changed source".into();
        assert_pixels_equal(
            &original,
            &render(&constants, &mut reused, &stale, &segment, size),
        );
    }
}

#[test]
#[ignore = "requires a working wgpu adapter"]
fn independent_bilingual_wraps_inside_portrait_and_highlights_only_source_pixels() {
    let size = XY::new(512, 768);
    let constants = constants(size);
    let source = "Source caption wraps into several lines so that both languages have their own measured height.";
    let translation = "Source remains unhighlighted in this translation, which also wraps into several lines without overlapping the original text.";
    let mut segment = segment(source);
    segment.words = vec![cap_project::CaptionWord {
        text: "Source".into(),
        start: 0.0,
        end: 4.0,
        hidden: false,
    }];
    let mut settings = settings(source, translation);
    settings.size = 80;
    settings.translation_style = Some(CaptionTextStyle {
        font: "Inter".into(),
        size: 64,
    });
    settings.active_word_highlight = true;
    settings.highlight_color = "#FF0000".into();
    settings.word_transition_duration = 0.0;
    let mut reused = layer(&constants);
    for highlight in ["color", "pill", "last-word-pill"] {
        if highlight == "last-word-pill" {
            settings.size = 100;
            settings.set_translation_font_size(12);
            segment.words[0].text = "height.".into();
        }
        settings.highlight_style = if highlight == "color" {
            "color"
        } else {
            "pill"
        }
        .into();
        let pixels = render(&constants, &mut reused, &settings, &segment, size);
        let source_runs = reused.text.buffer.layout_runs().collect::<Vec<_>>();
        let translated_runs = reused
            .translation_text
            .buffer
            .layout_runs()
            .collect::<Vec<_>>();
        assert!(source_runs.len() > 1);
        if highlight != "last-word-pill" {
            assert!(translated_runs.len() > 1);
        }
        assert!(
            source_runs
                .iter()
                .chain(translated_runs.iter())
                .all(|run| run.line_w <= size.x as f32 * 0.9 + 0.1)
        );
        let frame = reused.frame_box.unwrap().bounds;
        assert!(
            frame[0] >= 0.0
                && frame[1] >= 0.0
                && frame[2] <= size.x as f32
                && frame[3] <= size.y as f32
        );
        let source_height = source_runs
            .iter()
            .map(|run| run.line_top + run.line_height)
            .fold(0.0_f32, f32::max);
        let translation_height = translated_runs
            .iter()
            .map(|run| run.line_top + run.line_height)
            .fold(0.0_f32, f32::max);
        let font_size = reused.text.buffer.metrics().font_size;
        let fit = (frame[3] - frame[1]) / (source_height + translation_height + font_size);
        let translation_top = frame[1] + (font_size * 0.5 + source_height) * fit;
        let red_rows = pixels
            .chunks_exact(4)
            .enumerate()
            .filter(|(_, pixel)| {
                pixel[0] > 100 && pixel[0] > pixel[1].saturating_add(30) && pixel[3] > 50
            })
            .map(|(index, _)| index / size.x as usize)
            .collect::<Vec<_>>();
        assert!(!red_rows.is_empty());
        assert!(red_rows.iter().all(|row| (*row as f32) < translation_top));
        assert!(matches!(
            &reused.translation_text.shaped.as_ref().unwrap().colors,
            CaptionTextColors::Uniform(_)
        ));
        assert_pixels_equal(
            &pixels,
            &render(
                &constants,
                &mut layer(&constants),
                &settings,
                &segment,
                size,
            ),
        );
    }
}

#[test]
#[ignore = "requires a working wgpu adapter"]
fn caption_spacing_scales_for_landscape_portrait_and_keeps_manual_position() {
    for size in [XY::new(1280, 720), XY::new(720, 1280), XY::new(1920, 1080)] {
        let constants = constants(size);
        let segment = segment("开始录制");
        let mut settings = settings(&segment.text, "Start recording");
        settings.size = 42;
        settings.set_translation_font("Inter".into());
        settings.set_translation_font_size(30);
        settings.bottom_margin = Some(60);
        settings.position = "bottom-center".into();
        let mut reused = layer(&constants);
        render(&constants, &mut reused, &settings, &segment, size);
        let before = reused.frame_box.as_ref().unwrap().bounds;
        settings.translation_gap = Some(10);
        let pixels = render(&constants, &mut reused, &settings, &segment, size);
        let after = reused.frame_box.as_ref().unwrap().bounds;
        let scale = size.y as f32 / 1080.0;
        assert!((size.y as f32 - after[3] - 60.0 * scale).abs() < 0.01);
        assert!((before[1] - after[1] - 10.0 * scale).abs() < 0.01);
        assert!(after[0] >= 0.0 && after[2] <= size.x as f32 && after[1] >= 0.0);
        assert_pixels_equal(
            &pixels,
            &render(
                &constants,
                &mut layer(&constants),
                &settings,
                &segment,
                size,
            ),
        );
        settings.translation_style = None;
        let inherited = render(&constants, &mut reused, &settings, &segment, size);
        settings.translation_style = Some(settings.resolved_translation_style());
        assert_pixels_equal(
            &inherited,
            &render(&constants, &mut reused, &settings, &segment, size),
        );
        settings.translation_style = None;
        settings.translation_gap = Some(0);
        render(&constants, &mut reused, &settings, &segment, size);
        let zero_gap = reused.frame_box.as_ref().unwrap().bounds;
        settings.translation_gap = Some(20);
        render(&constants, &mut reused, &settings, &segment, size);
        let spaced = reused.frame_box.as_ref().unwrap().bounds;
        assert!((zero_gap[1] - spaced[1] - 20.0 * scale).abs() < 0.01);
        settings.position = "manual".into();
        settings.manual_position = Some(XY::new(0.4, 0.4));
        let manual = render(&constants, &mut reused, &settings, &segment, size);
        settings.bottom_margin = None;
        assert_pixels_equal(
            &manual,
            &render(&constants, &mut reused, &settings, &segment, size),
        );
    }
}

#[test]
fn balanced_caption_wrap_avoids_a_single_character_last_line_without_changing_font_size() {
    let mut fonts = fonts();
    let mut shaped = ShapedCaptionText {
        text: "中文和英文字幕，可以分别调整".into(),
        font_size: 42_f32.to_bits(),
        wrap_width: 560_f32.to_bits(),
        family: Family::Name("Noto Sans SC"),
        weight: Weight::MEDIUM,
        colors: CaptionTextColors::Uniform(Color::rgb(255, 255, 255)),
        wrap: true,
        balance: false,
    };
    let before = super::shape_caption_text(&mut fonts, &shaped);
    let widths = before
        .layout_runs()
        .map(|run| run.line_w)
        .collect::<Vec<_>>();
    assert_eq!(widths.len(), 2);
    assert!(widths[1] < widths[0] * 0.4);
    shaped.balance = true;
    let after = super::shape_caption_text(&mut fonts, &shaped);
    let balanced = after
        .layout_runs()
        .map(|run| run.line_w)
        .collect::<Vec<_>>();
    assert_eq!(balanced.len(), 2);
    assert!(balanced[1] >= balanced[0] * 0.4);
    assert_eq!(after.metrics().font_size, 42.0);
}

#[test]
#[ignore = "requires a working wgpu adapter"]
fn independent_caption_appearance_renders_source_and_translation_without_cache_coupling() {
    let size = XY::new(640, 720);
    let constants = constants(size);
    let segment = segment("Source");
    let mut settings = settings(&segment.text, "Translation");
    settings.font = "Inter".into();
    settings.translation_gap = Some(20);
    settings.edit_source_appearance(|a| {
        a.color = "#FF0000".into();
        a.outline = false;
        a.shadow = false;
    });
    settings.edit_translation_appearance(|a| {
        a.color = "#0000FF".into();
        a.outline = false;
        a.shadow = false;
        a.uppercase = true;
        a.font_weight = 400;
    });
    let mut reused = layer(&constants);
    let before = render(&constants, &mut reused, &settings, &segment, size);
    let red = |pixels: &[u8]| {
        pixels
            .chunks_exact(4)
            .filter(|p| p[0] > 150 && p[1] < 50 && p[2] < 50 && p[3] > 100)
            .count()
    };
    let blue = |pixels: &[u8]| {
        pixels
            .chunks_exact(4)
            .filter(|p| p[2] > 150 && p[1] < 50 && p[0] < 50 && p[3] > 100)
            .count()
    };
    assert!(red(&before) > 20 && blue(&before) > 20);
    assert_eq!(reused.text.shaped.as_ref().unwrap().text, "Source");
    assert_eq!(
        reused.translation_text.shaped.as_ref().unwrap().text,
        "TRANSLATION"
    );
    assert!(
        reused.text.shaped.as_ref().unwrap().colors
            == CaptionTextColors::Uniform(Color::rgba(255, 0, 0, 255))
    );
    settings.edit_translation_appearance(|a| {
        a.outline = true;
        a.outline_color = "#00FF00".into();
        a.outline_width = 10.;
        a.shadow = true;
        a.shadow_color = "#FFFF00".into();
        a.shadow_opacity = 80;
        a.shadow_size = 12.;
    });
    let effects = render(&constants, &mut reused, &settings, &segment, size);
    assert_ne!(before, effects);
    assert_eq!(red(&before), red(&effects));
    assert!(
        reused
            .translation_outline_text
            .shaped
            .as_ref()
            .unwrap()
            .colors
            == CaptionTextColors::Uniform(Color::rgba(0, 255, 0, 255))
    );
    assert_pixels_equal(
        &effects,
        &render(
            &constants,
            &mut layer(&constants),
            &settings,
            &segment,
            size,
        ),
    );
    settings.display_mode = CaptionDisplayMode::Translated;
    let only_translation = render(&constants, &mut reused, &settings, &segment, size);
    assert_eq!(red(&only_translation), 0);
    assert!(blue(&only_translation) > 20);
    assert_eq!(reused.text.shaped.as_ref().unwrap().text, "TRANSLATION");
    assert_pixels_equal(
        &only_translation,
        &render(
            &constants,
            &mut layer(&constants),
            &settings,
            &segment,
            size,
        ),
    );
    settings.display_mode = CaptionDisplayMode::Original;
    let only_source = render(&constants, &mut reused, &settings, &segment, size);
    assert!(red(&only_source) > 20);
    assert_eq!(blue(&only_source), 0);
    assert_pixels_equal(
        &only_source,
        &render(
            &constants,
            &mut layer(&constants),
            &settings,
            &segment,
            size,
        ),
    );
}
