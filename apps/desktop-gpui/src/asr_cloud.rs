//! Cloud speech recognition, ported from qiaomu-clipper's `native/asr_cloud.py`
//! and `src/utils/asr-settings.ts`.
//!
//! Audio is cut in the middle of pauses, the pieces are sent a few at a time,
//! and what comes back is put on the recording's timeline as caption lines.
//! Three request styles cover the services: Doubao's recorded-file "flash"
//! call (per-word times), an OpenAI-style `/audio/transcriptions` upload, and
//! an audio-capable `/chat/completions` model. Services that return no times
//! get their sentences spread over the piece by length.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use cap_project::{CaptionSegment, CaptionWord};
use futures_util::{StreamExt as _, stream};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    ai_store::{self, Endpoint, Kind},
    i18n::tr,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    DoubaoFlash,
    OpenAiTranscriptions,
    ChatAudio,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContextMode {
    None,
    /// `request.corpus.context`, a dialog-context JSON.
    Doubao,
    /// Whisper's `prompt`: only its tail is read, so it stays short.
    Prompt,
}

pub struct AsrService {
    pub id: &'static str,
    pub name: fn() -> &'static str,
    pub note: fn() -> &'static str,
    pub protocol: Protocol,
    pub base_url: &'static str,
    pub models: &'static [&'static str],
    pub key_url: Option<&'static str>,
    pub needs_key: bool,
    context: ContextMode,
    chunk_target: f64,
    chunk_max: f64,
}

impl AsrService {
    pub fn default_model(&self) -> &'static str {
        self.models.first().copied().unwrap_or_default()
    }

    /// The saved address and model, or the service's own.
    pub fn endpoint(&self) -> Endpoint {
        ai_store::endpoint(Kind::Asr, self.id).unwrap_or_else(|| Endpoint {
            base_url: self.base_url.to_string(),
            model: self.default_model().to_string(),
        })
    }

    /// Set up enough to use: a usable address and model, and a key unless the
    /// service runs on this computer.
    pub fn ready(&self) -> bool {
        let endpoint = self.endpoint();
        ai_store::endpoint(Kind::Asr, self.id).is_some()
            && ai_store::address_allowed(&endpoint.base_url)
            && !endpoint.model.trim().is_empty()
            && (!self.needs_key || ai_store::key(Kind::Asr, self.id).is_some())
    }

    /// Ask for sentence times only from models known to return them: asking
    /// another one for `verbose_json` can make it fail.
    fn wants_segments(&self, model: &str) -> bool {
        self.id == "groq" || (self.id == "openai" && model == "whisper-1")
    }
}

const fn openai_style(
    id: &'static str,
    name: fn() -> &'static str,
    note: fn() -> &'static str,
    base_url: &'static str,
    models: &'static [&'static str],
    key_url: Option<&'static str>,
) -> AsrService {
    AsrService {
        id,
        name,
        note,
        protocol: Protocol::OpenAiTranscriptions,
        base_url,
        models,
        key_url,
        needs_key: true,
        context: ContextMode::None,
        chunk_target: 20.0,
        chunk_max: 40.0,
    }
}

