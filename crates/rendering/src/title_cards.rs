use cap_project::{TextAlign, TextSegment, TitleCardImageFit, TitleCardTemplate, XY};

pub(crate) fn active_title_card<'a>(
    time: f64,
    segments: &'a [TextSegment],
    hidden: &[usize],
) -> Option<&'a TextSegment> {
    if !time.is_finite() {
        return None;
    }
    segments
        .iter()
        .enumerate()
        .filter(|(index, segment)| {
            segment.title_card.is_some()
                && segment.enabled
                && !hidden.contains(index)
                && segment.start.is_finite()
                && segment.end.is_finite()
                && time >= segment.start
                && time < segment.end
        })
        .max_by_key(|(index, segment)| (segment.track, *index))
        .map(|(_, segment)| segment)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TitleCardLayout {
    pub text: [f32; 4],
    pub image: Option<[f32; 4]>,
    pub secondary: Option<[f32; 4]>,
    pub accent: Option<[f32; 4]>,
    pub align: TextAlign,
    pub gap: f32,
    pub scale: f32,
}

pub(crate) fn title_card_layout(
    output: (u32, u32),
    template: TitleCardTemplate,
    has_image: bool,
) -> Option<TitleCardLayout> {
    if output.0 == 0 || output.1 == 0 {
        return None;
    }
    let width = output.0 as f32;
    let height = output.1 as f32;
    let short = width.min(height);
    let margin = short * 0.085;
    let gap = short * 0.026;
    let safe = [margin, margin, width - margin, height - margin];
    let scale = short / 1080.0;
    let full = [0.0, 0.0, width, height];
    let portrait = height > width * 1.15;
    let mut layout = TitleCardLayout {
        text: safe,
        image: has_image.then_some(full),
        secondary: None,
        accent: None,
        align: TextAlign::Center,
        gap,
        scale,
    };
    match template {
        TitleCardTemplate::Hero => {
            layout.text[1] = height * 0.19;
            layout.text[3] = height * 0.81;
        }
        TitleCardTemplate::Editorial => {
            layout.align = TextAlign::Left;
            layout.text[1] = height * 0.2;
            layout.text[2] = width - margin * 1.6;
            layout.text[3] = height * 0.84;
            layout.accent = Some([
                margin,
                height * 0.14,
                margin + short * 0.095,
                height * 0.14 + short * 0.005,
            ]);
        }
        TitleCardTemplate::Split => {
            layout.align = TextAlign::Left;
            if portrait {
                if has_image {
                    layout.image = Some([margin, margin, width - margin, height * 0.48]);
                    layout.text = [margin, height * 0.54, width - margin, height - margin];
                } else {
                    layout.text[1] = height * 0.15;
                    layout.text[3] = height * 0.57;
                    layout.secondary =
                        Some([margin, height * 0.67, width - margin, height - margin]);
                    layout.accent = Some([
                        margin,
                        height * 0.62,
                        width - margin,
                        height * 0.62 + short * 0.002,
                    ]);
                }
            } else {
                layout.text = [margin, height * 0.17, width * 0.49 - gap, height * 0.83];
                let right = [width * 0.53, margin, width - margin, height - margin];
                if has_image {
                    layout.image = Some(right);
                } else {
                    layout.image = None;
                    layout.secondary = Some([right[0], height * 0.25, right[2], height * 0.75]);
                    layout.accent = Some([
                        width * 0.51,
                        height * 0.2,
                        width * 0.51 + short * 0.002,
                        height * 0.8,
                    ]);
                }
            }
        }
    }
    Some(layout)
}

