use std::ops::Range;

use crate::{CaptionSegment, CaptionWord, caption_text_from_words, is_unspaced_script};

/// Netflix's Simplified Chinese guide caps a line at 16 characters; a line
/// near 12 reads at a glance in a screen recording.
pub const MAX_LINE_WIDTH: f32 = 16.0;
const IDEAL_LINE_WIDTH: f32 = 12.0;
const SHORT_LINE_WIDTH: f32 = 4.0;
const MAX_LINE_SECONDS: f32 = 7.0;
/// Silence long enough that a caption spanning it would sit over nothing.
const FORCED_GAP: f32 = 1.0;

const CONJUNCTIONS: [&str; 16] = [
    "但是", "所以", "如果", "然后", "因为", "而且", "还有", "不过", "那么", "可是", "或者", "并且",
    "接着", "于是", "其实", "比如",
];

fn is_punctuation(c: char) -> bool {
    c.is_ascii_punctuation() || "，。！？；：、…—“”‘’（）《》【】「」".contains(c)
}

fn is_cjk(c: char) -> bool {
    is_unspaced_script(c) && !is_punctuation(c)
}

/// Reading width: a Chinese character counts one, a Latin letter about half.
pub fn text_width(text: &str) -> f32 {
    text.chars()
        .filter(|c| !is_punctuation(*c) && !c.is_whitespace())
        .map(|c| if is_cjk(c) { 1.0 } else { 0.55 })
        .sum()
}

fn word_width(word: &CaptionWord) -> f32 {
    if word.hidden {
        0.0
    } else {
        text_width(&word.text)
    }
}

pub fn reads_as_chinese(words: &[CaptionWord]) -> bool {
    let (mut cjk, mut total) = (0usize, 0usize);
    for c in words.iter().flat_map(|word| word.text.chars()) {
        if c.is_alphanumeric() || is_cjk(c) {
            total += 1;
            cjk += usize::from(is_cjk(c));
        }
    }
    cjk >= 4 && cjk * 3 >= total * 2
}

fn ends_sentence(text: &str) -> bool {
    text.trim_end_matches(['”', '’', '」', '》', ')', '）'])
        .ends_with(['。', '！', '？', '…', '.', '!', '?'])
}

fn ends_clause(text: &str) -> bool {
    text.trim_end()
        .ends_with(['，', '、', '；', '：', ',', ';', ':'])
}

/// What else is known about each gap between two words. Index `i` is the gap
/// after word `i`.
#[derive(Clone, Debug, Default)]
pub struct BreakHints {
    /// The gap falls inside a word, from a word segmenter.
    pub inside_word: Vec<bool>,
    /// A language model's choice for the gap: `Some(true)` to break,
    /// `Some(false)` to keep going, `None` where it was not asked or failed.
    pub preferred: Vec<Option<bool>>,
}

fn gap_after(words: &[CaptionWord], index: usize) -> f32 {
    (words[index + 1].start - words[index].end).max(0.0)
}

fn starts_with_conjunction(words: &[CaptionWord], index: usize) -> bool {
    let mut text = String::new();
    for word in words[index..].iter().take(2) {
        text.push_str(word.text.trim());
    }
    CONJUNCTIONS
        .iter()
        .any(|conjunction| text.starts_with(conjunction))
}

/// The cost of ending a line after word `index`. Lower is a better place.
fn break_cost(words: &[CaptionWord], hints: &BreakHints, index: usize) -> f32 {
    let text = &words[index].text;
    let gap = gap_after(words, index);
    let mut cost: f32 = if ends_sentence(text) {
        0.0
    } else if ends_clause(text) || gap >= 0.5 {
        1.0
    } else if gap >= 0.3 {
        2.5
    } else if starts_with_conjunction(words, index + 1) {
        2.0
    } else {
        6.0
    };
    if short_clause_head(words, index + 1) {
        cost += 4.0;
    }
    if !ends_clause(text) && !ends_sentence(text) {
        cost += binding_cost(words, index);
    }
    match hints.preferred.get(index).copied().flatten() {
        Some(true) => cost = cost.min(0.5),
        Some(false) => cost += 2.0,
        None => {}
    }
    if hints.inside_word.get(index).copied().unwrap_or(false) {
        cost += 40.0;
    }
    cost
}

