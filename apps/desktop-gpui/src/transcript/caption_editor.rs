use std::{cell::RefCell, collections::HashMap, ops::Range, rc::Rc, sync::Arc};

use cap_project::{CaptionSegment, ProjectConfiguration};
use gpui::{
    AnyElement, AppContext, Context, Entity, Hsla, InteractiveElement, IntoElement, ListAlignment,
    ListState, MouseButton, ParentElement, Pixels, Point, SharedString, Styled, Subscription,
    WeakEntity, Window, deferred, div, prelude::FluentBuilder, px,
};

use crate::{editor_window::EditorWindow, i18n::tr, theme::Theme, ui};

struct Row {
    id: SharedString,
    text: SharedString,
    time: f64,
    output_time: Option<f64>,
}

#[derive(Clone)]
struct Match {
    row: usize,
    bytes: Range<usize>,
}

struct RowInput {
    input: Entity<ui::TextInputState>,
    _subscription: Subscription,
}

#[derive(Clone)]
struct DeleteMenu {
    id: SharedString,
    input: WeakEntity<ui::TextInputState>,
    origin: Point<Pixels>,
}

pub(super) struct CaptionEditor {
    rows: Arc<Vec<Row>>,
    inputs: Rc<RefCell<HashMap<SharedString, RowInput>>>,
    list: ListState,
    matches: Vec<Match>,
    menu: Option<DeleteMenu>,
}

impl Default for CaptionEditor {
    fn default() -> Self {
        Self {
            rows: Arc::default(),
            inputs: Rc::default(),
            list: ListState::new(0, ListAlignment::Top, px(160.)).with_uniform_item_height(px(42.)),
            matches: Vec::new(),
            menu: None,
        }
    }
}

fn apply_caption_text(
    segment: &mut CaptionSegment,
    text: String,
    selected: Option<Range<usize>>,
) -> bool {
    if segment.text == text {
        return false;
    }
    let (common, tail) = selected
        .filter(|range| {
            segment
                .text
                .get(..range.start)
                .is_some_and(|prefix| text.starts_with(prefix))
                && segment
                    .text
                    .get(range.end..)
                    .is_some_and(|suffix| text.ends_with(suffix))
        })
        .map_or_else(
            || {
                let common = segment
                    .text
                    .chars()
                    .zip(text.chars())
                    .take_while(|(a, b)| a == b)
                    .map(|(ch, _)| ch.len_utf8())
                    .sum::<usize>();
                let tail = segment.text[common..]
                    .chars()
                    .rev()
                    .zip(text[common..].chars().rev())
                    .take_while(|(a, b)| a == b)
                    .map(|(ch, _)| ch.len_utf8())
                    .sum::<usize>();
                (common, tail)
            },
            |range| (range.start, segment.text.len() - range.end),
        );
    if text.trim().is_empty() {
        segment.words.clear();
    } else if !segment.words.is_empty() {
        let mut from = 0;
        let spans = segment
            .words
            .iter()
            .map(|word| {
                let word = word.text.trim();
                let start = from + segment.text[from..].find(word)?;
                from = start + word.len();
                Some(start..from)
            })
            .collect::<Option<Vec<_>>>();
        let preserved = spans.and_then(|spans| {
            let mut prefix = spans.partition_point(|span| span.end <= common);
            let mut suffix = spans.partition_point(|span| span.start < segment.text.len() - tail);
            if prefix == suffix && prefix > 0 && common == segment.text.len() - tail {
                prefix -= 1;
            }
            if prefix == suffix && suffix < spans.len() {
                suffix += 1;
            }
            if prefix > suffix {
                return None;
            }
            let start_byte = prefix.checked_sub(1).map_or(0, |index| spans[index].end);
            let end_byte = spans.get(suffix).map_or(text.len(), |span| {
                text.len() - (segment.text.len() - span.start)
            });
            let middle = text.get(start_byte..end_byte)?.trim();
            let start = segment
                .words
                .get(prefix)
                .map_or(segment.start, |word| word.start);
            let end = suffix
                .checked_sub(1)
                .and_then(|index| segment.words.get(index))
                .map_or(segment.end, |word| word.end);
            let mut words = segment.words[..prefix].to_vec();
            words.extend(crate::transcription::sync_caption_words_with_text(
                middle,
                &segment.words[prefix..suffix],
                f64::from(start),
                f64::from(end),
            ));
            words.extend_from_slice(&segment.words[suffix..]);
            (cap_project::caption_text_from_words(&words) == text.trim()).then_some(words)
        });
        segment.words = preserved.unwrap_or_else(|| {
            crate::transcription::sync_caption_words_with_text(
                &text,
                &segment.words,
                f64::from(segment.start),
                f64::from(segment.end),
            )
        });
    }
    segment.text = text;
    true
}

