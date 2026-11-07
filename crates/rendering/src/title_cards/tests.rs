use super::*;
use crate::{takeover::InterpolatedTakeover, text::prepare_texts};
use cap_project::{TextLayout, TitleCardRole};

fn inside(bounds: [f32; 4], output: (u32, u32)) -> bool {
    bounds[0] >= 0.0
        && bounds[1] >= 0.0
        && bounds[2] <= output.0 as f32
        && bounds[3] <= output.1 as f32
        && bounds[0] < bounds[2]
        && bounds[1] < bounds[3]
}

fn separated(left: [f32; 4], right: [f32; 4]) -> bool {
    left[2] <= right[0] || right[2] <= left[0] || left[3] <= right[1] || right[3] <= left[1]
}

#[test]
fn templates_keep_text_inside_safe_edges_in_all_three_aspects() {
    for output in [(1920, 1080), (1080, 1920), (1080, 1080)] {
        for template in [
            TitleCardTemplate::Hero,
            TitleCardTemplate::Editorial,
            TitleCardTemplate::Split,
        ] {
            for image in [false, true] {
                let layout = title_card_layout(output, template, image).unwrap();
                assert!(inside(layout.text, output));
                assert!(layout.text[0] >= output.0.min(output.1) as f32 * 0.08);
                assert!(output.0 as f32 - layout.text[2] >= output.0.min(output.1) as f32 * 0.08);
                for rect in [layout.image, layout.secondary, layout.accent]
                    .into_iter()
                    .flatten()
                {
                    assert!(inside(rect, output));
                }
                if template == TitleCardTemplate::Split
                    && let Some(other) = layout.image.or(layout.secondary)
                {
                    assert!(separated(layout.text, other));
                }
            }
        }
    }
}

#[test]
fn split_changes_from_columns_to_rows_in_portrait_without_cropping_text() {
    let wide = title_card_layout((1920, 1080), TitleCardTemplate::Split, true).unwrap();
    assert!(wide.text[2] < wide.image.unwrap()[0]);
    let tall = title_card_layout((1080, 1920), TitleCardTemplate::Split, true).unwrap();
    assert!(tall.image.unwrap()[3] < tall.text[1]);
    assert!(tall.text[2] - tall.text[0] > 800.0);
}

#[test]
fn cover_keeps_source_aspect_and_moves_crop_toward_the_selected_focus() {
    let bounds = [0.0, 0.0, 1080.0, 1920.0];
    let (_, left) = image_placement(
        (1920, 1080),
        bounds,
        TitleCardImageFit::Cover,
        XY::new(0.0, 0.5),
    )
    .unwrap();
    let (_, right) = image_placement(
        (1920, 1080),
        bounds,
        TitleCardImageFit::Cover,
        XY::new(1.0, 0.5),
    )
    .unwrap();
    assert_eq!(left[0], 0.0);
    assert!((right[0] + right[2] - 1.0).abs() < 1e-6);
    assert!(right[0] > left[0]);
    for source in [(1920, 1080), (1080, 1920), (1000, 1000)] {
        for output in [(1920, 1080), (1080, 1920), (1080, 1080)] {
            let destination = [0.0, 0.0, output.0 as f32, output.1 as f32];
            let (rect, uv) = image_placement(
                source,
                destination,
                TitleCardImageFit::Cover,
                XY::new(0.3, 0.7),
            )
            .unwrap();
            assert_eq!(rect, destination);
            assert!(
                uv[0] >= 0.0
                    && uv[1] >= 0.0
                    && uv[0] + uv[2] <= 1.00001
                    && uv[1] + uv[3] <= 1.00001
            );
            let source_crop_aspect = source.0 as f32 * uv[2] / (source.1 as f32 * uv[3]);
            assert!((source_crop_aspect - output.0 as f32 / output.1 as f32).abs() < 1e-5);
        }
    }
}

#[test]
fn contain_preserves_the_entire_image_and_centers_letterboxing() {
    let bounds = [100.0, 200.0, 900.0, 1400.0];
    let (rect, uv) = image_placement(
        (1600, 900),
        bounds,
        TitleCardImageFit::Contain,
        XY::new(0.5, 0.5),
    )
    .unwrap();
    assert_eq!(uv, [0.0, 0.0, 1.0, 1.0]);
    assert_eq!(rect[0], bounds[0]);
    assert_eq!(rect[2], bounds[2]);
    assert!((rect[1] + rect[3] - bounds[1] - bounds[3]).abs() < 1e-5);
    assert!(((rect[2] - rect[0]) / (rect[3] - rect[1]) - 16.0 / 9.0).abs() < 1e-5);
}

