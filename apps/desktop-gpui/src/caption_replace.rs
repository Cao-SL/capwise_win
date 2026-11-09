//! Find and replace across the captions: a match is found in each caption
//! line's text as it reads on screen, so it can cover several Chinese
//! characters or English words, and the words it touches become one word that
//! keeps their combined timing.

use std::ops::Range;

use cap_project::{CaptionSegment, CaptionWord, caption_text_from_words};

/// Byte ranges of `needle` in `haystack`, ignoring ASCII case, without overlaps.
pub(crate) fn occurrences(haystack: &str, needle: &str) -> Vec<Range<usize>> {
    if needle.is_empty() {
        return Vec::new();
    }
    let hay = haystack.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = hay[from..].find(&needle) {
        let start = from + at;
        found.push(start..start + needle.len());
        from = start + needle.len();
    }
    found
}

/// Where each word sits in the caption line's text.
fn word_spans(words: &[CaptionWord]) -> (String, Vec<Range<usize>>) {
    let mut spans = Vec::with_capacity(words.len());
    for index in 0..words.len() {
        let after = caption_text_from_words(&words[..=index]);
        let word = words[index].text.trim();
        spans.push(after.len() - word.len()..after.len());
    }
    (caption_text_from_words(words), spans)
}

/// Replaces every match in one line's words; returns how many were replaced.
fn replace_in_words(
    words: &mut Vec<CaptionWord>,
    find: &str,
    replacement: &str,
    within: Option<&Range<f64>>,
) -> usize {
    let (text, spans) = word_spans(words);
    let mut replaced = 0;
    for hit in occurrences(&text, find).into_iter().rev() {
        let Some(first) = spans.iter().position(|span| span.end > hit.start) else {
            continue;
        };
        let Some(last) = spans.iter().rposition(|span| span.start < hit.end) else {
            continue;
        };
        if last < first {
            continue;
        }
        let (start, end) = (words[first].start, words[last].end);
        if let Some(range) = within
            && (f64::from(end) <= range.start || f64::from(start) >= range.end)
        {
            continue;
        }
        let head = &words[first].text.trim()[..hit.start.saturating_sub(spans[first].start)];
        let tail_from = hit.end.saturating_sub(spans[last].start).min(words[last].text.trim().len());
        let tail = &words[last].text.trim()[tail_from..];
        let merged = format!("{head}{replacement}{tail}");
        let merged = merged.trim();
        if merged.is_empty() {
            words.drain(first..=last);
        } else {
            let word = CaptionWord {
                text: merged.to_string(),
                start,
                end,
                hidden: false,
            };
            words.splice(first..=last, [word]);
        }
        replaced += 1;
    }
    replaced
}

/// `str::replace`, ignoring ASCII case.
fn replace_text(text: &str, find: &str, replacement: &str) -> (String, usize) {
    let hits = occurrences(text, find);
    let mut out = String::with_capacity(text.len());
    let mut from = 0;
    for hit in &hits {
        out.push_str(&text[from..hit.start]);
        out.push_str(replacement);
        from = hit.end;
    }
    out.push_str(&text[from..]);
    (out, hits.len())
}

/// Replaces `find` with `replacement` in every caption line, or only in the
/// lines' words overlapping `within` (source seconds) when given. Returns how
/// many matches were replaced.
pub fn replace_in_captions(
    segments: &mut Vec<CaptionSegment>,
    find: &str,
    replacement: &str,
    within: Option<Range<f64>>,
) -> usize {
    let find = find.trim();
    if find.is_empty() {
        return 0;
    }
    let mut total = 0;
    for segment in segments.iter_mut() {
        let text_follows_words = !segment.words.is_empty()
            && segment.text.split_whitespace().collect::<String>()
                == caption_text_from_words(&segment.words)
                    .split_whitespace()
                    .collect::<String>();
        if segment.words.is_empty() {
            if within.as_ref().is_some_and(|range| {
                f64::from(segment.end) <= range.start || f64::from(segment.start) >= range.end
            }) {
                continue;
            }
            let (text, count) = replace_text(&segment.text, find, replacement);
            segment.text = text;
            total += count;
            continue;
        }
        let count = replace_in_words(&mut segment.words, find, replacement, within.as_ref());
        if count == 0 {
            continue;
        }
        total += count;
        segment.text = if text_follows_words {
            caption_text_from_words(&segment.words)
        } else {
            replace_text(&segment.text, find, replacement).0
        };
        if let (Some(first), Some(last)) = (segment.words.first(), segment.words.last()) {
            segment.start = first.start;
            segment.end = last.end;
        }
    }
    segments.retain(|segment| !segment.text.trim().is_empty());
    total
}

