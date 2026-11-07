use super::*;
use crate::{TimelineFrameMapping, synchronize_captions};
use serde_json::{Value, json};

fn value(value: &impl Serialize) -> Value {
    serde_json::to_value(value).unwrap()
}

fn fixture() -> ProjectConfiguration {
    serde_json::from_value(json!({
        "textSizeVersion": 1,
        "textAnimVersion": 1,
        "timeline": {
            "segments": [{"recordingSegment": 0, "start": 0.0, "end": 10.0, "timescale": 1.0}],
            "zoomSegments": [{"start": 1.0, "end": 2.0, "amount": 1.5, "mode": "auto"}],
            "textSegments": [{"start": 1.0, "end": 2.0, "content": "Body label"}],
            "imageSegments": [{"start": 1.0, "end": 2.0, "path": "assets/photo.png"}],
            "keyboardSegments": [{
                "id": "keys", "start": 1.0, "end": 2.0, "displayText": "ab",
                "keys": [{"key": "a", "timeOffset": 0.0}, {"key": "b", "timeOffset": 500.0}]
            }],
            "audioSegments": [{
                "start": 0.0, "end": 10.0, "path": "assets/music.wav",
                "volumeDb": -6.0, "trimStart": 0.25, "fadeIn": 0.5, "fadeOut": 1.0
            }],
            "maskSegments": [{
                "start": 1.0, "end": 2.0, "maskType": "sensitive",
                "center": {"x": 0.5, "y": 0.5}, "size": {"x": 0.3, "y": 0.3},
                "keyframes": {"intensity": [{"time": 0.25, "value": 0.7}]}
            }],
            "camera3dSegments": [{
                "start": 1.0, "end": 2.0,
                "tracks": {"tiltX": [{"time": 0.25, "value": 10.0}]}
            }]
        },
        "captions": {
            "sourceTimed": true,
            "settings": {},
            "segments": [{
                "id": "spoken", "start": 1.0, "end": 2.0, "text": "Hello",
                "words": [{"text": "Hello", "start": 1.0, "end": 2.0}]
            }]
        },
        "markers": [{"id": "marker", "recordingSegment": 0, "time": 1.5, "label": "Body"}]
    }))
    .unwrap()
}

fn card(role: TitleCardRole, duration: f64) -> TextSegment {
    let mut segment = TextSegment::new_title_card(role, "灵录教程");
    segment.end = duration;
    segment
}

fn bounds(project: &ProjectConfiguration, role: TitleCardRole) -> (f64, f64) {
    let card = project.title_card(role).unwrap();
    (card.start, card.end)
}

#[test]
fn legacy_text_has_no_card_and_preserves_its_hold_mapping() {
    let mut project = fixture();
    let timeline = project.timeline.as_mut().unwrap();
    let segment = &mut timeline.text_segments[0];
    assert!(segment.title_card.is_none());
    assert!(value(segment).get("titleCard").is_none());
    segment.layout = TextLayout::Fullscreen;
    let before = value(&project);
    assert!(!project.normalize_title_cards().unwrap());
    assert_eq!(value(&project), before);
    let timeline = project.timeline.as_ref().unwrap();
    assert_eq!(timeline.duration(), 11.0);
    assert!(matches!(
        timeline.get_frame_mapping(1.5),
        Some(TimelineFrameMapping::Hold { .. })
    ));
    assert!(matches!(
        timeline.get_frame_mapping(3.0),
        Some(TimelineFrameMapping::Single { source, .. }) if source.source_time == 2.0
    ));
}

