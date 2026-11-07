use std::ops::Range;

use crate::disfluency::{DisfluencyKind, DisfluencyToken, detect_disfluencies, normalize_word};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FillerAction {
    Keep,
    HideCaption,
    Cut,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FillerTerm {
    Word(String),
    Kind(DisfluencyKindOrder),
}

/// [`DisfluencyKind`] as a sortable grouping key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DisfluencyKindOrder {
    Repetition,
    FalseStart,
    Discourse,
}

/// One place a filler was said, and what to do about it. `review` marks the
/// places the rules cannot settle: they are never acted on without a person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FillerOccurrence {
    pub term: FillerTerm,
    pub words: Range<usize>,
    pub suggestion: FillerAction,
    pub review: bool,
}

pub const CHINESE_FILLER_TERMS: [&str; 8] = ["呃", "额", "嗯", "啊", "哦", "然后", "就是", "那个"];

/// Heard as nothing but a hesitation wherever they stand, so a new transcript
/// can hide them from captions without asking.
pub fn is_certain_chinese_filler(text: &str) -> bool {
    matches!(normalize_word(text).as_str(), "呃" | "嗯")
}

const STALL_GAP: f64 = 0.25;
const STALL_TOTAL: f64 = 0.5;
const STRETCHED_PER_CHAR: f64 = 0.45;

struct Context {
    set_off_before: bool,
    set_off_after: bool,
    stalls: bool,
}

fn ends_clause(text: &str) -> bool {
    text.trim_end().ends_with([
        ',', '，', '、', '.', '。', '!', '！', '?', '？', ';', '；', ':', '：', '…',
    ])
}