/// Tried with real keys in Clipper on a 5-minute Chinese lecture: Doubao,
/// SiliconFlow, GLM, StepFun and MiMo. Groq and OpenAI follow the same
/// published API.
pub static SERVICES: &[AsrService] = &[
    AsrService {
        id: "doubao",
        name: tr::ai::asr::doubao::name,
        note: tr::ai::asr::doubao::note,
        protocol: Protocol::DoubaoFlash,
        base_url: "https://openspeech.bytedance.com/api/v3",
        models: &["bigmodel"],
        key_url: Some("https://console.volcengine.com/speech/new/experience/asr"),
        needs_key: true,
        context: ContextMode::Doubao,
        chunk_target: 300.0,
        chunk_max: 540.0,
    },
    openai_style(
        "siliconflow",
        tr::ai::asr::siliconflow::name,
        tr::ai::asr::siliconflow::note,
        "https://api.siliconflow.cn/v1",
        &[
            "Qwen/Qwen3-ASR-1.7B",
            "FunAudioLLM/SenseVoiceSmall",
            "XingChenAGI/XingChenASR-V3.2",
            "XingChenAGI/XingChenASR-Diarize-V3.0",
        ],
        Some("https://cloud.siliconflow.cn/account/ak"),
    ),
    AsrService {
        chunk_target: 18.0,
        chunk_max: 28.0,
        ..openai_style(
            "glm",
            tr::ai::asr::glm::name,
            tr::ai::asr::glm::note,
            "https://open.bigmodel.cn/api/paas/v4",
            &["glm-asr-2512"],
            Some("https://open.bigmodel.cn/usercenter/proj-mgmt/apikeys"),
        )
    },
    openai_style(
        "stepfun",
        tr::ai::asr::stepfun::name,
        tr::ai::asr::stepfun::note,
        "https://api.stepfun.com/v1",
        &["stepaudio-2.5-asr", "step-asr"],
        Some("https://platform.stepfun.com/interface-key"),
    ),
    AsrService {
        protocol: Protocol::ChatAudio,
        ..openai_style(
            "mimo",
            tr::ai::asr::mimo::name,
            tr::ai::asr::mimo::note,
            "https://api.xiaomimimo.com/v1",
            &["mimo-v2.5-asr"],
            Some("https://platform.xiaomimimo.com/"),
        )
    },
    AsrService {
        context: ContextMode::Prompt,
        ..openai_style(
            "groq",
            tr::ai::asr::groq::name,
            tr::ai::asr::groq::note,
            "https://api.groq.com/openai/v1",
            &["whisper-large-v3-turbo", "whisper-large-v3"],
            Some("https://console.groq.com/keys"),
        )
    },
    AsrService {
        context: ContextMode::Prompt,
        ..openai_style(
            "openai",
            tr::ai::asr::openai::name,
            tr::ai::asr::openai::note,
            "https://api.openai.com/v1",
            &["whisper-1", "gpt-4o-mini-transcribe", "gpt-4o-transcribe"],
            Some("https://platform.openai.com/api-keys"),
        )
    },
    AsrService {
        needs_key: false,
        ..openai_style(
            "local-server",
            tr::ai::asr::local_server::name,
            tr::ai::asr::local_server::note,
            "http://127.0.0.1:8765/v1",
            &["Qwen/Qwen3-ASR-0.6B", "whisper-1"],
            None,
        )
    },
    AsrService {
        needs_key: false,
        ..openai_style(
            "custom",
            tr::ai::asr::custom::name,
            tr::ai::asr::custom::note,
            "",
            &[],
            None,
        )
    },
];

pub fn service(id: &str) -> Option<&'static AsrService> {
    SERVICES.iter().find(|service| service.id == id)
}

pub fn is_cloud_model(model: &str) -> bool {
    service(model).is_some()
}

const DOUBAO_RESOURCE: &str = "volc.bigasr.auc_turbo";
const DOUBAO_OK: &str = "20000000";
const DOUBAO_SILENT: &str = "20000003";

const SAMPLE_RATE: usize = 16_000;
const FRAMES_PER_SECOND: usize = 50;
const CHUNK_MIN: f64 = 4.0;
const SILENCE_SECONDS: f64 = 0.15;
const CONCURRENCY: usize = 3;
const RETRIES: u32 = 3;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
const DOUBAO_CONTEXT_BUDGET: usize = 700;
const PROMPT_BUDGET: usize = 160;

/// On-screen line length, in CJK characters (a Latin letter counts ~half).
const LINE_MIN_WIDTH: f32 = 6.0;
const LINE_MAX_WIDTH: f32 = 16.0;

// ---- cutting the audio -----------------------------------------------------

fn decibels(power: f32) -> f32 {
    10.0 * power.max(1e-12).log10()
}

/// Midpoints (seconds) of the pauses in 16 kHz mono audio. "Quiet" is relative
/// to this recording: its mean level minus 13 dB, kept within -45..-22 dB, so a
/// loud lecture still has pauses.
fn silence_midpoints(samples: &[f32]) -> Vec<f64> {
    let frame = SAMPLE_RATE / FRAMES_PER_SECOND;
    if samples.len() < frame {
        return Vec::new();
    }
    let mean_power = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
    let threshold = (decibels(mean_power) - 13.0).clamp(-45.0, -22.0);
    let min_frames = (SILENCE_SECONDS * FRAMES_PER_SECOND as f64).ceil() as usize;
    let seconds_per_frame = 1.0 / FRAMES_PER_SECOND as f64;

    let mut mids = Vec::new();
    let mut run_start = None;
    let levels = samples
        .chunks(frame)
        .map(|chunk| decibels(chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len() as f32));
    for (index, level) in levels.chain(std::iter::once(f32::INFINITY)).enumerate() {
        match (level < threshold, run_start) {
            (true, None) => run_start = Some(index),
            (false, Some(start)) => {
                if index - start >= min_frames {
                    mids.push((start + index) as f64 / 2.0 * seconds_per_frame);
                }
                run_start = None;
            }
            _ => {}
        }
    }
    mids
}

