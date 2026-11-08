use super::*;

fn source() -> CaptionSettings {
    CaptionSettings {
        font: "Noto Sans SC".into(),
        size: 48,
        ..Default::default()
    }
}

fn style(font: &str, size: u32) -> CaptionTextStyle {
    CaptionTextStyle {
        font: font.into(),
        size,
    }
}

fn project(settings: CaptionSettings) -> ProjectConfiguration {
    ProjectConfiguration {
        captions: Some(CaptionsData {
            settings,
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[test]
fn old_caption_settings_keep_inherited_typography_without_serialized_migration() {
    for json in [
        r#"{"font":"Noto Sans SC","size":48}"#,
        r#"{"font":"Noto Sans SC","size":48,"translationStyle":null}"#,
    ] {
        let settings: CaptionSettings = serde_json::from_str(json).unwrap();
        assert!(settings.translation_style.is_none());
        assert_eq!(
            settings.resolved_translation_style(),
            style("Noto Sans SC", 48)
        );
        assert!(
            serde_json::to_value(&settings)
                .unwrap()
                .get("translationStyle")
                .is_none()
        );
    }
}

#[test]
fn editing_source_font_freezes_the_previously_inherited_translation() {
    let mut settings = source();
    settings.set_source_font("Noto Serif SC".into());
    assert_eq!(settings.font, "Noto Serif SC");
    assert_eq!(
        settings.resolved_translation_style(),
        style("Noto Sans SC", 48)
    );
    settings.set_source_font_size(72);
    assert_eq!(settings.size, 72);
    assert_eq!(
        settings.resolved_translation_style(),
        style("Noto Sans SC", 48)
    );
}

#[test]
fn editing_source_size_first_preserves_translation_font_and_size() {
    let mut settings = source();
    settings.set_source_font_size(60);
    assert_eq!(settings.font, "Noto Sans SC");
    assert_eq!(settings.size, 60);
    assert_eq!(
        settings.resolved_translation_style(),
        style("Noto Sans SC", 48)
    );
}

#[test]
fn editing_translation_freezes_its_other_field_and_keeps_source_unchanged() {
    let mut font_first = source();
    font_first.set_translation_font("Inter".into());
    assert_eq!(font_first.resolved_translation_style(), style("Inter", 48));
    font_first.set_translation_font_size(40);
    assert_eq!(font_first.resolved_translation_style(), style("Inter", 40));
    let mut size_first = source();
    size_first.set_translation_font_size(36);
    assert_eq!(
        size_first.resolved_translation_style(),
        style("Noto Sans SC", 36)
    );
    for settings in [font_first, size_first] {
        assert_eq!(settings.font, "Noto Sans SC");
        assert_eq!(settings.size, 48);
    }
}

#[test]
fn explicit_translation_style_roundtrips_and_survives_later_source_edits() {
    let mut settings = source();
    settings.set_translation_font("Inter".into());
    settings.set_translation_font_size(40);
    let bytes = serde_json::to_vec(&settings).unwrap();
    let mut reopened: CaptionSettings = serde_json::from_slice(&bytes).unwrap();
    reopened.set_source_font("Noto Serif SC".into());
    reopened.set_source_font_size(60);
    assert_eq!(reopened.resolved_translation_style(), style("Inter", 40));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["translationStyle"],
        serde_json::json!({"font":"Inter","size":40})
    );
}

#[test]
fn old_project_presets_preserve_explicit_translation_but_do_not_materialize_legacy_style() {
    let preset = project(CaptionSettings {
        font: "Montserrat".into(),
        size: 60,
        ..Default::default()
    });
    let inherited = project(source()).with_preset(preset.clone());
    let settings = &inherited.captions.unwrap().settings;
    assert!(settings.translation_style.is_none());
    assert_eq!(
        settings.resolved_translation_style(),
        style("Montserrat", 60)
    );
    let mut explicit = source();
    explicit.set_translation_font("Inter".into());
    explicit.set_translation_font_size(40);
    explicit.display_mode = crate::CaptionDisplayMode::Bilingual;
    explicit.set_translation("caption", "开始", "Start".into(), "en".into());
    let applied = project(explicit).with_preset(preset);
    let settings = &applied.captions.unwrap().settings;
    assert_eq!(settings.font, "Montserrat");
    assert_eq!(settings.size, 60);
    assert_eq!(settings.resolved_translation_style(), style("Inter", 40));
    assert_eq!(settings.display_mode, crate::CaptionDisplayMode::Bilingual);
    assert_eq!(
        settings.translation_for("caption", "开始").unwrap().text,
        "Start"
    );
}

#[test]
fn explicit_project_preset_translation_style_applies_to_existing_and_new_caption_tracks() {
    let mut preset = source();
    preset.translation_style = Some(style("Source Serif 4", 36));
    for current in [ProjectConfiguration::default(), project(source())] {
        let applied = current.with_preset(project(preset.clone()));
        assert_eq!(
            applied
                .captions
                .unwrap()
                .settings
                .resolved_translation_style(),
            style("Source Serif 4", 36)
        );
    }
}

#[test]
fn invalid_translation_field_types_are_rejected_without_mutating_the_input() {
    for value in [
        serde_json::json!({"font": 2, "size": 40}),
        serde_json::json!({"font": "Inter", "size": -1}),
        serde_json::json!({"font": "Inter", "size": 1.5}),
    ] {
        let bytes = serde_json::to_vec(&serde_json::json!({"translationStyle": value})).unwrap();
        assert!(serde_json::from_slice::<CaptionSettings>(&bytes).is_err());
    }
}

#[test]
fn caption_appearance_edits_are_independent_and_old_projects_keep_their_look() {
    let mut settings: CaptionSettings = serde_json::from_value(serde_json::json!({
        "color":"#FFFF00", "outline":true, "outlineColor":"#223344", "outlineWidth":4.0,
        "shadow":true, "shadowColor":"#332211", "shadowOpacity":60, "shadowSize":8.0
    }))
    .unwrap();
    assert!(settings.translation_appearance.is_none());
    let inherited = settings.source_appearance();
    assert_eq!(settings.resolved_translation_appearance(), inherited);
    assert!(
        serde_json::to_value(&settings)
            .unwrap()
            .get("translationAppearance")
            .is_none()
    );
    settings.edit_source_appearance(|appearance| {
        appearance.color = "#FF0000".into();
        appearance.outline = false;
        appearance.shadow = false;
        appearance.uppercase = true;
    });
    assert_eq!(settings.resolved_translation_appearance(), inherited);
    let source = settings.source_appearance();
    settings.edit_translation_appearance(|appearance| {
        appearance.color = "#0000FF".into();
        appearance.outline_color = "#00FF00".into();
        appearance.outline_width = 10.;
        appearance.shadow_color = "#FFFFFF".into();
        appearance.shadow_opacity = 80;
        appearance.shadow_size = 12.;
        appearance.font_weight = 400;
    });
    assert_eq!(settings.source_appearance(), source);
    let restored: CaptionSettings =
        serde_json::from_slice(&serde_json::to_vec(&settings).unwrap()).unwrap();
    assert_eq!(restored.source_appearance(), source);
    assert_eq!(
        restored.translation_appearance,
        settings.translation_appearance
    );
    let applied = project(restored.clone()).with_preset(project(CaptionSettings::default()));
    assert_eq!(
        applied.captions.unwrap().settings.translation_appearance,
        restored.translation_appearance
    );
}

#[test]
fn subtitle_text_export_respects_independent_capitalization() {
    let mut settings = source();
    settings.display_mode = crate::CaptionDisplayMode::Bilingual;
    settings.set_translation("appearance", "Source", "Translation".into(), "en".into());
    let segment: CaptionTrackSegment = serde_json::from_value(
        serde_json::json!({"id":"appearance", "text":"Source", "start":0., "end":1.}),
    )
    .unwrap();
    settings.edit_source_appearance(|a| a.uppercase = true);
    assert_eq!(
        crate::caption_presentation_text(&segment, &settings),
        "SOURCE\nTranslation"
    );
    settings.edit_translation_appearance(|a| a.uppercase = true);
    settings.edit_source_appearance(|a| a.uppercase = false);
    assert_eq!(
        crate::caption_presentation_text(&segment, &settings),
        "Source\nTRANSLATION"
    );
}

#[test]
fn source_size_changes_preserve_translation_outline_size() {
    let mut settings = source();
    settings.size = 72;
    settings.outline = true;
    settings.outline_width = 5.;
    settings.translation_style = Some(style("Inter", 40));
    settings.set_source_font_size(96);
    let appearance = settings.resolved_translation_appearance();
    assert_eq!(appearance.outline_width, 9.);
    assert_eq!(
        appearance.outline_width * settings.resolved_translation_style().size as f32 / 100.,
        3.6
    );
    settings.edit_source_appearance(|a| a.outline_width = 2.);
    assert_eq!(settings.resolved_translation_appearance(), appearance);
}
