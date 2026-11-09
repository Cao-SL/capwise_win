use std::io::Cursor;

use cap_utils::speech;
use gpui::{AppContext as _, Context, Entity, IntoElement, ParentElement, Styled, Window, div, px};

use crate::{
    ai_store::{self, Kind},
    i18n::tr,
    settings_window::SettingsWindow,
    store, ui,
};

pub(crate) struct SpeechSettings {
    key: Entity<ui::TextInputState>,
    model: Entity<ui::TextInputState>,
    voice: Entity<ui::TextInputState>,
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
            busy: false,
            notice: None,
        }
    }
}

impl SettingsWindow {
    fn run_speech_settings(&mut self, preview: bool, window: &mut Window, cx: &mut Context<Self>) {
        let state = &mut self.pages.speech;
        if state.busy {
            return;
        }
        let settings = speech::Settings {
            resource_id: state.model.read(cx).text().trim().to_string(),
            speaker: state.voice.read(cx).text().trim().to_string(),
        };
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
                let key = tokio::task::spawn_blocking(move || {
                    if draft_key.is_empty() {
                        return ai_store::key(Kind::Tts, "doubao")
                            .ok_or_else(|| tr::settings::speech::key_required().to_string());
                    }
                    Ok(draft_key)
                })
                .await
                .map_err(|error| error.to_string())??;
                let audio = speech::synthesize(&settings, &key, preview_text, false).await?;
                tokio::task::spawn_blocking(move || play_preview(audio.wav))
                    .await
                    .map_err(|error| error.to_string())??;
                Ok(tr::settings::speech::preview_done())
            },
            move |this, notice, _, cx| {
                let state = &mut this.pages.speech;
                state.busy = false;
                if !preview && notice.is_ok() {
                    state.key.update(cx, |input, cx| input.set_text("", cx));
                }
                state.notice = Some(notice);
                cx.notify();
            },
        );
        cx.notify();
    }

    pub(crate) fn render_speech_settings(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let state = &self.pages.speech;
        for input in [&state.key, &state.model, &state.voice] {
            input.update(cx, |input, cx| input.set_disabled(state.busy, cx));
        }
        let body = self
            .card(true)
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(div().text_size(px(12.)).child(tr::settings::speech::key()))
            .child(self.pages_input("doubao-tts-key", &state.key))
            .child(
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
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .child(self.button(
                        "doubao-tts-save",
                        (ui::ButtonVariant::Primary, None),
                        tr::common::save(),
                        state.busy,
                        cx,
                        |this, window, cx| this.run_speech_settings(false, window, cx),
                    ))
                    .child(self.button(
                        "doubao-tts-preview",
                        (ui::ButtonVariant::Gray, Some("icons/volume-2.svg")),
                        tr::settings::speech::preview(),
                        state.busy,
                        cx,
                        |this, window, cx| this.run_speech_settings(true, window, cx),
                    ))
                    .child(self.button(
                        "doubao-tts-voices",
                        (ui::ButtonVariant::Gray, Some("icons/arrow-up-right.svg")),
                        tr::settings::speech::voice_list(),
                        false,
                        cx,
                        |_, _, cx| {
                            cx.open_url(
                                "https://docs.volcengine.com/docs/doubao-voice/tone-list-3?lang=zh",
                            );
                        },
                    )),
            )
            .children(
                state
                    .busy
                    .then(|| tr::settings::speech::working().to_string())
                    .or_else(|| {
                        state.notice.as_ref().map(|notice| match notice {
                            Ok(message) => message.to_string(),
                            Err(message) => message.clone(),
                        })
                    })
                    .map(|message| {
                        div()
                            .text_size(px(12.))
                            .text_color(self.theme.settings_muted())
                            .child(message)
                    }),
            );
        self.section(
            tr::settings::speech::title(),
            Some(tr::settings::speech::description()),
            None,
            vec![body.into_any_element()],
        )
        .into_any_element()
    }
}

fn save_settings(settings: speech::Settings, draft_key: &str) -> Result<&'static str, String> {
    let value = serde_json::to_value(settings).map_err(|error| error.to_string())?;
    let previous_key = ai_store::key(Kind::Tts, "doubao");
    if draft_key.is_empty() && previous_key.is_none() {
        return Err(tr::settings::speech::key_required().into());
    }
    if !draft_key.is_empty() {
        ai_store::set_key(Kind::Tts, "doubao", draft_key)?;
    }
    if store::set_store_setting(speech::SETTINGS_SECTION, speech::SETTINGS_KEY, value) {
        return Ok(tr::settings::speech::saved());
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