const TRAILING_PARTICLES: [&str; 9] = ["的", "了", "着", "过", "吗", "呢", "吧", "得", "地"];
const DETERMINERS: [&str; 10] = [
    "这个", "那个", "一个", "这些", "那些", "一些", "这种", "那种", "这本", "每个",
];

/// Breaks that split what reads as one phrase: a line that starts with a
/// particle ("期待 / 的一个功能"), or one that ends on a determiner or a
/// modifying "的" ("这个 / 事情", "不同的 / 音色").
fn binding_cost(words: &[CaptionWord], index: usize) -> f32 {
    let next = words[index + 1].text.trim();
    if TRAILING_PARTICLES
        .iter()
        .any(|particle| next.starts_with(particle))
    {
        return 5.0;
    }
    let mut tail = String::new();
    for word in words[..=index].iter().rev().take(2) {
        tail.insert_str(0, word.text.trim());
    }
    if DETERMINERS
        .iter()
        .any(|determiner| tail.ends_with(determiner))
    {
        return 5.0;
    }
    if tail.ends_with('的') {
        return 3.0;
    }
    0.0
}

/// A line that starts with one or two characters and then a comma reads as
/// the tail of the line before ("一个功能叫 / 播放，就是").
fn short_clause_head(words: &[CaptionWord], index: usize) -> bool {
    let mut width = 0.0;
    for word in &words[index..] {
        width += word_width(word);
        if width > 3.0 {
            return false;
        }
        if ends_clause(&word.text) || ends_sentence(&word.text) {
            return true;
        }
    }
    false
}

/// Strong boundaries a line runs over instead of breaking at: two sentences
/// or a new clause squeezed onto one line read as one.
fn interior_cost(words: &[CaptionWord], line: Range<usize>) -> f32 {
    let mut cost = 0.0;
    for index in line.start..line.end.saturating_sub(1) {
        let text = &words[index].text;
        if ends_sentence(text) {
            cost += 4.0;
        } else if ends_clause(text) {
            cost += 1.5;
        } else if index + 1 > line.start && starts_with_conjunction(words, index + 1) {
            cost += 3.0;
        }
    }
    cost
}

fn line_cost(width: f32, seconds: f32, whole: bool) -> f32 {
    let mut cost = (width - IDEAL_LINE_WIDTH).powi(2) * 0.06;
    if width < SHORT_LINE_WIDTH && !whole {
        cost += 6.0 + (SHORT_LINE_WIDTH - width) * 2.0;
    }
    if seconds > MAX_LINE_SECONDS {
        cost += (seconds - MAX_LINE_SECONDS) * 2.0;
    }
    cost
}

/// Splits timed words into caption lines: each fits [`MAX_LINE_WIDTH`], none
/// spans a long silence, and the breaks fall where the hints and the text say
/// a reader can pause, chosen over the whole run rather than greedily.
pub fn plan_caption_lines(words: &[CaptionWord], hints: &BreakHints) -> Vec<Range<usize>> {
    let count = words.len();
    if count == 0 {
        return Vec::new();
    }
    let forced: Vec<bool> = (0..count.saturating_sub(1))
        .map(|index| gap_after(words, index) >= FORCED_GAP)
        .collect();
    let mut best = vec![f32::INFINITY; count + 1];
    let mut from = vec![0usize; count + 1];
    best[0] = 0.0;
    for end in 1..=count {
        let mut width = 0.0;
        for start in (0..end).rev() {
            width += word_width(&words[start]);
            if start + 1 < end && forced[start] {
                break;
            }
            if width > MAX_LINE_WIDTH && start + 1 < end {
                break;
            }
            if !best[start].is_finite() {
                continue;
            }
            let seconds = words[end - 1].end - words[start].start;
            let ends = if end == count || forced[end - 1] {
                0.0
            } else {
                break_cost(words, hints, end - 1)
            };
            let cost = best[start]
                + line_cost(width, seconds, start == 0 && end == count)
                + interior_cost(words, start..end)
                + ends;
            if cost < best[end] {
                best[end] = cost;
                from[end] = start;
            }
        }
    }
    let mut lines = Vec::new();
    let mut end = count;
    while end > 0 {
        let start = from[end];
        lines.push(start..end);
        end = start;
    }
    lines.reverse();
    lines
}