fn context_of(tokens: &[DisfluencyToken<'_>], words: &Range<usize>, chars: usize) -> Context {
    let first = &tokens[words.start];
    let last = &tokens[words.end - 1];
    let before = words.start.checked_sub(1).map(|index| &tokens[index]);
    let after = tokens.get(words.end);
    let gap_before = before.map_or(f64::MAX, |word| (first.start - word.end).max(0.0));
    let gap_after = after.map_or(f64::MAX, |word| (word.start - last.end).max(0.0));
    let duration = last.end - first.start;
    let spoken_gap = |gap: f64| if gap == f64::MAX { 0.0 } else { gap };
    Context {
        set_off_before: gap_before >= STALL_GAP || before.is_none_or(|word| ends_clause(word.text)),
        set_off_after: gap_after >= STALL_GAP || ends_clause(last.text),
        stalls: spoken_gap(gap_before) + spoken_gap(gap_after) >= STALL_TOTAL
            || duration >= STRETCHED_PER_CHAR * chars as f64,
    }
}

fn judge_chinese(term: &str, context: &Context, doubled: bool) -> Option<(FillerAction, bool)> {
    let set_off = context.set_off_before && context.set_off_after;
    let half_set_off = context.set_off_before || context.set_off_after;
    Some(match term {
        "呃" | "嗯" => (
            if context.stalls {
                FillerAction::Cut
            } else {
                FillerAction::HideCaption
            },
            false,
        ),
        // "金额" and "额外": only a word set off from its neighbours is a hesitation.
        "额" => {
            if !set_off {
                return None;
            }
            (
                if context.stalls {
                    FillerAction::Cut
                } else {
                    FillerAction::HideCaption
                },
                false,
            )
        }
        // After a word these are particles ("对啊"), which carry the speaker's tone.
        "啊" | "哦" => {
            if !context.set_off_before {
                (FillerAction::Keep, false)
            } else if set_off && context.stalls {
                (FillerAction::Cut, false)
            } else if set_off {
                (FillerAction::HideCaption, false)
            } else {
                (FillerAction::Keep, true)
            }
        }
        _ => {
            if doubled {
                (FillerAction::Cut, false)
            } else if set_off {
                (FillerAction::HideCaption, false)
            } else if half_set_off {
                (FillerAction::Keep, true)
            } else {
                (FillerAction::Keep, false)
            }
        }
    })
}

fn chinese_occurrences(tokens: &[DisfluencyToken<'_>]) -> Vec<FillerOccurrence> {
    let norms: Vec<String> = tokens
        .iter()
        .map(|token| normalize_word(token.text))
        .collect();
    let mut occurrences = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let matched = CHINESE_FILLER_TERMS.iter().find_map(|term| {
            let mut joined = String::new();
            for (offset, norm) in norms[index..].iter().enumerate() {
                joined.push_str(norm);
                if joined == *term {
                    return Some((*term, offset + 1));
                }
                if !term.starts_with(joined.as_str()) {
                    return None;
                }
            }
            None
        });
        let Some((term, length)) = matched else {
            index += 1;
            continue;
        };
        let words = index..index + length;
        let next = norms[words.end..]
            .iter()
            .take(length)
            .cloned()
            .collect::<String>();
        let doubled = next == term;
        let context = context_of(tokens, &words, term.chars().count());
        if let Some((suggestion, review)) = judge_chinese(term, &context, doubled) {
            occurrences.push(FillerOccurrence {
                term: FillerTerm::Word(term.to_string()),
                words: words.clone(),
                suggestion,
                review,
            });
        }
        index = words.end;
    }
    occurrences
}

fn english_occurrences(tokens: &[DisfluencyToken<'_>]) -> Vec<FillerOccurrence> {
    detect_disfluencies(tokens)
        .into_iter()
        .filter(|filler| {
            tokens[filler.words.clone()]
                .iter()
                .all(|token| !token.text.chars().any(crate::is_unspaced_script))
        })
        .map(|filler| {
            let chars = filler.phrase.chars().count().max(1);
            let context = context_of(tokens, &filler.words, chars.min(2));
            match filler.kind {
                DisfluencyKind::Hesitation => FillerOccurrence {
                    term: FillerTerm::Word(filler.phrase),
                    suggestion: if context.stalls {
                        FillerAction::Cut
                    } else {
                        FillerAction::HideCaption
                    },
                    words: filler.words,
                    review: false,
                },
                DisfluencyKind::Repetition => FillerOccurrence {
                    term: FillerTerm::Kind(DisfluencyKindOrder::Repetition),
                    words: filler.words,
                    suggestion: FillerAction::Cut,
                    review: false,
                },
                DisfluencyKind::FalseStart => FillerOccurrence {
                    term: FillerTerm::Kind(DisfluencyKindOrder::FalseStart),
                    words: filler.words,
                    suggestion: FillerAction::Keep,
                    review: true,
                },
                DisfluencyKind::Discourse => FillerOccurrence {
                    term: FillerTerm::Kind(DisfluencyKindOrder::Discourse),
                    words: filler.words,
                    suggestion: FillerAction::Keep,
                    review: true,
                },
            }
        })
        .collect()
}

/// Every filler in a timed transcript with a suggestion, sorted by position.
/// The rules only suggest a cut with evidence from the audio or a repeat: a
/// wrong cut breaks a sentence, while a wrong caption is one click from coming
/// back.
pub fn review_fillers(tokens: &[DisfluencyToken<'_>]) -> Vec<FillerOccurrence> {
    let mut occurrences = chinese_occurrences(tokens);
    occurrences.extend(english_occurrences(tokens));
    occurrences.sort_by_key(|occurrence| occurrence.words.start);
    occurrences
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lays out `(text, duration, gap before)` the way ASR times Chinese: one
    /// token per character.
    fn tokens(parts: &[(&'static str, f64, f64)]) -> Vec<DisfluencyToken<'static>> {
        let mut time = 0.0;
        let mut out = Vec::new();
        for (text, duration, gap) in parts {
            time += gap;
            out.push(DisfluencyToken {
                text,
                start: time,
                end: time + duration,
            });
            time += duration;
        }
        out
    }

    fn suggestions(parts: &[(&'static str, f64, f64)]) -> Vec<(String, FillerAction, bool)> {
        review_fillers(&tokens(parts))
            .into_iter()
            .map(|occurrence| {
                let FillerTerm::Word(term) = occurrence.term else {
                    panic!("expected a word");
                };
                (term, occurrence.suggestion, occurrence.review)
            })
            .collect()
    }

    #[test]
    fn a_stalled_hesitation_is_cut_and_a_light_one_only_leaves_the_caption() {
        let stalled = suggestions(&[
            ("很", 0.2, 0.0),
            ("好", 0.2, 0.0),
            ("呃", 0.3, 0.4),
            ("用", 0.2, 0.4),
        ]);
        assert_eq!(stalled, [("呃".to_string(), FillerAction::Cut, false)]);
        let light = suggestions(&[("我", 0.2, 0.0), ("嗯", 0.15, 0.05), ("看", 0.2, 0.05)]);
        assert_eq!(
            light,
            [("嗯".to_string(), FillerAction::HideCaption, false)]
        );
    }

    #[test]
    fn then_between_two_actions_is_left_alone() {
        let kept = suggestions(&[
            ("打", 0.2, 0.0),
            ("开", 0.2, 0.0),
            ("然", 0.15, 0.02),
            ("后", 0.15, 0.0),
            ("点", 0.2, 0.02),
        ]);
        assert_eq!(kept, [("然后".to_string(), FillerAction::Keep, false)]);
    }

    #[test]
    fn then_set_off_on_both_sides_leaves_the_caption_and_one_side_asks() {
        let hidden = suggestions(&[
            ("好，", 0.2, 0.0),
            ("然", 0.15, 0.1),
            ("后，", 0.15, 0.0),
            ("看", 0.2, 0.1),
        ]);
        assert_eq!(
            hidden,
            [("然后".to_string(), FillerAction::HideCaption, false)]
        );
        let asks = suggestions(&[
            ("好，", 0.2, 0.0),
            ("然", 0.15, 0.1),
            ("后", 0.15, 0.0),
            ("看", 0.2, 0.02),
        ]);
        assert_eq!(asks, [("然后".to_string(), FillerAction::Keep, true)]);
    }

    #[test]
    fn a_repeated_then_cuts_the_first() {
        let repeated = suggestions(&[
            ("然", 0.15, 0.0),
            ("后", 0.15, 0.0),
            ("然", 0.15, 0.02),
            ("后", 0.15, 0.0),
            ("看", 0.2, 0.02),
        ]);
        assert_eq!(repeated[0], ("然后".to_string(), FillerAction::Cut, false));
    }

    #[test]
    fn particles_and_words_containing_a_filler_are_safe() {
        let particle = suggestions(&[("对", 0.2, 0.0), ("啊", 0.2, 0.02), ("你", 0.2, 0.02)]);
        assert_eq!(particle, [("啊".to_string(), FillerAction::Keep, false)]);
        assert!(suggestions(&[("金", 0.2, 0.0), ("额", 0.2, 0.02), ("是", 0.2, 0.02)]).is_empty());
    }

    #[test]
    fn repeated_chinese_characters_are_not_english_repetitions() {
        assert!(suggestions(&[("谢", 0.2, 0.0), ("谢", 0.2, 0.0), ("你", 0.2, 0.0)]).is_empty());
    }

    #[test]
    fn english_fillers_keep_their_groups() {
        let parts = tokens(&[
            ("the", 0.2, 0.0),
            ("the", 0.2, 0.02),
            ("cat", 0.2, 0.02),
            ("um", 0.2, 0.5),
            ("sat", 0.2, 0.5),
        ]);
        let found = review_fillers(&parts);
        assert_eq!(
            found[0].term,
            FillerTerm::Kind(DisfluencyKindOrder::Repetition)
        );
        assert_eq!(found[0].suggestion, FillerAction::Cut);
        assert_eq!(found[1].term, FillerTerm::Word("um".to_string()));
        assert_eq!(found[1].suggestion, FillerAction::Cut);
    }
}