#[test]
fn measured_blocks_form_a_centered_group_without_empty_fixed_rows() {
    let container = [100.0, 200.0, 900.0, 1000.0];
    let blocks = stacked_bounds(container, &[28.0, 216.0, 80.0], 24.0).unwrap();
    assert_eq!(blocks[1][1] - blocks[0][3], 24.0);
    assert_eq!(blocks[2][1] - blocks[1][3], 24.0);
    assert_eq!(blocks[0][1] - container[1], container[3] - blocks[2][3]);
    let title_only = stacked_bounds(container, &[108.0], 24.0).unwrap();
    assert_eq!(title_only[0][1], 546.0);
    assert!(stacked_bounds(container, &[800.0, 20.0], 24.0).is_none());
}

#[test]
fn title_card_is_complete_on_the_first_and_last_frames_and_absent_at_the_next_boundary() {
    for role in [TitleCardRole::Intro, TitleCardRole::Outro] {
        let mut card = TextSegment::new_title_card(role, "第一帧就应清晰可见");
        card.start = if role == TitleCardRole::Intro {
            0.0
        } else {
            10.0
        };
        card.end = card.start + 2.0;
        card.animation_in = cap_project::TextAnimation::Fade;
        card.animation_out = cap_project::TextAnimation::Letters;
        card.layout_transition = 1.0;
        let segments = [card];
        for time in [
            segments[0].start,
            segments[0].start + 0.5,
            segments[0].end - 1.0 / 30.0,
        ] {
            assert!(active_title_card(time, &segments, &[]).is_some());
            let takeover = InterpolatedTakeover::sample(time, &segments).unwrap();
            assert_eq!(takeover.t, 1.0);
            assert_eq!(takeover.layout, TextLayout::Fullscreen);
            assert!(prepare_texts(XY::new(1920, 1080), time, &segments, &[]).is_empty());
        }
        assert!(active_title_card(segments[0].end, &segments, &[]).is_none());
        assert!(InterpolatedTakeover::sample(segments[0].end, &segments).is_none());
        assert!(active_title_card(segments[0].start, &segments, &[0]).is_none());
    }
}

#[test]
fn legacy_fullscreen_text_keeps_its_existing_edge_transition() {
    let mut legacy = TextSegment::new_title_card(TitleCardRole::Intro, "Legacy title");
    legacy.title_card = None;
    legacy.layout_transition = 0.5;
    let segments = [legacy];
    assert!(active_title_card(0.0, &segments, &[]).is_none());
    assert!(InterpolatedTakeover::sample(0.0, &segments).is_none());
    assert!(InterpolatedTakeover::sample(0.25, &segments).unwrap().t < 1.0);
    assert_eq!(InterpolatedTakeover::sample(1.0, &segments).unwrap().t, 1.0);
    assert_eq!(
        prepare_texts(XY::new(1920, 1080), 1.0, &segments, &[]).len(),
        1
    );
}

#[test]
fn invalid_dimensions_and_nonfinite_geometry_are_rejected() {
    assert!(title_card_layout((0, 1080), TitleCardTemplate::Hero, false).is_none());
    assert!(
        image_placement(
            (0, 10),
            [0.0, 0.0, 100.0, 100.0],
            TitleCardImageFit::Cover,
            XY::new(0.5, 0.5)
        )
        .is_none()
    );
    assert!(
        image_placement(
            (10, 10),
            [0.0, 0.0, 100.0, 100.0],
            TitleCardImageFit::Cover,
            XY::new(f64::NAN, 0.5)
        )
        .is_none()
    );
    assert!(stacked_bounds([0.0, 0.0, 100.0, 100.0], &[f32::INFINITY], 0.0).is_none());
}

fn title_fonts() -> glyphon::FontSystem {
    let mut db = glyphon::fontdb::Database::new();
    db.load_font_file(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/desktop-gpui/assets/caption-fonts/NotoSansSC-Bold.otf"),
    )
    .unwrap();
    glyphon::FontSystem::new_with_locale_and_db("zh-CN".to_owned(), db)
}