/// Pieces of about `target` seconds cut in the middle of a pause, never
/// longer than `maximum`.
fn plan_chunks(mids: &[f64], total: f64, target: f64, maximum: f64) -> Vec<(f64, f64)> {
    let mut chunks = Vec::new();
    let mut start = 0.0;
    while total - start > maximum {
        let cut = mids
            .iter()
            .copied()
            .filter(|mid| (start + CHUNK_MIN..=start + maximum).contains(mid))
            .min_by(|a, b| {
                (a - (start + target))
                    .abs()
                    .total_cmp(&(b - (start + target)).abs())
            })
            .unwrap_or(start + maximum);
        chunks.push((start, cut));
        start = cut;
    }
    if total - start >= 0.3 || chunks.is_empty() {
        chunks.push((start, total));
    }
    chunks
}

fn wav_bytes(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE as u32).to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE as u32 * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&((sample.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    out
}

// ---- what comes back -------------------------------------------------------

/// One stretch of speech, in milliseconds from the piece's start.
#[derive(Debug, Deserialize)]
struct Utterance {
    start_time: f64,
    end_time: f64,
    #[serde(default)]
    text: String,
    #[serde(default)]
    words: Vec<Word>,
}

#[derive(Debug, Deserialize)]
struct Word {
    start_time: f64,
    end_time: f64,
    #[serde(default)]
    text: String,
}

#[derive(Debug, Deserialize)]
struct FlashResponse {
    result: Option<FlashResult>,
}

#[derive(Debug, Default, Deserialize)]
struct FlashResult {
    #[serde(default)]
    utterances: Vec<Utterance>,
}

#[derive(Debug, Deserialize)]
struct TranscriptionResponse {
    #[serde(default)]
    text: String,
    #[serde(default)]
    segments: Option<Vec<TranscriptionSegment>>,
}

#[derive(Debug, Deserialize)]
struct TranscriptionSegment {
    start: f64,
    end: f64,
    #[serde(default)]
    text: String,
}

/// Emotion and sound-event marks some recognisers add (SenseVoice's
/// `<|tags|>`, emoji); a caption has no use for them.
fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<|") {
        out.push_str(&rest[..start]);
        rest = rest[start..].find("|>").map_or("", |end| &rest[start + end + 2..]);
    }
    out.push_str(rest);
    out.chars()
        .filter(|c| !matches!(*c as u32, 0x1F300..=0x1FAFF | 0x2600..=0x27BF | 0xFE0F))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Sentences, each a stretch of the piece in proportion to its length.
fn spread_sentences(text: &str, duration_ms: f64) -> Vec<Utterance> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    for c in text.chars() {
        current.push(c);
        if "。！？!?；;…".contains(c) {
            sentences.push(std::mem::take(&mut current));
        }
    }
    sentences.push(current);
    let sentences: Vec<String> = sentences
        .into_iter()
        .map(|sentence| sentence.trim().to_string())
        .filter(|sentence| !sentence.is_empty())
        .collect();
    let total: f32 = sentences.iter().map(|s| width(s).max(0.5)).sum();
    let mut at = 0.0;
    sentences
        .into_iter()
        .map(|sentence| {
            let span = duration_ms * f64::from(width(&sentence).max(0.5) / total.max(0.5));
            let utterance = Utterance {
                start_time: at,
                end_time: at + span,
                text: sentence,
                words: Vec::new(),
            };
            at += span;
            utterance
        })
        .collect()
}

// ---- one request -----------------------------------------------------------

enum Failure {
    Fatal(String),
    Retry(String),
}

fn jitter() -> f64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.subsec_nanos());
    f64::from(nanos % 1000) / 1000.0
}

pub(crate) fn backoff(attempt: u32, retry_after: Option<f64>) -> Duration {
    let wait = retry_after
        .filter(|seconds| *seconds > 0.0)
        .unwrap_or(2f64.powi(attempt as i32) + jitter());
    Duration::from_secs_f64(wait.min(30.0))
}

fn mentions(detail: &str, words: &[&str]) -> bool {
    let lower = detail.to_lowercase();
    words.iter().any(|word| lower.contains(word))
}

fn quota_failure(detail: &str) -> bool {
    mentions(
        detail,
        &["quota", "balance", "insufficient", "额度", "余额", "欠费"],
    )
}

