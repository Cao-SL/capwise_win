use glyphon::{Attrs, FontSystem};

pub(super) fn matched_font_attrs<'a>(fonts: &FontSystem, attrs: Attrs<'a>) -> Attrs<'a> {
    let attrs = if attrs.family == glyphon::Family::Name("LXGW WenKai Lite Medium") {
        attrs.family(glyphon::Family::Name("LXGW WenKai Lite"))
    } else {
        attrs
    };
    let face = fonts
        .db()
        .query(&glyphon::fontdb::Query {
            families: &[attrs.family],
            weight: attrs.weight,
            stretch: attrs.stretch,
            style: attrs.style,
        })
        .and_then(|id| fonts.db().face(id));
    match face {
        // cosmic-text 0.14 skips the requested family without an exact weight,
        // style and stretch match, even when fontdb found a suitable face.
        Some(face) => attrs
            .weight(face.weight)
            .style(face.style)
            .stretch(face.stretch),
        None => attrs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glyphon::{Buffer, Family, Metrics, Shaping, Weight, fontdb};

    #[test]
    fn unavailable_bold_keeps_the_selected_family_for_chinese_and_latin() {
        let mut db = fontdb::Database::new();
        db.load_font_data(
            include_bytes!("../../../../apps/desktop-gpui/assets/caption-fonts/SGH-Medium.ttf")
                .to_vec(),
        );
        db.load_font_data(
            include_bytes!("../../../../apps/desktop-gpui/assets/caption-fonts/Inter-Bold.ttf")
                .to_vec(),
        );
        let selected = db
            .query(&fontdb::Query {
                families: &[Family::Name("宋骨黑体")],
                ..Default::default()
            })
            .unwrap();
        let mut fonts = FontSystem::new_with_locale_and_db("zh-CN".to_string(), db);
        let attrs = matched_font_attrs(
            &fonts,
            Attrs::new()
                .family(Family::Name("宋骨黑体"))
                .weight(Weight::BOLD),
        );
        let mut buffer = Buffer::new(&mut fonts, Metrics::new(50.0, 60.0));
        buffer.set_size(&mut fonts, Some(1800.0), None);
        buffer.set_text(
            &mut fonts,
            "录屏字幕 AI RSS 2026",
            &attrs,
            Shaping::Advanced,
        );
        buffer.shape_until_scroll(&mut fonts, false);
        let glyphs: Vec<_> = buffer.layout_runs().flat_map(|run| run.glyphs).collect();
        assert!(!glyphs.is_empty());
        assert!(glyphs.iter().all(|glyph| glyph.font_id == selected));
        assert_eq!(attrs.weight, Weight::MEDIUM);
    }
}