#[test]
fn upsert_is_role_idempotent_and_preserves_identity_and_track() {
    let mut project = fixture();
    let mut intro = card(TitleCardRole::Intro, 2.0);
    intro.track = 4;
    let first = project.upsert_title_card(intro).unwrap();
    assert!(first.changed);
    let before = value(&project);
    let mut replacement = card(TitleCardRole::Intro, 2.0);
    replacement.track = 9;
    let repeated = project.upsert_title_card(replacement.clone()).unwrap();
    assert!(!repeated.changed);
    assert_eq!(repeated.id, first.id);
    assert_eq!(value(&project), before);
    replacement.content = "Updated title".into();
    assert!(project.upsert_title_card(replacement).unwrap().changed);
    assert_eq!(project.title_card(TitleCardRole::Intro).unwrap().track, 4);
    let timeline = project.timeline.as_ref().unwrap();
    assert_eq!(timeline.text_segments.len(), 2);
    assert_eq!(timeline.zoom_segments[0].start, 3.0);
}

#[test]
fn bookends_hold_recording_clock_without_shifting_music_or_source_evidence() {
    let mut project = fixture();
    synchronize_captions(&mut project, &[10.0]);
    let source = value(&project.captions);
    let markers = value(&project.markers);
    let music = value(&project.timeline.as_ref().unwrap().audio_segments);
    project
        .upsert_title_card(card(TitleCardRole::Intro, 2.0))
        .unwrap();
    project
        .upsert_title_card(card(TitleCardRole::Outro, 3.0))
        .unwrap();
    assert_eq!(bounds(&project, TitleCardRole::Intro), (0.0, 2.0));
    assert_eq!(bounds(&project, TitleCardRole::Outro), (12.0, 15.0));
    let timeline = project.timeline.as_ref().unwrap();
    assert_eq!(timeline.duration(), 15.0);
    assert_eq!(value(&timeline.audio_segments), music);
    assert_eq!(value(&project.captions), source);
    assert_eq!(value(&project.markers), markers);
    assert!(matches!(
        timeline.get_frame_mapping(0.0),
        Some(TimelineFrameMapping::Hold { source, output_end })
            if source.source_time == 0.0 && output_end == 2.0
    ));
    assert!(matches!(
        timeline.get_frame_mapping(2.5),
        Some(TimelineFrameMapping::Single { source, .. }) if source.source_time == 0.5
    ));
    assert!(matches!(
        timeline.get_frame_mapping(12.5),
        Some(TimelineFrameMapping::Hold { source, output_end })
            if source.source_time < 10.0 && source.source_time > 9.9 && output_end == 15.0
    ));
    assert_eq!(timeline.image_segments[0].start, 3.0);
    assert_eq!(timeline.mask_segments[0].start, 3.0);
    assert_eq!(timeline.mask_segments[0].keyframes.intensity[0].time, 0.25);
    assert_eq!(timeline.camera3d_segments[0].start, 3.0);
    assert_eq!(timeline.camera3d_segments[0].tracks.tilt_x[0].time, 0.25);
    assert_eq!(timeline.keyboard_segments[0].start, 3.0);
    assert_eq!(timeline.keyboard_segments[0].keys[1].time_offset, 500.0);
}

#[test]
fn source_caption_projection_does_not_apply_intro_offset_twice() {
    let mut project = fixture();
    synchronize_captions(&mut project, &[10.0]);
    let source = value(&project.captions);
    project
        .upsert_title_card(card(TitleCardRole::Intro, 2.0))
        .unwrap();
    let shifted = value(&project.timeline.as_ref().unwrap().caption_segments);
    synchronize_captions(&mut project, &[10.0]);
    assert_eq!(
        value(&project.timeline.as_ref().unwrap().caption_segments),
        shifted
    );
    project
        .set_title_card_duration(TitleCardRole::Intro, 4.0)
        .unwrap();
    synchronize_captions(&mut project, &[10.0]);
    let track = &project.timeline.as_ref().unwrap().caption_segments[0];
    assert_eq!((track.start, track.end), (5.0, 6.0));
    assert_eq!((track.words[0].start, track.words[0].end), (5.0, 6.0));
    assert_eq!(value(&project.captions), source);
    project.remove_title_card(TitleCardRole::Intro).unwrap();
    synchronize_captions(&mut project, &[10.0]);
    let track = &project.timeline.as_ref().unwrap().caption_segments[0];
    assert_eq!((track.start, track.end), (1.0, 2.0));
    assert_eq!(value(&project.captions), source);
}