impl CaptionEditor {
    fn refresh(&mut self, project: &ProjectConfiguration) {
        let mut output_times = HashMap::new();
        if let Some(timeline) = &project.timeline {
            for segment in &timeline.caption_segments {
                output_times
                    .entry(cap_project::source_caption_id(&segment.id))
                    .or_insert(segment.start);
            }
        }
        let mut rows: Vec<_> = project
            .captions
            .iter()
            .flat_map(|captions| &captions.segments)
            .map(|segment| Row {
                id: segment.id.clone().into(),
                text: segment.text.clone().into(),
                time: f64::from(segment.start),
                output_time: output_times.get(segment.id.as_str()).copied(),
            })
            .collect();
        rows.sort_by(|a, b| a.time.total_cmp(&b.time));
        if self.list.item_count() != rows.len() {
            self.list.reset(rows.len());
        }
        self.rows = Arc::new(rows);
    }

    fn find(&mut self, query: &str) {
        let query = query.trim();
        self.matches = self
            .rows
            .iter()
            .enumerate()
            .flat_map(|(row, caption)| {
                crate::caption_replace::occurrences(&caption.text, query)
                    .into_iter()
                    .map(move |bytes| Match { row, bytes })
            })
            .collect();
    }
}

impl EditorWindow {
    pub(super) fn prepare_caption_editor(&mut self) {
        if !self.transcript.dirty {
            return;
        }
        self.transcript.dirty = false;
        let editor = &mut self.transcript.caption_editor;
        editor.refresh(&self.project);
        if let Some(search) = self.transcript.search.as_mut() {
            editor.find(&search.query);
            search.current = search.current.min(editor.matches.len().saturating_sub(1));
        }
    }

    pub(super) fn caption_search_changed(&mut self, query: String) {
        self.prepare_caption_editor();
        let Some(search) = self.transcript.search.as_mut() else {
            return;
        };
        let editor = &mut self.transcript.caption_editor;
        editor.find(&query);
        search.query = query;
        search.replaced = None;
        search.current = 0;
        if let Some(first) = editor.matches.first() {
            editor.list.scroll_to_reveal_item(first.row);
        }
    }

    pub(super) fn caption_search_count(&self) -> usize {
        self.transcript.caption_editor.matches.len()
    }

    pub(super) fn caption_editor_count(&self) -> usize {
        self.transcript.caption_editor.rows.len()
    }

    pub(super) fn caption_step_match(&mut self, step: i32, cx: &mut Context<Self>) {
        let Some(search) = self.transcript.search.as_mut() else {
            return;
        };
        let editor = &self.transcript.caption_editor;
        let count = editor.matches.len();
        if count == 0 {
            return;
        }
        search.current =
            (search.current as i64 + i64::from(step)).rem_euclid(count as i64) as usize;
        let row = editor.matches[search.current].row;
        editor.list.scroll_to_reveal_item(row);
        if let Some(time) = editor.rows[row].output_time {
            self.seek_to_time(time, cx);
        }
    }

