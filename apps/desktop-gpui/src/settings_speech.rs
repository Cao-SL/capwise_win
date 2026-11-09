use std::io::Cursor;

use cap_utils::speech;
use gpui::{
    AppContext as _, Context, Entity, IntoElement, ParentElement, Styled, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::{
    ai_store::{self, Kind},
    i18n::tr,
    settings_window::SettingsWindow,
    store, ui,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Credential {
    Checking,
    Missing,
    Recognition,
    Dedicated,
}

impl Credential {
    fn ready(self) -> bool {
        matches!(self, Self::Recognition | Self::Dedicated)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Checking => tr::settings::speech::checking(),
            Self::Missing => tr::settings::speech::disconnected(),
            Self::Recognition => tr::settings::speech::recognition_key(),
            Self::Dedicated => tr::settings::speech::dedicated_key(),
        }
    }
}

pub(crate) struct SpeechSettings {
    key: Entity<ui::TextInputState>,
    model: Entity<ui::TextInputState>,
    voice: Entity<ui::TextInputState>,
    credential: Credential,
    editing_key: bool,
    advanced: bool,
    busy: bool,
    notice: Option<Result<&'static str, String>>,
}

impl SpeechSettings {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<SettingsWindow>) -> Self {
        let saved: speech::Settings = store::store_section(speech::SETTINGS_SECTION)
            .remove(speech::SETTINGS_KEY)
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default();
        Self {
            key: cx.new(|cx| {
                let mut input = ui::TextInputState::single_line(window, cx);
                input.set_secret(true, cx);
                input.set_placeholder(tr::settings::speech::key_hint());
                input
            }),
            model: cx.new(|cx| {
                let mut input = ui::TextInputState::single_line(window, cx);
                input.set_text(saved.resource_id, cx);
                input
            }),
            voice: cx.new(|cx| {
                let mut input = ui::TextInputState::single_line(window, cx);
                input.set_text(saved.speaker, cx);
                input
            }),
            credential: Credential::Checking,
            editing_key: false,
            advanced: false,
            busy: false,
            notice: None,
        }
    }

    fn settings(&self, cx: &mut Context<SettingsWindow>) -> speech::Settings {
        speech::Settings {
            resource_id: self.model.read(cx).text().trim().to_string(),
            speaker: self.voice.read(cx).text().trim().to_string(),
        }
    }
}

impl SettingsWindow {
    pub(crate) fn check_speech_credential(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pages.speech.busy {
            return;
        }
        self.pages.speech.credential = Credential::Checking;
        self.spawn_tokio(
            window,
            cx,
            async {
                tokio::task::spawn_blocking(|| {
                    stored_key().map_or(Credential::Missing, |(source, _)| source)
                })
                .await
                .unwrap_or(Credential::Missing)
            },
            |this, source, _, cx| {
                this.pages.speech.credential = source;
                cx.notify();
            },
        );
        cx.notify();
    }