/// Caption segments for planned lines. A line drops the soft punctuation at
/// its end, as Chinese subtitles do.
pub fn segments_from_lines(
    words: &[CaptionWord],
    lines: &[Range<usize>],
    id_prefix: &str,
) -> Vec<CaptionSegment> {
    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let mut words = words[line.clone()].to_vec();
            if let Some(last) = words.last_mut() {
                let trimmed = last.text.trim_end_matches(['，', '。', '、', '；', '：']);
                if !trimmed.is_empty() {
                    last.text = trimmed.to_string();
                }
            }
            CaptionSegment {
                id: format!("{id_prefix}{index}"),
                start: words.first().map_or(0.0, |word| word.start),
                end: words.last().map_or(0.0, |word| word.end),
                text: caption_text_from_words(&words),
                words,
            }
        })
        .collect()
}

/// What a language model is shown: the words as they read, nothing else.
pub fn model_input(words: &[CaptionWord]) -> String {
    words.iter().map(|word| word.text.trim()).collect()
}

/// The gaps a model marked with `|` in its copy of [`model_input`]. `None`
/// when the reply changed any character, which would break word timing, so
/// the caller falls back to the rules alone.
pub fn model_breaks(words: &[CaptionWord], reply: &str) -> Option<Vec<Option<bool>>> {
    let kept = |c: &char| !c.is_whitespace() && *c != '|' && *c != '｜';
    let expected: String = model_input(words).chars().filter(kept).collect();
    let reply = reply.trim();
    if reply.chars().filter(kept).collect::<String>() != expected {
        return None;
    }
    let mut ends = Vec::with_capacity(words.len());
    let mut total = 0usize;
    for word in words {
        total += word.text.chars().filter(kept).count();
        ends.push(total);
    }
    let mut preferred = vec![Some(false); words.len().saturating_sub(1)];
    let mut seen = 0usize;
    for c in reply.chars() {
        if c == '|' || c == '｜' || c == '\n' {
            if let Some(word) = ends.iter().position(|end| *end >= seen)
                && word + 1 < words.len()
                && seen > 0
            {
                preferred[word] = Some(true);
            }
        } else if kept(&c) {
            seen += 1;
        }
    }
    Some(preferred)
}