    pub(super) fn caption_replace_matches(
        &mut self,
        all: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prepare_caption_editor();
        let Some(search) = self.transcript.search.as_ref() else {
            return;
        };
        let editor = &self.transcript.caption_editor;
        let selected = if all {
            editor.matches.as_slice()
        } else {
            editor
                .matches
                .get(search.current..search.current + 1)
                .unwrap_or_default()
        };
        let replacement = search.replace.read(cx).text().trim().to_string();
        let mut updated = HashMap::<SharedString, Vec<Range<usize>>>::new();
        let mut count = 0;
        for found in selected.iter().rev() {
            let row = &editor.rows[found.row];
            if row.text.get(found.bytes.clone()) == Some(replacement.as_str()) {
                continue;
            }
            updated
                .entry(row.id.clone())
                .or_default()
                .push(found.bytes.clone());
            count += 1;
        }
        if !updated.is_empty() {
            let replacement_for_edit = replacement.clone();
            self.edit_project_global("caption-editor-replace", window, cx, move |project| {
                let Some(captions) = project.captions.as_mut() else {
                    return false;
                };
                let mut changed = false;
                for segment in &mut captions.segments {
                    for range in updated.remove(segment.id.as_str()).into_iter().flatten() {
                        let mut text = segment.text.clone();
                        text.replace_range(range.clone(), &replacement_for_edit);
                        changed |= apply_caption_text(segment, text, Some(range));
                    }
                }
                changed
            });
            crate::caption_replace::remember_term(&replacement);
        }
        if let Some(search) = self.transcript.search.as_mut() {
            search.replaced = Some(count);
        }
    }

    fn caption_editor_edit(
        &mut self,
        id: &str,
        text: String,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.transcript.open || !self.transcript.caption_only {
            return;
        }
        self.edit_project_global("caption-editor-text", window, cx, move |project| {
            project
                .captions
                .as_mut()
                .and_then(|captions| {
                    captions
                        .segments
                        .iter_mut()
                        .find(|segment| segment.id == id)
                })
                .is_some_and(|segment| apply_caption_text(segment, text, selected))
        });
    }

    pub(super) fn dismiss_caption_editor_menu(&mut self) -> bool {
        self.transcript.caption_editor.menu.take().is_some()
    }

    pub(super) fn render_caption_editor_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.transcript.caption_editor.menu.clone()?;
        let items = vec![ui::MenuItem::new(
            tr::editor::workbench::delete_selected_text(),
            false,
        )];
        let state = ui::MenuState::new(menu.origin, &items);
        Some(
            deferred(
                ui::Menu::editor(&self.theme, "caption-editor-text-menu", items, &state)
                    .on_select(cx.listener(move |this, _: &usize, window, cx| {
                        this.transcript.caption_editor.menu = None;
                        if let Some(input) = menu.input.upgrade() {
                            let (mut text, range) = {
                                let input = input.read(cx);
                                (input.text().to_string(), input.selection())
                            };
                            if !range.is_empty() && text.get(range.clone()).is_some() {
                                text.replace_range(range.clone(), "");
                                window.focus(&this.transcript.focus, cx);
                                this.caption_editor_edit(&menu.id, text, Some(range), window, cx);
                            }
                        }
                        this.invalidate_transcript(cx);
                    }))
                    .on_dismiss(cx.listener(|this, _, _, cx| {
                        this.transcript.caption_editor.menu = None;
                        this.invalidate_transcript(cx);
                    })),
            )
            .into_any_element(),
        )
    }

    pub(super) fn render_caption_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let editor = &self.transcript.caption_editor;
        let theme = self.theme;
        let rows = editor.rows.clone();
        if rows.is_empty() {
            return div()
                .px(px(16.))
                .py(px(20.))
                .text_color(Hsla::from(theme.editor.text_2))
                .child(tr::editor::workbench::caption_empty())
                .into_any_element();
        }
        let inputs = editor.inputs.clone();
        let view = cx.weak_entity();
        let current_match = self
            .transcript
            .search
            .as_ref()
            .and_then(|search| editor.matches.get(search.current))
            .cloned();
        div()
            .flex_1()
            .min_h_0()
            .child(
                gpui::list(editor.list.clone(), move |index, window, cx| {
                    let row = &rows[index];
                    let mut inputs = inputs.borrow_mut();
                    let input = &inputs
                        .entry(row.id.clone())
                        .or_insert_with(|| {
                            let input = cx.new(|cx| {
                                let mut input = ui::TextInputState::multi_line(window, cx);
                                input.set_placeholder(
                                    tr::editor::workbench::caption_text_placeholder(),
                                );
                                input.set_text(row.text.clone(), cx);
                                input
                            });
                            let window = window.window_handle();
                            let id = row.id.clone();
                            let view = view.clone();
                            let subscription = cx.subscribe(&input, move |input, event, cx| {
                                let _ = window.update(cx, |_, window, cx| {
                                    let _ = view.update(cx, |this, cx| match event {
                                        ui::TextInputEvent::Changed => {
                                            let text = input.read(cx).text().to_string();
                                            this.caption_editor_edit(&id, text, None, window, cx);
                                        }
                                        ui::TextInputEvent::Blurred => {
                                            input.update(cx, |input, cx| {
                                                input.place_cursor(input.selection().end, cx);
                                            });
                                        }
                                        ui::TextInputEvent::Cancelled => {
                                            this.transcript.caption_editor.menu = None;
                                            window.focus(&this.transcript.focus, cx);
                                            this.invalidate_transcript(cx);
                                        }
                                        _ => {}
                                    });
                                });
                            });
                            RowInput {
                                input,
                                _subscription: subscription,
                            }
                        })
                        .input;
                    if input.read(cx).text() != row.text.as_ref() {
                        input.update(cx, |input, cx| input.set_text(row.text.clone(), cx));
                    }
                    caption_row(
                        &theme,
                        row,
                        input,
                        current_match.as_ref().filter(|hit| hit.row == index),
                        view.clone(),
                    )
                })
                .size_full(),
            )
            .into_any_element()
    }
}

