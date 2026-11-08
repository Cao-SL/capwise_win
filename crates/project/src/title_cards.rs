use std::fmt;

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::{
    ProjectConfiguration, TextAlign, TextAnimation, TextBackgroundStyle, TextLayout, TextSegment,
    TimelineConfiguration, XY,
};

pub const TITLE_CARD_TEMPLATE_VERSION: u32 = 1;
pub const MIN_TITLE_CARD_DURATION: f64 = 0.1;
pub const MAX_TITLE_CARD_DURATION: f64 = 30.0;

#[derive(Type, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TitleCardRole {
    Intro,
    Outro,
}

#[derive(Type, Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TitleCardTemplate {
    #[default]
    Hero,
    Editorial,
    Split,
}

#[derive(Type, Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TitleCardImageFit {
    #[default]
    Cover,
    Contain,
}

#[derive(Type, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum TitleCardBackground {
    Solid {
        color: String,
    },
    ProjectImage {
        path: String,
        #[serde(default)]
        fit: TitleCardImageFit,
        #[serde(default = "center_focus")]
        focus: XY<f64>,
        #[serde(default)]
        dim: f32,
    },
}

impl Default for TitleCardBackground {
    fn default() -> Self {
        Self::Solid {
            color: "#111827".to_string(),
        }
    }
}

fn center_focus() -> XY<f64> {
    XY::new(0.5, 0.5)
}

fn template_version() -> u32 {
    TITLE_CARD_TEMPLATE_VERSION
}

#[derive(Type, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TitleCard {
    pub id: String,
    pub role: TitleCardRole,
    #[serde(default)]
    pub template: TitleCardTemplate,
    #[serde(default = "template_version")]
    pub template_version: u32,
    #[serde(default)]
    pub kicker: String,
    #[serde(default)]
    pub subtitle: String,
    #[serde(default)]
    pub background: TitleCardBackground,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub custom_text_layout: bool,
}

impl TitleCard {
    pub fn new(role: TitleCardRole) -> Self {
        Self {
            id: uuid::Uuid::new_v4().simple().to_string(),
            role,
            template: TitleCardTemplate::default(),
            template_version: TITLE_CARD_TEMPLATE_VERSION,
            kicker: String::new(),
            subtitle: String::new(),
            background: TitleCardBackground::default(),
            custom_text_layout: false,
        }
    }