fn header(response: &reqwest::Response, name: &str) -> String {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// The service's own error message, or the start of its body.
fn message_of(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            let error = value.get("error");
            error
                .and_then(|e| e.get("message"))
                .or(error)
                .or_else(|| value.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.chars().take(160).collect())
}

async fn http_failure(service: &AsrService, response: reqwest::Response) -> Failure {
    let status = response.status().as_u16();
    let retry_after = header(&response, "Retry-After").parse::<f64>().ok();
    let detail = message_of(&response.text().await.unwrap_or_default());
    let name = (service.name)();
    if status == 401 || status == 403 {
        return Failure::Fatal(tr::asr::auth_failed(name));
    }
    if status == 402 || quota_failure(&detail) {
        return Failure::Fatal(tr::asr::quota(name, &detail));
    }
    let message = tr::asr::failed(name, status, &detail);
    if status == 429 || status >= 500 {
        if let Some(seconds) = retry_after {
            tokio::time::sleep(backoff(0, Some(seconds))).await;
        }
        Failure::Retry(message)
    } else {
        Failure::Fatal(message)
    }
}

struct Request<'a> {
    service: &'static AsrService,
    endpoint: &'a Endpoint,
    key: &'a str,
    context: &'a str,
}

async fn send_once(
    client: &reqwest::Client,
    request: &Request<'_>,
    wav: &[u8],
    duration_ms: f64,
) -> Result<Vec<Utterance>, Failure> {
    let Request {
        service,
        endpoint,
        key,
        context,
    } = *request;
    let base = endpoint.base_url.trim().trim_end_matches('/');
    let name = (service.name)();
    let unreadable = || Failure::Fatal(tr::asr::unreadable(name));
    let builder = match service.protocol {
        Protocol::DoubaoFlash => {
            let mut options = json!({ "model_name": endpoint.model, "enable_itn": true, "enable_punc": true, "show_utterances": true });
            if !context.is_empty() {
                options["corpus"] = json!({ "context": context });
            }
            let audio = base64::engine::general_purpose::STANDARD.encode(wav);
            client
                .post(format!("{base}/auc/bigmodel/recognize/flash"))
                .header("X-Api-Key", key)
                .header("X-Api-Resource-Id", DOUBAO_RESOURCE)
                .header("X-Api-Request-Id", uuid::Uuid::new_v4().to_string())
                .header("X-Api-Sequence", "-1")
                .json(&json!({ "user": { "uid": "capwise" }, "audio": { "data": audio }, "request": options }))
        }
        Protocol::OpenAiTranscriptions => {
            let segments = service.wants_segments(&endpoint.model);
            let file = reqwest::multipart::Part::bytes(wav.to_vec())
                .file_name("chunk.wav")
                .mime_str("audio/wav")
                .map_err(|error| Failure::Fatal(error.to_string()))?;
            let mut form = reqwest::multipart::Form::new()
                .text("model", endpoint.model.clone())
                .text("response_format", if segments { "verbose_json" } else { "json" });
            if segments {
                form = form.text("timestamp_granularities[]", "segment");
            }
            if !context.is_empty() {
                form = form.text("prompt", context.to_string());
            }
            let builder = client
                .post(format!("{base}/audio/transcriptions"))
                .multipart(form.part("file", file));
            if key.is_empty() { builder } else { builder.bearer_auth(key) }
        }
        Protocol::ChatAudio => {
            let audio = base64::engine::general_purpose::STANDARD.encode(wav);
            let builder = client.post(format!("{base}/chat/completions")).json(&json!({
                "model": endpoint.model,
                "messages": [{ "role": "user", "content": [{ "type": "input_audio", "input_audio": { "data": format!("data:audio/wav;base64,{audio}") } }] }],
                "asr_options": { "language": "auto" },
                "stream": false,
            }));
            if key.is_empty() { builder } else { builder.bearer_auth(key) }
        }
    };
    let response = builder
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|error| Failure::Retry(error.to_string()))?;
    if !response.status().is_success() {
        return Err(http_failure(service, response).await);
    }

    match service.protocol {
        Protocol::DoubaoFlash => {
            // Doubao answers 200 and reports the outcome in a header.
            let code = header(&response, "X-Api-Status-Code");
            let detail = header(&response, "X-Api-Message");
            if code == DOUBAO_SILENT {
                return Ok(Vec::new());
            }
            if !code.is_empty() && code != DOUBAO_OK {
                if quota_failure(&detail) {
                    return Err(Failure::Fatal(tr::asr::quota(name, &detail)));
                }
                if code.starts_with("4500001")
                    && mentions(&detail, &["auth", "key", "token", "permission", "resource", "unauthor"])
                {
                    return Err(Failure::Fatal(tr::asr::auth_failed(name)));
                }
                let message = tr::asr::failed(name, &code, &detail);
                return Err(if code.starts_with("55") {
                    Failure::Retry(message)
                } else {
                    Failure::Fatal(message)
                });
            }
            let parsed: FlashResponse = response.json().await.map_err(|_| unreadable())?;
            Ok(parsed.result.unwrap_or_default().utterances)
        }
        Protocol::OpenAiTranscriptions => {
            let parsed: TranscriptionResponse = response.json().await.map_err(|_| unreadable())?;
            match parsed.segments.filter(|segments| !segments.is_empty()) {
                Some(segments) => Ok(segments
                    .into_iter()
                    .map(|segment| Utterance {
                        start_time: segment.start * 1000.0,
                        end_time: segment.end.max(segment.start) * 1000.0,
                        text: tidy(&segment.text),
                        words: Vec::new(),
                    })
                    .filter(|utterance| !utterance.text.is_empty())
                    .collect()),
                None => Ok(spread_sentences(&tidy(&parsed.text), duration_ms)),
            }
        }
        Protocol::ChatAudio => {
            let value: Value = response.json().await.map_err(|_| unreadable())?;
            let text = value
                .pointer("/choices/0/message/content")
                .and_then(Value::as_str)
                .unwrap_or_default();
            Ok(spread_sentences(&tidy(text), duration_ms))
        }
    }
}

