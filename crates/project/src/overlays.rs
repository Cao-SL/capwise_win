use crate::{
    ImageSegment, MaskKind, MaskSegment, OverlayTrackKind, TextSegment, TimelineConfiguration, XY,
    mask_effect_contract,
};

pub fn default_text_segment(start: f64, end: f64, track: u32) -> TextSegment {
    TextSegment {
        start,
        end,
        track,
        enabled: true,
        content: "Text".to_string(),
        center: XY::new(0.5, 0.5),
        size: XY::new(0.1, 0.055),
        font_family: "Noto Sans SC".to_string(),
        font_size: 48.0,
        font_weight: 700.0,
        italic: false,
        color: "#111111".to_string(),
        background_color: None,
        background_style: Default::default(),
        uppercase: false,
        stroke_width: 0.0,
        stroke_color: "#000000".to_string(),
        gradient_color: None,
        glow: 0.0,
        fade_duration: 0.15,
        align: Default::default(),
        letter_spacing: 0.0,
        line_height: 1.2,
        opacity: 1.0,
        shadow: 0.0,
        animation_in: Default::default(),
        animation_out: Default::default(),
        animation_in_duration: 0.15,
        animation_out_duration: 0.15,
        layout: Default::default(),
        layout_transition: 0.5,
        title_card: None,
    }
}

pub fn default_mask_segment(start: f64, end: f64, track: u32) -> MaskSegment {
    let contract = mask_effect_contract();
    MaskSegment {
        start,
        end,
        track,
        enabled: true,
        mask_type: MaskKind::Sensitive,
        center: XY::new(0.5, 0.5),
        size: XY::new(0.35, 0.35),
        feather: 0.1,
        opacity: 1.0,
        pixelation: contract.blur_encoding_offset + contract.default_amount,
        darkness: 0.5,
        fade_duration: 0.0,
        keyframes: Default::default(),
    }
}

pub fn insert_text_segment(timeline: &mut TimelineConfiguration, segment: TextSegment) -> usize {
    insert(&mut timeline.text_segments, segment, |item| {
        (item.track, item.start, item.end)
    })
}

pub fn insert_mask_segment(timeline: &mut TimelineConfiguration, segment: MaskSegment) -> usize {
    insert(&mut timeline.mask_segments, segment, |item| {
        (item.track, item.start, item.end)
    })
}

pub fn insert_image_segment(timeline: &mut TimelineConfiguration, segment: ImageSegment) -> usize {
    insert(&mut timeline.image_segments, segment, |item| {
        (item.track, item.start, item.end)
    })
}

pub fn overlay_front_layer(timeline: &TimelineConfiguration, start: f64, end: f64) -> u32 {
    timeline
        .overlay_spans()
        .filter(|(_, _, _, segment_start, segment_end)| {
            *segment_start < end && start < *segment_end
        })
        .map(|(_, _, layer, _, _)| layer.saturating_add(1))
        .max()
        .unwrap_or(0)
}

fn insert<T>(segments: &mut Vec<T>, segment: T, key: impl Fn(&T) -> (u32, f64, f64)) -> usize {
    let (track, start, end) = key(&segment);
    segments.push(segment);
    segments.sort_by(|a, b| {
        let (a_track, a_start, a_end) = key(a);
        let (b_track, b_start, b_end) = key(b);
        a_track
            .cmp(&b_track)
            .then(a_start.total_cmp(&b_start))
            .then(a_end.total_cmp(&b_end))
    });
    segments
        .iter()
        .rposition(|item| key(item) == (track, start, end))
        .unwrap_or(segments.len().saturating_sub(1))
}

#[derive(Clone, Copy, Debug)]
pub struct OverlayRange {
    pub start: f64,
    pub end: f64,
    pub track: u32,
}

impl OverlayRange {
    pub fn new(
        timeline: &TimelineConfiguration,
        kind: OverlayTrackKind,
        start: f64,
        end: f64,
        total: f64,
    ) -> Result<Self, &'static str> {
        if !start.is_finite()
            || !end.is_finite()
            || !total.is_finite()
            || start < 0.
            || start >= end
            || end > total
        {
            return Err("Choose a positive time range within the video");
        }
        if kind != OverlayTrackKind::Text
            && timeline.text_segments.iter().any(|segment| {
                segment.enabled
                    && segment.title_card.is_some()
                    && segment.start < end
                    && start < segment.end
            })
        {
            return Err("Choose a range on the recording, outside the intro and outro");
        }
        let track = overlay_front_layer(timeline, start, end);
        if track == u32::MAX {
            return Err("There are too many overlay layers");
        }
        Ok(Self { start, end, track })
    }

    pub fn at_playhead(
        timeline: &TimelineConfiguration,
        kind: OverlayTrackKind,
        start: f64,
        length: f64,
        total: f64,
    ) -> Result<Self, &'static str> {
        if !length.is_finite() || length <= 0. {
            return Err("Choose a positive overlay duration");
        }
        let end = if kind == OverlayTrackKind::Text {
            total
        } else {
            timeline
                .text_segments
                .iter()
                .filter(|segment| {
                    segment.enabled && segment.title_card.is_some() && segment.start >= start
                })
                .fold(total, |end, segment| end.min(segment.start))
        };
        Self::new(timeline, kind, start, (start + length).min(end), total)
    }

    pub fn text(self, content: String) -> TextSegment {
        TextSegment {
            content,
            ..default_text_segment(self.start, self.end, self.track)
        }
    }

    pub fn image(
        self,
        path: String,
        name: String,
        width: u32,
        height: u32,
        output_aspect: f64,
    ) -> ImageSegment {
        let ratio = f64::from(width) / f64::from(height) / output_aspect;
        let size = if ratio >= 1. {
            XY::new(0.3, 0.3 / ratio)
        } else {
            XY::new(0.3 * ratio, 0.3)
        };
        ImageSegment {
            start: self.start,
            end: self.end,
            track: self.track,
            path,
            name,
            size,
            ..Default::default()
        }
    }

    pub fn redact(
        self,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        pixelate: bool,
    ) -> Result<MaskSegment, &'static str> {
        if [x, y, width, height].iter().any(|value| !value.is_finite())
            || x < 0.
            || y < 0.
            || width <= 0.
            || height <= 0.
            || x + width > 1. + f64::EPSILON
            || y + height > 1. + f64::EPSILON
        {
            return Err("The mask rectangle must fit within the normalized canvas (0–1)");
        }
        let mut segment = default_mask_segment(self.start, self.end, self.track);
        segment.center = XY::new(x + width / 2., y + height / 2.);
        segment.size = XY::new(width, height);
        segment.feather = 0.;
        if pixelate {
            segment.pixelation = mask_effect_contract().default_amount;
        }
        Ok(segment)
    }
}
