use std::{path::PathBuf, sync::Arc};

use cap_project::{ProjectConfiguration, TextSegment, TitleCardRole};
use gpui::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, Hsla, InteractiveElement,
    IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::{
    editor_title_cards::title_card_lane,
    editor_window::EditorWindow,
    i18n::tr,
    title_card_presets::{
        MaterializedTitleCard, PresetEntry, PresetError, PresetListing, TitleCardPresetStore,
    },
    ui,
};

#[derive(Default)]
pub(crate) struct TitleCardLibraryUi {
    open: bool,
    loaded: bool,
    entries: Vec<PresetEntry>,
    issues: Vec<(String, String)>,
    input: Option<Entity<ui::TextInputState>>,
    form: Option<NameAction>,
    removing: Option<String>,
    request: Option<Arc<()>>,
    task: Option<gpui::Task<()>>,
    busy: Option<Busy>,
    error: Option<String>,
    message: Option<String>,
}

impl Drop for TitleCardLibraryUi {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.detach();
        }
    }
}

#[derive(Clone)]
enum NameAction {
    Save {
        role: TitleCardRole,
        card_id: String,
    },
    Rename {
        id: String,
    },
}

#[derive(Clone, Copy)]
enum Busy {
    Loading,
    Saving,
    Renaming,
    Removing,
    Applying,
}

impl Busy {
    fn label(self) -> &'static str {
        match self {
            Self::Loading => tr::editor::title_card_library::loading(),
            Self::Saving => tr::editor::title_card_library::saving(),
            Self::Renaming => tr::editor::title_card_library::renaming(),
            Self::Removing => tr::editor::title_card_library::removing(),
            Self::Applying => tr::editor::title_card_library::applying(),
        }
    }
}

enum Job {
    List,
    Save { name: String, segment: TextSegment },
    Rename { id: String, name: String },
    Remove { id: String },
    Apply { id: String, role: TitleCardRole },
}

enum Outcome {
    Listed(PresetListing),
    Saved(PresetEntry),
    Renamed {
        id: String,
        name: String,
    },
    Removed {
        id: String,
    },
    Applied {
        card: MaterializedTitleCard,
        role: TitleCardRole,
    },
}

impl Job {
    fn busy(&self) -> Busy {
        match self {
            Self::List => Busy::Loading,
            Self::Save { .. } => Busy::Saving,
            Self::Rename { .. } => Busy::Renaming,
            Self::Remove { .. } => Busy::Removing,
            Self::Apply { .. } => Busy::Applying,
        }
    }

    fn run(self, project: &std::path::Path) -> Result<Outcome, PresetError> {
        let store = TitleCardPresetStore::default();
        match self {
            Self::List => store.list().map(Outcome::Listed),
            Self::Save { name, segment } => {
                store.save(&name, project, &segment).map(Outcome::Saved)
            }
            Self::Rename { id, name } => {
                store.rename(&id, &name)?;
                Ok(Outcome::Renamed {
                    id,
                    name: name.trim().into(),
                })
            }
            Self::Remove { id } => {
                store.remove(&id)?;
                Ok(Outcome::Removed { id })
            }
            Self::Apply { id, role } => store
                .materialize(&id, project, role)
                .map(|card| Outcome::Applied { card, role }),
        }
    }
}

struct RequestGuard {
    token: Arc<()>,
    project: PathBuf,
    snapshot: serde_json::Value,
}

impl RequestGuard {
    fn owns(&self, current: Option<&Arc<()>>) -> bool {
        current.is_some_and(|current| Arc::ptr_eq(current, &self.token))
    }

    fn matches_project(&self, path: &std::path::Path, project: &ProjectConfiguration) -> bool {
        self.project == path
            && serde_json::to_value(project).is_ok_and(|value| value == self.snapshot)
    }

    fn accepts_application(
        &self,
        path: &std::path::Path,
        project: &ProjectConfiguration,
        recording_busy: bool,
    ) -> bool {
        !recording_busy && self.matches_project(path, project)
    }
}

fn card_to_save(
    project: &ProjectConfiguration,
    role: TitleCardRole,
    id: &str,
) -> Option<TextSegment> {
    project
        .title_card(role)
        .filter(|segment| {
            segment
                .title_card
                .as_ref()
                .is_some_and(|card| card.id == id)
        })
        .cloned()
}

fn error_message(error: PresetError) -> String {
    if matches!(error, PresetError::Invalid("name")) {
        tr::editor::title_card_library::invalid_name().into()
    } else {
        tr::editor::title_card_library::failed(error)
    }
}