#[test]
fn legacy_output_captions_migrate_back_to_the_original_source_times() {
    let mut project = fixture();
    project.captions.as_mut().unwrap().source_timed = false;
    project
        .upsert_title_card(card(TitleCardRole::Intro, 2.0))
        .unwrap();
    assert_eq!(project.captions.as_ref().unwrap().segments[0].start, 3.0);
    synchronize_captions(&mut project, &[10.0]);
    let captions = project.captions.as_ref().unwrap();
    assert!(captions.source_timed);
    assert_eq!(captions.segments[0].start, 1.0);
    assert_eq!(captions.segments[0].words[0].start, 1.0);
    assert_eq!(
        project.timeline.as_ref().unwrap().caption_segments[0].start,
        3.0
    );
    synchronize_captions(&mut project, &[10.0]);
    assert_eq!(
        project.timeline.as_ref().unwrap().caption_segments[0].start,
        3.0
    );
}

#[test]
fn outro_reanchors_after_trim_speed_append_and_delete_without_duration_drift() {
    let mut project = fixture();
    project.timeline.as_mut().unwrap().audio_segments.clear();
    project
        .upsert_title_card(card(TitleCardRole::Intro, 2.0))
        .unwrap();
    project
        .upsert_title_card(card(TitleCardRole::Outro, 3.0))
        .unwrap();
    let outro_id = project
        .title_card(TitleCardRole::Outro)
        .unwrap()
        .title_card
        .as_ref()
        .unwrap()
        .id
        .clone();
    let music = value(&project.timeline.as_ref().unwrap().audio_segments);
    project.timeline.as_mut().unwrap().segments[0].end = 8.0;
    assert!(project.normalize_title_cards().unwrap());
    assert_eq!(bounds(&project, TitleCardRole::Outro), (10.0, 13.0));
    project.timeline.as_mut().unwrap().segments[0].timescale = 2.0;
    assert!(project.normalize_title_cards().unwrap());
    assert_eq!(bounds(&project, TitleCardRole::Outro), (6.0, 9.0));
    let mut clip = project.timeline.as_ref().unwrap().segments[0].clone();
    clip.end = 4.0;
    clip.timescale = 1.0;
    project.timeline.as_mut().unwrap().segments.push(clip);
    assert!(project.normalize_title_cards().unwrap());
    assert_eq!(bounds(&project, TitleCardRole::Outro), (10.0, 13.0));
    project.timeline.as_mut().unwrap().segments.remove(0);
    assert!(project.normalize_title_cards().unwrap());
    assert_eq!(bounds(&project, TitleCardRole::Outro), (6.0, 9.0));
    assert!(!project.normalize_title_cards().unwrap());
    assert_eq!(
        value(&project.timeline.as_ref().unwrap().audio_segments),
        music
    );
    assert_eq!(
        project
            .title_card(TitleCardRole::Outro)
            .unwrap()
            .title_card
            .as_ref()
            .unwrap()
            .id,
        outro_id
    );
}

#[test]
fn outro_anchor_includes_existing_overlapping_and_closing_fullscreen_holds() {
    let mut project = fixture();
    for (start, end) in [(3.0, 5.0), (4.0, 6.0), (13.0, 14.0)] {
        project.timeline.as_mut().unwrap().text_segments.push(
            serde_json::from_value(json!({
                "start": start, "end": end, "content": "Legacy hold", "layout": "fullscreen"
            }))
            .unwrap(),
        );
    }
    project
        .upsert_title_card(card(TitleCardRole::Intro, 2.0))
        .unwrap();
    project
        .upsert_title_card(card(TitleCardRole::Outro, 3.0))
        .unwrap();
    assert_eq!(bounds(&project, TitleCardRole::Outro), (16.0, 19.0));
    assert_eq!(project.timeline.as_ref().unwrap().duration(), 19.0);
    assert!(!project.normalize_title_cards().unwrap());
}