async fn recognize(
    client: &reqwest::Client,
    request: &Request<'_>,
    samples: &[f32],
) -> Result<Vec<Utterance>, String> {
    let wav = wav_bytes(samples);
    let duration_ms = samples.len() as f64 / SAMPLE_RATE as f64 * 1000.0;
    let mut last = String::new();
    for attempt in 0..=RETRIES {
        match send_once(client, request, &wav, duration_ms).await {
            Ok(utterances) => return Ok(utterances),
            Err(Failure::Fatal(message)) => return Err(message),
            Err(Failure::Retry(message)) => {
                last = message;
                if attempt < RETRIES {
                    tokio::time::sleep(backoff(attempt, None)).await;
                }
            }
        }
    }
    Err(tr::asr::connection_failed((request.service.name)(), &last))
}

// ---- words back onto the timeline -------------------------------------------

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xac00..=0xd7af | 0xf900..=0xfaff)
}

fn is_punctuation(c: char) -> bool {
    c.is_ascii_punctuation() || "，。！？；：、…—“”‘’（）《》【】「」".contains(c)
}

/// Clause ends where a caption line may break.
fn ends_clause(text: &str) -> bool {
    text.chars()
        .last()
        .is_some_and(|c| "，。！？；：、…,.!?;:".contains(c))
}

/// Soft punctuation a Chinese caption line drops at its end.
fn trim_line_end(text: &str) -> &str {
    text.trim_end_matches(['，', '。', '、', '；', '：'])
}

fn width(text: &str) -> f32 {
    text.chars()
        .filter(|c| !is_punctuation(*c))
        .map(|c| if is_cjk(c) { 1.0 } else { 0.55 })
        .sum()
}

fn join(words: &[CaptionWord]) -> String {
    cap_project::caption_text_from_words(words)
}