/// Runs of words a model can take in one request: they end at a sentence or a
/// long silence, so no request cuts a thought in half.
pub fn model_windows(words: &[CaptionWord], max_width: f32) -> Vec<Range<usize>> {
    let mut windows = Vec::new();
    let (mut start, mut width) = (0usize, 0.0f32);
    for index in 0..words.len() {
        width += word_width(&words[index]);
        let last = index + 1 == words.len();
        let natural =
            !last && (ends_sentence(&words[index].text) || gap_after(words, index) >= FORCED_GAP);
        if last || (natural && width >= max_width * 0.6) || width >= max_width * 1.4 {
            windows.push(start..index + 1);
            start = index + 1;
            width = 0.0;
        }
    }
    windows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(text: &str, gaps: &[(usize, f32)]) -> Vec<CaptionWord> {
        let mut time = 0.0;
        let mut words: Vec<CaptionWord> = Vec::new();
        for (index, c) in text.chars().enumerate() {
            if is_punctuation(c) {
                if let Some(last) = words.last_mut() {
                    last.text.push(c);
                }
                continue;
            }
            if let Some((_, gap)) = gaps.iter().find(|(at, _)| *at == index) {
                time += gap;
            }
            words.push(CaptionWord {
                text: c.to_string(),
                start: time,
                end: time + 0.2,
                hidden: false,
            });
            time += 0.2;
        }
        words
    }

    fn lines(words: &[CaptionWord], hints: &BreakHints) -> Vec<String> {
        plan_caption_lines(words, hints)
            .into_iter()
            .map(|range| model_input(&words[range]))
            .collect()
    }

    #[test]
    fn a_long_unpunctuated_sentence_does_not_leave_an_orphan() {
        let words = chars("那我们现在讲一讲这个事情是怎么发生的", &[]);
        let found = lines(&words, &BreakHints::default());
        assert!(found.iter().all(|line| text_width(line) <= MAX_LINE_WIDTH));
        assert!(
            found
                .iter()
                .all(|line| text_width(line) >= SHORT_LINE_WIDTH),
            "{found:?}"
        );
    }

    #[test]
    fn a_word_segmenter_keeps_words_whole() {
        let words = chars("那我们现在讲一讲这个事情是怎么发生的", &[]);
        let mut inside_word = vec![false; words.len() - 1];
        for pair in ["现在", "这个", "事情", "怎么", "发生"] {
            let at = model_input(&words).find(pair).unwrap() / 3;
            inside_word[at] = true;
        }
        let found = lines(
            &words,
            &BreakHints {
                inside_word,
                preferred: Vec::new(),
            },
        );
        assert!(found.iter().all(|line| !line.ends_with('发')), "{found:?}");
    }

    #[test]
    fn punctuation_and_pauses_win_over_an_even_split() {
        let words = chars("如果你热爱学习热爱读书，我推荐你一定要读这本书", &[]);
        let found = lines(&words, &BreakHints::default());
        assert_eq!(found[0], "如果你热爱学习热爱读书，");
        let paused = chars("这本书有一套非常经典的阅读方法值得一学", &[(6, 0.6)]);
        assert_eq!(lines(&paused, &BreakHints::default())[0], "这本书有一套");
    }

    #[test]
    fn a_long_silence_always_ends_a_line() {
        let words = chars("好的我们开始", &[(2, 1.5)]);
        assert_eq!(lines(&words, &BreakHints::default()), ["好的", "我们开始"]);
    }

    #[test]
    fn model_breaks_are_used_only_when_the_text_is_untouched() {
        let words = chars("如果你热爱学习热爱读书我推荐你读这本书", &[]);
        let preferred = model_breaks(&words, "如果你热爱学习热爱读书|我推荐你读这本书").unwrap();
        assert_eq!(preferred[10], Some(true));
        assert_eq!(preferred.iter().filter(|b| **b == Some(true)).count(), 1);
        assert!(model_breaks(&words, "如果你热爱学习和读书|我推荐你读这本书").is_none());
        let found = lines(
            &words,
            &BreakHints {
                inside_word: Vec::new(),
                preferred,
            },
        );
        assert_eq!(found, ["如果你热爱学习热爱读书", "我推荐你读这本书"]);
    }

    #[test]
    fn segments_drop_soft_punctuation_at_a_line_end() {
        let words = chars("热爱读书，我推荐你", &[]);
        let segments = segments_from_lines(&words, &[0..4, 4..words.len()], "segment-");
        assert_eq!(segments[0].text, "热爱读书");
        assert_eq!(segments[1].id, "segment-1");
    }

    #[test]
    fn hidden_fillers_take_no_room_on_a_line() {
        let mut words = chars("呃呃呃呃呃呃呃呃呃呃这本书有一套非常经典", &[]);
        for word in &mut words[..10] {
            word.hidden = true;
        }
        assert_eq!(plan_caption_lines(&words, &BreakHints::default()).len(), 1);
    }
}