    fn run_speech_settings(&mut self, preview: bool, window: &mut Window, cx: &mut Context<Self>) {
        let state = &mut self.pages.speech;
        if state.busy {
            return;
        }
        let settings = state.settings(cx);
        if settings.validate().is_err() {
            state.notice = Some(Err(tr::settings::speech::invalid_settings().into()));
            cx.notify();
            return;
        }
        let draft_key = state.key.read(cx).text().trim().to_string();
        let preview_text = tr::settings::speech::sample();
        state.busy = true;
        state.notice = None;
        self.spawn_tokio(
            window,
            cx,
            async move {
                if !preview {
                    return tokio::task::spawn_blocking(move || {
                        save_settings(settings, &draft_key)
                    })
                    .await
                    .map_err(|error| error.to_string())?;
                }
                let (source, key) = tokio::task::spawn_blocking(stored_key)
                    .await
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| tr::settings::speech::key_required().to_string())?;
                let audio = speech::synthesize(&settings, &key, preview_text, false)
                    .await
                    .map_err(|error| {
                        if source == Credential::Recognition {
                            format!("{} {error}", tr::settings::speech::recognition_permission())
                        } else {
                            error
                        }
                    })?;
                tokio::task::spawn_blocking(move || play_preview(audio.wav))
                    .await
                    .map_err(|error| error.to_string())??;
                Ok((tr::settings::speech::preview_done(), source))
            },
            move |this, result, _, cx| {
                let state = &mut this.pages.speech;
                state.busy = false;
                state.notice = Some(result.map(|(message, source)| {
                    state.credential = source;
                    if !preview {
                        state.editing_key = false;
                        state.key.update(cx, |input, cx| input.set_text("", cx));
                    }
                    message
                }));
                cx.notify();
            },
        );
        cx.notify();
    }

    pub(crate) fn render_speech_settings(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let state = &self.pages.speech;
        let ready = state.credential.ready();
        let show_key = state.editing_key || state.credential == Credential::Missing;
        for input in [&state.key, &state.model, &state.voice] {
            input.update(cx, |input, cx| input.set_disabled(state.busy, cx));
        }
        let credential_actions = if show_key {
            Some(
                self.button(
                    "doubao-tts-get-key",
                    (ui::ButtonVariant::Gray, Some("icons/arrow-up-right.svg")),
                    tr::settings::ai::get_key(),
                    false,
                    cx,
                    |_, _, cx| cx.open_url(speech::API_KEY_URL),
                )
                .into_any_element(),
            )
        } else if ready {
            Some(
                self.button(
                    "doubao-tts-replace-key",
                    (ui::ButtonVariant::Gray, None),
                    tr::settings::speech::replace_key(),
                    state.busy,
                    cx,
                    |this, _, cx| {
                        this.pages.speech.editing_key = true;
                        this.pages.speech.notice = None;
                        cx.notify();
                    },
                )
                .into_any_element(),
            )
        } else {
            None
        };
        let connection = self
            .card(true)
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .child(div().text_size(px(13.)).child(state.credential.label()))
                    .children(credential_actions),
            )
            .when(show_key, |body| {
                body.child(div().text_size(px(12.)).child(tr::settings::speech::key()))
                    .child(self.pages_input("doubao-tts-key", &state.key))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(self.button(
                                "doubao-tts-save-key",
                                (ui::ButtonVariant::Primary, None),
                                tr::common::save(),
                                state.busy,
                                cx,
                                |this, window, cx| this.run_speech_settings(false, window, cx),
                            ))
                            .when(ready, |row| {
                                row.child(self.button(
                                    "doubao-tts-cancel-key",
                                    (ui::ButtonVariant::Gray, None),
                                    tr::common::cancel(),
                                    state.busy,
                                    cx,
                                    |this, _, cx| {
                                        let state = &mut this.pages.speech;
                                        state.editing_key = false;
                                        state.key.update(cx, |input, cx| input.set_text("", cx));
                                        state.notice = None;
                                        cx.notify();
                                    },
                                ))
                            }),
                    )
            });
        let mut children = vec![connection.into_any_element()];
        if ready {
            let settings = state.settings(cx);
            let voices = [
                (speech::VOICE, "Vivi 2.0"),
                ("zh_female_xiaohe_uranus_bigtts", "小何 2.0"),
                ("zh_male_m191_uranus_bigtts", "云舟 2.0"),
                ("zh_male_taocheng_uranus_bigtts", "小天 2.0"),
            ];
            let model = if settings.resource_id == speech::MODEL {
                tr::settings::speech::default_model()
            } else {
                tr::settings::speech::cloned_model()
            };
            let voice = voices
                .iter()
                .find_map(|(id, name)| (*id == settings.speaker).then_some(*name))
                .unwrap_or_else(tr::settings::speech::custom_voice);
            let voice_card = self
                .card(true)
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(div().text_size(px(13.)).child(format!("{model} · {voice}")))
                .when(settings.resource_id == speech::MODEL && !show_key, |body| {
                    let options = voices
                        .iter()
                        .map(|(id, name)| ui::SegmentOption {
                            label: Some((*name).into()),
                            icon: None,
                            selected: *id == settings.speaker,
                            disabled: state.busy,
                        })
                        .collect();
                    body.child(
                        ui::SegmentedControl::settings(
                            &self.theme,
                            "doubao-tts-voice-presets",
                            options,
                        )
                        .stretch()
                        .on_select(cx.listener(
                            move |this, index: &usize, _, cx| {
                                if let Some((id, _)) = voices.get(*index) {
                                    this.pages
                                        .speech
                                        .voice
                                        .update(cx, |input, cx| input.set_text(*id, cx));
                                    this.pages.speech.notice = None;
                                    cx.notify();
                                }
                            },
                        )),
                    )
                })
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(8.))
                        .child(self.button(
                            "doubao-tts-save",
                            (ui::ButtonVariant::Primary, None),
                            tr::common::save(),
                            state.busy || show_key,
                            cx,
                            |this, window, cx| this.run_speech_settings(false, window, cx),
                        ))
                        .child(self.button(
                            "doubao-tts-preview",
                            (ui::ButtonVariant::Gray, Some("icons/volume-2.svg")),
                            tr::settings::speech::preview(),
                            state.busy || show_key,
                            cx,
                            |this, window, cx| this.run_speech_settings(true, window, cx),
                        ))
                        .child(self.button(
                            "doubao-tts-advanced",
                            (ui::ButtonVariant::Gray, Some("icons/settings.svg")),
                            tr::settings::speech::advanced(),
                            state.busy || show_key,
                            cx,
                            |this, _, cx| {
                                this.pages.speech.advanced = !this.pages.speech.advanced;
                                cx.notify();
                            },
                        )),
                )
                .when(state.advanced && !show_key, |body| {
                    body.child(
                        div()
                            .text_size(px(12.))
                            .child(tr::settings::speech::model()),
                    )
                    .child(self.pages_input("doubao-tts-model", &state.model))
                    .child(
                        div()
                            .text_size(px(12.))
                            .child(tr::settings::speech::voice()),
                    )
                    .child(self.pages_input("doubao-tts-voice", &state.voice))
                    .child(div().flex().flex_wrap().gap(px(8.)).child(self.button(
                        "doubao-tts-voices",
                        (ui::ButtonVariant::Gray, Some("icons/arrow-up-right.svg")),
                        tr::settings::speech::voice_list(),
                        false,
                        cx,
                        |_, _, cx| {
                            cx.open_url(
                                "https://docs.volcengine.com/docs/doubao-voice/tone-list-3?lang=zh",
                            )
                        },
                    )))
                });
            children.push(voice_card.into_any_element());
        }
        if let Some(message) = state
            .busy
            .then(|| tr::settings::speech::working().to_string())
            .or_else(|| {
                state.notice.as_ref().map(|notice| match notice {
                    Ok(message) => message.to_string(),
                    Err(message) => message.clone(),
                })
            })
        {
            children.push(
                div()
                    .text_size(px(12.))
                    .text_color(self.theme.settings_muted())
                    .child(message)
                    .into_any_element(),
            );
        }
        self.section(
            tr::settings::speech::title(),
            Some(tr::settings::speech::description()),
            None,
            children,
        )
        .into_any_element()
    }
}