/// The utterance's words, with the punctuation the utterance text carries
/// between them attached to the word before it. Without word times, the text
/// is spread over the utterance by length, a character or a word at a time.
fn utterance_words(utterance: &Utterance, offset: f64) -> Vec<CaptionWord> {
    let seconds = |ms: f64| (offset + ms / 1000.0) as f32;
    let mut words: Vec<CaptionWord> = utterance
        .words
        .iter()
        .filter(|word| word.start_time >= 0.0 && !word.text.trim().is_empty())
        .map(|word| CaptionWord {
            text: word.text.trim().to_string(),
            start: seconds(word.start_time),
            end: seconds(word.end_time.max(word.start_time)),
        })
        .collect();

    if words.is_empty() {
        let mut pieces: Vec<String> = Vec::new();
        let mut after_space = true;
        for c in utterance.text.chars() {
            if c.is_whitespace() {
                after_space = true;
                continue;
            }
            match pieces.last_mut() {
                Some(last) if is_punctuation(c) => last.push(c),
                Some(last)
                    if !after_space
                        && !is_cjk(c)
                        && last.chars().last().is_some_and(|p| !is_cjk(p) && !is_punctuation(p)) =>
                {
                    last.push(c)
                }
                _ => pieces.push(c.to_string()),
            }
            after_space = false;
        }
        let total: f32 = pieces.iter().map(|piece| width(piece).max(0.5)).sum();
        let (start, end) = (seconds(utterance.start_time), seconds(utterance.end_time));
        let mut at = start;
        return pieces
            .into_iter()
            .map(|piece| {
                let span = (end - start) * width(&piece).max(0.5) / total.max(0.5);
                let word = CaptionWord { text: piece, start: at, end: at + span };
                at += span;
                word
            })
            .collect();
    }

    let mut rest = utterance.text.as_str();
    for index in 0..words.len() {
        let Some(found) = rest.find(words[index].text.as_str()) else {
            continue;
        };
        if index > 0 {
            let marks: String = rest[..found].chars().filter(|c| is_punctuation(*c)).collect();
            words[index - 1].text.push_str(&marks);
        }
        rest = &rest[found + words[index].text.len()..];
    }
    if let Some(last) = words.last_mut() {
        let marks: String = rest.chars().filter(|c| is_punctuation(*c)).collect();
        last.text.push_str(&marks);
    }
    words
}