fn measure_title(
    fonts: &mut glyphon::FontSystem,
    content: &str,
    size: f32,
    width: f32,
) -> (f32, f32) {
    let metrics = glyphon::Metrics::new(size, size * 1.16);
    let mut buffer = glyphon::Buffer::new(fonts, metrics);
    buffer.set_size(fonts, Some(width), None);
    buffer.set_wrap(fonts, glyphon::Wrap::Word);
    buffer.set_text(
        fonts,
        content,
        &glyphon::Attrs::new()
            .family(glyphon::Family::Name("Noto Sans SC"))
            .weight(glyphon::Weight(700)),
        glyphon::Shaping::Advanced,
    );
    buffer.shape_until_scroll(fonts, false);
    buffer
        .layout_runs()
        .fold((0.0_f32, 0.0), |(width, height), run| {
            (width.max(run.line_w), height + metrics.line_height)
        })
}

#[test]
fn title_wrap_balances_real_cjk_glyphs_at_punctuation() {
    let layout = title_card_layout((1920, 1080), TitleCardTemplate::Split, true).unwrap();
    let width = layout.text[2] - layout.text[0];
    let mut fonts = title_fonts();
    let title = "把复杂操作，讲清楚";
    assert!(measure_title(&mut fonts, title, 96.0, width).1 > 96.0 * 1.16);
    let wrapped = balanced_title_lines(title, width, 96.0 * 1.16, |content| {
        measure_title(&mut fonts, content, 96.0, width)
    })
    .unwrap();
    assert_eq!(wrapped, "把复杂操作，\n讲清楚");
    let (actual_width, actual_height) = measure_title(&mut fonts, &wrapped, 96.0, width);
    assert!(actual_width <= width);
    assert!((actual_height - 96.0 * 1.16 * 2.0).abs() < 0.01);
}

#[test]
fn title_wrap_preserves_english_words_and_chinese_closing_punctuation() {
    let mut fonts = title_fonts();
    for (title, width) in [
        ("Create clear product walkthroughs", 1100.0),
        ("把操作讲清楚。", 600.0),
    ] {
        let wrapped = balanced_title_lines(title, width, 96.0 * 1.16, |content| {
            measure_title(&mut fonts, content, 96.0, width)
        })
        .unwrap();
        assert_eq!(wrapped.lines().count(), 2);
        for line in wrapped.lines() {
            assert!(line.chars().filter(|ch| ch.is_alphanumeric()).count() >= 2);
            assert!(!"，。！？；：、,.!?;:)]}）】》」』”’".contains(line.chars().next().unwrap()));
        }
        if title.is_ascii() {
            assert_eq!(
                wrapped.split_whitespace().collect::<Vec<_>>(),
                title.split_whitespace().collect::<Vec<_>>()
            );
        } else {
            assert_eq!(wrapped.replace('\n', ""), title);
        }
    }
}

#[test]
fn title_wrap_reduces_short_type_instead_of_leaving_a_single_character() {
    let mut fonts = title_fonts();
    assert!(
        balanced_title_lines("讲清楚", 200.0, 96.0 * 1.16, |content| {
            measure_title(&mut fonts, content, 96.0, 200.0)
        })
        .is_none()
    );
    assert_eq!(
        balanced_title_lines("讲清楚", 200.0, 64.0 * 1.16, |content| {
            measure_title(&mut fonts, content, 64.0, 200.0)
        })
        .as_deref(),
        Some("讲清楚")
    );
}

#[test]
fn title_wrap_preserves_explicit_breaks_and_bounds_candidate_work() {
    for title in ["把复杂操作，讲清\n楚".to_owned(), "字".repeat(65)] {
        assert_eq!(
            balanced_title_lines(&title, 600.0, 100.0, |_| panic!(
                "must preserve authored text"
            )),
            Some(title)
        );
    }
    let mut calls = 0;
    let title = "字".repeat(64);
    let wrapped = balanced_title_lines(&title, 320.0, 20.0, |content| {
        calls += 1;
        let width = content.chars().count() as f32 * 10.0;
        (width.min(320.0), (width / 320.0).ceil() * 20.0)
    })
    .unwrap();
    assert!(calls <= 129);
    assert_eq!(
        wrapped
            .lines()
            .map(str::chars)
            .map(Iterator::count)
            .collect::<Vec<_>>(),
        vec![32, 32]
    );
}