pub(crate) fn image_placement(
    source: (u32, u32),
    destination: [f32; 4],
    fit: TitleCardImageFit,
    focus: XY<f64>,
) -> Option<([f32; 4], [f32; 4])> {
    let width = destination[2] - destination[0];
    let height = destination[3] - destination[1];
    if source.0 == 0
        || source.1 == 0
        || destination.iter().any(|value| !value.is_finite())
        || width <= 0.0
        || height <= 0.0
        || !focus.x.is_finite()
        || !focus.y.is_finite()
    {
        return None;
    }
    let source_aspect = source.0 as f32 / source.1 as f32;
    let destination_aspect = width / height;
    let center = [
        (destination[0] + destination[2]) * 0.5,
        (destination[1] + destination[3]) * 0.5,
    ];
    match fit {
        TitleCardImageFit::Cover => {
            let crop_width = (destination_aspect / source_aspect).min(1.0);
            let crop_height = (source_aspect / destination_aspect).min(1.0);
            let x = (focus.x as f32 - crop_width * 0.5).clamp(0.0, 1.0 - crop_width);
            let y = (focus.y as f32 - crop_height * 0.5).clamp(0.0, 1.0 - crop_height);
            Some((destination, [x, y, crop_width, crop_height]))
        }
        TitleCardImageFit::Contain => {
            let fitted_width = width.min(height * source_aspect);
            let fitted_height = height.min(width / source_aspect);
            Some((
                [
                    center[0] - fitted_width * 0.5,
                    center[1] - fitted_height * 0.5,
                    center[0] + fitted_width * 0.5,
                    center[1] + fitted_height * 0.5,
                ],
                [0.0, 0.0, 1.0, 1.0],
            ))
        }
    }
}

pub(crate) fn stacked_bounds(
    container: [f32; 4],
    heights: &[f32],
    gap: f32,
) -> Option<Vec<[f32; 4]>> {
    if heights.is_empty()
        || heights
            .iter()
            .any(|height| !height.is_finite() || *height < 0.0)
        || !gap.is_finite()
        || gap < 0.0
    {
        return None;
    }
    let height = heights.iter().sum::<f32>() + gap * heights.len().saturating_sub(1) as f32;
    if height > container[3] - container[1] + 0.01 {
        return None;
    }
    let mut top = (container[1] + container[3] - height) * 0.5;
    Some(
        heights
            .iter()
            .map(|height| {
                let bounds = [container[0], top, container[2], top + height];
                top += height + gap;
                bounds
            })
            .collect(),
    )
}

pub(crate) fn balanced_title_lines(
    content: &str,
    max_width: f32,
    line_height: f32,
    mut measure: impl FnMut(&str) -> (f32, f32),
) -> Option<String> {
    if content.contains(['\n', '\r']) || content.chars().take(65).count() > 64 {
        return Some(content.to_owned());
    }
    let (_, natural_height) = measure(content);
    if natural_height <= line_height + 0.01 || natural_height > line_height * 2.0 + 0.01 {
        return Some(content.to_owned());
    }
    let mut best: Option<(f32, String)> = None;
    let mut previous = None;
    for (index, next) in content.char_indices() {
        let Some(before) = previous.replace(next) else {
            continue;
        };
        if !(before.is_whitespace() || next.is_whitespace() || is_cjk(before) || is_cjk(next))
            || "，。！？；：、,.!?;:)]}）】》」』”’".contains(next)
            || "([{（【《「『“‘".contains(before)
        {
            continue;
        }
        let left = content[..index].trim_end();
        let right = content[index..].trim_start();
        if left.chars().filter(|ch| ch.is_alphanumeric()).count() < 2
            || right.chars().filter(|ch| ch.is_alphanumeric()).count() < 2
        {
            continue;
        }
        let (left_width, left_height) = measure(left);
        let (right_width, right_height) = measure(right);
        if [left_width, right_width]
            .iter()
            .any(|width| !width.is_finite() || *width > max_width + 0.01)
            || [left_height, right_height]
                .iter()
                .any(|height| !height.is_finite() || *height > line_height + 0.01)
        {
            continue;
        }
        let punctuation = left
            .chars()
            .next_back()
            .is_some_and(|ch| "，。！？；：、,.!?;:".contains(ch));
        let score = (left_width - right_width).abs() / max_width
            + if punctuation { 0.0 } else { 0.35 }
            + if left_width < right_width {
                0.0001
            } else {
                0.0
            };
        if best
            .as_ref()
            .is_none_or(|(best_score, _)| score < *best_score)
        {
            best = Some((score, format!("{left}\n{right}")));
        }
    }
    best.map(|(_, content)| content)
}

fn is_cjk(ch: char) -> bool {
    matches!(ch, '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}' | '\u{20000}'..='\u{323af}')
}

#[cfg(test)]
mod tests;