fn stored_key() -> Option<(Credential, String)> {
    if let Some(key) = ai_store::key(Kind::Tts, "doubao") {
        return Some((Credential::Dedicated, key));
    }
    speech::can_reuse_recognition_key(&store::store_section(speech::SETTINGS_SECTION))
        .then(|| ai_store::key(Kind::Asr, "doubao"))
        .flatten()
        .map(|key| (Credential::Recognition, key))
}

fn save_settings(
    settings: speech::Settings,
    draft_key: &str,
) -> Result<(&'static str, Credential), String> {
    let value = serde_json::to_value(settings).map_err(|error| error.to_string())?;
    let previous_key = ai_store::key(Kind::Tts, "doubao");
    let source = if draft_key.is_empty() {
        stored_key()
            .map(|(source, _)| source)
            .ok_or_else(|| tr::settings::speech::key_required().to_string())?
    } else {
        ai_store::set_key(Kind::Tts, "doubao", draft_key)?;
        Credential::Dedicated
    };
    if store::set_store_setting(speech::SETTINGS_SECTION, speech::SETTINGS_KEY, value) {
        return Ok((tr::settings::speech::saved(), source));
    }
    if !draft_key.is_empty() {
        ai_store::set_key(
            Kind::Tts,
            "doubao",
            previous_key.as_deref().unwrap_or_default(),
        )?;
    }
    Err(tr::settings::speech::save_failed().into())
}

fn play_preview(wav: Vec<u8>) -> Result<(), String> {
    let play = || -> anyhow::Result<()> {
        let (_stream, handle) = rodio::OutputStream::try_default()?;
        let sink = rodio::Sink::try_new(&handle)?;
        sink.set_volume(0.7);
        sink.append(rodio::Decoder::new(Cursor::new(wav))?);
        sink.sleep_until_end();
        Ok(())
    };
    play().map_err(|_| tr::settings::speech::play_failed().into())
}