fn suggested_name(content: &str) -> String {
    content
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        .take(64)
        .collect::<String>()
        .trim()
        .trim_start_matches('.')
        .trim()
        .into()
}

impl EditorWindow {
    fn set_title_card_library_input_busy(&self, busy: bool, cx: &mut Context<Self>) {
        if let Some(input) = &self.sidebar.title_cards.library.input {
            input.update(cx, |input, cx| input.set_disabled(busy, cx));
        }
    }

    fn run_title_card_library_job(
        &mut self,
        job: Job,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sidebar.title_cards.library.busy.is_some() || !self.project_ready() {
            return;
        }
        self.end_field_edit(cx);
        self.close_color_picker(cx);
        let Some(instance) = self.instance.as_ref().map(Arc::downgrade) else {
            return;
        };
        let snapshot = match serde_json::to_value(&self.project) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.sidebar.title_cards.library.error =
                    Some(tr::editor::title_card_library::failed(error));
                cx.notify();
                return;
            }
        };
        let guard = RequestGuard {
            token: Arc::new(()),
            project: self.project_path.clone(),
            snapshot,
        };
        let project = guard.project.clone();
        let busy = job.busy();
        let state = &mut self.sidebar.title_cards.library;
        state.request = Some(guard.token.clone());
        state.busy = Some(busy);
        state.error = None;
        state.message = None;
        self.set_title_card_library_input_busy(true, cx);
        self.sidebar.title_cards.library.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { job.run(&project) })
                .await;
            let mut pending_result = Some(result);
            this.update_in(cx, |this, window, cx| {
                if !guard.owns(this.sidebar.title_cards.library.request.as_ref()) {
                    return;
                }
                let same_instance =
                    this.project_path == guard.project && this.holds_instance(&instance);
                let recording_busy = matches!(busy, Busy::Applying) && this.voiceover.is_busy();
                let same_project = !matches!(busy, Busy::Applying)
                    || guard.accepts_application(&this.project_path, &this.project, recording_busy);
                let state = &mut this.sidebar.title_cards.library;
                state.request = None;
                state.task = None;
                state.busy = None;
                this.set_title_card_library_input_busy(false, cx);
                if !same_instance || !same_project || !this.project_ready() {
                    this.sidebar.title_cards.library.error = Some(if recording_busy {
                        tr::editor::voiceover::busy().into()
                    } else {
                        tr::editor::title_card_library::project_changed().into()
                    });
                    cx.notify();
                    return;
                }
                let Some(result) = pending_result.take() else {
                    return;
                };
                match result {
                    Ok(Outcome::Listed(listing)) => {
                        let state = &mut this.sidebar.title_cards.library;
                        state.entries = listing.entries;
                        state.issues = listing.issues;
                        state.loaded = true;
                    }
                    Ok(Outcome::Saved(entry)) => {
                        let state = &mut this.sidebar.title_cards.library;
                        state.entries.push(entry);
                        state.entries.sort_by(|a, b| {
                            a.preset.name.cmp(&b.preset.name).then(a.id.cmp(&b.id))
                        });
                        state.form = None;
                        state.message = Some(tr::editor::title_card_library::saved().into());
                    }
                    Ok(Outcome::Renamed { id, name }) => {
                        let state = &mut this.sidebar.title_cards.library;
                        if let Some(entry) = state.entries.iter_mut().find(|entry| entry.id == id) {
                            entry.preset.name = name;
                        }
                        state.entries.sort_by(|a, b| {
                            a.preset.name.cmp(&b.preset.name).then(a.id.cmp(&b.id))
                        });
                        state.form = None;
                        state.message = Some(tr::editor::title_card_library::renamed().into());
                    }
                    Ok(Outcome::Removed { id }) => {
                        let state = &mut this.sidebar.title_cards.library;
                        state.entries.retain(|entry| entry.id != id);
                        state.issues.retain(|(issue, _)| *issue != id);
                        state.removing = None;
                        state.message = Some(tr::editor::title_card_library::removed().into());
                    }
                    Ok(Outcome::Applied { mut card, role }) => {
                        card.set_track(title_card_lane(&this.project));
                        let applied =
                            this.title_card_edit("title-card-template", window, cx, |project| {
                                card.apply(project).map(|change| change.changed)
                            });
                        if applied && card.is_referenced(&this.project) {
                            card.retain_if_referenced(&this.project);
                            this.sidebar.title_cards.library.form = None;
                            this.sidebar.title_cards.library.message =
                                Some(tr::editor::title_card_library::applied().into());
                            this.focus_title_card(role, cx);
                        } else {
                            cx.background_executor()
                                .spawn(async move {
                                    drop(card);
                                })
                                .detach();
                            this.sidebar.title_cards.library.error =
                                this.sidebar.title_cards.error.clone();
                            if this.sidebar.title_cards.library.error.is_none() {
                                this.focus_title_card(role, cx);
                            }
                        }
                    }
                    Err(error) => {
                        this.sidebar.title_cards.library.error = Some(error_message(error))
                    }
                }
                cx.notify();
                window.refresh();
            })
            .ok();
            if let Some(result) = pending_result {
                cx.background_executor()
                    .spawn(async move {
                        drop(result);
                    })
                    .detach();
            }
        }));
        cx.notify();
    }

    fn toggle_title_card_library(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sidebar.title_cards.library.busy.is_some() {
            return;
        }
        let state = &mut self.sidebar.title_cards.library;
        state.open = !state.open;
        if !state.open {
            state.form = None;
            state.removing = None;
        } else {
            self.run_title_card_library_job(Job::List, window, cx);
        }
        cx.notify();
    }

    fn begin_title_card_library_name(
        &mut self,
        action: NameAction,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sidebar.title_cards.library.busy.is_some() {
            return;
        }
        self.end_field_edit(cx);
        self.close_color_picker(cx);
        if self.sidebar.title_cards.library.input.is_none() {
            let input = cx.new(|cx| ui::TextInputState::single_line(window, cx));
            input.update(cx, |input, _| {
                input.set_placeholder(tr::editor::title_card_library::name_placeholder())
            });
            let subscription = cx.subscribe_in(
                &input,
                window,
                |this: &mut Self, _, event, window, cx| match event {
                    ui::TextInputEvent::Confirmed => {
                        this.commit_title_card_library_name(window, cx)
                    }
                    ui::TextInputEvent::Cancelled => {
                        this.cancel_title_card_library_form(window, cx)
                    }
                    ui::TextInputEvent::Changed => cx.notify(),
                    ui::TextInputEvent::Blurred => {}
                },
            );
            self.push_text_subscription(subscription);
            self.sidebar.title_cards.library.input = Some(input);
        }
        let state = &mut self.sidebar.title_cards.library;
        state.open = true;
        state.removing = None;
        state.error = None;
        state.message = None;
        state.form = Some(action);
        if let Some(input) = &state.input {
            input.update(cx, |input, cx| input.set_text(name, cx));
            window.focus(&input.read(cx).focus_handle(), cx);
        }
        cx.notify();
    }

    fn cancel_title_card_library_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sidebar.title_cards.library.busy.is_none() {
            self.sidebar.title_cards.library.form = None;
            self.sidebar.title_cards.library.removing = None;
            window.focus(&self.focus_handle_for_menu(), cx);
            cx.notify();
        }
    }

    fn commit_title_card_library_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = &self.sidebar.title_cards.library;
        if state.busy.is_some() {
            return;
        }
        let Some(action) = state.form.clone() else {
            return;
        };
        let Some(name) = state
            .input
            .as_ref()
            .map(|input| input.read(cx).text().to_string())
        else {
            return;
        };
        let job = match action {
            NameAction::Save { role, card_id } => {
                self.end_field_edit(cx);
                let Some(segment) = card_to_save(&self.project, role, &card_id) else {
                    self.sidebar.title_cards.library.error =
                        Some(tr::editor::title_card_library::project_changed().into());
                    cx.notify();
                    return;
                };
                Job::Save { name, segment }
            }
            NameAction::Rename { id } => Job::Rename { id, name },
        };
        self.run_title_card_library_job(job, window, cx);
    }

    fn cancel_title_card_library_apply(&mut self, cx: &mut Context<Self>) {
        if matches!(self.sidebar.title_cards.library.busy, Some(Busy::Applying)) {
            let state = &mut self.sidebar.title_cards.library;
            state.request = None;
            if let Some(task) = state.task.take() {
                task.detach();
            }
            state.busy = None;
            state.error = None;
            state.message = Some(tr::editor::title_card_library::cancelled().into());
            self.set_title_card_library_input_busy(false, cx);
            cx.notify();
        }
    }

    pub(crate) fn render_title_card_library(
        &self,
        selected_role: Option<TitleCardRole>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let state = &self.sidebar.title_cards.library;
        let busy = state.busy.is_some();
        let header = div()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .gap(px(8.))
            .child(
                div().flex().ml(px(-7.)).child(
                    ui::EditorButton::plain(&theme, "title-card-library-toggle")
                        .label(tr::editor::title_card_library::title())
                        .left_icon("icons/folder-open.svg")
                        .icon_size(px(14.))
                        .right_icon(if state.open {
                            "icons/chevron-down.svg"
                        } else {
                            "icons/chevron-right.svg"
                        })
                        .text_size(px(12.))
                        .height(px(26.))
                        .pressed(state.open)
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_title_card_library(window, cx)
                        })),
                ),
            );
        if !state.open {
            return header.into_any_element();
        }
        let mut body = div()
            .flex()
            .flex_col()
            .min_w_0()
            .flex_shrink_0()
            .gap(px(8.));
        body = body.child(
            div()
                .text_size(px(12.))
                .text_color(Hsla::from(theme.editor.text_2))
                .child(tr::editor::title_card_library::hint()),
        );
        if let Some(role) = selected_role {
            body = body.child(
                ui::Button::plain(
                    &theme,
                    "title-card-library-save-current",
                    ui::ButtonVariant::Gray,
                    ui::ButtonSize::Sm,
                )
                .label(tr::editor::title_card_library::save_current())
                .icon("icons/save.svg")
                .full_width()
                .disabled(busy || state.form.is_some())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.end_field_edit(cx);
                    let Some(segment) = this.project.title_card(role) else {
                        return;
                    };
                    let Some(card) = segment.title_card.as_ref() else {
                        return;
                    };
                    let action = NameAction::Save {
                        role,
                        card_id: card.id.clone(),
                    };
                    let name = suggested_name(&segment.content);
                    this.begin_title_card_library_name(action, name, window, cx);
                })),
            );
        }
        if let (Some(form), Some(input)) = (&state.form, &state.input) {
            let label = match form {
                NameAction::Save {
                    role: TitleCardRole::Intro,
                    ..
                } => tr::editor::title_card_library::save_intro(),
                NameAction::Save {
                    role: TitleCardRole::Outro,
                    ..
                } => tr::editor::title_card_library::save_outro(),
                NameAction::Rename { .. } => tr::editor::title_card_library::rename(),
            };
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        ui::Field::stacked(&theme, label).child(
                            div().flex().w_full().child(
                                ui::TextInput::plain(&theme, "title-card-library-name", input)
                                    .flex(true),
                            ),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .child(
                                div().flex_1().min_w_0().child(
                                    ui::Button::plain(
                                        &theme,
                                        "title-card-library-name-save",
                                        ui::ButtonVariant::Primary,
                                        ui::ButtonSize::Sm,
                                    )
                                    .label(tr::editor::title_card_library::save())
                                    .disabled(busy)
                                    .full_width()
                                    .on_click(cx.listener(
                                        |this, _, window, cx| {
                                            this.commit_title_card_library_name(window, cx)
                                        },
                                    )),
                                ),
                            )
                            .child(
                                div().flex_1().min_w_0().child(
                                    ui::Button::plain(
                                        &theme,
                                        "title-card-library-name-cancel",
                                        ui::ButtonVariant::Gray,
                                        ui::ButtonSize::Sm,
                                    )
                                    .label(tr::editor::title_card_library::cancel())
                                    .disabled(busy)
                                    .full_width()
                                    .on_click(cx.listener(
                                        |this, _, window, cx| {
                                            this.cancel_title_card_library_form(window, cx)
                                        },
                                    )),
                                ),
                            ),
                    ),
            );
        }
        if let Some(busy) = state.busy {
            body = body.child(
                div()
                    .text_size(px(12.))
                    .text_color(Hsla::from(theme.editor.text_2))
                    .child(busy.label()),
            );
            if matches!(busy, Busy::Applying) {
                body = body.child(
                    ui::Button::plain(
                        &theme,
                        "title-card-library-apply-cancel",
                        ui::ButtonVariant::Gray,
                        ui::ButtonSize::Sm,
                    )
                    .label(tr::editor::title_card_library::cancel())
                    .full_width()
                    .on_click(
                        cx.listener(|this, _, _, cx| this.cancel_title_card_library_apply(cx)),
                    ),
                );
            }
        }
        for message in [&state.error, &state.message].into_iter().flatten() {
            body = body.child(
                div()
                    .text_size(px(12.))
                    .text_color(Hsla::from(theme.editor.text_2))
                    .child(message.clone()),
            );
        }
        if state.loaded && state.entries.is_empty() && state.issues.is_empty() {
            body = body.child(
                div()
                    .text_size(px(12.))
                    .text_color(Hsla::from(theme.editor.text_2))
                    .child(tr::editor::title_card_library::empty()),
            );
        }
        let mut list = div()
            .id("title-card-library-list")
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .max_h(px(240.))
            .overflow_y_scroll()
            .gap(px(12.));
        for entry in &state.entries {
            let id = entry.id.clone();
            let name = entry.preset.name.clone();
            let removing = state.removing.as_ref() == Some(&id);
            let mut row = div()
                .flex()
                .flex_col()
                .min_w_0()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(name.clone()),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(Hsla::from(theme.editor.text_2))
                        .child(tr::editor::title_card_library::details(
                            format!("{:.2}", entry.preset.duration),
                            entry
                                .preset
                                .content
                                .split_whitespace()
                                .collect::<Vec<_>>()
                                .join(" ")
                                .chars()
                                .take(80)
                                .collect::<String>(),
                        )),
                );
            if removing {
                row = row.child(self.render_title_card_template_removal(&id, &name, cx));
            } else {
                let mut actions = div().flex().gap(px(6.));
                for role in [TitleCardRole::Intro, TitleCardRole::Outro] {
                    let exists = self.project.title_card(role).is_some();
                    let label = match (role, exists) {
                        (TitleCardRole::Intro, false) => {
                            tr::editor::title_card_library::use_intro()
                        }
                        (TitleCardRole::Outro, false) => {
                            tr::editor::title_card_library::use_outro()
                        }
                        (TitleCardRole::Intro, true) => {
                            tr::editor::title_card_library::replace_intro()
                        }
                        (TitleCardRole::Outro, true) => {
                            tr::editor::title_card_library::replace_outro()
                        }
                    };
                    let id = id.clone();
                    actions = actions.child(
                        div().flex_1().min_w_0().child(
                            ui::Button::plain(
                                &theme,
                                SharedString::from(format!("use-{role:?}-{id}")),
                                ui::ButtonVariant::Gray,
                                ui::ButtonSize::Sm,
                            )
                            .label(label)
                            .disabled(busy || state.form.is_some())
                            .full_width()
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.run_title_card_library_job(
                                        Job::Apply {
                                            id: id.clone(),
                                            role,
                                        },
                                        window,
                                        cx,
                                    )
                                },
                            )),
                        ),
                    );
                }
                let rename_id = id.clone();
                let rename_name = name.clone();
                let remove_id = id.clone();
                row = row.child(actions).child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .child(
                            div().flex_1().min_w_0().child(
                                ui::Button::plain(
                                    &theme,
                                    SharedString::from(format!("rename-{id}")),
                                    ui::ButtonVariant::Gray,
                                    ui::ButtonSize::Xs,
                                )
                                .label(tr::editor::title_card_library::rename())
                                .disabled(busy || state.form.is_some())
                                .full_width()
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        this.begin_title_card_library_name(
                                            NameAction::Rename {
                                                id: rename_id.clone(),
                                            },
                                            rename_name.clone(),
                                            window,
                                            cx,
                                        )
                                    },
                                )),
                            ),
                        )
                        .child(
                            div().flex_1().min_w_0().child(
                                ui::Button::plain(
                                    &theme,
                                    SharedString::from(format!("remove-{id}")),
                                    ui::ButtonVariant::Gray,
                                    ui::ButtonSize::Xs,
                                )
                                .label(tr::editor::title_card_library::remove())
                                .disabled(busy || state.form.is_some())
                                .full_width()
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.sidebar.title_cards.library.removing =
                                            Some(remove_id.clone());
                                        cx.notify();
                                    },
                                )),
                            ),
                        ),
                );
            }
            list = list.child(row);
        }
        for (id, error) in &state.issues {
            let remove_id = id.clone();
            let mut row = div().flex().flex_col().gap(px(6.)).child(
                div()
                    .text_size(px(12.))
                    .text_color(Hsla::from(theme.editor.text_2))
                    .child(tr::editor::title_card_library::unreadable(error)),
            );
            if state.removing.as_ref() == Some(id) {
                row = row.child(self.render_title_card_template_removal(
                    id,
                    tr::editor::title_card_library::unreadable_name(),
                    cx,
                ));
            } else {
                row = row.child(
                    ui::Button::plain(
                        &theme,
                        SharedString::from(format!("remove-broken-{id}")),
                        ui::ButtonVariant::Gray,
                        ui::ButtonSize::Xs,
                    )
                    .label(tr::editor::title_card_library::remove())
                    .disabled(busy || state.form.is_some())
                    .full_width()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.sidebar.title_cards.library.removing = Some(remove_id.clone());
                        cx.notify();
                    })),
                );
            }
            list = list.child(row);
        }
        header
            .child(body)
            .child(list)
            .child(
                div().flex_shrink_0().child(
                    ui::Button::plain(
                        &theme,
                        "title-card-library-refresh",
                        ui::ButtonVariant::Gray,
                        ui::ButtonSize::Xs,
                    )
                    .label(tr::editor::title_card_library::refresh())
                    .disabled(busy || state.form.is_some())
                    .full_width()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.run_title_card_library_job(Job::List, window, cx)
                    })),
                ),
            )
            .into_any_element()
    }

    fn render_title_card_template_removal(
        &self,
        id: &str,
        name: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let id = id.to_string();
        let busy = self.sidebar.title_cards.library.busy.is_some();
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(Hsla::from(theme.editor.text_2))
                    .child(tr::editor::title_card_library::remove_confirm(name)),
            )
            .child(
                div()
                    .flex()
                    .gap(px(6.))
                    .child(
                        div().flex_1().min_w_0().child(
                            ui::Button::plain(
                                &theme,
                                SharedString::from(format!("confirm-remove-{id}")),
                                ui::ButtonVariant::Destructive,
                                ui::ButtonSize::Sm,
                            )
                            .label(tr::editor::title_card_library::remove())
                            .disabled(busy)
                            .full_width()
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.run_title_card_library_job(
                                        Job::Remove { id: id.clone() },
                                        window,
                                        cx,
                                    )
                                },
                            )),
                        ),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            ui::Button::plain(
                                &theme,
                                "cancel-template-remove",
                                ui::ButtonVariant::Gray,
                                ui::ButtonSize::Sm,
                            )
                            .label(tr::editor::title_card_library::cancel())
                            .disabled(busy)
                            .full_width()
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.cancel_title_card_library_form(window, cx)
                                },
                            )),
                        ),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_card_library_request_rejects_cancel_replacement_and_changed_project() {
        let original = ProjectConfiguration::default();
        let guard = RequestGuard {
            token: Arc::new(()),
            project: PathBuf::from("a.cap"),
            snapshot: serde_json::to_value(&original).unwrap(),
        };
        assert!(guard.owns(Some(&guard.token)));
        assert!(!guard.owns(None));
        assert!(!guard.owns(Some(&Arc::new(()))));
        assert!(guard.matches_project(std::path::Path::new("a.cap"), &original));
        assert!(!guard.matches_project(std::path::Path::new("b.cap"), &original));
        let mut changed = original.clone();
        changed.background.padding += 1.0;
        assert!(!guard.matches_project(std::path::Path::new("a.cap"), &changed));
    }

    #[test]
    fn title_card_library_late_application_rejects_recording_even_with_unchanged_snapshot() {
        let project = ProjectConfiguration::default();
        let path = std::path::Path::new("a.cap");
        let guard = RequestGuard {
            token: Arc::new(()),
            project: path.into(),
            snapshot: serde_json::to_value(&project).unwrap(),
        };
        assert!(guard.owns(Some(&guard.token)));
        assert!(guard.accepts_application(path, &project, false));
        assert!(!guard.accepts_application(path, &project, true));
        assert!(guard.matches_project(path, &project));
    }

    #[test]
    fn title_card_library_name_form_cannot_save_a_replaced_card() {
        let mut project: ProjectConfiguration = serde_json::from_value(serde_json::json!({
            "timeline": {"segments": [{"start": 0.0, "end": 5.0, "timescale": 1.0}], "zoomSegments": []}
        })).unwrap();
        let card = TextSegment::new_title_card(TitleCardRole::Intro, "First");
        let id = card.title_card.as_ref().unwrap().id.clone();
        project.upsert_title_card(card).unwrap();
        assert_eq!(
            card_to_save(&project, TitleCardRole::Intro, &id)
                .unwrap()
                .content,
            "First"
        );
        project.remove_title_card(TitleCardRole::Intro).unwrap();
        project
            .upsert_title_card(TextSegment::new_title_card(TitleCardRole::Intro, "New"))
            .unwrap();
        assert!(card_to_save(&project, TitleCardRole::Intro, &id).is_none());
    }
}