    pub fn validate(&self) -> Result<(), TitleCardError> {
        if self.id.is_empty()
            || self.id.len() > 128
            || !self
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(TitleCardError::InvalidMetadata("id"));
        }
        if self.template_version != TITLE_CARD_TEMPLATE_VERSION {
            return Err(TitleCardError::UnsupportedTemplateVersion);
        }
        if self.kicker.len() > 512 || self.kicker.contains('\0') {
            return Err(TitleCardError::InvalidMetadata("kicker"));
        }
        if self.subtitle.len() > 4096 || self.subtitle.contains('\0') {
            return Err(TitleCardError::InvalidMetadata("subtitle"));
        }
        match &self.background {
            TitleCardBackground::Solid { color } => {
                if color.len() != 7
                    || !color.starts_with('#')
                    || !color.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
                {
                    return Err(TitleCardError::InvalidMetadata("background color"));
                }
            }
            TitleCardBackground::ProjectImage {
                path, focus, dim, ..
            } => {
                if path.is_empty()
                    || path.len() > 1024
                    || path.chars().any(char::is_control)
                    || path.contains(['\\', ':'])
                    || path
                        .split('/')
                        .any(|component| matches!(component, "" | "." | ".."))
                {
                    return Err(TitleCardError::InvalidMetadata("background path"));
                }
                if !focus.x.is_finite()
                    || !focus.y.is_finite()
                    || !(0.0..=1.0).contains(&focus.x)
                    || !(0.0..=1.0).contains(&focus.y)
                    || !dim.is_finite()
                    || !(0.0..=1.0).contains(dim)
                {
                    return Err(TitleCardError::InvalidMetadata("background framing"));
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn deserialize_title_card<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<TitleCard>, D::Error> {
    let card = Option::<TitleCard>::deserialize(deserializer)?;
    if let Some(card) = &card {
        card.validate().map_err(serde::de::Error::custom)?;
    }
    Ok(card)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TitleCardError {
    MissingTimeline,
    MissingFootage,
    MissingCard,
    DuplicateRole,
    DuplicateId,
    InvalidDuration,
    InvalidTimeline,
    ContentBeforeIntro,
    UnanchoredIntro,
    UnsupportedTemplateVersion,
    InvalidMetadata(&'static str),
}

impl fmt::Display for TitleCardError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingTimeline => formatter.write_str("Title cards need a timeline"),
            Self::MissingFootage => formatter.write_str("Title cards need recorded footage"),
            Self::MissingCard => formatter.write_str("The title card does not exist"),
            Self::DuplicateRole => formatter.write_str("Only one title card per role is allowed"),
            Self::DuplicateId => formatter.write_str("Title card identities must be unique"),
            Self::InvalidDuration => {
                formatter.write_str("Title card duration must be 0.1–30 seconds")
            }
            Self::InvalidTimeline => formatter.write_str("The timeline contains invalid times"),
            Self::ContentBeforeIntro => {
                formatter.write_str("Move overlays out of the intro before shortening it")
            }
            Self::UnanchoredIntro => formatter.write_str("The intro must start at zero"),
            Self::UnsupportedTemplateVersion => {
                formatter.write_str("This title card template version is not supported")
            }
            Self::InvalidMetadata(field) => write!(formatter, "Invalid title card {field}"),
        }
    }
}

impl std::error::Error for TitleCardError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TitleCardChange {
    pub id: String,
    pub index: usize,
    pub changed: bool,
}

impl TextSegment {
    pub fn new_title_card(role: TitleCardRole, content: impl Into<String>) -> Self {
        Self {
            start: 0.0,
            end: if role == TitleCardRole::Intro {
                1.0
            } else {
                2.0
            },
            track: 0,
            enabled: true,
            content: content.into(),
            center: XY::new(0.5, 0.5),
            size: XY::new(0.8, 0.6),
            font_family: "Noto Sans SC".into(),
            font_size: 96.0,
            font_weight: 700.0,
            italic: false,
            color: "#FFFFFF".into(),
            background_color: None,
            background_style: TextBackgroundStyle::Box,
            uppercase: false,
            stroke_width: 0.0,
            stroke_color: "#000000".into(),
            gradient_color: None,
            glow: 0.0,
            fade_duration: 0.0,
            align: TextAlign::Center,
            letter_spacing: 0.0,
            line_height: 1.16,
            opacity: 1.0,
            shadow: 0.0,
            animation_in: TextAnimation::None,
            animation_out: TextAnimation::None,
            animation_in_duration: 0.0,
            animation_out_duration: 0.0,
            layout: TextLayout::Fullscreen,
            layout_transition: 0.0,
            title_card: Some(TitleCard::new(role)),
        }
    }
}

impl ProjectConfiguration {
    pub fn title_card(&self, role: TitleCardRole) -> Option<&TextSegment> {
        self.timeline
            .as_ref()?
            .text_segments
            .iter()
            .find(|segment| has_role(segment, role))
    }

    pub fn upsert_title_card(
        &mut self,
        mut segment: TextSegment,
    ) -> Result<TitleCardChange, TitleCardError> {
        validate_segment(&segment)?;
        let timeline = self
            .timeline
            .as_ref()
            .ok_or(TitleCardError::MissingTimeline)?;
        validate_cards(timeline)?;
        validate_footage(timeline)?;
        let old_intro = intro_duration(timeline);
        let card = segment
            .title_card
            .as_mut()
            .ok_or(TitleCardError::MissingCard)?;
        let role = card.role;
        let existing = timeline
            .text_segments
            .iter()
            .position(|segment| has_role(segment, role));
        if let Some(index) = existing {
            let previous = &timeline.text_segments[index];
            card.id = previous.title_card.as_ref().unwrap().id.clone();
            segment.track = previous.track;
        } else if timeline.text_segments.iter().any(|segment| {
            segment
                .title_card
                .as_ref()
                .is_some_and(|other| other.id == card.id)
        }) {
            return Err(TitleCardError::DuplicateId);
        }
        let id = card.id.clone();
        let duration = segment.end - segment.start;
        segment.start = 0.0;
        segment.end = duration;
        segment.layout = TextLayout::Fullscreen;

        let mut next = self.clone();
        let migrated_text = next.migrate_text_settings();
        let timeline = next.timeline.as_mut().unwrap();
        let index = existing.unwrap_or(timeline.text_segments.len());
        if existing.is_some() {
            timeline.text_segments[index] = segment;
        } else {
            timeline.text_segments.push(segment);
        }
        let delta = intro_duration(timeline) - old_intro;
        shift_body(&mut next, delta)?;
        normalize_cards(next.timeline.as_mut().unwrap())?;
        let changed = migrated_text
            || existing.is_none()
            || delta != 0.0
            || self.timeline.as_ref().unwrap().text_segments
                != next.timeline.as_ref().unwrap().text_segments;
        if changed {
            *self = next;
        }
        Ok(TitleCardChange { id, index, changed })
    }

    pub fn remove_title_card(&mut self, role: TitleCardRole) -> Result<bool, TitleCardError> {
        let Some(timeline) = &self.timeline else {
            return Ok(false);
        };
        validate_cards(timeline)?;
        let Some(index) = timeline
            .text_segments
            .iter()
            .position(|segment| has_role(segment, role))
        else {
            return Ok(false);
        };
        let old_intro = intro_duration(timeline);
        let mut next = self.clone();
        let timeline = next.timeline.as_mut().unwrap();
        timeline.text_segments.remove(index);
        let delta = intro_duration(timeline) - old_intro;
        shift_body(&mut next, delta)?;
        match normalize_cards(next.timeline.as_mut().unwrap()) {
            Ok(_) | Err(TitleCardError::MissingFootage) => {}
            Err(error) => return Err(error),
        }
        next.hidden_text_segments.retain_mut(|hidden| {
            if *hidden == index {
                return false;
            }
            if *hidden > index {
                *hidden -= 1;
            }
            true
        });
        *self = next;
        Ok(true)
    }

    pub fn set_title_card_duration(
        &mut self,
        role: TitleCardRole,
        duration: f64,
    ) -> Result<TitleCardChange, TitleCardError> {
        validate_duration(duration)?;
        let mut segment = self
            .title_card(role)
            .ok_or(TitleCardError::MissingCard)?
            .clone();
        segment.end = segment.start + duration;
        self.upsert_title_card(segment)
    }

    pub fn normalize_title_cards(&mut self) -> Result<bool, TitleCardError> {
        let Some(timeline) = &self.timeline else {
            return Ok(false);
        };
        if !timeline
            .text_segments
            .iter()
            .any(|segment| segment.title_card.is_some())
        {
            return Ok(false);
        }
        let mut next = timeline.clone();
        let changed = normalize_cards(&mut next)?;
        if changed {
            self.timeline = Some(next);
        }
        Ok(changed)
    }
}

fn has_role(segment: &TextSegment, role: TitleCardRole) -> bool {
    segment
        .title_card
        .as_ref()
        .is_some_and(|card| card.role == role)
}

fn validate_duration(duration: f64) -> Result<(), TitleCardError> {
    if !duration.is_finite()
        || !((MIN_TITLE_CARD_DURATION - 1e-9)..=(MAX_TITLE_CARD_DURATION + 1e-9))
            .contains(&duration)
    {
        return Err(TitleCardError::InvalidDuration);
    }
    Ok(())
}

fn validate_segment(segment: &TextSegment) -> Result<(), TitleCardError> {
    let card = segment
        .title_card
        .as_ref()
        .ok_or(TitleCardError::MissingCard)?;
    card.validate()?;
    if !segment.start.is_finite() || segment.start < 0.0 || !segment.end.is_finite() {
        return Err(TitleCardError::InvalidDuration);
    }
    validate_duration(segment.end - segment.start)?;
    if segment.content.len() > 8192 || segment.content.contains('\0') {
        return Err(TitleCardError::InvalidMetadata("title"));
    }
    Ok(())
}

fn validate_cards(timeline: &TimelineConfiguration) -> Result<(), TitleCardError> {
    let mut intro: Option<&TitleCard> = None;
    let mut outro: Option<&TitleCard> = None;
    for segment in &timeline.text_segments {
        let Some(card) = &segment.title_card else {
            continue;
        };
        validate_segment(segment)?;
        let slot = match card.role {
            TitleCardRole::Intro => &mut intro,
            TitleCardRole::Outro => &mut outro,
        };
        if slot.replace(card).is_some() {
            return Err(TitleCardError::DuplicateRole);
        }
    }
    if intro
        .zip(outro)
        .is_some_and(|(intro, outro)| intro.id == outro.id)
    {
        return Err(TitleCardError::DuplicateId);
    }
    Ok(())
}

fn validate_footage(timeline: &TimelineConfiguration) -> Result<(), TitleCardError> {
    if timeline.segments.is_empty() {
        return Err(TitleCardError::MissingFootage);
    }
    if timeline.segments.iter().any(|segment| {
        !segment.start.is_finite()
            || !segment.end.is_finite()
            || !segment.timescale.is_finite()
            || segment.start < 0.0
            || segment.end < segment.start
            || segment.timescale <= 0.0
    }) {
        return Err(TitleCardError::InvalidTimeline);
    }
    let duration = timeline.clips_duration();
    if !duration.is_finite() || duration <= 0.0 {
        return Err(TitleCardError::MissingFootage);
    }
    Ok(())
}

fn intro_duration(timeline: &TimelineConfiguration) -> f64 {
    timeline
        .text_segments
        .iter()
        .find(|segment| has_role(segment, TitleCardRole::Intro) && segment.enabled)
        .map_or(0.0, |segment| segment.end - segment.start)
}

fn normalize_cards(timeline: &mut TimelineConfiguration) -> Result<bool, TitleCardError> {
    validate_cards(timeline)?;
    if !timeline
        .text_segments
        .iter()
        .any(|segment| segment.title_card.is_some())
    {
        return Ok(false);
    }
    validate_footage(timeline)?;
    let mut changed = false;
    for segment in &mut timeline.text_segments {
        let Some(card) = &segment.title_card else {
            continue;
        };
        if card.role == TitleCardRole::Intro && segment.start != 0.0 {
            return Err(TitleCardError::UnanchoredIntro);
        }
        if segment.layout != TextLayout::Fullscreen {
            segment.layout = TextLayout::Fullscreen;
            changed = true;
        }
    }
    let body_end = timeline.duration_without_title_outro();
    if !body_end.is_finite() || body_end <= 0.0 {
        return Err(TitleCardError::InvalidTimeline);
    }
    for segment in &mut timeline.text_segments {
        if has_role(segment, TitleCardRole::Outro) && segment.start != body_end {
            let duration = segment.end - segment.start;
            segment.start = body_end;
            segment.end = body_end + duration;
            if !segment.end.is_finite() {
                return Err(TitleCardError::InvalidTimeline);
            }
            changed = true;
        }
    }
    Ok(changed)
}

fn shifted_time(time: f64, delta: f64) -> Result<f64, TitleCardError> {
    let shifted = time + delta;
    if !time.is_finite() || !shifted.is_finite() {
        return Err(TitleCardError::InvalidTimeline);
    }
    if shifted < -1e-9 {
        return Err(TitleCardError::ContentBeforeIntro);
    }
    Ok(shifted.max(0.0))
}

fn shifted_caption_time(time: f32, delta: f64) -> Result<f32, TitleCardError> {
    let shifted = shifted_time(f64::from(time), delta)? as f32;
    if !shifted.is_finite() {
        return Err(TitleCardError::InvalidTimeline);
    }
    Ok(shifted)
}

fn shift_body(project: &mut ProjectConfiguration, delta: f64) -> Result<(), TitleCardError> {
    if delta == 0.0 {
        return Ok(());
    }
    let timeline = project
        .timeline
        .as_mut()
        .ok_or(TitleCardError::MissingTimeline)?;
    macro_rules! shift_track {
        ($segments:expr) => {
            for segment in $segments {
                segment.start = shifted_time(segment.start, delta)?;
                segment.end = shifted_time(segment.end, delta)?;
            }
        };
    }
    shift_track!(&mut timeline.style_segments);
    shift_track!(&mut timeline.image_segments);
    shift_track!(&mut timeline.zoom_segments);
    shift_track!(&mut timeline.scene_segments);
    shift_track!(&mut timeline.camera3d_segments);
    shift_track!(&mut timeline.mask_segments);
    shift_track!(&mut timeline.shape_segments);
    shift_track!(&mut timeline.keyboard_segments);
    shift_track!(
        timeline
            .text_segments
            .iter_mut()
            .filter(|segment| segment.title_card.is_none())
    );
    for segment in &mut timeline.caption_segments {
        segment.start = shifted_time(segment.start, delta)?;
        segment.end = shifted_time(segment.end, delta)?;
        for word in &mut segment.words {
            word.start = shifted_caption_time(word.start, delta)?;
            word.end = shifted_caption_time(word.end, delta)?;
        }
    }
    // Legacy captions still use output time until synchronize_captions migrates
    // them. Source-timed captions and recording markers must never be shifted.
    if let Some(captions) = &mut project.captions
        && !captions.source_timed
    {
        for segment in &mut captions.segments {
            segment.start = shifted_caption_time(segment.start, delta)?;
            segment.end = shifted_caption_time(segment.end, delta)?;
            for word in &mut segment.words {
                word.start = shifted_caption_time(word.start, delta)?;
                word.end = shifted_caption_time(word.end, delta)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