#[test]
fn removing_bookends_restores_body_and_is_idempotent() {
    let mut project = fixture();
    let before = project.clone();
    project
        .upsert_title_card(card(TitleCardRole::Intro, 2.0))
        .unwrap();
    project
        .upsert_title_card(card(TitleCardRole::Outro, 3.0))
        .unwrap();
    project.remove_title_card(TitleCardRole::Intro).unwrap();
    assert_eq!(bounds(&project, TitleCardRole::Outro), (10.0, 13.0));
    assert!(project.remove_title_card(TitleCardRole::Outro).unwrap());
    assert!(!project.remove_title_card(TitleCardRole::Outro).unwrap());
    assert_eq!(value(&project), value(&before));
}

#[test]
fn disabled_intro_changes_hold_only_when_explicitly_enabled() {
    let mut project = fixture();
    let mut intro = card(TitleCardRole::Intro, 2.0);
    intro.enabled = false;
    project.upsert_title_card(intro).unwrap();
    project
        .upsert_title_card(card(TitleCardRole::Outro, 3.0))
        .unwrap();
    assert_eq!(bounds(&project, TitleCardRole::Outro), (10.0, 13.0));
    assert_eq!(
        project.timeline.as_ref().unwrap().zoom_segments[0].start,
        1.0
    );
    let mut intro = project.title_card(TitleCardRole::Intro).unwrap().clone();
    intro.enabled = true;
    project.upsert_title_card(intro).unwrap();
    assert_eq!(bounds(&project, TitleCardRole::Outro), (12.0, 15.0));
    assert_eq!(
        project.timeline.as_ref().unwrap().zoom_segments[0].start,
        3.0
    );
}

#[test]
fn invalid_edits_and_shortening_over_cover_overlays_leave_the_project_untouched() {
    let mut project = fixture();
    project
        .upsert_title_card(card(TitleCardRole::Intro, 2.0))
        .unwrap();
    for duration in [f64::NAN, f64::INFINITY, -1.0, 0.0, 31.0] {
        let before = value(&project);
        assert_eq!(
            project.set_title_card_duration(TitleCardRole::Intro, duration),
            Err(TitleCardError::InvalidDuration)
        );
        assert_eq!(value(&project), before);
    }
    project.timeline.as_mut().unwrap().image_segments[0].start = 0.25;
    let before = value(&project);
    assert_eq!(
        project.remove_title_card(TitleCardRole::Intro),
        Err(TitleCardError::ContentBeforeIntro)
    );
    assert_eq!(value(&project), before);
    assert_eq!(
        project.set_title_card_duration(TitleCardRole::Intro, 0.5),
        Err(TitleCardError::ContentBeforeIntro)
    );
    assert_eq!(value(&project), before);
}

#[test]
fn card_metadata_rejects_unsafe_paths_unbounded_text_and_unknown_versions() {
    let segment = card(TitleCardRole::Intro, 2.0);
    for path in [
        "/tmp/photo.png",
        "../photo.png",
        "assets/../photo.png",
        "https://example.com/a.png",
        "C:\\photo.png",
        "assets//photo.png",
    ] {
        let mut data = value(&segment);
        data["titleCard"]["background"] = json!({"type": "projectImage", "path": path});
        assert!(
            serde_json::from_value::<TextSegment>(data).is_err(),
            "{path}"
        );
    }
    let mut data = value(&segment);
    data["titleCard"]["subtitle"] = json!("字".repeat(4096));
    assert!(serde_json::from_value::<TextSegment>(data).is_err());
    let mut data = value(&segment);
    data["titleCard"]["templateVersion"] = json!(2);
    assert!(serde_json::from_value::<TextSegment>(data).is_err());
    let mut data = value(&segment);
    data["titleCard"]["background"] =
        json!({"type": "projectImage", "path": "assets/封面.png", "focus": {"x": 1.1, "y": 0.5}});
    assert!(serde_json::from_value::<TextSegment>(data).is_err());
}