fn caption_row(
    theme: &Theme,
    row: &Row,
    input: &Entity<ui::TextInputState>,
    found: Option<&Match>,
    editor: WeakEntity<EditorWindow>,
) -> AnyElement {
    let menu_input = input.downgrade();
    let seek_editor = editor.clone();
    let time = row.output_time;
    let stamp = crate::editor_timeline::format_time(row.time);
    let menu_id = row.id.clone();
    div()
        .w_full()
        .flex()
        .items_start()
        .gap(px(8.))
        .px(px(12.))
        .py(px(4.))
        .border_b_1()
        .border_color(Hsla::from(theme.editor.line))
        .when(found.is_some(), |row| row.bg(Hsla::from(theme.editor.ctl)))
        .child(
            ui::EditorButton::plain(
                theme,
                SharedString::from(format!("caption-editor-time-{}", row.id)),
            )
            .width(px(72.))
            .height(px(34.))
            .label(stamp)
            .disabled(time.is_none())
            .on_click(move |_, _, cx| {
                if let Some(time) = time {
                    let _ = seek_editor.update(cx, |this, cx| this.seek_to_time(time, cx));
                }
            }),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                    let Some(input) = menu_input.upgrade() else {
                        return;
                    };
                    if !input.read(cx).is_focused(window) || !input.read(cx).has_selection() {
                        return;
                    }
                    cx.stop_propagation();
                    let _ = editor.update(cx, |this, cx| {
                        this.transcript.caption_editor.menu = Some(DeleteMenu {
                            id: menu_id.clone(),
                            input: menu_input.clone(),
                            origin: event.position,
                        });
                        this.invalidate_transcript(cx);
                    });
                })
                .child(
                    ui::TextInput::bare(
                        theme,
                        SharedString::from(format!("caption-editor-line-{}", row.id)),
                        input,
                    )
                    .radius(px(6.))
                    .padding_x(px(8.))
                    .padding_y(px(7.))
                    .text_size(px(14.))
                    .focus_border(Hsla::from(theme.editor.accent))
                    .highlight(
                        found.map(|hit| hit.bytes.clone()),
                        Hsla::from(theme.amber_6),
                    ),
                ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deleting_the_first_repeated_word_keeps_the_second_words_timing() {
        let mut segment: CaptionSegment = serde_json::from_value(serde_json::json!({
            "id": "qa", "start": 0.0, "end": 3.0, "text": "词语词语保留",
            "words": [
                {"text": "词语", "start": 0.0, "end": 1.0},
                {"text": "词语", "start": 1.0, "end": 2.0},
                {"text": "保留", "start": 2.0, "end": 3.0, "hidden": true}
            ]
        }))
        .unwrap();
        assert!(apply_caption_text(
            &mut segment,
            "词语保留".into(),
            Some(0.."词语".len())
        ));
        assert_eq!(segment.text, "词语保留");
        assert_eq!(segment.words.len(), 2);
        assert_eq!((segment.words[0].start, segment.words[0].end), (1., 2.));
        assert!(segment.words[1].hidden);
        assert_eq!((segment.start, segment.end), (0., 3.));
    }
}