/// Adds a corrected spelling to the user's transcription terms (Settings →
/// AI models), which the recognisers are given as context.
pub fn remember_term(term: &str) {
    let term = term.trim();
    if term.is_empty() || term.chars().count() > 40 {
        return;
    }
    let section = crate::store::store_section(crate::store::GENERAL_SETTINGS);
    let mut terms: Vec<String> = section
        .get("transcriptionHints")
        .and_then(serde_json::Value::as_array)
        .map(|values| values.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    if terms.iter().any(|known| known == term) {
        return;
    }
    terms.push(term.to_string());
    let terms = crate::store::normalize_transcription_hints(terms);
    crate::store::set_store_setting(
        crate::store::GENERAL_SETTINGS,
        "transcriptionHints",
        serde_json::Value::Array(terms.into_iter().map(serde_json::Value::String).collect()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, words: &[&str]) -> CaptionSegment {
        CaptionSegment {
            id: "segment-0".into(),
            start: 0.0,
            end: words.len() as f32,
            text: text.into(),
            words: words
                .iter()
                .enumerate()
                .map(|(index, word)| CaptionWord {
                    text: (*word).into(),
                    start: index as f32,
                    end: index as f32 + 1.0,
                    hidden: false,
                })
                .collect(),
        }
    }

    #[test]
    fn chinese_characters_merge_into_the_replacement_with_their_timing() {
        let mut segments = vec![line("欢迎使用零路", &["欢", "迎", "使", "用", "零", "路"])];
        assert_eq!(replace_in_captions(&mut segments, "零路", "灵录", None), 1);
        assert_eq!(segments[0].text, "欢迎使用灵录");
        let last = segments[0].words.last().unwrap();
        assert_eq!(last.text, "灵录");
        assert_eq!((last.start, last.end), (4.0, 6.0));
        assert_eq!(segments[0].words.len(), 5);
    }

    #[test]
    fn english_matches_ignore_case_and_keep_neighbouring_punctuation() {
        let mut segments = vec![line("Welcome to cap wise, folks", &["Welcome", "to", "cap", "wise,", "folks"])];
        assert_eq!(replace_in_captions(&mut segments, "Cap wise", "Capwise", None), 1);
        assert_eq!(segments[0].text, "Welcome to Capwise, folks");
        assert_eq!(segments[0].words[2].text, "Capwise,");
    }

    #[test]
    fn a_match_inside_one_word_keeps_the_rest_of_it() {
        let mut segments = vec![line("通一千问模型", &["通一千问", "模型"])];
        assert_eq!(replace_in_captions(&mut segments, "一", "义", None), 1);
        assert_eq!(segments[0].text, "通义千问模型");
    }

    #[test]
    fn only_the_given_time_range_is_replaced() {
        let mut segments = vec![
            line("零路", &["零", "路"]),
            CaptionSegment {
                start: 10.0,
                end: 12.0,
                words: vec![
                    CaptionWord { text: "零".into(), start: 10.0, end: 11.0, hidden: false },
                    CaptionWord { text: "路".into(), start: 11.0, end: 12.0, hidden: false },
                ],
                ..line("零路", &[])
            },
        ];
        assert_eq!(replace_in_captions(&mut segments, "零路", "灵录", Some(10.0..12.0)), 1);
        assert_eq!(segments[0].text, "零路");
        assert_eq!(segments[1].text, "灵录");
    }

    #[test]
    fn replacing_with_nothing_removes_the_words_and_empty_lines() {
        let mut segments = vec![line("嗯", &["嗯"]), line("你好", &["你", "好"])];
        assert_eq!(replace_in_captions(&mut segments, "嗯", "", None), 1);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "你好");
        assert_eq!(replace_in_captions(&mut segments, "  ", "x", None), 0);
    }
}