#[test]
fn project_roundtrip_preserves_card_identity_background_and_fractional_hold() {
    let mut project = fixture();
    let mut outro = card(TitleCardRole::Outro, 0.1);
    let metadata = outro.title_card.as_mut().unwrap();
    metadata.template = TitleCardTemplate::Split;
    metadata.kicker = "Quick guide".into();
    metadata.subtitle = "三步完成录制".into();
    metadata.background = TitleCardBackground::ProjectImage {
        path: "assets/封面.png".into(),
        fit: TitleCardImageFit::Cover,
        focus: XY::new(0.3, 0.6),
        dim: 0.25,
    };
    project.upsert_title_card(outro).unwrap();
    let encoded = value(&project);
    let mut reopened: ProjectConfiguration = serde_json::from_value(encoded.clone()).unwrap();
    assert!(!reopened.normalize_title_cards().unwrap());
    assert_eq!(value(&reopened), encoded);
    assert!(
        !reopened
            .set_title_card_duration(TitleCardRole::Outro, 0.1)
            .unwrap()
            .changed
    );
}

#[test]
fn duplicate_roles_and_ids_are_rejected_without_partial_repairs() {
    let mut project = fixture();
    let intro = card(TitleCardRole::Intro, 2.0);
    let mut outro = card(TitleCardRole::Outro, 3.0);
    outro.title_card.as_mut().unwrap().id = intro.title_card.as_ref().unwrap().id.clone();
    project.upsert_title_card(intro).unwrap();
    let before = value(&project);
    assert_eq!(
        project.upsert_title_card(outro),
        Err(TitleCardError::DuplicateId)
    );
    assert_eq!(value(&project), before);
    let duplicate = project.title_card(TitleCardRole::Intro).unwrap().clone();
    project
        .timeline
        .as_mut()
        .unwrap()
        .text_segments
        .push(duplicate);
    let before = value(&project);
    assert_eq!(
        project.normalize_title_cards(),
        Err(TitleCardError::DuplicateRole)
    );
    assert_eq!(value(&project), before);
}

#[test]
fn empty_footage_prevents_new_holds_but_does_not_trap_existing_cards() {
    let mut project = fixture();
    project
        .upsert_title_card(card(TitleCardRole::Intro, 2.0))
        .unwrap();
    project
        .upsert_title_card(card(TitleCardRole::Outro, 3.0))
        .unwrap();
    project.timeline.as_mut().unwrap().segments.clear();
    let before = value(&project);
    assert_eq!(
        project.normalize_title_cards(),
        Err(TitleCardError::MissingFootage)
    );
    assert_eq!(value(&project), before);
    assert!(project.remove_title_card(TitleCardRole::Intro).unwrap());
    assert!(project.remove_title_card(TitleCardRole::Outro).unwrap());
    let before = value(&project);
    assert_eq!(
        project.upsert_title_card(card(TitleCardRole::Intro, 2.0)),
        Err(TitleCardError::MissingFootage)
    );
    assert_eq!(value(&project), before);
}

#[test]
fn adding_a_card_migrates_legacy_body_text_once_without_rescaling_the_card() {
    let mut project = fixture();
    project.text_size_version = 0;
    project.text_anim_version = 0;
    let label = &mut project.timeline.as_mut().unwrap().text_segments[0];
    label.font_size = 60.0;
    label.size.y = 0.4;
    project
        .upsert_title_card(TextSegment::new_title_card(TitleCardRole::Intro, "封面"))
        .unwrap();
    assert_eq!(
        project.timeline.as_ref().unwrap().text_segments[0].font_size,
        120.0
    );
    assert_eq!(
        project.title_card(TitleCardRole::Intro).unwrap().font_size,
        96.0
    );
    assert!(!project.migrate_text_settings());
    assert_eq!(
        project.timeline.as_ref().unwrap().text_segments[0].font_size,
        120.0
    );
    let mut reopened: ProjectConfiguration = serde_json::from_value(value(&project)).unwrap();
    assert!(!reopened.migrate_text_settings());
    assert_eq!(
        reopened.title_card(TitleCardRole::Intro).unwrap().font_size,
        96.0
    );
}
