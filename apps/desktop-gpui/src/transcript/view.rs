use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use cap_project::disfluency::{Disfluency, DisfluencyKind, normalize_word};
use cap_project::filler_review::{DisfluencyKindOrder, FillerAction, FillerOccurrence, FillerTerm};
use gpui::{
    AnyElement, AppContext, ClipboardItem, Context, FontWeight, HighlightStyle, Hsla,
    InteractiveElement, IntoElement, KeyDownEvent, ListOffset, MouseButton, MouseDownEvent,
    MouseMoveEvent, ParentElement, Pixels, Point, SharedString, StatefulInteractiveElement,
    StrikethroughStyle, Styled, StyledText, TextLayout, UnderlineStyle, WeakEntity, Window, div,
    prelude::FluentBuilder, px, svg,
};

use super::cuts::{self, CutReport, RestoreBlocker};
use super::model::{TranscriptDoc, TranscriptEdl, set_captions_hidden};
use super::{
    CleanupTab, DragUnit, FOLLOW_PAUSE, MAX_PLAYER_SHARE, MIN_PLAYER_SHARE, PAUSE_KEEP_RANGE,
    PAUSE_LONGER_RANGE, SearchState, WordDrag, WordSelection, category_index,
};
use crate::editor_window::EditorWindow;
use crate::i18n::tr;
use crate::theme::Theme;
use crate::ui;

const TEXT_SIZE: f32 = 15.;
const LINE_HEIGHT: f32 = 25.;
const STAMP_WIDTH: f32 = 46.;
const FOLLOW_MARGIN: f32 = 48.;
const FOLLOW_ANCHOR: f32 = 0.32;
const LISTEN_BEFORE: f64 = 1.5;
const LISTEN_AFTER: f64 = 1.0;

/// Chinese and Japanese have no spaces between words, and a transcript word
/// there is a character or two, so a query is found in the running text.
fn find_text_matches(doc: &TranscriptDoc, query: &str) -> Vec<Range<usize>> {
    let needle: String = query
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut text = String::new();
    let mut spans = Vec::with_capacity(doc.words.len());
    for word in &doc.words {
        if !text.is_empty() && !cap_project::joins_without_space(&text, &word.text) {
            text.push(' ');
        }
        let start = text.len();
        text.push_str(&word.text.to_lowercase());
        spans.push(start..text.len());
    }
    let mut matches = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find(&needle) {
        let (start, end) = (from + at, from + at + needle.len());
        let first = spans.partition_point(|span| span.end <= start);
        let last = spans.partition_point(|span| span.start < end);
        if first < last {
            matches.push(first..last);
        }
        from = end;
    }
    matches
}

pub(super) fn find_matches(doc: &TranscriptDoc, query: &str) -> Vec<Range<usize>> {
    if query.chars().any(cap_project::is_unspaced_script) {
        return find_text_matches(doc, query);
    }
    let needle: Vec<String> = query
        .split_whitespace()
        .map(normalize_word)
        .filter(|word| !word.is_empty())
        .collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let normalized: Vec<String> = doc
        .words
        .iter()
        .map(|word| normalize_word(&word.text))
        .collect();
    let mut matches = Vec::new();
    let mut index = 0;
    while index + needle.len() <= normalized.len() {
        let last = needle.len() - 1;
        let hit = needle.iter().enumerate().all(|(offset, word)| {
            let candidate = &normalized[index + offset];
            if offset == last {
                candidate.starts_with(word.as_str())
            } else {
                candidate == word
            }
        });
        if hit {
            matches.push(index..index + needle.len());
            index += needle.len();
        } else {
            index += 1;
        }
    }
    matches
}

fn sentence_bounds(doc: &TranscriptDoc, word: usize) -> (usize, usize) {
    let ends = |index: usize| {
        doc.words[index]
            .text
            .trim_end_matches(['"', '\'', '”', '’', ')'])
            .ends_with(['.', '!', '?', '…'])
    };
    let paragraph = &doc.paragraphs[doc.paragraph_of(word)].words;
    let mut start = word;
    while start > paragraph.start && !ends(start - 1) {
        start -= 1;
    }
    let mut end = word;
    while end + 1 < paragraph.end && !ends(end) {
        end += 1;
    }
    (start, end)
}

fn unit_bounds(doc: &TranscriptDoc, word: usize, unit: DragUnit) -> (usize, usize) {
    match unit {
        DragUnit::Word => (word, word),
        DragUnit::Sentence => sentence_bounds(doc, word),
        DragUnit::Paragraph => {
            let range = &doc.paragraphs[doc.paragraph_of(word)].words;
            (range.start, range.end - 1)
        }
    }
}

fn kind_label(kind: DisfluencyKind) -> &'static str {
    match kind {
        DisfluencyKind::Hesitation => tr::editor::transcript::fillers::hesitation(),
        DisfluencyKind::Repetition => tr::editor::transcript::fillers::repetition(),
        DisfluencyKind::FalseStart => tr::editor::transcript::fillers::false_start(),
        DisfluencyKind::Discourse => tr::editor::transcript::fillers::discourse(),
    }
}

fn seconds_label(value: f64) -> String {
    tr::editor::transcript::seconds(crate::i18n::decimal(format!("{value:.1}"))).to_string()
}

#[derive(Clone, Copy)]
struct Palette {
    text_1: Hsla,
    text_2: Hsla,
    text_3: Hsla,
    accent: Hsla,
    selection: Hsla,
    active_bg: Hsla,
    filler_bg: Hsla,
    filler_text: Hsla,
    match_bg: Hsla,
    current_match_bg: Hsla,
    restore_bg: Hsla,
}

impl Palette {
    fn new(theme: &Theme) -> Self {
        let editor = theme.editor;
        let accent = Hsla::from(editor.accent);
        Self {
            text_1: Hsla::from(editor.text_1),
            text_2: Hsla::from(editor.text_2),
            text_3: Hsla::from(editor.text_3),
            accent,
            selection: accent.opacity(0.24),
            active_bg: accent.opacity(0.12),
            filler_bg: Hsla::from(theme.amber_3),
            filler_text: Hsla::from(theme.amber_11),
            match_bg: Hsla::from(theme.amber_6).opacity(0.55),
            current_match_bg: Hsla::from(theme.amber_6),
            restore_bg: accent.opacity(0.14),
        }
    }
}

struct ParagraphPainter {
    doc: Arc<TranscriptDoc>,
    edl: Option<Arc<TranscriptEdl>>,
    selection: Option<Range<usize>>,
    active: Option<usize>,
    hovered_cut: Option<Range<usize>>,
    matches: Arc<Vec<Range<usize>>>,
    current_match: Option<Range<usize>>,
    ignored: Arc<HashSet<usize>>,
    palette: Palette,
    editor: WeakEntity<EditorWindow>,
    layouts: Rc<RefCell<HashMap<usize, TextLayout>>>,
}

impl ParagraphPainter {
    fn is_kept(&self, word: usize) -> bool {
        self.edl.as_ref().is_none_or(|edl| edl.is_kept(word))
    }

    fn in_match(&self, word: usize) -> Option<bool> {
        let index = self.matches.partition_point(|range| range.end <= word);
        let range = self
            .matches
            .get(index)
            .filter(|range| range.contains(&word))?;
        Some(self.current_match.as_ref() == Some(range))
    }

    fn word_style(&self, word: usize) -> HighlightStyle {
        let palette = &self.palette;
        let mut style = HighlightStyle::default();
        let kept = self.is_kept(word);
        if !kept {
            style.color = Some(palette.text_3);
            style.strikethrough = Some(StrikethroughStyle {
                thickness: px(1.),
                color: Some(palette.text_3),
            });
            if self
                .hovered_cut
                .as_ref()
                .is_some_and(|run| run.contains(&word))
            {
                style.background_color = Some(palette.restore_bg);
                style.color = Some(palette.text_2);
            }
        } else if self.doc.is_hidden(word) {
            style.color = Some(palette.text_3);
            style.underline = Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(palette.text_3),
                wavy: true,
            });
        } else if let Some(occurrence) = self.doc.marked_occurrence(word)
            && !self.ignored.contains(&occurrence.words.start)
        {
            style.background_color = Some(palette.filler_bg);
            style.color = Some(palette.filler_text);
        }
        if kept && self.active == Some(word) {
            style.color = Some(palette.accent);
            style.background_color = Some(palette.active_bg);
        }
        if let Some(current) = self.in_match(word) {
            style.background_color = Some(if current {
                palette.current_match_bg
            } else {
                palette.match_bg
            });
        }
        if self
            .selection
            .as_ref()
            .is_some_and(|range| range.contains(&word))
        {
            style.background_color = Some(palette.selection);
        }
        style
    }

    fn paragraph(&self, index: usize) -> AnyElement {
        let Some(paragraph) = self.doc.paragraphs.get(index) else {
            return div().into_any_element();
        };
        let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
        for (offset, span) in paragraph.spans.iter().enumerate() {
            let style = self.word_style(paragraph.words.start + offset);
            if style == HighlightStyle::default() {
                continue;
            }
            match highlights.last_mut() {
                Some((range, previous))
                    if *previous == style
                        && offset > 0
                        && range.end == paragraph.spans[offset - 1].end =>
                {
                    range.end = span.end;
                }
                _ => highlights.push((span.clone(), style)),
            }
        }
        let text = StyledText::new(paragraph.text.clone()).with_highlights(highlights);
        let layout = text.layout().clone();

        let first_kept = paragraph.words.clone().find(|word| self.is_kept(*word));
        let stamp = first_kept
            .and_then(|word| self.edl.as_ref()?.output_start_of(&self.doc.voiced[word]))
            .map(crate::editor_timeline::format_time);
        let hovering_cut_here = self
            .hovered_cut
            .as_ref()
            .is_some_and(|run| run.start < paragraph.words.end && paragraph.words.start < run.end);

        let editor = self.editor.clone();
        let down_layout = layout.clone();
        let move_editor = self.editor.clone();
        let move_layout = layout.clone();
        let stamp_editor = self.editor.clone();
        let layouts = self.layouts.clone();
        let paint_layout = layout.clone();
        div()
            .id(("transcript-paragraph", index))
            .w_full()
            .flex()
            .flex_row()
            .items_start()
            .pb(px(14.))
            .child(
                div()
                    .id(("transcript-stamp", index))
                    .flex_none()
                    .w(px(STAMP_WIDTH))
                    .pt(px(3.))
                    .text_size(px(11.))
                    .line_height(px(LINE_HEIGHT - 4.))
                    .text_color(self.palette.text_3)
                    .cursor_pointer()
                    .hover(|style| style.text_color(self.palette.text_2))
                    .children(stamp)
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        stamp_editor
                            .update(cx, |this, cx| {
                                this.transcript_select_paragraph(index, window, cx);
                            })
                            .ok();
                    }),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .when(hovering_cut_here, |this| this.cursor_pointer())
                    .child(text)
                    .child(
                        gpui::canvas(
                            move |_, _, _| {
                                layouts.borrow_mut().insert(index, paint_layout);
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_0(),
                    )
                    .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                        let byte = byte_at(&down_layout, event.position);
                        editor
                            .update(cx, |this, cx| {
                                this.transcript_mouse_down(index, byte, event, window, cx);
                            })
                            .ok();
                    })
                    .on_mouse_move(move |event, window, cx| {
                        let byte = byte_at(&move_layout, event.position);
                        move_editor
                            .update(cx, |this, cx| {
                                this.transcript_mouse_move(index, byte, event, window, cx);
                            })
                            .ok();
                    }),
            )
            .into_any_element()
    }
}