/// Caption lines for one utterance: break after a clause once the line is long
/// enough to read, and before a word that would make it too long.
fn caption_lines(words: Vec<CaptionWord>) -> Vec<Vec<CaptionWord>> {
    let mut lines = Vec::new();
    let mut line: Vec<CaptionWord> = Vec::new();
    for word in words {
        if !line.is_empty() && width(&join(&line)) + width(&word.text) > LINE_MAX_WIDTH {
            lines.push(std::mem::take(&mut line));
        }
        let breaks = ends_clause(&word.text);
        line.push(word);
        if breaks && width(&join(&line)) >= LINE_MIN_WIDTH {
            lines.push(std::mem::take(&mut line));
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn segment_from_line(index: usize, mut words: Vec<CaptionWord>) -> CaptionSegment {
    if let Some(last) = words.last_mut() {
        let trimmed = trim_line_end(&last.text).to_string();
        if !trimmed.is_empty() {
            last.text = trimmed;
        }
    }
    CaptionSegment {
        id: format!("segment-{index}"),
        start: words.first().map_or(0.0, |word| word.start),
        end: words.last().map_or(0.0, |word| word.end),
        text: join(&words),
        words,
    }
}

fn segments_from(pieces: Vec<(f64, Vec<Utterance>)>) -> Vec<CaptionSegment> {
    pieces
        .into_iter()
        .flat_map(|(offset, utterances)| {
            utterances
                .into_iter()
                .flat_map(move |utterance| caption_lines(utterance_words(&utterance, offset)))
        })
        .filter(|line| !line.is_empty())
        .enumerate()
        .map(|(index, words)| segment_from_line(index, words))
        .collect()
}

// ---- the job -----------------------------------------------------------------

fn terms(hints: &[String]) -> impl Iterator<Item = &str> {
    hints.iter().map(|term| term.trim()).filter(|term| !term.is_empty())
}

/// Doubao's `corpus.context`: the names and terms the user asked to keep.
fn doubao_context(hints: &[String]) -> String {
    let mut text = String::new();
    for term in terms(hints) {
        if text.chars().count() + term.chars().count() + 1 > DOUBAO_CONTEXT_BUDGET {
            break;
        }
        if !text.is_empty() {
            text.push('、');
        }
        text.push_str(term);
    }
    if text.is_empty() {
        return String::new();
    }
    json!({ "context_type": "dialog_ctx", "context_data": [{ "text": text }] }).to_string()
}

/// Whisper reads only the end of its prompt, so the last terms are kept.
fn prompt_context(hints: &[String]) -> String {
    let joined = terms(hints).collect::<Vec<_>>().join(", ");
    let chars: Vec<char> = joined.chars().collect();
    chars[chars.len().saturating_sub(PROMPT_BUDGET)..].iter().collect()
}

fn context_for(service: &AsrService, hints: &[String]) -> String {
    match service.context {
        ContextMode::None => String::new(),
        ContextMode::Doubao => doubao_context(hints),
        ContextMode::Prompt => prompt_context(hints),
    }
}

/// Recognise 16 kHz mono audio with a configured cloud service. Must run on a
/// thread inside the tokio runtime that is allowed to block (`spawn_blocking`).
pub fn transcribe(
    id: &str,
    samples: &[f32],
    hints: &[String],
) -> Result<Vec<CaptionSegment>, String> {
    let service = service(id).ok_or_else(|| format!("Unknown recognition service {id}"))?;
    let endpoint = service.endpoint();
    let key = ai_store::key(Kind::Asr, id).unwrap_or_default();
    if service.needs_key && key.is_empty() {
        return Err(tr::asr::no_key((service.name)()));
    }
    let context = context_for(service, hints);
    let request = Request {
        service,
        endpoint: &endpoint,
        key: &key,
        context: &context,
    };
    let total = samples.len() as f64 / SAMPLE_RATE as f64;
    let chunks = plan_chunks(
        &silence_midpoints(samples),
        total,
        service.chunk_target,
        service.chunk_max,
    );
    tracing::info!(service = id, pieces = chunks.len(), seconds = total, "cloud transcription");
    let client = reqwest::Client::new();

    tokio::runtime::Handle::current().block_on(async {
        let results: Vec<Result<(f64, Vec<Utterance>), String>> =
            stream::iter(chunks.into_iter().map(|(start, end)| {
                let from = (start * SAMPLE_RATE as f64) as usize;
                let to = ((end * SAMPLE_RATE as f64) as usize).min(samples.len());
                let piece = &samples[from.min(to)..to];
                let (client, request) = (&client, &request);
                async move { recognize(client, request, piece).await.map(|found| (start, found)) }
            }))
            .buffered(CONCURRENCY)
            .collect()
            .await;
        Ok(segments_from(results.into_iter().collect::<Result<Vec<_>, _>>()?))
    })
}

/// Checks a service with two seconds of tone: any answer, even empty text,
/// means the address, key and model work. Returns the round trip in ms.
pub fn test(id: &str, key: &str, endpoint: &Endpoint) -> Result<u128, String> {
    let service = service(id).ok_or_else(|| format!("Unknown recognition service {id}"))?;
    if !ai_store::address_allowed(&endpoint.base_url) {
        return Err(tr::ai::address_invalid().to_string());
    }
    if endpoint.model.trim().is_empty() {
        return Err(tr::ai::model_missing().to_string());
    }
    if service.needs_key && key.trim().is_empty() {
        return Err(tr::asr::no_key((service.name)()));
    }
    let tone: Vec<f32> = (0..SAMPLE_RATE * 2)
        .map(|index| (index as f32 * 440.0 * std::f32::consts::TAU / SAMPLE_RATE as f32).sin() * 0.2)
        .collect();
    let key = key.trim().to_string();
    let request = Request {
        service,
        endpoint,
        key: &key,
        context: "",
    };
    let started = std::time::Instant::now();
    tokio::runtime::Handle::current()
        .block_on(async { recognize(&reqwest::Client::new(), &request, &tone).await })?;
    Ok(started.elapsed().as_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_cut_in_pauses_and_never_exceed_the_maximum() {
        let mids = [100.0, 290.0, 310.0, 600.0, 880.0];
        let chunks = plan_chunks(&mids, 1000.0, 300.0, 540.0);
        assert_eq!(chunks.first(), Some(&(0.0, 290.0)));
        assert!(chunks.iter().all(|(a, b)| b - a <= 540.0));
        assert_eq!(chunks.last().map(|c| c.1), Some(1000.0));
        assert_eq!(plan_chunks(&[], 120.0, 300.0, 540.0), vec![(0.0, 120.0)]);
    }

    #[test]
    fn pauses_are_found_relative_to_the_recording_level() {
        let mut samples = vec![0.3f32; SAMPLE_RATE];
        samples.extend(vec![0.0; SAMPLE_RATE / 2]);
        samples.extend(vec![0.3; SAMPLE_RATE]);
        let mids = silence_midpoints(&samples);
        assert_eq!(mids.len(), 1);
        assert!((mids[0] - 1.25).abs() < 0.05, "{mids:?}");
    }

    #[test]
    fn doubao_utterances_become_short_chinese_lines_with_word_times() {
        let raw = r#"{"result":{"text":"大家好，欢迎来到灵录的演示。","utterances":[
            {"start_time":0,"end_time":4000,"text":"大家好，欢迎来到灵录的演示。","words":[
                {"start_time":0,"end_time":200,"text":"大"},{"start_time":200,"end_time":400,"text":"家"},{"start_time":400,"end_time":600,"text":"好"},
                {"start_time":800,"end_time":1000,"text":"欢"},{"start_time":1000,"end_time":1200,"text":"迎"},{"start_time":1200,"end_time":1400,"text":"来"},
                {"start_time":1400,"end_time":1600,"text":"到"},{"start_time":1600,"end_time":1800,"text":"灵"},{"start_time":1800,"end_time":2000,"text":"录"},
                {"start_time":2000,"end_time":2200,"text":"的"},{"start_time":2200,"end_time":2400,"text":"演"},{"start_time":2400,"end_time":2600,"text":"示"}]}]}}"#;
        let parsed: FlashResponse = serde_json::from_str(raw).unwrap();
        let segments = segments_from(vec![(10.0, parsed.result.unwrap().utterances)]);
        let texts: Vec<&str> = segments.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["大家好，欢迎来到灵录的演示"]);
        assert_eq!(segments[0].words[2].text, "好，");
        assert!((segments[0].start - 10.0).abs() < 1e-3);
        assert!((segments[0].end - 12.6).abs() < 1e-3);
    }

    #[test]
    fn long_lines_break_at_clauses_and_english_keeps_spaces() {
        let word = |text: &str, i: usize| CaptionWord { text: text.into(), start: i as f32, end: i as f32 + 0.5 };
        let text = "我们今天先介绍一下这个产品的背景，然后再看具体的使用方法";
        let utterance = Utterance {
            start_time: 0.0,
            end_time: 20_000.0,
            text: text.into(),
            words: text
                .chars()
                .filter(|c| !is_punctuation(*c))
                .enumerate()
                .map(|(i, c)| Word { start_time: i as f64 * 500.0, end_time: i as f64 * 500.0 + 400.0, text: c.to_string() })
                .collect(),
        };
        let lines = caption_lines(utterance_words(&utterance, 0.0));
        assert!(lines.iter().all(|line| width(&join(line)) <= LINE_MAX_WIDTH));
        assert_eq!(join(&lines[0]), "我们今天先介绍一下这个产品的背景，");
        let english = vec![word("Hello", 0), word("world.", 1), word("Capwise", 2)];
        assert_eq!(join(&english), "Hello world. Capwise");
    }

    #[test]
    fn untimed_text_is_spread_over_the_piece_by_sentence_and_word() {
        let utterances = spread_sentences("第一句话。Second sentence here!", 10_000.0);
        assert_eq!(utterances.len(), 2);
        assert!((utterances[1].end_time - 10_000.0).abs() < 1.0);
        let segments = segments_from(vec![(5.0, utterances)]);
        assert_eq!(segments[0].text, "第一句话");
        assert_eq!(segments[1].text, "Second sentence here!");
        assert_eq!(segments[1].words.len(), 3);
        assert!(segments[1].end <= 15.0 + 1e-3);
    }

    #[test]
    fn openai_segments_keep_their_own_times() {
        let raw = r#"{"text":"a b","segments":[{"start":1.5,"end":3.0,"text":" Hello there. "}]}"#;
        let parsed: TranscriptionResponse = serde_json::from_str(raw).unwrap();
        let segment = &parsed.segments.unwrap()[0];
        assert_eq!(tidy(&segment.text), "Hello there.");
        assert_eq!(tidy("<|zh|><|NEUTRAL|>你好😊 世界"), "你好 世界");
    }

    #[test]
    fn context_follows_each_service() {
        let hints = vec!["乔木".to_string(), " ".into(), "Capwise".into()];
        let value: Value = serde_json::from_str(&doubao_context(&hints)).unwrap();
        assert_eq!(value["context_data"][0]["text"], "乔木、Capwise");
        assert_eq!(doubao_context(&[]), "");
        assert_eq!(prompt_context(&hints), "乔木, Capwise");
        let long: Vec<String> = (0..100).map(|i| format!("term{i}")).collect();
        assert_eq!(prompt_context(&long).chars().count(), PROMPT_BUDGET);
        assert!(prompt_context(&long).ends_with("term99"));
    }

    #[test]
    fn catalogue_ids_are_unique_and_never_a_local_model_name() {
        let mut ids: Vec<&str> = SERVICES.iter().map(|service| service.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), SERVICES.len());
        for local in ["best", "best-max", "small", "medium"] {
            assert!(!is_cloud_model(local));
        }
        assert_eq!(service("glm").unwrap().chunk_max, 28.0);
        assert_eq!(service("mimo").unwrap().protocol, Protocol::ChatAudio);
        assert!(!service("local-server").unwrap().needs_key);
    }
}