const ROW_HEIGHT: f32 = 30.;
const MAX_VISIBLE_ROWS: usize = 8;

fn snippet_around(doc: &TranscriptDoc, words: Range<usize>) -> (String, Range<usize>) {
    let paragraph = &doc.paragraphs[doc.paragraph_of(words.start)].words;
    let before_start = paragraph.start.max(words.start.saturating_sub(7));
    let after_end = doc.paragraphs[doc.paragraph_of(words.end - 1)]
        .words
        .end
        .min(words.end + 7);
    let phrase = doc.text_of(words.clone());
    let push = |snippet: &mut String, text: &str| {
        if !snippet.is_empty() && !cap_project::joins_without_space(snippet, text) {
            snippet.push(' ');
        }
        snippet.push_str(text);
    };
    let mut snippet = String::new();
    if before_start < words.start {
        snippet.push('…');
        push(&mut snippet, &doc.text_of(before_start..words.start));
    }
    push(&mut snippet, &phrase);
    let highlight = snippet.len() - phrase.len()..snippet.len();
    if words.end < after_end {
        push(&mut snippet, &doc.text_of(words.end..after_end));
        snippet.push('…');
    }
    (snippet, highlight)
}

fn row_stamp(palette: Palette, stamp: String) -> impl IntoElement {
    div()
        .flex_none()
        .w(px(38.))
        .text_size(px(11.))
        .text_color(palette.text_3)
        .child(stamp)
}

#[allow(clippy::too_many_arguments)]
fn filler_row(
    theme: &Theme,
    palette: Palette,
    doc: &TranscriptDoc,
    edl: &TranscriptEdl,
    index: usize,
    words: Range<usize>,
    selected: bool,
    editor: WeakEntity<EditorWindow>,
) -> AnyElement {
    let (snippet, highlight) = snippet_around(doc, words.clone());
    let text = StyledText::new(snippet).with_highlights([(
        highlight,
        HighlightStyle {
            background_color: Some(palette.filler_bg),
            color: Some(palette.filler_text),
            ..Default::default()
        },
    )]);
    let stamp = edl
        .output_start_of(&doc.voiced[words.start])
        .map(crate::editor_timeline::format_time)
        .unwrap_or_default();
    let on_row = {
        let editor = editor.clone();
        let words = words.clone();
        move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut gpui::App| {
            editor
                .update(cx, |this, cx| {
                    window.focus(&this.transcript.focus, cx);
                    this.transcript_focus_filler(words.clone(), false, cx);
                })
                .ok();
        }
    };
    let on_listen = {
        let editor = editor.clone();
        let words = words.clone();
        move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut gpui::App| {
            editor
                .update(cx, |this, cx| {
                    this.transcript_focus_filler(words.clone(), true, cx);
                })
                .ok();
        }
    };
    let on_keep = {
        let editor = editor.clone();
        let start = words.start;
        move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut gpui::App| {
            editor
                .update(cx, |this, cx| {
                    this.transcript.cleanup.ignored.insert(start);
                    this.invalidate_transcript(cx);
                })
                .ok();
        }
    };
    let on_hide = {
        let editor = editor.clone();
        let words = words.clone();
        move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut gpui::App| {
            editor
                .update(cx, |this, cx| {
                    this.transcript_set_hidden(&[words.clone()], true, window, cx);
                })
                .ok();
        }
    };
    let on_cut = move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut gpui::App| {
        editor
            .update(cx, |this, cx| {
                this.transcript_remove_filler(words.clone(), window, cx);
            })
            .ok();
    };
    div()
        .id(("transcript-filler", index))
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.))
        .h(px(ROW_HEIGHT))
        .px(px(6.))
        .rounded(px(6.))
        .when(selected, |this| this.bg(palette.selection.opacity(0.6)))
        .hover(|style| style.bg(Hsla::from(theme.editor.ctl)))
        .cursor_pointer()
        .on_click(on_row)
        .child(row_stamp(palette, stamp))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(13.))
                .text_color(palette.text_2)
                .truncate()
                .child(text),
        )
        .child(
            ui::EditorButton::plain(theme, ("transcript-filler-listen", index))
                .left_icon("icons/play.svg")
                .tooltip(theme, tr::editor::transcript::fillers::listen())
                .on_click(on_listen),
        )
        .child(
            ui::EditorButton::plain(theme, ("transcript-filler-keep", index))
                .label(tr::editor::transcript::fillers::action_keep())
                .on_click(on_keep),
        )
        .child(
            ui::EditorButton::plain(theme, ("transcript-filler-hide", index))
                .label(tr::editor::transcript::fillers::action_hide())
                .on_click(on_hide),
        )
        .child(
            ui::EditorButton::plain(theme, ("transcript-filler-cut", index))
                .label(tr::editor::transcript::fillers::action_cut())
                .on_click(on_cut),
        )
        .into_any_element()
}

fn pause_row(
    theme: &Theme,
    palette: Palette,
    doc: &TranscriptDoc,
    pause: &cuts::PauseCut,
    keep: f64,
    editor: WeakEntity<EditorWindow>,
) -> AnyElement {
    let word = pause.after_word;
    let next = pause.next_word;
    let stamp = crate::editor_timeline::format_time(pause.output.start);
    let before = doc.text_of(word.saturating_sub(3)..word + 1);
    let after = doc.text_of(next..(next + 3).min(doc.words.len()));
    let text = format!(
        "{before}  {} → {}  {after}",
        seconds_label(pause.length),
        seconds_label(keep)
    );
    div()
        .id(("transcript-pause", word))
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.))
        .h(px(ROW_HEIGHT))
        .px(px(6.))
        .rounded(px(6.))
        .hover(|style| style.bg(Hsla::from(theme.editor.ctl)))
        .cursor_pointer()
        .on_click(move |_, window, cx| {
            editor
                .update(cx, |this, cx| {
                    window.focus(&this.transcript.focus, cx);
                    this.transcript_focus_filler(word..next + 1, true, cx);
                })
                .ok();
        })
        .child(row_stamp(palette, stamp))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(13.))
                .text_color(palette.text_2)
                .truncate()
                .child(text),
        )
        .into_any_element()
}

fn byte_at(layout: &TextLayout, position: Point<Pixels>) -> usize {
    match layout.index_for_position(position) {
        Ok(index) | Err(index) => index,
    }
}

impl EditorWindow {
    /// Opens the transcript panel with its find-and-replace bar focused.
    pub(crate) fn open_transcript_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.transcript.open {
            self.toggle_transcript(window, cx);
        }
        self.transcript_open_search(window, cx);
    }

    pub(crate) fn open_transcript_cleanup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.transcript.open {
            self.toggle_transcript(window, cx);
        }
        self.transcript.cleanup.open = true;
        cx.notify();
    }

    /// Debug-build screenshot harness: the transcript panel open with its search
    /// and replace fields filled.
    pub(crate) fn harness_transcript_search(
        &mut self,
        find: &str,
        replace: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_transcript_search(window, cx);
        if let Some(search) = &self.transcript.search {
            let (input, replace_input) = (search.input.clone(), search.replace.clone());
            input.update(cx, |input, cx| input.set_text(find.to_string(), cx));
            replace_input.update(cx, |input, cx| input.set_text(replace.to_string(), cx));
        }
        self.transcript_search_changed(find.to_string(), cx);
    }

    /// Debug-build screenshot harness: press "Replace all".
    pub(crate) fn harness_transcript_replace_all(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.transcript_replace(true, window, cx);
    }

    pub(crate) fn toggle_transcript(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.project_ready() {
            return;
        }
        if self.transcript.open {
            self.transcript.open = false;
            self.transcript.drag = None;
            self.transcript.resize = None;
            self.focus_root(window, cx);
        } else {
            if self.clips.open {
                self.close_clips(window, cx);
            }
            self.transcript.open = true;
            self.transcript.dirty = true;
            let started = Instant::now();
            self.transcript_refresh();
            tracing::info!(
                elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
                words = self
                    .transcript
                    .doc
                    .as_ref()
                    .map_or(0, |doc| doc.words.len()),
                paragraphs = self
                    .transcript
                    .doc
                    .as_ref()
                    .map_or(0, |doc| doc.paragraphs.len()),
                "transcript opened"
            );
            self.transcript_sync_active(false, cx);
            if let Some(active) = self.transcript.active {
                self.transcript_reveal(active, true);
            }
            window.focus(&self.transcript.focus, cx);
            self.transcript_load_energy(cx);
        }
        cx.notify();
    }

    pub(crate) fn transcript_refresh(&mut self) -> bool {
        if !self.transcript.open {
            return false;
        }
        let durations = self.clip_display_durations.clone();
        self.transcript.refresh(&self.project, &durations)
    }

    pub(crate) fn transcript_sync_active(&mut self, follow: bool, cx: &mut Context<Self>) {
        if !self.transcript.open {
            return;
        }
        let (Some(doc), Some(edl)) = (&self.transcript.doc, &self.transcript.edl) else {
            return;
        };
        let active = edl
            .output_to_source(self.playhead_time())
            .and_then(|source| doc.word_at_source(source))
            .filter(|word| edl.is_kept(*word));
        if active == self.transcript.active {
            return;
        }
        self.transcript.active = active;
        if follow
            && let Some(active) = active
            && self
                .transcript
                .user_scrolled
                .is_none_or(|at| at.elapsed() > FOLLOW_PAUSE)
            && self.transcript.drag.is_none()
        {
            self.transcript_reveal(active, true);
        }
        self.invalidate_transcript(cx);
    }

    pub(crate) fn transcript_reveal(&mut self, word: usize, follow: bool) {
        let Some(doc) = self.transcript.doc.clone() else {
            return;
        };
        let paragraph = doc.paragraph_of(word);
        let viewport = self.transcript.list.viewport_bounds();
        let height = viewport.size.height;
        if height <= px(0.) {
            self.transcript.list.scroll_to(ListOffset {
                item_ix: paragraph,
                offset_in_item: px(0.),
            });
            return;
        }
        let position = self
            .transcript
            .layouts
            .borrow()
            .get(&paragraph)
            .zip(doc.paragraphs[paragraph].span(word))
            .and_then(|(layout, span)| layout.position_for_index(span.start));
        let visible_paragraph =
            self.transcript
                .list
                .bounds_for_item(paragraph)
                .is_some_and(|bounds| {
                    bounds.bottom() > viewport.top() && bounds.top() < viewport.bottom()
                });
        match position.filter(|_| visible_paragraph) {
            Some(position) => {
                let top = viewport.top() + px(FOLLOW_MARGIN);
                let bottom = viewport.bottom() - px(FOLLOW_MARGIN + LINE_HEIGHT);
                if position.y >= top && position.y <= bottom {
                    return;
                }
                let target = if follow {
                    viewport.top() + height * FOLLOW_ANCHOR
                } else if position.y < top {
                    top
                } else {
                    bottom
                };
                self.transcript.list.scroll_by(position.y - target);
            }
            None => {
                self.transcript.list.scroll_to(ListOffset {
                    item_ix: paragraph,
                    offset_in_item: px(0.),
                });
                if follow {
                    self.transcript.list.scroll_by(-(height * FOLLOW_ANCHOR));
                }
            }
        }
    }

    fn transcript_doc(&self) -> Option<(Arc<TranscriptDoc>, Arc<TranscriptEdl>)> {
        Some((self.transcript.doc.clone()?, self.transcript.edl.clone()?))
    }

    fn transcript_seek_word(&mut self, word: usize, cx: &mut Context<Self>) {
        let Some((doc, edl)) = self.transcript_doc() else {
            return;
        };
        let Some(output) = doc
            .voiced
            .get(word)
            .and_then(|word| edl.output_start_of(word))
        else {
            return;
        };
        self.transcript.user_scrolled = None;
        self.seek_to_time(output, cx);
        self.transcript.active = edl.is_kept(word).then_some(word);
    }

    pub(crate) fn transcript_mouse_down(
        &mut self,
        paragraph: usize,
        byte: usize,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((doc, edl)) = self.transcript_doc() else {
            return;
        };
        let Some(word) = doc.paragraphs.get(paragraph).map(|p| p.word_at_byte(byte)) else {
            return;
        };
        window.focus(&self.transcript.focus, cx);
        cx.stop_propagation();
        self.set_selection(None, cx);

        if !edl.is_kept(word) && !event.modifiers.shift && event.click_count == 1 {
            if let Some(run) = edl.cut_run_of(word) {
                let words = edl.cut_runs[run].clone();
                self.transcript_restore(words, window, cx);
            }
            return;
        }

        if doc.is_hidden(word) && !event.modifiers.shift && event.click_count == 1 {
            let words = doc
                .occurrence_at(word)
                .map_or(word..word + 1, |occurrence| occurrence.words.clone());
            self.transcript_set_hidden(&[words], false, window, cx);
            return;
        }

        let unit = match event.click_count {
            2 => DragUnit::Sentence,
            count if count >= 3 => DragUnit::Paragraph,
            _ => DragUnit::Word,
        };
        let origin = unit_bounds(&doc, word, unit);
        if event.modifiers.shift
            && let Some(selection) = self
                .transcript
                .selection
                .or(self.transcript.caret.map(WordSelection::single))
        {
            self.transcript.selection = Some(WordSelection {
                anchor: selection.anchor,
                focus: word,
            });
            self.transcript.drag = Some(WordDrag {
                unit: DragUnit::Word,
                origin: (selection.anchor, selection.anchor),
            });
        } else {
            self.transcript.selection = Some(WordSelection {
                anchor: origin.0,
                focus: origin.1,
            });
            self.transcript.drag = Some(WordDrag { unit, origin });
            self.transcript_seek_word(origin.0, cx);
        }
        self.transcript.caret = None;
        self.invalidate_transcript(cx);
    }

    pub(crate) fn transcript_mouse_move(
        &mut self,
        paragraph: usize,
        byte: usize,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((doc, edl)) = self.transcript_doc() else {
            return;
        };
        let Some(word) = doc.paragraphs.get(paragraph).map(|p| p.word_at_byte(byte)) else {
            return;
        };
        if let Some(drag) = self.transcript.drag {
            if !event.dragging() {
                self.transcript.drag = None;
                return;
            }
            let (low, high) = unit_bounds(&doc, word, drag.unit);
            let selection = if word < drag.origin.0 {
                WordSelection {
                    anchor: drag.origin.1,
                    focus: low,
                }
            } else {
                WordSelection {
                    anchor: drag.origin.0,
                    focus: high.max(drag.origin.1),
                }
            };
            if self.transcript.selection != Some(selection) {
                self.transcript.selection = Some(selection);
                self.invalidate_transcript(cx);
            }
            return;
        }
        let hovered = (!edl.is_kept(word)).then(|| edl.cut_run_of(word)).flatten();
        if hovered != self.transcript.hovered_cut {
            self.transcript.hovered_cut = hovered;
            self.invalidate_transcript(cx);
        }
    }

    fn transcript_select_paragraph(
        &mut self,
        paragraph: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.transcript.doc.clone() else {
            return;
        };
        let Some(range) = doc.paragraphs.get(paragraph).map(|p| p.words.clone()) else {
            return;
        };
        window.focus(&self.transcript.focus, cx);
        cx.stop_propagation();
        self.transcript.selection = Some(WordSelection {
            anchor: range.start,
            focus: range.end - 1,
        });
        self.transcript_seek_word(range.start, cx);
        self.invalidate_transcript(cx);
    }

    fn transcript_end_drag(&mut self, cx: &mut Context<Self>) {
        if self.transcript.drag.take().is_some() {
            self.invalidate_transcript(cx);
        }
    }

    pub(crate) fn transcript_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ui::text_input_has_focus(window, cx) {
            return;
        }
        let Some((doc, edl)) = self.transcript_doc() else {
            return;
        };
        if doc.words.is_empty() {
            return;
        }
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        let command = modifiers.secondary();
        let last = doc.words.len() - 1;
        let handled = match keystroke.key.as_str() {
            "f" if command => {
                self.transcript_open_search(window, cx);
                true
            }
            "a" if command && !modifiers.shift => {
                self.transcript.selection = Some(WordSelection {
                    anchor: 0,
                    focus: last,
                });
                true
            }
            "c" if command && !modifiers.shift => {
                self.transcript_copy(cx);
                true
            }
            "backspace" | "delete" if !command && !modifiers.alt => {
                if self.transcript.selection.is_some() {
                    self.transcript_delete_selection(window, cx);
                    true
                } else {
                    false
                }
            }
            "h" if !command && !modifiers.alt => {
                let Some(selection) = self.transcript.selection else {
                    return;
                };
                let words = selection.range();
                let hide = !self
                    .transcript
                    .doc
                    .as_ref()
                    .is_some_and(|doc| doc.is_range_hidden(words.clone()));
                self.transcript_set_hidden(&[words], hide, window, cx);
                true
            }
            "enter" if !command => {
                self.transcript_play_selection(cx);
                true
            }
            "escape" => {
                if self.transcript.search.is_some() {
                    self.transcript_close_search(window, cx);
                    true
                } else if self.transcript.selection.take().is_some() {
                    true
                } else if self.transcript.cleanup.open {
                    self.transcript.cleanup.open = false;
                    true
                } else {
                    false
                }
            }
            "left" | "right" | "up" | "down" | "home" | "end" if !modifiers.alt => {
                let from = self
                    .transcript
                    .selection
                    .map(|selection| selection.focus)
                    .or(self.transcript.caret)
                    .or(self.transcript.active)
                    .unwrap_or(0);
                let target = match keystroke.key.as_str() {
                    "left" if command => doc.paragraphs[doc.paragraph_of(from)].words.start,
                    "right" if command => doc.paragraphs[doc.paragraph_of(from)].words.end - 1,
                    "left" => from.saturating_sub(1),
                    "right" => (from + 1).min(last),
                    "up" if command => 0,
                    "down" if command => last,
                    "home" => 0,
                    "end" => last,
                    "up" => self.transcript_vertical(&doc, from, -1),
                    _ => self.transcript_vertical(&doc, from, 1),
                };
                let had_selection = self.transcript.selection.is_some();
                if modifiers.shift {
                    let anchor = self
                        .transcript
                        .selection
                        .map_or(from, |selection| selection.anchor);
                    self.transcript.selection = Some(WordSelection {
                        anchor,
                        focus: target,
                    });
                } else {
                    let target = if had_selection
                        || self.transcript.caret.is_some()
                        || self.transcript.active.is_some()
                    {
                        target
                    } else {
                        0
                    };
                    self.transcript.selection = Some(WordSelection::single(target));
                    if edl.is_kept(target) {
                        self.transcript_seek_word(target, cx);
                    }
                }
                self.transcript.caret = None;
                self.transcript_reveal(target, false);
                true
            }
            _ => false,
        };
        if handled {
            cx.stop_propagation();
            window.prevent_default();
            self.invalidate_transcript(cx);
        }
    }

    fn transcript_vertical(&self, doc: &TranscriptDoc, from: usize, direction: i32) -> usize {
        let paragraph = doc.paragraph_of(from);
        let layouts = self.transcript.layouts.borrow();
        let fallback = || {
            if direction < 0 {
                paragraph
                    .checked_sub(1)
                    .map_or(0, |previous| doc.paragraphs[previous].words.end - 1)
            } else {
                doc.paragraphs
                    .get(paragraph + 1)
                    .map_or(doc.words.len() - 1, |next| next.words.start)
            }
        };
        let Some(position) = layouts
            .get(&paragraph)
            .zip(doc.paragraphs[paragraph].span(from))
            .and_then(|(layout, span)| layout.position_for_index(span.start))
        else {
            return fallback();
        };
        let line = px(LINE_HEIGHT);
        let target = Point {
            x: position.x + px(2.),
            y: position.y + line * 0.5 + line * direction as f32,
        };
        for (index, layout) in layouts.iter() {
            let bounds = layout.bounds();
            if target.y >= bounds.top() && target.y <= bounds.bottom() {
                let probe = Point {
                    x: target.x.max(bounds.left()),
                    y: target.y,
                };
                return doc.paragraphs[*index].word_at_byte(byte_at(layout, probe));
            }
        }
        fallback()
    }

    fn transcript_copy(&mut self, cx: &mut Context<Self>) {
        let (Some((doc, edl)), Some(selection)) =
            (self.transcript_doc(), self.transcript.selection)
        else {
            return;
        };
        let text = selection
            .range()
            .filter(|word| edl.is_kept(*word))
            .map(|word| doc.words[word].text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        if !text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn transcript_play_selection(&mut self, cx: &mut Context<Self>) {
        let (Some((doc, edl)), Some(selection)) =
            (self.transcript_doc(), self.transcript.selection)
        else {
            return;
        };
        let mut kept = selection.range().filter(|word| edl.is_kept(*word));
        let Some(first) = kept.next() else {
            return;
        };
        let last = kept.next_back().unwrap_or(first);
        let (Some(from), Some(until)) = (
            edl.output_start_of(&doc.voiced[first]),
            edl.output_end_of(&doc.voiced[last]),
        ) else {
            return;
        };
        self.play_range(from, until + 0.15, cx);
    }

    pub(super) fn transcript_cut_runs(
        &mut self,
        runs: &[Range<usize>],
        reason: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> CutReport {
        let Some(doc) = self.transcript.doc.clone() else {
            return CutReport::default();
        };
        let energy = self.transcript.energy.clone();
        let edl = self.transcript.edl.clone();
        let cuts = cuts::plan_word_cuts(
            &doc,
            energy.as_deref(),
            edl.as_ref().map(|edl| edl.kept.as_slice()),
            runs,
        );
        self.transcript_apply_cuts(&cuts, reason, window, cx)
    }

    fn transcript_cut_filler_runs(
        &mut self,
        runs: &[Range<usize>],
        reason: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> CutReport {
        let Some(doc) = self.transcript.doc.clone() else {
            return CutReport::default();
        };
        let energy = self.transcript.energy.clone();
        let edl = self.transcript.edl.clone();
        let cuts = cuts::plan_filler_cuts(
            &doc,
            energy.as_deref(),
            edl.as_ref().map(|edl| edl.kept.as_slice()),
            runs,
        );
        self.transcript_apply_cuts(&cuts, reason, window, cx)
    }

    fn transcript_apply_cuts(
        &mut self,
        cuts: &[Range<f64>],
        reason: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> CutReport {
        if cuts.is_empty() {
            return CutReport::default();
        }
        let started = Instant::now();
        let durations = self.clip_display_durations.clone();
        let mut report = CutReport::default();
        self.edit_timeline_project(
            |project| {
                if !crate::editor_edits::ensure_timeline(project, &durations) {
                    return false;
                }
                let Some(timeline) = project.timeline.as_mut() else {
                    return false;
                };
                report = cuts::apply_source_cuts(timeline, &durations, cuts);
                report.applied > 0
            },
            window,
            cx,
        );
        self.transcript.dirty = true;
        self.transcript_refresh();
        tracing::info!(
            reason,
            cuts = cuts.len(),
            applied = report.applied,
            skipped = report.skipped(),
            elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
            "transcript cut"
        );
        if report.applied > 0 {
            self.note_edit(reason, Some(crate::editor_timeline::TrackKind::Clip));
        }
        if report.transition > 0 {
            self.show_notice(
                tr::editor::transcript::skipped_transition(report.transition),
                window,
                cx,
            );
        } else if report.hold > 0 {
            self.show_notice(
                tr::editor::transcript::skipped_hold(report.hold),
                window,
                cx,
            );
        } else if report.last_clip > 0 {
            self.show_notice(tr::editor::transcript::skipped_last_clip(), window, cx);
        }
        self.transcript_sync_active(false, cx);
        report
    }

    pub(super) fn transcript_delete_selection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some((doc, edl)), Some(selection)) =
            (self.transcript_doc(), self.transcript.selection)
        else {
            return;
        };
        let range = selection.range();
        if range.clone().all(|word| !edl.is_kept(word)) {
            self.transcript_restore(range, window, cx);
            return;
        }
        let first_kept = range.clone().find(|word| edl.is_kept(*word));
        let report = self.transcript_cut_runs(
            std::slice::from_ref(&range),
            "transcript-delete",
            window,
            cx,
        );
        if report.applied == 0 {
            return;
        }
        self.transcript.selection = None;
        self.transcript.caret = Some(range.end.min(doc.words.len() - 1));
        if !self.playing
            && let (Some(first), Some(edl)) = (first_kept, self.transcript.edl.clone())
        {
            let join = (first..doc.words.len())
                .find(|word| edl.is_kept(*word))
                .and_then(|word| edl.output_start_of(&doc.voiced[word]));
            if let Some(join) = join {
                self.seek_to_time(join, cx);
                self.transcript_sync_active(false, cx);
            }
        }
    }

    pub(super) fn transcript_restore(
        &mut self,
        words: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.transcript.doc.clone() else {
            return;
        };
        let durations = self.clip_display_durations.clone();
        let removed_takes = self
            .project
            .timeline
            .as_ref()
            .map(|timeline| self.history.removed_takes(timeline))
            .unwrap_or_default();
        let mut outcome = Ok(0);
        self.edit_timeline_project(
            |project| {
                let Some(timeline) = project.timeline.as_mut() else {
                    return false;
                };
                outcome =
                    cuts::restore_words(timeline, &durations, &removed_takes, &doc, words.clone());
                matches!(outcome, Ok(count) if count > 0)
            },
            window,
            cx,
        );
        self.transcript.dirty = true;
        self.transcript_refresh();
        self.transcript.hovered_cut = None;
        match outcome {
            Ok(count) if count > 0 => {
                self.note_edit(
                    "transcript-restore",
                    Some(crate::editor_timeline::TrackKind::Clip),
                );
                self.transcript.selection = Some(WordSelection {
                    anchor: words.start,
                    focus: words.end - 1,
                });
            }
            Ok(_) => {}
            Err(RestoreBlocker::Transition) => {
                self.show_notice(tr::editor::transcript::restore_transition(), window, cx);
            }
            Err(RestoreBlocker::NotAdjacent) => {
                self.show_notice(tr::editor::transcript::restore_blocked(), window, cx);
            }
        }
        self.invalidate_transcript(cx);
    }

    pub(super) fn transcript_open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(search) = &self.transcript.search {
            let input = search.input.clone();
            input.update(cx, |input, cx| input.focus_and_select_all(window, cx));
            return;
        }
        let input = cx.new(|cx| {
            let mut input = ui::TextInputState::single_line(window, cx);
            input.set_placeholder(tr::editor::transcript::search_placeholder());
            input
        });
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, input, event: &ui::TextInputEvent, window, cx| match event {
                ui::TextInputEvent::Changed => {
                    let query = input.read(cx).text().to_string();
                    this.transcript_search_changed(query, cx);
                }
                ui::TextInputEvent::Confirmed => {
                    let back = window.modifiers().shift;
                    this.transcript_step_match(if back { -1 } else { 1 }, cx);
                }
                ui::TextInputEvent::Cancelled => this.transcript_close_search(window, cx),
                ui::TextInputEvent::Blurred => {}
            },
        );
        self.push_text_subscription(subscription);
        let replace = cx.new(|cx| {
            let mut input = ui::TextInputState::single_line(window, cx);
            input.set_placeholder(tr::editor::transcript::replace_placeholder());
            input
        });
        let subscription = cx.subscribe_in(
            &replace,
            window,
            |this: &mut Self, _, event: &ui::TextInputEvent, window, cx| match event {
                ui::TextInputEvent::Confirmed => this.transcript_replace(false, window, cx),
                ui::TextInputEvent::Cancelled => this.transcript_close_search(window, cx),
                _ => {}
            },
        );
        self.push_text_subscription(subscription);
        input.update(cx, |input, cx| input.focus_and_select_all(window, cx));
        self.transcript.search = Some(SearchState {
            input,
            replace,
            replaced: None,
            matches: Arc::new(Vec::new()),
            current: 0,
            query: String::new(),
        });
        if let Some(selection) = self.transcript.selection
            && let Some(doc) = self.transcript.doc.clone()
            && selection.range().len() <= 6
        {
            let text = doc.text_of(selection.range());
            if let Some(search) = &self.transcript.search {
                search
                    .input
                    .update(cx, |input, cx| input.set_text(text.clone(), cx));
            }
            self.transcript_search_changed(text, cx);
        }
        self.invalidate_transcript(cx);
    }

    fn transcript_close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.transcript.search = None;
        window.focus(&self.transcript.focus, cx);
        self.invalidate_transcript(cx);
    }

    pub(super) fn transcript_search_changed(&mut self, query: String, cx: &mut Context<Self>) {
        let Some(doc) = self.transcript.doc.clone() else {
            return;
        };
        let matches = find_matches(&doc, &query);
        let playhead_word = self.transcript.active.unwrap_or(0);
        let Some(search) = self.transcript.search.as_mut() else {
            return;
        };
        search.replaced = None;
        search.query = query;
        search.current = matches
            .iter()
            .position(|range| range.start >= playhead_word)
            .unwrap_or(0);
        search.matches = Arc::new(matches);
        let first = search.matches.get(search.current).map(|range| range.start);
        if let Some(first) = first {
            self.transcript_reveal(first, false);
        }
        self.invalidate_transcript(cx);
    }

    fn transcript_step_match(&mut self, step: i32, cx: &mut Context<Self>) {
        let Some(search) = self.transcript.search.as_mut() else {
            return;
        };
        let count = search.matches.len();
        if count == 0 {
            return;
        }
        search.current =
            (search.current as i64 + i64::from(step)).rem_euclid(count as i64) as usize;
        let range = search.matches[search.current].clone();
        self.transcript.selection = Some(WordSelection {
            anchor: range.start,
            focus: range.end - 1,
        });
        self.transcript_seek_word(range.start, cx);
        self.transcript_reveal(range.start, false);
        self.invalidate_transcript(cx);
    }

    /// Replaces the current match, or every match, in the captions. The
    /// replacement is also remembered as a transcription term, so the next
    /// transcription spells it right.
    fn transcript_replace(&mut self, all: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(search), Some(doc)) = (&self.transcript.search, self.transcript.doc.clone())
        else {
            return;
        };
        let find = search.query.trim().to_string();
        let replacement = search.replace.read(cx).text().trim().to_string();
        if find.is_empty() || search.matches.is_empty() {
            return;
        }
        let within = if all {
            None
        } else {
            let Some(range) = search.matches.get(search.current) else {
                return;
            };
            let (first, last) = (&doc.words[range.start], &doc.words[range.end - 1]);
            Some(first.start..last.end)
        };
        let recording_durations = self
            .summary()
            .map(|summary| summary.clip_display_durations.clone())
            .unwrap_or_default();
        let replaced = std::rc::Rc::new(std::cell::Cell::new(0));
        let counted = replaced.clone();
        let (find_for_edit, replacement_for_edit) = (find.clone(), replacement.clone());
        self.edit_project_global("transcript-replace", window, cx, move |project| {
            let Some(captions) = project.captions.as_mut() else {
                return false;
            };
            let count = crate::caption_replace::replace_in_captions(
                &mut captions.segments,
                &find_for_edit,
                &replacement_for_edit,
                within,
            );
            if count == 0 {
                return false;
            }
            counted.set(count);
            let segments = captions.segments.clone();
            if let Some(timeline) = project.timeline.as_mut() {
                timeline.caption_segments = cap_project::derive_caption_track_segments(
                    &segments,
                    timeline,
                    &recording_durations,
                );
            }
            true
        });
        if replaced.get() > 0 {
            crate::caption_replace::remember_term(&replacement);
        }
        if let Some(search) = self.transcript.search.as_mut() {
            search.replaced = Some(replaced.get());
        }
        self.invalidate_transcript(cx);
    }

    fn transcript_delete_matches(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(search) = &self.transcript.search else {
            return;
        };
        let Some(edl) = self.transcript.edl.clone() else {
            return;
        };
        let runs: Vec<Range<usize>> = search
            .matches
            .iter()
            .filter(|range| (*range).clone().any(|word| edl.is_kept(word)))
            .cloned()
            .collect();
        self.transcript.selection = None;
        self.transcript_cut_runs(&runs, "transcript-delete-matches", window, cx);
        self.invalidate_transcript(cx);
    }

    fn transcript_live_fillers(&self) -> Vec<(usize, Disfluency)> {
        let (Some(doc), Some(edl)) = (&self.transcript.doc, &self.transcript.edl) else {
            return Vec::new();
        };
        doc.fillers
            .iter()
            .enumerate()
            .filter(|(_, filler)| {
                !self
                    .transcript
                    .cleanup
                    .ignored
                    .contains(&filler.words.start)
                    && filler.words.clone().all(|word| edl.is_kept(word))
            })
            .map(|(index, filler)| (index, filler.clone()))
            .collect()
    }

    /// Occurrences nothing has been decided about: still in the video, still
    /// in the captions, and not set aside.
    fn transcript_pending_occurrences(&self) -> Vec<(usize, FillerOccurrence)> {
        let (Some(doc), Some(edl)) = (&self.transcript.doc, &self.transcript.edl) else {
            return Vec::new();
        };
        doc.review
            .iter()
            .enumerate()
            .filter(|(_, occurrence)| {
                !self
                    .transcript
                    .cleanup
                    .ignored
                    .contains(&occurrence.words.start)
                    && occurrence.words.clone().all(|word| edl.is_kept(word))
                    && !doc.is_range_hidden(occurrence.words.clone())
            })
            .map(|(index, occurrence)| (index, occurrence.clone()))
            .collect()
    }

    pub(super) fn transcript_set_hidden(
        &mut self,
        runs: &[Range<usize>],
        hidden: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.transcript.doc.clone() else {
            return;
        };
        let spans: Vec<Range<f64>> = runs
            .iter()
            .flat_map(|run| run.clone())
            .map(|word| doc.words[word].start..doc.words[word].end)
            .collect();
        let recording_durations = self
            .summary()
            .map(|summary| summary.clip_display_durations.clone())
            .unwrap_or_default();
        self.edit_project_global("transcript-hide-captions", window, cx, move |project| {
            let Some(captions) = project.captions.as_mut() else {
                return false;
            };
            if !set_captions_hidden(captions, &spans, hidden) {
                return false;
            }
            let segments = captions.segments.clone();
            if let Some(timeline) = project.timeline.as_mut() {
                timeline.caption_segments = cap_project::derive_caption_track_segments(
                    &segments,
                    timeline,
                    &recording_durations,
                );
            }
            true
        });
        self.transcript.dirty = true;
        self.transcript_refresh();
        self.invalidate_transcript(cx);
    }

    pub(super) fn transcript_apply_suggestions(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pending = self.transcript_pending_occurrences();
        let runs_for = |action: FillerAction| -> Vec<Range<usize>> {
            pending
                .iter()
                .filter(|(_, occurrence)| !occurrence.review && occurrence.suggestion == action)
                .map(|(_, occurrence)| occurrence.words.clone())
                .collect()
        };
        let (hide, cut) = (
            runs_for(FillerAction::HideCaption),
            runs_for(FillerAction::Cut),
        );
        if !hide.is_empty() {
            self.transcript_set_hidden(&hide, true, window, cx);
        }
        if !cut.is_empty() {
            self.transcript.selection = None;
            self.transcript_cut_filler_runs(
                &cut,
                "transcript-apply-filler-suggestions",
                window,
                cx,
            );
        }
        self.invalidate_transcript(cx);
    }

    /// Applies one action to every occurrence of `term`, whatever was done to
    /// each before, so the by-word control can be changed back and forth.
    fn transcript_set_term_action(
        &mut self,
        term: &FillerTerm,
        action: FillerAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.transcript.doc.clone() else {
            return;
        };
        let runs: Vec<Range<usize>> = doc
            .review
            .iter()
            .filter(|occurrence| occurrence.term == *term)
            .map(|occurrence| occurrence.words.clone())
            .collect();
        for run in &runs {
            if action == FillerAction::Keep {
                self.transcript.cleanup.ignored.insert(run.start);
            } else {
                self.transcript.cleanup.ignored.remove(&run.start);
            }
        }
        self.transcript_set_hidden(&runs, action == FillerAction::HideCaption, window, cx);
        if action == FillerAction::Cut {
            self.transcript.selection = None;
            self.transcript_cut_filler_runs(&runs, "transcript-cut-filler-term", window, cx);
        } else {
            let cut: Vec<Range<usize>> = {
                let edl = self.transcript.edl.clone();
                runs.iter()
                    .filter(|run| {
                        edl.as_ref()
                            .is_some_and(|edl| (*run).clone().any(|word| !edl.is_kept(word)))
                    })
                    .cloned()
                    .collect()
            };
            for run in cut {
                self.transcript_restore(run, window, cx);
            }
        }
        self.invalidate_transcript(cx);
    }

    pub(super) fn transcript_remove_fillers(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let categories = self.transcript.cleanup.categories;
        let runs: Vec<Range<usize>> = self
            .transcript_live_fillers()
            .into_iter()
            .filter(|(_, filler)| categories[category_index(filler.kind)])
            .map(|(_, filler)| filler.words)
            .collect();
        self.transcript.selection = None;
        self.transcript_cut_runs(&runs, "transcript-remove-fillers", window, cx);
        self.invalidate_transcript(cx);
    }

    fn transcript_remove_filler(
        &mut self,
        words: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.transcript.selection = None;
        self.transcript_cut_filler_runs(&[words], "transcript-remove-filler", window, cx);
        self.invalidate_transcript(cx);
    }

    fn transcript_focus_filler(
        &mut self,
        words: Range<usize>,
        listen: bool,
        cx: &mut Context<Self>,
    ) {
        let Some((doc, edl)) = self.transcript_doc() else {
            return;
        };
        self.transcript.selection = Some(WordSelection {
            anchor: words.start,
            focus: words.end - 1,
        });
        self.transcript_reveal(words.start, false);
        if listen
            && let (Some(start), Some(end)) = (
                edl.output_start_of(&doc.voiced[words.start]),
                edl.output_end_of(&doc.voiced[words.end - 1]),
            )
        {
            self.play_range((start - LISTEN_BEFORE).max(0.0), end + LISTEN_AFTER, cx);
        } else {
            self.transcript_seek_word(words.start, cx);
        }
        self.invalidate_transcript(cx);
    }

    pub(super) fn transcript_pause_plan(&self) -> Vec<cuts::PauseCut> {
        let (Some(doc), Some(edl)) = (&self.transcript.doc, &self.transcript.edl) else {
            return Vec::new();
        };
        cuts::plan_pause_cuts(
            doc,
            edl,
            self.transcript.energy.as_deref(),
            self.transcript.cleanup.pause_longer_than,
            self.transcript.cleanup.pause_keep,
        )
    }

    pub(super) fn transcript_shorten_pauses(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cuts: Vec<Range<f64>> = self
            .transcript_pause_plan()
            .into_iter()
            .flat_map(|pause| pause.cuts)
            .collect();
        self.transcript_apply_cuts(&cuts, "transcript-shorten-pauses", window, cx);
        self.invalidate_transcript(cx);
    }

    fn transcript_step_pause(&mut self, longer: bool, step: f64, cx: &mut Context<Self>) {
        let cleanup = &mut self.transcript.cleanup;
        let (value, (min, max)) = if longer {
            (&mut cleanup.pause_longer_than, PAUSE_LONGER_RANGE)
        } else {
            (&mut cleanup.pause_keep, PAUSE_KEEP_RANGE)
        };
        *value = ((*value + step) * 10.0).round() / 10.0;
        *value = value.clamp(min, max);
        if cleanup.pause_keep >= cleanup.pause_longer_than {
            if longer {
                cleanup.pause_keep = (cleanup.pause_longer_than - 0.1).max(PAUSE_KEEP_RANGE.0);
            } else {
                cleanup.pause_longer_than = (cleanup.pause_keep + 0.1).min(PAUSE_LONGER_RANGE.1);
            }
        }
        cleanup.persist_pauses();
        self.invalidate_transcript(cx);
    }

    pub(crate) fn transcript_resize_move(
        &mut self,
        x: f32,
        row_width: f32,
        cx: &mut Context<Self>,
    ) {
        let Some((start_x, start_share)) = self.transcript.resize else {
            return;
        };
        if row_width <= 0.0 {
            return;
        }
        let share =
            (start_share + (x - start_x) / row_width).clamp(MIN_PLAYER_SHARE, MAX_PLAYER_SHARE);
        if (share - self.transcript.player_share).abs() > f32::EPSILON {
            self.transcript.player_share = share;
            cx.notify();
        }
    }

    pub(crate) fn transcript_resize_end(&mut self, cx: &mut Context<Self>) {
        if self.transcript.resize.take().is_some() {
            self.transcript.persist_split();
            cx.notify();
        }
    }

    pub(crate) fn render_transcript(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let project_changed = self.transcript.dirty;
        self.transcript_refresh();
        if project_changed {
            self.transcript_load_energy(cx);
        }
        self.transcript.layouts.borrow_mut().clear();
        let theme = self.theme;
        let palette = Palette::new(&theme);
        let editor = cx.entity().downgrade();

        let body: AnyElement = match (self.transcript.doc.clone(), self.transcript.edl.clone()) {
            (Some(doc), edl) if !doc.is_empty() => {
                let search = self.transcript.search.as_ref();
                let painter = ParagraphPainter {
                    doc,
                    edl,
                    selection: self.transcript.selection.map(WordSelection::range),
                    active: self.transcript.active,
                    hovered_cut: self.transcript.hovered_cut.and_then(|run| {
                        self.transcript
                            .edl
                            .as_ref()
                            .and_then(|edl| edl.cut_runs.get(run).cloned())
                    }),
                    matches: search
                        .map_or_else(|| Arc::new(Vec::new()), |search| search.matches.clone()),
                    current_match: search
                        .and_then(|search| search.matches.get(search.current).cloned()),
                    ignored: Arc::new(self.transcript.cleanup.ignored.clone()),
                    palette,
                    editor: editor.clone(),
                    layouts: self.transcript.layouts.clone(),
                };
                let scroll_editor = editor.clone();
                self.transcript.list.set_scroll_handler(move |_, _, cx| {
                    scroll_editor
                        .update(cx, |this, _| {
                            this.transcript.user_scrolled = Some(Instant::now())
                        })
                        .ok();
                });
                div()
                    .id("transcript-body")
                    .flex_1()
                    .min_h_0()
                    .px(px(14.))
                    .pt(px(10.))
                    .text_size(px(TEXT_SIZE))
                    .line_height(px(LINE_HEIGHT))
                    .text_color(palette.text_1)
                    .child(
                        gpui::list(self.transcript.list.clone(), move |index, _, _| {
                            painter.paragraph(index)
                        })
                        .size_full(),
                    )
                    .into_any_element()
            }
            _ => self.render_transcript_empty(cx),
        };

        let focused = self.transcript.focus.is_focused(window);
        div()
            .id("transcript-panel")
            .size_full()
            .flex()
            .flex_col()
            .rounded(px(12.))
            .border_1()
            .border_color(if focused {
                Hsla::from(theme.editor.line_strong)
            } else {
                self.card_line()
            })
            .bg(self.panel_bg())
            .shadow(theme.editor.card_shadow())
            .overflow_hidden()
            .track_focus(&self.transcript.focus)
            .on_key_down(cx.listener(Self::transcript_key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    window.focus(&this.transcript.focus, cx);
                    if this.transcript.selection.take().is_some() {
                        this.invalidate_transcript(cx);
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.transcript_end_drag(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.transcript_end_drag(cx)),
            )
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered && this.transcript.hovered_cut.take().is_some() {
                    this.invalidate_transcript(cx);
                }
            }))
            .child(self.render_transcript_header(cx))
            .children(self.render_transcript_search(cx))
            .children(self.render_transcript_cleanup(cx))
            .child(body)
            .child(self.render_transcript_footer())
            .into_any_element()
    }

    fn render_transcript_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let words = self
            .transcript
            .edl
            .as_ref()
            .map(|edl| edl.kept_count)
            .or_else(|| self.transcript.doc.as_ref().map(|doc| doc.words.len()));
        let has_doc = self
            .transcript
            .doc
            .as_ref()
            .is_some_and(|doc| !doc.is_empty());
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .flex_none()
            .h(px(44.))
            .px(px(12.))
            .border_b_1()
            .border_color(Hsla::from(theme.editor.line))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_baseline()
                    .gap(px(8.))
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(Hsla::from(theme.editor.text_1))
                            .child(tr::editor::transcript::title()),
                    )
                    .children(words.filter(|_| has_doc).map(|count| {
                        div()
                            .text_size(px(12.))
                            .text_color(Hsla::from(theme.editor.text_3))
                            .truncate()
                            .child(SharedString::from(tr::editor::transcript::words(count)))
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(2.))
                    .child(
                        ui::EditorButton::plain(&theme, "transcript-search")
                            .left_icon("icons/search.svg")
                            .tooltip(&theme, tr::editor::transcript::search())
                            .disabled(!has_doc)
                            .pressed(self.transcript.search.is_some())
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.transcript.search.is_some() {
                                    this.transcript_close_search(window, cx);
                                } else {
                                    this.transcript_open_search(window, cx);
                                }
                            })),
                    )
                    .child(
                        ui::EditorButton::plain(&theme, "transcript-cleanup")
                            .left_icon("icons/scissors.svg")
                            .label(tr::editor::transcript::cleanup())
                            .tooltip(&theme, tr::editor::transcript::cleanup_tooltip())
                            .disabled(!has_doc)
                            .pressed(self.transcript.cleanup.open)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.transcript.cleanup.open = !this.transcript.cleanup.open;
                                this.invalidate_transcript(cx);
                            })),
                    ),
            )
    }

    fn render_transcript_search(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let search = self.transcript.search.as_ref()?;
        let theme = self.theme;
        let count = search.matches.len();
        let label = if search.query.trim().is_empty() {
            SharedString::default()
        } else if count == 0 {
            SharedString::from(tr::editor::transcript::search_none())
        } else {
            SharedString::from(tr::editor::transcript::search_count(
                search.current + 1,
                count,
            ))
        };
        let replaced = search
            .replaced
            .map(|count| SharedString::from(tr::editor::transcript::replaced(count)));
        let find_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .child(
                div().flex_1().min_w_0().child(
                    ui::TextInput::plain(&theme, "transcript-search-input", &search.input)
                        .height(px(28.))
                        .text_size(px(13.)),
                ),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(12.))
                    .text_color(Hsla::from(theme.editor.text_3))
                    .child(label),
            )
            .child(
                ui::EditorButton::plain(&theme, "transcript-search-previous")
                    .left_icon("icons/chevron-up.svg")
                    .tooltip(&theme, tr::editor::transcript::previous_match())
                    .disabled(count == 0)
                    .on_click(cx.listener(|this, _, _, cx| this.transcript_step_match(-1, cx))),
            )
            .child(
                ui::EditorButton::plain(&theme, "transcript-search-next")
                    .left_icon("icons/chevron-down.svg")
                    .tooltip(&theme, tr::editor::transcript::next_match())
                    .disabled(count == 0)
                    .on_click(cx.listener(|this, _, _, cx| this.transcript_step_match(1, cx))),
            )
            .child(
                ui::EditorButton::plain(&theme, "transcript-search-delete")
                    .left_icon("icons/trash.svg")
                    .label(tr::editor::transcript::delete_matches())
                    .tooltip(&theme, tr::editor::transcript::delete_matches_tooltip())
                    .disabled(count == 0)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.transcript_delete_matches(window, cx);
                    })),
            );
        let replace_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .child(
                div().flex_1().min_w_0().child(
                    ui::TextInput::plain(&theme, "transcript-replace-input", &search.replace)
                        .height(px(28.))
                        .text_size(px(13.)),
                ),
            )
            .children(replaced.map(|label| {
                div()
                    .flex_none()
                    .text_size(px(12.))
                    .text_color(Hsla::from(theme.editor.text_3))
                    .child(label)
            }))
            .child(
                ui::EditorButton::plain(&theme, "transcript-replace-one")
                    .label(tr::editor::transcript::replace())
                    .disabled(count == 0)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.transcript_replace(false, window, cx);
                    })),
            )
            .child(
                ui::EditorButton::plain(&theme, "transcript-replace-all")
                    .label(tr::editor::transcript::replace_all())
                    .disabled(count == 0)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.transcript_replace(true, window, cx);
                    })),
            );
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .flex_none()
                .px(px(12.))
                .py(px(8.))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .border_b_1()
                .border_color(Hsla::from(theme.editor.line))
                .child(find_row)
                .child(replace_row)
                .into_any_element(),
        )
    }

    fn render_transcript_cleanup(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.transcript.cleanup.open {
            return None;
        }
        let doc = self.transcript.doc.clone()?;
        let edl = self.transcript.edl.clone()?;
        let theme = self.theme;
        let palette = Palette::new(&theme);
        let cleanup = &self.transcript.cleanup;
        let pending = self.transcript_pending_occurrences();
        let suggested = |action: FillerAction| {
            pending
                .iter()
                .filter(|(_, occurrence)| !occurrence.review && occurrence.suggestion == action)
                .count()
        };
        let actionable = suggested(FillerAction::HideCaption) + suggested(FillerAction::Cut);
        let pauses = self.transcript_pause_plan();
        let saved: f64 = pauses.iter().map(|pause| pause.saves).sum();
        let on_pauses = cleanup.tab == CleanupTab::Pauses;

        let tabs = ui::SegmentedControl::editor(
            &theme,
            "transcript-cleanup-tabs",
            vec![
                ui::SegmentOption::new(tr::editor::transcript::fillers::title(), !on_pauses),
                ui::SegmentOption::new(tr::editor::transcript::pauses::title(), on_pauses),
            ],
        )
        .on_select(cx.listener(|this, index: &usize, _, cx| {
            this.transcript.cleanup.tab = if *index == 1 {
                CleanupTab::Pauses
            } else {
                CleanupTab::Fillers
            };
            this.invalidate_transcript(cx);
        }));

        let action = if on_pauses {
            ui::Button::plain(
                &theme,
                "transcript-shorten-pauses",
                ui::ButtonVariant::Primary,
                ui::ButtonSize::Sm,
            )
            .label(tr::editor::transcript::pauses::apply())
            .disabled(pauses.is_empty())
            .on_click(cx.listener(|this, _, window, cx| {
                this.transcript_shorten_pauses(window, cx);
            }))
        } else {
            ui::Button::plain(
                &theme,
                "transcript-apply-filler-suggestions",
                ui::ButtonVariant::Primary,
                ui::ButtonSize::Sm,
            )
            .label(tr::editor::transcript::fillers::apply_suggestions())
            .disabled(actionable == 0)
            .on_click(cx.listener(|this, _, window, cx| {
                this.transcript_apply_suggestions(window, cx);
            }))
        };

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .flex_none()
            .gap(px(8.))
            .px(px(12.))
            .py(px(8.))
            .child(tabs)
            .child(action);

        let body = if on_pauses {
            self.render_transcript_pause_list(&doc, pauses, saved, palette, cx)
        } else {
            self.render_transcript_filler_list(&doc, &edl, pending, palette, cx)
        };

        Some(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .max_h(gpui::relative(0.6))
                .border_b_1()
                .border_color(Hsla::from(theme.editor.line))
                .bg(Hsla::from(theme.editor.card_2))
                .child(header)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .px(px(6.))
                        .pb(px(8.))
                        .child(body),
                )
                .into_any_element(),
        )
    }

    fn render_transcript_filler_list(
        &self,
        doc: &Arc<TranscriptDoc>,
        edl: &Arc<TranscriptEdl>,
        pending: Vec<(usize, FillerOccurrence)>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let cleanup = &self.transcript.cleanup;
        let count_of = |action: FillerAction, in_list: &[(usize, FillerOccurrence)]| {
            in_list
                .iter()
                .filter(|(_, occurrence)| !occurrence.review && occurrence.suggestion == action)
                .count()
        };
        let (cut, hide, keep) = (
            count_of(FillerAction::Cut, &pending),
            count_of(FillerAction::HideCaption, &pending),
            count_of(FillerAction::Keep, &pending),
        );
        let cut_runs: Vec<Range<usize>> = pending
            .iter()
            .filter(|(_, occurrence)| {
                !occurrence.review && occurrence.suggestion == FillerAction::Cut
            })
            .map(|(_, occurrence)| occurrence.words.clone())
            .collect();
        let saved: f64 = cuts::plan_filler_cuts(
            doc,
            self.transcript.energy.as_deref(),
            Some(edl.kept.as_slice()),
            &cut_runs,
        )
        .iter()
        .map(|cut| cut.end - cut.start)
        .sum();
        let count_review = pending.iter().filter(|(_, o)| o.review).count();
        let summary = div()
            .flex_none()
            .px(px(6.))
            .pb(px(6.))
            .text_size(px(12.))
            .text_color(palette.text_2)
            .child(SharedString::from(if pending.is_empty() {
                tr::editor::transcript::fillers::none().to_string()
            } else if cut + hide == 0 {
                tr::editor::transcript::fillers::needs_review(count_review).to_string()
            } else if cut > 0 {
                format!(
                    "{} · {}",
                    tr::editor::transcript::fillers::suggestion_summary(cut, hide, keep),
                    tr::editor::transcript::fillers::shorter_by(crate::i18n::decimal(format!(
                        "{saved:.1}"
                    ))),
                )
            } else {
                tr::editor::transcript::fillers::suggestion_summary(cut, hide, keep).to_string()
            }));

        let listed: Vec<(usize, FillerOccurrence)> = pending
            .iter()
            .filter(|(_, occurrence)| occurrence.review)
            .cloned()
            .collect();
        let count = listed.len();
        let rows = {
            let doc = doc.clone();
            let edl = edl.clone();
            let listed = Arc::new(listed);
            let selection = self.transcript.selection.map(WordSelection::range);
            let editor = cx.entity().downgrade();
            gpui::uniform_list("transcript-filler-rows", count, move |range, _, _| {
                range
                    .filter_map(|row| listed.get(row))
                    .map(|(index, occurrence)| {
                        filler_row(
                            &theme,
                            palette,
                            &doc,
                            &edl,
                            *index,
                            occurrence.words.clone(),
                            selection.as_ref() == Some(&occurrence.words),
                            editor.clone(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .track_scroll(&cleanup.filler_rows)
            .min_h(px(ROW_HEIGHT * count.min(2) as f32))
            .h(px(ROW_HEIGHT * count.min(MAX_VISIBLE_ROWS) as f32))
        };

        let mut terms: Vec<FillerTerm> = doc
            .review
            .iter()
            .map(|occurrence| occurrence.term.clone())
            .collect();
        terms.sort();
        terms.dedup();
        let term_rows = cleanup.by_term_open.then(|| {
            terms
                .into_iter()
                .enumerate()
                .map(|(row, term)| self.render_filler_term_row(doc, edl, row, term, palette, cx))
                .collect::<Vec<_>>()
        });
        let by_term_open = cleanup.by_term_open;

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(summary)
            .when(count > 0 && !by_term_open, |this| {
                this.when(cut + hide > 0, |this| {
                    this.child(
                        div()
                            .flex_none()
                            .px(px(6.))
                            .pb(px(2.))
                            .text_size(px(12.))
                            .text_color(palette.text_3)
                            .child(SharedString::from(
                                tr::editor::transcript::fillers::needs_review(count),
                            )),
                    )
                })
                .child(rows)
            })
            .when(!doc.review.is_empty(), |this| {
                this.child(
                    div()
                        .id("transcript-filler-by-term")
                        .flex()
                        .flex_row()
                        .flex_none()
                        .items_center()
                        .gap(px(4.))
                        .mt(px(4.))
                        .h(px(26.))
                        .px(px(6.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .text_size(px(12.))
                        .text_color(palette.text_2)
                        .hover(|style| style.bg(Hsla::from(theme.editor.ctl)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.transcript.cleanup.by_term_open =
                                !this.transcript.cleanup.by_term_open;
                            this.invalidate_transcript(cx);
                        }))
                        .child(tr::editor::transcript::fillers::by_term())
                        .child(if by_term_open { "⌃" } else { "›" }),
                )
            })
            .when_some(term_rows, |this, rows| {
                this.child(
                    div()
                        .id("transcript-filler-terms")
                        .flex()
                        .flex_col()
                        .flex_none()
                        .max_h(px(ROW_HEIGHT * MAX_VISIBLE_ROWS as f32))
                        .overflow_y_scroll()
                        .children(rows),
                )
            })
            .into_any_element()
    }

    fn render_filler_term_row(
        &self,
        doc: &Arc<TranscriptDoc>,
        edl: &Arc<TranscriptEdl>,
        row: usize,
        term: FillerTerm,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let ignored = &self.transcript.cleanup.ignored;
        let occurrences: Vec<&FillerOccurrence> = doc
            .review
            .iter()
            .filter(|occurrence| occurrence.term == term)
            .collect();
        let suggested = |action: FillerAction| {
            occurrences
                .iter()
                .filter(|occurrence| occurrence.suggestion == action)
                .count()
        };
        let all = |test: &dyn Fn(&FillerOccurrence) -> bool| {
            occurrences.iter().all(|occurrence| test(occurrence))
        };
        let all_cut = all(&|occurrence| occurrence.words.clone().all(|word| !edl.is_kept(word)));
        let all_hidden = all(&|occurrence| doc.is_range_hidden(occurrence.words.clone()));
        let all_kept = all(&|occurrence| ignored.contains(&occurrence.words.start));
        let label: SharedString = match &term {
            FillerTerm::Word(word) => word.clone().into(),
            FillerTerm::Kind(DisfluencyKindOrder::Repetition) => {
                kind_label(DisfluencyKind::Repetition).into()
            }
            FillerTerm::Kind(DisfluencyKindOrder::FalseStart) => {
                kind_label(DisfluencyKind::FalseStart).into()
            }
            FillerTerm::Kind(DisfluencyKindOrder::Discourse) => {
                kind_label(DisfluencyKind::Discourse).into()
            }
        };
        let control = ui::SegmentedControl::editor(
            &theme,
            ("transcript-filler-term", row),
            vec![
                ui::SegmentOption::new(tr::editor::transcript::fillers::action_keep(), all_kept),
                ui::SegmentOption::new(tr::editor::transcript::fillers::action_hide(), all_hidden),
                ui::SegmentOption::new(tr::editor::transcript::fillers::action_cut(), all_cut),
            ],
        )
        .on_select(cx.listener(move |this, index: &usize, window, cx| {
            let action = match index {
                0 => FillerAction::Keep,
                1 => FillerAction::HideCaption,
                _ => FillerAction::Cut,
            };
            this.transcript_set_term_action(&term, action, window, cx);
        }));
        div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .justify_between()
            .gap(px(8.))
            .px(px(6.))
            .py(px(3.))
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().text_size(px(13.)).text_color(palette.text_1).child(
                        SharedString::from(format!(
                            "{label} · {}",
                            tr::editor::transcript::fillers::occurrences(occurrences.len())
                        )),
                    ))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(palette.text_3)
                            .truncate()
                            .child(SharedString::from(
                                tr::editor::transcript::fillers::term_suggestion(
                                    suggested(FillerAction::Cut),
                                    suggested(FillerAction::HideCaption),
                                    suggested(FillerAction::Keep),
                                ),
                            )),
                    ),
            )
            .child(div().flex_none().child(control))
            .into_any_element()
    }

    fn render_transcript_pause_list(
        &self,
        doc: &Arc<TranscriptDoc>,
        pauses: Vec<cuts::PauseCut>,
        saved: f64,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let cleanup = &self.transcript.cleanup;
        let control = |label: &'static str, id: &'static str, value: f64, longer: bool| {
            div()
                .flex()
                .flex_row()
                .flex_none()
                .items_center()
                .justify_between()
                .h(px(30.))
                .px(px(6.))
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(palette.text_1)
                        .child(label),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(2.))
                        .child(
                            ui::EditorButton::plain(&theme, (id, 0usize))
                                .left_icon("icons/minus.svg")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.transcript_step_pause(longer, -0.1, cx);
                                })),
                        )
                        .child(
                            div()
                                .w(px(46.))
                                .text_size(px(13.))
                                .text_color(palette.text_1)
                                .flex()
                                .justify_center()
                                .child(seconds_label(value)),
                        )
                        .child(
                            ui::EditorButton::plain(&theme, (id, 1usize))
                                .left_icon("icons/plus.svg")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.transcript_step_pause(longer, 0.1, cx);
                                })),
                        ),
                )
        };
        let longer = control(
            tr::editor::transcript::pauses::longer_than(),
            "transcript-pause-longer",
            cleanup.pause_longer_than,
            true,
        );
        let shorter = control(
            tr::editor::transcript::pauses::keep(),
            "transcript-pause-keep",
            cleanup.pause_keep,
            false,
        );
        let summary = if pauses.is_empty() {
            SharedString::from(tr::editor::transcript::pauses::none())
        } else {
            SharedString::from(tr::editor::transcript::pauses::summary(
                pauses.len(),
                crate::i18n::decimal(format!("{saved:.1}")),
            ))
        };
        let count = pauses.len();
        let rows = {
            let doc = doc.clone();
            let pauses = Arc::new(pauses);
            let keep = cleanup.pause_keep;
            let editor = cx.entity().downgrade();
            gpui::uniform_list("transcript-pause-rows", count, move |range, _, _| {
                range
                    .filter_map(|row| pauses.get(row))
                    .map(|pause| pause_row(&theme, palette, &doc, pause, keep, editor.clone()))
                    .collect::<Vec<_>>()
            })
            .track_scroll(&cleanup.pause_rows)
            .min_h(px(ROW_HEIGHT * count.min(2) as f32))
            .h(px(ROW_HEIGHT * count.min(MAX_VISIBLE_ROWS) as f32))
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(longer)
            .child(shorter)
            .child(
                div()
                    .flex_none()
                    .px(px(6.))
                    .py(px(6.))
                    .text_size(px(12.))
                    .text_color(palette.text_3)
                    .child(summary),
            )
            .child(
                div()
                    .flex_none()
                    .h(px(1.))
                    .mx(px(6.))
                    .mb(px(4.))
                    .bg(Hsla::from(theme.editor.line)),
            )
            .child(rows)
            .into_any_element()
    }

    fn render_transcript_footer(&self) -> impl IntoElement {
        let theme = self.theme;
        let doc = self.transcript.doc.as_ref();
        let edl = self.transcript.edl.as_ref();
        let message: SharedString = match (doc, edl) {
            (Some(doc), Some(edl)) if !doc.is_empty() => {
                if let Some(run) = self
                    .transcript
                    .hovered_cut
                    .and_then(|run| edl.cut_runs.get(run))
                {
                    tr::editor::transcript::restore_hint(run.len()).into()
                } else if let Some(selection) = self.transcript.selection {
                    let range = selection.range();
                    let kept = range.clone().filter(|word| edl.is_kept(*word)).count();
                    if kept == 0 {
                        tr::editor::transcript::selection_cut(range.len()).into()
                    } else {
                        let mut kept_words = range.clone().filter(|word| edl.is_kept(*word));
                        let first = kept_words.next().unwrap_or(range.start);
                        let last = kept_words.next_back().unwrap_or(first);
                        let seconds = edl
                            .output_end_of(&doc.voiced[last])
                            .zip(edl.output_start_of(&doc.voiced[first]))
                            .map_or(0.0, |(end, start)| (end - start).max(0.0));
                        tr::editor::transcript::selection(
                            kept,
                            crate::i18n::decimal(format!("{seconds:.1}")),
                        )
                        .into()
                    }
                } else {
                    tr::editor::transcript::hint().into()
                }
            }
            _ => SharedString::default(),
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .flex_none()
            .h(px(32.))
            .px(px(12.))
            .border_t_1()
            .border_color(Hsla::from(theme.editor.line))
            .text_size(px(12.))
            .text_color(Hsla::from(theme.editor.text_3))
            .child(div().truncate().child(message))
    }

    fn render_transcript_empty(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        self.ensure_captions_init(cx);
        let snapshot = crate::transcription::ui_snapshot(&self.project_path);
        let model = self.selected_caption_model();
        let downloaded = snapshot.downloaded.contains(model.name);
        let download = snapshot
            .download
            .as_ref()
            .filter(|download| download.state == crate::transcription::DownloadState::Downloading);
        let has_audio = self
            .summary()
            .is_some_and(|summary| summary.has_microphone || summary.has_system_audio);
        let generating = snapshot.generating;

        let action: AnyElement = if !has_audio {
            div()
                .text_size(px(12.))
                .text_color(Hsla::from(theme.editor.text_3))
                .child(tr::editor::transcript::no_audio())
                .into_any_element()
        } else if downloaded {
            ui::Button::plain(
                &theme,
                "transcript-transcribe",
                ui::ButtonVariant::Primary,
                ui::ButtonSize::Md,
            )
            .label(if generating {
                tr::editor::transcript::transcribing()
            } else {
                tr::editor::transcript::transcribe()
            })
            .disabled(generating || snapshot.deleting.is_some())
            .on_click(cx.listener(|this, _, window, cx| {
                this.generate_transcript(false, window, cx);
                this.spawn_transcript_generation_poll(cx);
            }))
            .into_any_element()
        } else {
            let percent = download.map_or(0, |download| {
                download.progress.clamp(0.0, 100.0).round() as u32
            });
            ui::Button::plain(
                &theme,
                "transcript-download-model",
                ui::ButtonVariant::Primary,
                ui::ButtonSize::Md,
            )
            .when(download.is_none(), |button| {
                button.icon("icons/download.svg")
            })
            .label(if download.is_some() {
                tr::editor::tabs::captions::downloading((model.label)(), percent)
            } else {
                tr::editor::tabs::captions::download((model.label)())
            })
            .disabled(download.is_some())
            .on_click(cx.listener(|this, _, _, cx| {
                this.start_caption_model_download(cx);
            }))
            .into_any_element()
        };

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(10.))
            .px(px(32.))
            .child(
                svg()
                    .path("icons/scan-text.svg")
                    .size(px(28.))
                    .text_color(Hsla::from(theme.editor.text_3)),
            )
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(Hsla::from(theme.editor.text_1))
                    .child(tr::editor::transcript::empty_title()),
            )
            .child(
                div()
                    .max_w(px(300.))
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_center()
                    .text_color(Hsla::from(theme.editor.text_2))
                    .child(tr::editor::transcript::empty_description()),
            )
            .child(div().pt(px(6.)).child(action))
            .children(snapshot.generation_error.map(|error| {
                div()
                    .max_w(px(300.))
                    .text_size(px(12.))
                    .text_center()
                    .text_color(Hsla::from(theme.red_9))
                    .child(error)
            }))
            .into_any_element()
    }

    fn transcript_energy_offsets(&self) -> Option<Vec<(f64, f64)>> {
        let instance = self.instance.as_ref()?;
        Some(
            instance
                .segment_medias
                .iter()
                .enumerate()
                .map(|(index, media)| {
                    let clip = self
                        .project
                        .clips
                        .iter()
                        .find(|clip| clip.index == index as u32)
                        .map(|clip| clip.offsets)
                        .unwrap_or_default();
                    let repair = media.audio_timing_repair;
                    (
                        f64::from(clip.mic + repair.mic_offset_secs),
                        f64::from(clip.system_audio + repair.system_audio_offset_secs),
                    )
                })
                .collect(),
        )
    }

    pub(super) fn transcript_energy_loaded(
        &mut self,
        loaded_with: Vec<(f64, f64)>,
        current: Option<Vec<(f64, f64)>>,
        envelope: super::energy::EnergyEnvelope,
        cx: &mut Context<Self>,
    ) -> bool {
        self.transcript.energy_loading = false;
        if current.as_ref() != Some(&loaded_with) {
            self.transcript.energy = None;
            self.transcript.energy_offsets.clear();
            self.transcript.dirty = true;
            self.transcript_load_energy(cx);
            self.invalidate_transcript(cx);
            return false;
        }
        if !envelope.is_empty() {
            self.transcript.energy = Some(Arc::new(envelope));
            self.transcript.dirty = true;
        }
        self.invalidate_transcript(cx);
        true
    }

    fn transcript_load_energy(&mut self, cx: &mut Context<Self>) {
        if self.transcript.energy_loading {
            return;
        }
        let Some(instance) = self.instance.clone() else {
            return;
        };
        let durations = self.clip_display_durations.clone();
        let Some(offsets) = self.transcript_energy_offsets() else {
            return;
        };
        if self.transcript.energy.is_some() && self.transcript.energy_offsets == offsets {
            return;
        }
        self.transcript.energy_offsets = offsets.clone();
        self.transcript.energy_loading = true;
        let loaded_with = offsets.clone();
        cx.spawn(async move |this, cx| {
            let task = cx.update(|cx| {
                gpui_tokio::Tokio::spawn(cx, async move {
                    let mut tracks = Vec::with_capacity(instance.segment_medias.len());
                    for media in instance.segment_medias.iter() {
                        tracks.push((
                            media.audio.get().await.ok().flatten(),
                            media.system_audio.get().await.ok().flatten(),
                        ));
                    }
                    tracks
                })
            });
            let tracks = task.await.unwrap_or_default();
            let started = Instant::now();
            let envelope = cx
                .background_executor()
                .spawn(async move {
                    let clips = tracks
                        .iter()
                        .zip(&offsets)
                        .zip(&durations)
                        .map(|(((mic, system), (mic_offset, system_offset)), duration)| {
                            let tracks: Vec<super::energy::EnvelopeTrack> = [
                                mic.as_ref().map(|audio| (audio, *mic_offset)),
                                system.as_ref().map(|audio| (audio, *system_offset)),
                            ]
                            .into_iter()
                            .flatten()
                            .map(|(audio, offset_seconds)| super::energy::EnvelopeTrack {
                                slices: audio.sample_slices().collect(),
                                channels: audio.channels(),
                                offset_seconds,
                            })
                            .collect();
                            super::energy::clip_envelope(&tracks, *duration)
                        })
                        .collect();
                    super::energy::EnergyEnvelope::new(clips, &durations)
                })
                .await;
            tracing::info!(
                elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
                silences = envelope.silences().len(),
                silence_threshold_db = envelope.silence_threshold_db(0.0),
                "transcript loudness envelope ready"
            );
            this.update(cx, |this, cx| {
                let current = this.transcript_energy_offsets();
                this.transcript_energy_loaded(loaded_with, current, envelope, cx);
            })
            .ok();
        })
        .detach();
    }

    fn spawn_transcript_generation_poll(&mut self, cx: &mut Context<Self>) {
        let path = self.project_path.clone();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(250))
                    .await;
                let generating = crate::transcription::ui_snapshot(&path).generating;
                if this
                    .update(cx, |this, cx| this.invalidate_transcript(cx))
                    .is_err()
                    || !generating
                {
                    break;
                }
            }
        })
        .detach();
    }
}
