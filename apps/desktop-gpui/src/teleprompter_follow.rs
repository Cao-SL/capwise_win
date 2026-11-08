use std::{collections::VecDeque, ops::Range, time::Duration};

use futures_util::{Sink, SinkExt, Stream, StreamExt};
use serde::Deserialize;
use tokio::{
    sync::{mpsc, oneshot},
    time::Instant,
};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message, client::IntoClientRequest, protocol::WebSocketConfig},
};

pub const ENDPOINT: &str = "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async";
pub const SAMPLE_RATE: u32 = 16_000;
pub const CHUNK_SAMPLES: usize = 3_200;
const MAX_FRAME: usize = 1024 * 1024;
const SEND_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const IDLE_TIMEOUT: Duration = Duration::from_secs(45);
const FINISH_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_AUDIO_AGE: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FollowError {
    InvalidKey,
    InvalidAudio,
    StaleAudio,
    AudioBackpressure,
    AudioClosed,
    InvalidAudioClock,
    NoAudio,
    InvalidScript,
    Protocol(&'static str),
    Server { code: u32, reason: ServerFailure },
    Http { status: u16 },
    Network,
    Timeout(&'static str),
    Disconnected,
    ConsumerClosed,
    ConsumerLagged,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerFailure {
    InvalidRequest,
    EmptyAudio,
    AudioTimeout,
    AudioFormat,
    Busy,
    Internal,
    Rejected,
}

fn server_error(code: u32) -> FollowError {
    let reason = match code {
        45_000_001 => ServerFailure::InvalidRequest,
        45_000_002 => ServerFailure::EmptyAudio,
        45_000_081 => ServerFailure::AudioTimeout,
        45_000_151 => ServerFailure::AudioFormat,
        55_000_031 => ServerFailure::Busy,
        55_000_000..=55_099_999 => ServerFailure::Internal,
        _ => ServerFailure::Rejected,
    };
    FollowError::Server { code, reason }
}

fn network_error(error: tokio_tungstenite::tungstenite::Error) -> FollowError {
    match error {
        tokio_tungstenite::tungstenite::Error::Http(response) => FollowError::Http {
            status: response.status().as_u16(),
        },
        _ => FollowError::Network,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FollowGeneration(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionEvent {
    pub generation: FollowGeneration,
    pub event: StreamingEvent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BillingResource {
    Duration,
    Concurrent,
}

impl BillingResource {
    fn id(self) -> &'static str {
        match self {
            Self::Duration => "volc.seedasr.sauc.duration",
            Self::Concurrent => "volc.seedasr.sauc.concurrent",
        }
    }
}

pub struct StreamingConfig {
    api_key: String,
    pub resource: BillingResource,
}

impl StreamingConfig {
    pub fn new(api_key: String, resource: BillingResource) -> Result<Self, FollowError> {
        if api_key.trim().is_empty()
            || api_key.len() > 4096
            || !api_key.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
        {
            return Err(FollowError::InvalidKey);
        }
        Ok(Self { api_key, resource })
    }
}

#[derive(Debug)]
pub struct PcmChunk {
    samples: Vec<i16>,
    captured_at: Instant,
}

impl PcmChunk {
    pub fn captured_16khz_mono(
        samples: Vec<i16>,
        captured_at: std::time::Instant,
    ) -> Result<Self, FollowError> {
        if samples.is_empty() || samples.len() > CHUNK_SAMPLES {
            return Err(FollowError::InvalidAudio);
        }
        let chunk = Self {
            samples,
            captured_at: Instant::from_std(captured_at),
        };
        chunk.validate_age(Instant::now())?;
        Ok(chunk)
    }

    fn validate_age(&self, now: Instant) -> Result<(), FollowError> {
        let age = now
            .checked_duration_since(self.captured_at)
            .ok_or(FollowError::InvalidAudioClock)?;
        if age > MAX_AUDIO_AGE {
            return Err(FollowError::StaleAudio);
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum AudioInput {
    Pcm(PcmChunk),
    Finish,
}

pub fn audio_channel() -> (mpsc::Sender<AudioInput>, mpsc::Receiver<AudioInput>) {
    mpsc::channel(4)
}

pub fn try_enqueue_audio(
    sender: &mpsc::Sender<AudioInput>,
    input: AudioInput,
) -> Result<(), FollowError> {
    if let AudioInput::Pcm(chunk) = &input {
        chunk.validate_age(Instant::now())?;
    }
    sender.try_send(input).map_err(|error| match error {
        mpsc::error::TrySendError::Full(_) => FollowError::AudioBackpressure,
        mpsc::error::TrySendError::Closed(_) => FollowError::AudioClosed,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecognitionUpdate {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub definite: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecognitionPacket {
    pub sequence: Option<i32>,
    pub final_packet: bool,
    pub updates: Vec<RecognitionUpdate>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamingEvent {
    Connected { request_id: String },
    Ready,
    Recognition(RecognitionPacket),
    Finished,
}

pub fn event_channel() -> (mpsc::Sender<SessionEvent>, mpsc::Receiver<SessionEvent>) {
    mpsc::channel(16)
}

fn packet(message_type: u8, flags: u8, serialization: u8, payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0x11, message_type << 4 | flags, serialization << 4, 0];
    bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

pub fn initialization_packet() -> Vec<u8> {
    let body = serde_json::json!({
        "user": { "uid": "capwise-prompter" },
        "audio": { "format": "pcm", "codec": "raw", "rate": SAMPLE_RATE, "bits": 16, "channel": 1 },
        "request": { "model_name": "bigmodel", "show_utterances": true, "result_type": "single", "enable_nonstream": true, "enable_itn": false, "enable_punc": true, "enable_ddc": false }
    }).to_string();
    packet(1, 0, 1, body.as_bytes())
}

pub fn audio_packet(samples: &[i16], final_packet: bool) -> Result<Vec<u8>, FollowError> {
    if samples.len() > CHUNK_SAMPLES || (samples.is_empty() && !final_packet) {
        return Err(FollowError::InvalidAudio);
    }
    let payload: Vec<_> = samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect();
    Ok(packet(2, if final_packet { 2 } else { 0 }, 0, &payload))
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, FollowError> {
    let next = cursor
        .checked_add(4)
        .ok_or(FollowError::Protocol("integer overflow"))?;
    let value = bytes
        .get(*cursor..next)
        .ok_or(FollowError::Protocol("truncated integer"))?;
    *cursor = next;
    Ok(u32::from_be_bytes(
        value
            .try_into()
            .map_err(|_| FollowError::Protocol("invalid integer"))?,
    ))
}

#[derive(Deserialize)]
struct WireUtterance {
    start_time: u64,
    end_time: u64,
    text: String,
    #[serde(default)]
    definite: bool,
}

#[derive(Deserialize)]
struct WireResult {
    #[serde(default)]
    utterances: Vec<WireUtterance>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum WireResults {
    One(WireResult),
    Many(Vec<WireResult>),
}

#[derive(Deserialize)]
struct WireResponse {
    code: Option<u32>,
    result: Option<WireResults>,
    error: Option<WireError>,
}

#[derive(Deserialize)]
struct WireError {
    code: u32,
}

pub fn decode_packet(bytes: &[u8]) -> Result<RecognitionPacket, FollowError> {
    if bytes.len() > MAX_FRAME {
        return Err(FollowError::Protocol("response too large"));
    }
    let header = bytes
        .get(..4)
        .ok_or(FollowError::Protocol("truncated header"))?;
    if header[0] >> 4 != 1 || header[0] & 15 == 0 {
        return Err(FollowError::Protocol("unsupported header"));
    }
    let mut cursor = usize::from(header[0] & 15) * 4;
    if cursor > bytes.len() {
        return Err(FollowError::Protocol("truncated extended header"));
    }
    let kind = header[1] >> 4;
    let flags = header[1] & 15;
    if !matches!(kind, 9 | 15) || flags > 3 {
        return Err(FollowError::Protocol("unexpected message type or flags"));
    }
    if header[2] & 15 != 0 {
        return Err(FollowError::Protocol("unnegotiated compression"));
    }
    if (kind == 9 && header[2] >> 4 != 1) || header[2] >> 4 > 1 {
        return Err(FollowError::Protocol("expected JSON result"));
    }
    let sequence = if kind == 9 && flags & 1 != 0 {
        Some(read_u32(bytes, &mut cursor)? as i32)
    } else {
        None
    };
    if flags == 1 && sequence.is_some_and(|sequence| sequence < 0) {
        return Err(FollowError::Protocol("negative nonfinal sequence"));
    }
    let error_code = if kind == 15 {
        Some(read_u32(bytes, &mut cursor)?)
    } else {
        None
    };
    let length = read_u32(bytes, &mut cursor)? as usize;
    if length > MAX_FRAME || length != bytes.len().saturating_sub(cursor) {
        return Err(FollowError::Protocol("invalid payload size"));
    }
    let payload = &bytes[cursor..];
    if let Some(code) = error_code {
        std::str::from_utf8(payload).map_err(|_| FollowError::Protocol("invalid error UTF-8"))?;
        return Err(server_error(code));
    }
    let response: WireResponse = serde_json::from_slice(payload)
        .map_err(|_| FollowError::Protocol("invalid recognition JSON"))?;
    if let Some(error) = response.error {
        return Err(server_error(error.code));
    }
    if let Some(code) = response.code.filter(|code| !matches!(code, 0 | 20_000_000)) {
        return Err(server_error(code));
    }
    let results = match response.result {
        None => Vec::new(),
        Some(WireResults::One(result)) => vec![result],
        Some(WireResults::Many(results)) => results,
    };
    let mut updates = Vec::new();
    for utterance in results.into_iter().flat_map(|result| result.utterances) {
        if updates.len() >= 128
            || utterance.end_time < utterance.start_time
            || utterance.text.chars().count() > 20_000
        {
            return Err(FollowError::Protocol("invalid utterance"));
        }
        updates.push(RecognitionUpdate {
            start_ms: utterance.start_time,
            end_ms: utterance.end_time,
            text: utterance.text,
            definite: utterance.definite,
        });
    }
    Ok(RecognitionPacket {
        sequence,
        final_packet: flags & 2 != 0,
        updates,
    })
}

fn emit(
    events: &mpsc::Sender<SessionEvent>,
    generation: FollowGeneration,
    event: StreamingEvent,
) -> Result<(), FollowError> {
    events
        .try_send(SessionEvent { generation, event })
        .map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => FollowError::ConsumerLagged,
            mpsc::error::TrySendError::Closed(_) => FollowError::ConsumerClosed,
        })
}

async fn send<S>(socket: &mut S, message: Message) -> Result<(), FollowError>
where
    S: Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    tokio::time::timeout(SEND_TIMEOUT, socket.send(message))
        .await
        .map_err(|_| FollowError::Timeout("send"))?
        .map_err(network_error)
}

pub async fn run_session(
    generation: FollowGeneration,
    config: StreamingConfig,
    audio: mpsc::Receiver<AudioInput>,
    events: mpsc::Sender<SessionEvent>,
    cancel: oneshot::Receiver<()>,
) -> Result<(), FollowError> {
    cancellable(run_connected(generation, config, audio, events), cancel).await
}

async fn cancellable(
    future: impl std::future::Future<Output = Result<(), FollowError>>,
    cancel: oneshot::Receiver<()>,
) -> Result<(), FollowError> {
    tokio::select! {
        biased;
        _ = cancel => Err(FollowError::Cancelled),
        result = future => result,
    }
}

async fn run_connected(
    generation: FollowGeneration,
    config: StreamingConfig,
    audio: mpsc::Receiver<AudioInput>,
    events: mpsc::Sender<SessionEvent>,
) -> Result<(), FollowError> {
    let mut request = ENDPOINT
        .into_client_request()
        .map_err(|_| FollowError::Protocol("invalid endpoint"))?;
    let request_id = uuid::Uuid::new_v4().to_string();
    for (name, value) in [
        ("x-api-key", config.api_key.as_str()),
        ("x-api-resource-id", config.resource.id()),
        ("x-api-request-id", request_id.as_str()),
        ("x-api-sequence", "-1"),
    ] {
        let mut value = value
            .parse::<tokio_tungstenite::tungstenite::http::HeaderValue>()
            .map_err(|_| FollowError::InvalidKey)?;
        value.set_sensitive(name == "x-api-key");
        request.headers_mut().insert(name, value);
    }
    let websocket_config = WebSocketConfig {
        max_message_size: Some(MAX_FRAME),
        max_frame_size: Some(MAX_FRAME),
        write_buffer_size: 0,
        max_write_buffer_size: MAX_FRAME * 2,
        ..Default::default()
    };
    let (mut socket, _) = tokio::time::timeout(
        CONNECT_TIMEOUT,
        connect_async_with_config(request, Some(websocket_config), true),
    )
    .await
    .map_err(|_| FollowError::Timeout("connect"))?
    .map_err(network_error)?;
    send(&mut socket, Message::Binary(initialization_packet())).await?;
    emit(
        &events,
        generation,
        StreamingEvent::Connected { request_id },
    )?;
    pump(socket, audio, events, generation).await
}

async fn pump<S>(
    mut socket: S,
    mut audio: mpsc::Receiver<AudioInput>,
    events: mpsc::Sender<SessionEvent>,
    generation: FollowGeneration,
) -> Result<(), FollowError>
where
    S: Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    let initialized_at = Instant::now();
    let mut ready = false;
    let mut last_capture = None;
    let mut received_at = Instant::now();
    let mut finished_at = None;
    let mut sent_audio = false;
    let mut next_audio_at = Instant::now();
    let mut ping = tokio::time::interval_at(
        Instant::now() + Duration::from_secs(15),
        Duration::from_secs(15),
    );
    loop {
        let finish_deadline = finished_at.unwrap_or(Instant::now()) + FINISH_TIMEOUT;
        tokio::select! {
            _ = tokio::time::sleep_until(initialized_at + CONNECT_TIMEOUT), if !ready => return Err(FollowError::Timeout("initialization")),
            _ = tokio::time::sleep_until(received_at + IDLE_TIMEOUT) => return Err(FollowError::Timeout("response")),
            _ = tokio::time::sleep_until(finish_deadline), if finished_at.is_some() => return Err(FollowError::Timeout("final result")),
            _ = ping.tick() => send(&mut socket, Message::Ping(Vec::new())).await?,
            input = async { tokio::time::sleep_until(next_audio_at).await; audio.recv().await }, if ready && finished_at.is_none() => {
                match input {
                    Some(AudioInput::Pcm(chunk)) => {
                        chunk.validate_age(Instant::now())?;
                        if last_capture.is_some_and(|last| chunk.captured_at <= last) {
                            return Err(FollowError::InvalidAudioClock);
                        }
                        last_capture = Some(chunk.captured_at);
                        send(&mut socket, Message::Binary(audio_packet(&chunk.samples, false)?)).await?;
                        sent_audio = true;
                        next_audio_at = Instant::now() + Duration::from_secs_f64(chunk.samples.len() as f64 / f64::from(SAMPLE_RATE));
                    }
                    Some(AudioInput::Finish) | None => {
                        if !sent_audio { return Err(FollowError::NoAudio); }
                        send(&mut socket, Message::Binary(audio_packet(&[], true)?)).await?;
                        finished_at = Some(Instant::now());
                    }
                }
            }
            message = socket.next() => {
                received_at = Instant::now();
                match message {
                    Some(Ok(Message::Binary(bytes))) => {
                        let packet = decode_packet(&bytes)?;
                        let final_packet = packet.final_packet;
                        if final_packet && finished_at.is_none() { return Err(FollowError::Protocol("unexpected final result")); }
                        if !ready {
                            // The official sauc SDK awaits this full-server ACK before its audio loop; the payload may be empty.
                            if !packet.updates.is_empty() {
                                return Err(FollowError::Protocol("recognition before audio"));
                            }
                            ready = true;
                            emit(&events, generation, StreamingEvent::Ready)?;
                            continue;
                        }
                        emit(&events, generation, StreamingEvent::Recognition(packet))?;
                        if final_packet {
                            emit(&events, generation, StreamingEvent::Finished)?;
                            let _ = tokio::time::timeout(SEND_TIMEOUT, socket.close()).await;
                            return Ok(());
                        }
                    }
                    Some(Ok(Message::Ping(bytes))) => send(&mut socket, Message::Pong(bytes)).await?,
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) | None => return Err(FollowError::Disconnected),
                    Some(Ok(_)) => return Err(FollowError::Protocol("expected binary websocket message")),
                    Some(Err(error)) => return Err(network_error(error)),
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScriptAnchor {
    pub sentence: usize,
    pub bytes: Range<usize>,
    pub confidence: f32,
    pub read_offset: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StayReason {
    Unmatched,
    Ambiguous,
    AwaitingConfirmation,
    Duplicate,
    ManualHold,
    StaleGeneration,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FollowDecision {
    Stay(StayReason),
    Candidate(ScriptAnchor),
    Follow(ScriptAnchor),
    Move(ScriptAnchor),
}

struct Sentence {
    bytes: Range<usize>,
    tokens: Vec<ScriptToken>,
}

#[derive(Clone, Debug)]
struct ScriptToken {
    value: String,
    bytes: Range<usize>,
}

impl ScriptToken {
    fn protected(&self) -> bool {
        self.value.starts_with('#')
            || matches!(
                self.value.as_str(),
                "不" | "没"
                    | "沒"
                    | "无"
                    | "無"
                    | "未"
                    | "别"
                    | "別"
                    | "勿"
                    | "no"
                    | "not"
                    | "never"
                    | "cannot"
                    | "can't"
                    | "don't"
                    | "doesn't"
                    | "isn't"
                    | "won't"
            )
    }
}

fn digit(character: char) -> Option<u32> {
    match character {
        '零' | '〇' => Some(0),
        '一' => Some(1),
        '二' | '两' | '兩' => Some(2),
        '三' => Some(3),
        '四' => Some(4),
        '五' => Some(5),
        '六' => Some(6),
        '七' => Some(7),
        '八' => Some(8),
        '九' => Some(9),
        _ if character.is_ascii_digit() => character.to_digit(10),
        _ => None,
    }
}

fn integer(text: &str) -> Option<String> {
    if text.is_empty() || text.chars().count() > 16 {
        return None;
    }
    if text.chars().all(|c| digit(c).is_some()) {
        let value: String = text
            .chars()
            .filter_map(digit)
            .filter_map(|d| char::from_digit(d, 10))
            .collect();
        let value = value.trim_start_matches('0');
        return Some(if value.is_empty() { "0" } else { value }.into());
    }
    let mut total = 0u64;
    let mut section = 0u64;
    let mut number = 0u64;
    let mut previous_unit = 10_000u64;
    let mut had_digit = false;
    let mut explicit_zero = false;
    for character in text.chars() {
        if let Some(value) = digit(character) {
            if had_digit && number != 0 {
                return None;
            }
            number = u64::from(value);
            explicit_zero |= value == 0;
            had_digit = true;
        } else {
            let unit = match character {
                '十' => 10,
                '百' => 100,
                '千' => 1000,
                '万' | '萬' => 10_000,
                _ => return None,
            };
            if unit == 10_000 {
                let value = section.checked_add(number)?;
                if total != 0 || value == 0 {
                    return None;
                }
                total = value.checked_mul(unit)?;
                section = 0;
                previous_unit = 10_000;
            } else {
                if unit >= previous_unit || (!had_digit && unit != 10) {
                    return None;
                }
                section = section.checked_add(number.max(1).checked_mul(unit)?)?;
                previous_unit = unit;
            }
            number = 0;
            had_digit = false;
            explicit_zero = false;
        }
    }
    if number > 0 && previous_unit >= 100 && !explicit_zero {
        return None;
    }
    Some(total.checked_add(section)?.checked_add(number)?.to_string())
}

fn number_at(text: &str) -> Option<(String, usize)> {
    let sign = text
        .chars()
        .next()
        .filter(|c| matches!(c, '-' | '−' | '负' | '負' | '+'));
    let sign_length = sign.map(char::len_utf8).unwrap_or(0);
    let text = &text[sign_length..];
    let (prefix, rest) = if let Some(rest) = text.strip_prefix("百分之") {
        ("百分之".len(), rest)
    } else {
        (0, text)
    };
    if !rest
        .chars()
        .next()
        .is_some_and(|c| digit(c).is_some() || c == '十')
    {
        return None;
    }
    let length = rest
        .char_indices()
        .take_while(|(_, c)| {
            digit(*c).is_some() || matches!(c, '十' | '百' | '千' | '万' | '萬' | '.' | '点' | '點')
        })
        .last()
        .map(|(i, c)| i + c.len_utf8())?;
    let mut raw = &rest[..length];
    // A sentence-ending period is punctuation, while a decimal needs digits on both sides.
    raw = raw.trim_end_matches('.');
    let mut parts = raw.split(['.', '点', '點']);
    let whole = integer(parts.next()?)?;
    let fraction = parts.next();
    if parts.next().is_some() {
        return None;
    }
    let value = if let Some(fraction) = fraction {
        if fraction.is_empty() || !fraction.chars().all(|c| digit(c).is_some()) {
            return None;
        }
        let fraction: String = fraction
            .chars()
            .filter_map(digit)
            .filter_map(|d| char::from_digit(d, 10))
            .collect();
        let fraction = fraction.trim_end_matches('0');
        if fraction.is_empty() {
            whole
        } else {
            format!("{whole}.{fraction}")
        }
    } else {
        whole
    };
    let suffix = rest[raw.len()..]
        .chars()
        .next()
        .filter(|c| matches!(c, '%' | '％'));
    let percent = prefix > 0 || suffix.is_some();
    Some((
        format!(
            "#{}:{}{value}",
            if percent { "percent" } else { "number" },
            if sign.is_some_and(|c| c != '+') {
                "-"
            } else {
                ""
            }
        ),
        sign_length + prefix + raw.len() + suffix.map(char::len_utf8).unwrap_or(0),
    ))
}

fn tokens(text: &str, base: usize) -> Vec<ScriptToken> {
    let mut tokens = Vec::new();
    let mut offset = 0;
    while offset < text.len() {
        let rest = &text[offset..];
        let character = rest.chars().next().expect("nonempty remainder");
        if let Some((value, length)) = number_at(rest) {
            tokens.push(ScriptToken {
                value,
                bytes: base + offset..base + offset + length,
            });
            offset += length;
        } else if matches!(character as u32, 0x3400..=0x9fff | 0xf900..=0xfaff | 0x20000..=0x323af)
        {
            tokens.push(ScriptToken {
                value: character.to_string(),
                bytes: base + offset..base + offset + character.len_utf8(),
            });
            offset += character.len_utf8();
        } else if character.is_alphanumeric() {
            let length = rest.char_indices().take_while(|(_, c)| c.is_alphanumeric() && !matches!(*c as u32, 0x3400..=0x9fff | 0xf900..=0xfaff | 0x20000..=0x323af) || matches!(c, '\'' | '’'))
                .last().map(|(i, c)| i + c.len_utf8()).unwrap_or(character.len_utf8());
            tokens.push(ScriptToken {
                value: rest[..length].to_lowercase().replace('’', "'"),
                bytes: base + offset..base + offset + length,
            });
            offset += length;
        } else {
            offset += character.len_utf8();
        }
    }
    tokens
}

fn aligned_tokens(spoken: &[ScriptToken], script: &[ScriptToken]) -> Option<(f32, usize)> {
    let width = script.len() + 1;
    let mut matrix = vec![0u16; (spoken.len() + 1) * width];
    for (i, first) in spoken.iter().enumerate() {
        for (j, second) in script.iter().enumerate() {
            matrix[(i + 1) * width + j + 1] = if first.value == second.value {
                matrix[i * width + j] + 1
            } else {
                matrix[i * width + j + 1].max(matrix[(i + 1) * width + j])
            };
        }
    }
    let matched = usize::from(matrix[spoken.len() * width + script.len()]);
    let confidence = matched as f32 / spoken.len() as f32;
    if matched < 3 || confidence < 0.85 {
        return None;
    }
    let (mut i, mut j) = (spoken.len(), script.len());
    let mut pairs = Vec::with_capacity(matched);
    while i > 0 && j > 0 {
        if spoken[i - 1].value == script[j - 1].value {
            pairs.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if matrix[(i - 1) * width + j] >= matrix[i * width + j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    pairs.reverse();
    let first = pairs.first()?.1;
    let last = pairs.last()?.1;
    if last - first + 1 > matched + 2 {
        return None;
    }
    if spoken
        .iter()
        .enumerate()
        .any(|(i, token)| token.protected() && !pairs.iter().any(|(a, _)| *a == i))
    {
        return None;
    }
    let context_start = first.saturating_sub(1);
    if script[context_start..=last]
        .iter()
        .enumerate()
        .any(|(j, token)| token.protected() && !pairs.iter().any(|(_, b)| *b == j + context_start))
    {
        return None;
    }
    Some((confidence, script[last].bytes.start))
}

struct PartialMatch {
    utterance: u64,
    sentence: usize,
    values: Vec<String>,
    since: Instant,
    end_ms: u64,
}

pub struct ScriptFollower {
    sentences: Vec<Sentence>,
    current: usize,
    pending_jump: Option<usize>,
    completed: VecDeque<u64>,
    manual_hold: bool,
    latest_seen: Option<u64>,
    ignore_through: Option<u64>,
    generation: Option<FollowGeneration>,
    active: bool,
    partial: Option<PartialMatch>,
}

impl ScriptFollower {
    pub fn new(script: &str) -> Result<Self, FollowError> {
        if script.len() > 100_000 {
            return Err(FollowError::InvalidScript);
        }
        let mut sentences = Vec::new();
        let mut start = 0;
        for (offset, character) in script.char_indices() {
            if matches!(
                character,
                '。' | '！' | '？' | '.' | '!' | '?' | '\n' | '；' | ';'
            ) && !(character == '.'
                && offset > 0
                && script[..offset]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_digit())
                && script[offset + 1..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit()))
            {
                let end = offset + character.len_utf8();
                let tokens = tokens(&script[start..end], start);
                if !tokens.is_empty() {
                    sentences.push(Sentence {
                        bytes: start..end,
                        tokens,
                    });
                }
                start = end;
            }
        }
        if start < script.len() {
            let tokens = tokens(&script[start..], start);
            if !tokens.is_empty() {
                sentences.push(Sentence {
                    bytes: start..script.len(),
                    tokens,
                });
            }
        }
        if sentences.is_empty()
            || sentences.len() > 1024
            || sentences.iter().any(|sentence| sentence.tokens.len() > 256)
        {
            return Err(FollowError::InvalidScript);
        }
        Ok(Self {
            sentences,
            current: 0,
            pending_jump: None,
            completed: VecDeque::new(),
            manual_hold: false,
            latest_seen: None,
            ignore_through: None,
            generation: None,
            active: false,
            partial: None,
        })
    }

    pub fn begin_session(&mut self, generation: FollowGeneration) -> Result<(), FollowError> {
        if self.generation.is_some_and(|current| generation <= current) {
            return Err(FollowError::Protocol("generation must increase"));
        }
        self.generation = Some(generation);
        self.active = true;
        self.manual_hold = false;
        self.completed.clear();
        self.latest_seen = None;
        self.ignore_through = None;
        self.pending_jump = None;
        self.partial = None;
        Ok(())
    }

    pub fn cancel_session(&mut self) {
        self.active = false;
        self.partial = None;
        self.pending_jump = None;
    }

    pub fn replace_script(
        &mut self,
        script: &str,
        generation: FollowGeneration,
    ) -> Result<(), FollowError> {
        if self.generation.is_some_and(|current| generation <= current) {
            return Err(FollowError::Protocol("generation must increase"));
        }
        let mut replacement = Self::new(script)?;
        replacement.begin_session(generation)?;
        *self = replacement;
        Ok(())
    }

    pub fn update_for(
        &mut self,
        generation: FollowGeneration,
        update: &RecognitionUpdate,
    ) -> FollowDecision {
        if !self.active || self.generation != Some(generation) {
            return FollowDecision::Stay(StayReason::StaleGeneration);
        }
        self.update(update)
    }

    pub fn sentence_at_byte(&self, offset: usize) -> usize {
        self.sentences
            .iter()
            .position(|sentence| offset < sentence.bytes.end)
            .unwrap_or(self.sentences.len() - 1)
    }

    pub fn current_sentence(&self) -> usize {
        self.current
    }

    pub fn suspend(&mut self) {
        self.manual_hold = true;
        self.pending_jump = None;
        self.partial = None;
    }

    pub fn resume_at(&mut self, sentence: usize) -> Result<(), FollowError> {
        if sentence >= self.sentences.len() {
            return Err(FollowError::InvalidScript);
        }
        self.current = sentence;
        self.manual_hold = false;
        self.ignore_through = self.latest_seen;
        self.pending_jump = None;
        self.partial = None;
        Ok(())
    }

    fn update(&mut self, update: &RecognitionUpdate) -> FollowDecision {
        let older_utterance = self.latest_seen.is_some_and(|last| update.start_ms < last);
        self.latest_seen = Some(self.latest_seen.unwrap_or(0).max(update.start_ms));
        if self.manual_hold {
            return FollowDecision::Stay(StayReason::ManualHold);
        }
        if older_utterance
            || self.completed.contains(&update.start_ms)
            || self
                .completed
                .back()
                .is_some_and(|last| update.start_ms < *last)
            || self
                .ignore_through
                .is_some_and(|last| update.start_ms <= last)
        {
            return FollowDecision::Stay(StayReason::Duplicate);
        }
        if update.text.len() > 20_000 || update.end_ms < update.start_ms {
            self.partial = None;
            return FollowDecision::Stay(StayReason::Unmatched);
        }
        let spoken = tokens(&update.text, 0);
        if spoken.len() < 3 || spoken.len() > 128 {
            self.pending_jump = None;
            self.partial = None;
            return FollowDecision::Stay(StayReason::Unmatched);
        }
        if update.definite {
            self.completed.push_back(update.start_ms);
            if self.completed.len() > 64 {
                self.completed.pop_front();
            }
        }
        let mut scored: Vec<_> = self
            .sentences
            .iter()
            .enumerate()
            .filter_map(|(index, sentence)| {
                let (confidence, read_offset) = aligned_tokens(&spoken, &sentence.tokens)?;
                Some((index, confidence, read_offset))
            })
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        let Some(&(_, best_score, _)) = scored.first() else {
            self.pending_jump = None;
            self.partial = None;
            return FollowDecision::Stay(StayReason::Unmatched);
        };
        scored.retain(|(_, score, _)| *score >= best_score - 0.05);
        if scored.len() > 1 {
            let nearby: Vec<_> = scored
                .iter()
                .copied()
                .filter(|(index, _, _)| index.abs_diff(self.current) <= 2)
                .collect();
            if nearby.len() != 1 {
                self.pending_jump = None;
                self.partial = None;
                return FollowDecision::Stay(StayReason::Ambiguous);
            }
            scored = nearby;
        }
        let (sentence, confidence, read_offset) = scored[0];
        let anchor = ScriptAnchor {
            sentence,
            bytes: self.sentences[sentence].bytes.clone(),
            confidence,
            read_offset,
        };
        if !update.definite {
            let values: Vec<_> = spoken.iter().map(|token| token.value.clone()).collect();
            let previous = self.partial.take().filter(|previous| {
                previous.utterance == update.start_ms
                    && previous.sentence == sentence
                    && values.starts_with(&previous.values)
                    && update.end_ms >= previous.end_ms
            });
            let since = previous
                .as_ref()
                .map(|previous| previous.since)
                .unwrap_or_else(Instant::now);
            let stable = previous.as_ref().is_some_and(|previous| {
                (update.end_ms > previous.end_ms || values.len() > previous.values.len())
                    && Instant::now().saturating_duration_since(since) >= Duration::from_millis(180)
            });
            self.partial = Some(PartialMatch {
                utterance: update.start_ms,
                sentence,
                values,
                since,
                end_ms: update.end_ms,
            });
            return if stable
                && spoken.len() >= 4
                && confidence >= 0.93
                && sentence.abs_diff(self.current) <= 1
            {
                FollowDecision::Follow(anchor)
            } else {
                FollowDecision::Candidate(anchor)
            };
        }
        self.partial = None;
        let near = sentence.abs_diff(self.current) <= 2;
        let confirmed = self
            .pending_jump
            .is_some_and(|pending| sentence == pending + 1);
        if !near && !confirmed {
            self.pending_jump = Some(sentence);
            return FollowDecision::Stay(StayReason::AwaitingConfirmation);
        }
        self.current = sentence;
        self.pending_jump = None;
        FollowDecision::Move(anchor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GENERATION: FollowGeneration = FollowGeneration(7);

    fn pcm(samples: Vec<i16>) -> Result<PcmChunk, FollowError> {
        PcmChunk::captured_16khz_mono(samples, Instant::now().into_std())
    }

    async fn pump(
        socket: MockSocket,
        audio: mpsc::Receiver<AudioInput>,
        events: mpsc::Sender<SessionEvent>,
    ) -> Result<(), FollowError> {
        super::pump(socket, audio, events, GENERATION).await
    }

    fn update(start_ms: u64, text: &str, definite: bool) -> RecognitionUpdate {
        RecognitionUpdate {
            start_ms,
            end_ms: start_ms + 800,
            text: text.into(),
            definite,
        }
    }

    fn server_packet(flags: u8, sequence: Option<i32>, body: serde_json::Value) -> Vec<u8> {
        let body = body.to_string();
        let mut bytes = vec![0x11, 0x90 | flags, 0x10, 0];
        if let Some(sequence) = sequence {
            bytes.extend_from_slice(&sequence.to_be_bytes());
        }
        bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
        bytes.extend_from_slice(body.as_bytes());
        bytes
    }

    fn script() -> &'static str {
        "今天介绍录屏编辑。\n首先打开录制窗口。\n然后选择屏幕区域。\n接下来检查麦克风。\n调整相机画面大小。\n按下按钮开始录制。\n录制结束生成字幕。\n最后导出完整视频。"
    }

    fn moved(decision: FollowDecision, expected: usize) {
        let FollowDecision::Move(anchor) = decision else {
            panic!("expected a confirmed move, got {decision:?}");
        };
        assert_eq!(anchor.sentence, expected);
        assert!(anchor.confidence >= 0.85);
    }

    #[test]
    fn reads_ahead_and_rereads_without_forcing_monotonic_scroll() {
        let mut follower = ScriptFollower::new(script()).unwrap();
        moved(follower.update(&update(0, "今天介绍录屏编辑", true)), 0);
        moved(follower.update(&update(1000, "首先打开录制窗口", true)), 1);
        moved(follower.update(&update(2000, "然后选择屏幕区域", true)), 2);
        moved(follower.update(&update(3000, "首先打开录制窗口", true)), 1);
        assert_eq!(follower.current_sentence(), 1);
    }

    #[test]
    fn a_large_skip_requires_a_second_different_utterance() {
        let mut follower = ScriptFollower::new(script()).unwrap();
        let first = update(4000, "录制结束生成字幕", true);
        assert_eq!(
            follower.update(&first),
            FollowDecision::Stay(StayReason::AwaitingConfirmation)
        );
        assert_eq!(
            follower.update(&first),
            FollowDecision::Stay(StayReason::Duplicate)
        );
        assert_eq!(follower.current_sentence(), 0);
        moved(follower.update(&update(5000, "最后导出完整视频", true)), 7);
    }

    #[test]
    fn mixed_chinese_english_and_unicode_byte_ranges_align() {
        let script = "你好，欢迎回来。\n接下来点击 Export 导出视频。\nNow press the record button.";
        let mut follower = ScriptFollower::new(script).unwrap();
        let FollowDecision::Move(anchor) =
            follower.update(&update(0, "接下来点击 export 导出视频", true))
        else {
            panic!("mixed script did not align");
        };
        assert_eq!(anchor.sentence, 1);
        assert!(script.get(anchor.bytes).unwrap().contains("Export"));
        moved(
            follower.update(&update(1000, "now press the Record button", true)),
            2,
        );
    }

    #[test]
    fn repeated_paragraphs_require_context_and_do_not_choose_the_first_match() {
        let mut follower = ScriptFollower::new("点击按钮开始录制。\n检查画面显示状态。\n这一段解释编辑方法。\n调节背景音乐音量。\n点击按钮开始录制。").unwrap();
        follower.resume_at(2).unwrap();
        assert_eq!(
            follower.update(&update(0, "点击按钮开始录制", true)),
            FollowDecision::Stay(StayReason::Ambiguous)
        );
        assert_eq!(follower.current_sentence(), 2);
        moved(follower.update(&update(1000, "调节背景音乐音量", true)), 3);
        moved(follower.update(&update(2000, "点击按钮开始录制", true)), 4);
    }

    #[test]
    fn improvised_speech_silence_and_short_acknowledgements_stay_put() {
        let mut follower = ScriptFollower::new(script()).unwrap();
        follower.resume_at(2).unwrap();
        for (index, text) in [
            "",
            "嗯",
            "好的",
            "突然想起昨天出去吃了一碗很好吃的面",
            "有人敲门先暂停一会儿",
        ]
        .iter()
        .enumerate()
        {
            assert!(matches!(
                follower.update(&update(index as u64 * 1000, text, true)),
                FollowDecision::Stay(_)
            ));
            assert_eq!(follower.current_sentence(), 2);
        }
    }

    #[test]
    fn partial_rewrites_replace_candidates_without_advancing() {
        let mut follower = ScriptFollower::new(script()).unwrap();
        assert!(matches!(
            follower.update(&update(0, "首先打开录制窗口", false)),
            FollowDecision::Candidate(ScriptAnchor { sentence: 1, .. })
        ));
        assert_eq!(follower.current_sentence(), 0);
        assert_eq!(
            follower.update(&update(0, "刚才那句话说错了先聊一下别的", false)),
            FollowDecision::Stay(StayReason::Unmatched)
        );
        assert_eq!(follower.current_sentence(), 0);
        moved(follower.update(&update(0, "然后选择屏幕区域", true)), 2);
        assert_eq!(
            follower.update(&update(0, "首先打开录制窗口", false)),
            FollowDecision::Stay(StayReason::Duplicate)
        );
    }

    #[test]
    fn manual_takeover_and_invalid_scripts_are_explicit() {
        let mut follower = ScriptFollower::new(script()).unwrap();
        follower.suspend();
        assert_eq!(
            follower.update(&update(0, "首先打开录制窗口", true)),
            FollowDecision::Stay(StayReason::ManualHold)
        );
        follower.resume_at(1).unwrap();
        assert_eq!(
            follower.update(&update(0, "首先打开录制窗口", true)),
            FollowDecision::Stay(StayReason::Duplicate)
        );
        moved(follower.update(&update(1000, "然后选择屏幕区域", true)), 2);
        assert_eq!(follower.resume_at(999), Err(FollowError::InvalidScript));
        assert!(ScriptFollower::new("……").is_err());
        assert!(ScriptFollower::new(&"中".repeat(50_000)).is_err());
    }

    #[test]
    fn wire_requests_use_new_streaming_resource_and_raw_s16le() {
        let packet = initialization_packet();
        assert_eq!(&packet[..4], &[0x11, 0x10, 0x10, 0]);
        let body: serde_json::Value = serde_json::from_slice(&packet[8..]).unwrap();
        assert_eq!(body["audio"]["rate"], 16000);
        assert_eq!(body["audio"]["channel"], 1);
        assert_eq!(body["request"]["result_type"], "single");
        assert_eq!(body["request"]["enable_nonstream"], true);
        assert_eq!(body["request"]["enable_ddc"], false);
        let packet = audio_packet(&[1, -2, i16::MAX], false).unwrap();
        assert_eq!(&packet[..8], &[0x11, 0x20, 0, 0, 0, 0, 0, 6]);
        assert_eq!(&packet[8..], &[1, 0, 254, 255, 255, 127]);
        assert_eq!(&audio_packet(&[], true).unwrap()[..4], &[0x11, 0x22, 0, 0]);
        assert!(audio_packet(&[], false).is_err());
        assert!(pcm(vec![0; CHUNK_SAMPLES + 1]).is_err());
        assert!(pcm(vec![0; CHUNK_SAMPLES]).is_ok());
        assert!(StreamingConfig::new("key\r\ninjected".into(), BillingResource::Duration).is_err());
        assert_eq!(
            BillingResource::Concurrent.id(),
            "volc.seedasr.sauc.concurrent"
        );
        assert!(ENDPOINT.ends_with("/sauc/bigmodel_async"));
    }

    #[test]
    fn wire_partial_final_and_rewritten_text_keep_utterance_identity() {
        for (text, definite, flags, sequence) in [
            ("开始录音", false, 1, 1),
            ("开始录屏", false, 1, 2),
            ("开始录屏", true, 3, -3),
        ] {
            let frame = server_packet(
                flags,
                Some(sequence),
                serde_json::json!({"result":{"utterances":[{"start_time":0,"end_time":1000,"text":text,"definite":definite}]}}),
            );
            let decoded = decode_packet(&frame).unwrap();
            assert_eq!(decoded.sequence, Some(sequence));
            assert_eq!(decoded.final_packet, definite);
            assert_eq!(
                decoded.updates,
                vec![RecognitionUpdate {
                    start_ms: 0,
                    end_ms: 1000,
                    text: text.into(),
                    definite
                }]
            );
        }
        let array = server_packet(0, None, serde_json::json!({"result":[{"utterances":[]}]}));
        assert!(decode_packet(&array).unwrap().updates.is_empty());
    }

    #[test]
    fn malformed_frames_are_rejected_without_panics_or_partial_results() {
        let frame = server_packet(1, Some(1), serde_json::json!({"result":{"utterances":[]}}));
        for length in 0..frame.len() {
            assert!(decode_packet(&frame[..length]).is_err());
        }
        for (index, value) in [
            (0, 0x21),
            (0, 0x10),
            (1, 0xa1),
            (1, 0x94),
            (2, 0x11),
            (2, 0x20),
        ] {
            let mut invalid = frame.clone();
            invalid[index] = value;
            assert!(decode_packet(&invalid).is_err());
        }
        let mut trailing = frame.clone();
        trailing.push(0);
        assert!(decode_packet(&trailing).is_err());
        assert!(decode_packet(&vec![0; MAX_FRAME + 1]).is_err());
        let negative = server_packet(1, Some(-2), serde_json::json!({"result":{}}));
        assert!(decode_packet(&negative).is_err());
        let time_reversed = server_packet(
            1,
            Some(1),
            serde_json::json!({"result":{"utterances":[{"start_time":10,"end_time":0,"text":"invalid"}]}}),
        );
        assert!(decode_packet(&time_reversed).is_err());
    }

    #[test]
    fn server_errors_and_consumer_backpressure_are_terminal() {
        let mut error = vec![0x11, 0xf0, 0x10, 0];
        error.extend_from_slice(&45000081u32.to_be_bytes());
        error.extend_from_slice(&7u32.to_be_bytes());
        error.extend_from_slice(b"timeout");
        assert_eq!(
            decode_packet(&error),
            Err(FollowError::Server {
                code: 45000081,
                reason: ServerFailure::AudioTimeout
            })
        );
        let (sender, receiver) = event_channel();
        for _ in 0..16 {
            emit(&sender, GENERATION, StreamingEvent::Finished).unwrap();
        }
        assert_eq!(
            emit(&sender, GENERATION, StreamingEvent::Finished),
            Err(FollowError::ConsumerLagged)
        );
        drop(receiver);
        assert_eq!(
            emit(&sender, GENERATION, StreamingEvent::Finished),
            Err(FollowError::ConsumerClosed)
        );
    }

    #[tokio::test]
    async fn pre_cancelled_session_does_not_connect_or_send_audio() {
        let (_, audio) = audio_channel();
        let (events, mut received) = event_channel();
        let (cancel, cancellation) = oneshot::channel();
        cancel.send(()).unwrap();
        let config =
            StreamingConfig::new("synthetic-not-a-real-key".into(), BillingResource::Duration)
                .unwrap();
        assert_eq!(
            run_session(GENERATION, config, audio, events, cancellation).await,
            Err(FollowError::Cancelled)
        );
        assert!(
            received
                .recv()
                .await
                .map(|event| {
                    assert_eq!(event.generation, GENERATION);
                    event.event
                })
                .is_none()
        );
    }
    struct MockSocket {
        incoming: VecDeque<Result<Message, tokio_tungstenite::tungstenite::Error>>,
        after_finish: Option<Message>,
        sent: std::sync::Arc<std::sync::Mutex<Vec<Message>>>,
        block_writes: bool,
        ready_after: Option<std::pin::Pin<Box<tokio::time::Sleep>>>,
    }

    impl Default for MockSocket {
        fn default() -> Self {
            Self {
                incoming: VecDeque::from([Ok(Message::Binary(server_packet(
                    1,
                    Some(1),
                    serde_json::json!({"result":{}}),
                )))]),
                after_finish: None,
                sent: Default::default(),
                block_writes: false,
                ready_after: None,
            }
        }
    }

    impl Stream for MockSocket {
        type Item = Result<Message, tokio_tungstenite::tungstenite::Error>;
        fn poll_next(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Self::Item>> {
            let this = self.get_mut();
            if let Some(delay) = &mut this.ready_after
                && std::future::Future::poll(delay.as_mut(), cx).is_pending()
            {
                return std::task::Poll::Pending;
            }
            this.ready_after = None;
            if let Some(message) = this.incoming.pop_front() {
                return std::task::Poll::Ready(Some(message));
            }
            let ended = this.sent.lock().unwrap().iter().any(
                |message| matches!(message, Message::Binary(bytes) if bytes.get(1) == Some(&0x22)),
            );
            if ended && let Some(message) = this.after_finish.take() {
                return std::task::Poll::Ready(Some(Ok(message)));
            }
            std::task::Poll::Pending
        }
    }

    impl Sink<Message> for MockSocket {
        type Error = tokio_tungstenite::tungstenite::Error;
        fn poll_ready(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            if self.block_writes {
                std::task::Poll::Pending
            } else {
                std::task::Poll::Ready(Ok(()))
            }
        }
        fn start_send(self: std::pin::Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
            self.sent.lock().unwrap().push(item);
            Ok(())
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_close(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    fn finished_audio() -> mpsc::Receiver<AudioInput> {
        let (sender, receiver) = audio_channel();
        assert!(
            sender
                .try_send(AudioInput::Pcm(pcm(vec![0; CHUNK_SAMPLES]).unwrap()))
                .is_ok()
        );
        assert!(sender.try_send(AudioInput::Finish).is_ok());
        receiver
    }

    #[tokio::test(start_paused = true)]
    async fn ending_without_microphone_data_is_not_a_successful_session() {
        let (sender, audio) = audio_channel();
        assert!(sender.try_send(AudioInput::Finish).is_ok());
        let (events, mut received) = event_channel();
        assert_eq!(
            pump(MockSocket::default(), audio, events).await,
            Err(FollowError::NoAudio)
        );
        assert_eq!(
            received.recv().await.map(|event| event.event),
            Some(StreamingEvent::Ready)
        );
        assert_eq!(received.recv().await, None);
    }

    #[tokio::test(start_paused = true)]
    async fn finish_requires_a_final_result_before_marking_finished() {
        let final_packet =
            server_packet(3, Some(-1), serde_json::json!({"result":{"utterances":[]}}));
        let socket = MockSocket {
            after_finish: Some(Message::Binary(final_packet)),
            ..Default::default()
        };
        let sent = socket.sent.clone();
        let (events, mut received) = event_channel();
        assert_eq!(pump(socket, finished_audio(), events).await, Ok(()));
        assert_eq!(
            received.recv().await.map(|event| event.event),
            Some(StreamingEvent::Ready)
        );
        assert!(matches!(
            received.recv().await.map(|event| {
                assert_eq!(event.generation, GENERATION);
                event.event
            }),
            Some(StreamingEvent::Recognition(RecognitionPacket {
                final_packet: true,
                ..
            }))
        ));
        assert_eq!(
            received.recv().await.map(|event| {
                assert_eq!(event.generation, GENERATION);
                event.event
            }),
            Some(StreamingEvent::Finished)
        );
        assert_eq!(
            received.recv().await.map(|event| {
                assert_eq!(event.generation, GENERATION);
                event.event
            }),
            None
        );
        let sent = sent.lock().unwrap();
        assert_eq!(
            sent.iter()
                .filter(|message| matches!(message, Message::Binary(_)))
                .count(),
            2
        );
    }

    #[tokio::test(start_paused = true)]
    async fn disconnect_and_premature_final_never_report_completion() {
        for message in [
            Message::Close(None),
            Message::Binary(server_packet(3, Some(-1), serde_json::json!({"result":{}}))),
        ] {
            let socket = MockSocket {
                incoming: VecDeque::from([Ok(message)]),
                ..Default::default()
            };
            let (_sender, audio) = audio_channel();
            let (events, mut received) = event_channel();
            assert!(matches!(
                pump(socket, audio, events).await,
                Err(FollowError::Disconnected | FollowError::Protocol("unexpected final result"))
            ));
            assert_eq!(
                received.recv().await.map(|event| {
                    assert_eq!(event.generation, GENERATION);
                    event.event
                }),
                None
            );
        }
        let socket = MockSocket {
            after_finish: Some(Message::Close(None)),
            ..Default::default()
        };
        let (events, mut received) = event_channel();
        assert_eq!(
            pump(socket, finished_audio(), events).await,
            Err(FollowError::Disconnected)
        );
        assert_eq!(
            received.recv().await.map(|event| event.event),
            Some(StreamingEvent::Ready)
        );
        assert_eq!(received.recv().await, None);
    }

    #[tokio::test(start_paused = true)]
    async fn silent_peer_final_wait_and_blocked_writes_have_bounded_timeouts() {
        let (_sender, audio) = audio_channel();
        let (events, _received) = event_channel();
        assert_eq!(
            pump(MockSocket::default(), audio, events).await,
            Err(FollowError::Timeout("response"))
        );
        let (events, _received) = event_channel();
        assert_eq!(
            pump(MockSocket::default(), finished_audio(), events).await,
            Err(FollowError::Timeout("final result"))
        );
        let socket = MockSocket {
            block_writes: true,
            ..Default::default()
        };
        let (events, _received) = event_channel();
        assert_eq!(
            pump(socket, finished_audio(), events).await,
            Err(FollowError::Timeout("send"))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn cancellation_during_an_active_stream_and_stale_audio_stop_promptly() {
        let (_sender, audio) = audio_channel();
        let (events, _received) = event_channel();
        let (cancel, cancellation) = oneshot::channel();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel.send(()).unwrap();
        });
        assert_eq!(
            cancellable(pump(MockSocket::default(), audio, events), cancellation).await,
            Err(FollowError::Cancelled)
        );
        let chunk = pcm(vec![0; CHUNK_SAMPLES]).unwrap();
        tokio::time::advance(Duration::from_secs(2)).await;
        let (sender, audio) = audio_channel();
        assert!(sender.try_send(AudioInput::Pcm(chunk)).is_ok());
        let (events, _received) = event_channel();
        assert_eq!(
            pump(MockSocket::default(), audio, events).await,
            Err(FollowError::StaleAudio)
        );
    }

    #[test]
    fn decimals_percentages_and_source_utf8_offsets_survive_normalization() {
        let script = "🙂先将音量调整到 25%，停留 0.8 秒再开始录屏。
最后点击 Export 导出完整视频。";
        let mut follower = ScriptFollower::new(script).unwrap();
        assert_eq!(follower.sentences.len(), 2);
        let FollowDecision::Move(anchor) = follower.update(&update(
            0,
            "先将音量调整到百分之二十五停留零点八秒再开始录屏",
            true,
        )) else {
            panic!("equivalent spoken numbers must align");
        };
        assert_eq!(anchor.sentence, 0);
        assert_eq!(
            script.get(anchor.read_offset..).unwrap().chars().next(),
            Some('屏')
        );
        assert!(script.get(anchor.bytes).unwrap().contains("0.8"));
        moved(
            follower.update(&update(1000, "最后点击 export 导出完整视频", true)),
            1,
        );
        let script = "停留零点八秒再开始录屏。";
        let mut follower = ScriptFollower::new(script).unwrap();
        moved(follower.update(&update(0, "停留0.8秒再开始录屏", true)), 0);
    }

    #[test]
    fn similar_sentences_cannot_drop_negation_or_change_numeric_values() {
        for (script, recognized) in [
            (
                "现在不可以删除已经保存的项目文件。",
                "现在可以删除已经保存的项目文件",
            ),
            (
                "现在可以删除已经保存的项目文件。",
                "现在不可以删除已经保存的项目文件",
            ),
            (
                "不可以删除已经保存的项目文件。",
                "可以删除已经保存的项目文件",
            ),
            (
                "将音量调整到25%以后继续播放这个视频。",
                "将音量调整到百分之三十五以后继续播放这个视频",
            ),
            (
                "先停留0.8秒以后继续播放这个视频。",
                "先停留零点九秒以后继续播放这个视频",
            ),
            (
                "将音量调整到25以后继续播放这个视频。",
                "将音量调整到百分之二十五以后继续播放这个视频",
            ),
            (
                "设为-0.8以后继续播放这个视频。",
                "设为0.8以后继续播放这个视频",
            ),
            (
                "Please do not delete the saved project files.",
                "please do delete the saved project files",
            ),
            (
                "Don't delete the saved project files.",
                "delete the saved project files",
            ),
        ] {
            let mut follower = ScriptFollower::new(script).unwrap();
            assert_eq!(
                follower.update(&update(0, recognized, true)),
                FollowDecision::Stay(StayReason::Unmatched),
                "{script} / {recognized}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn stable_nearby_partial_follows_original_bytes_without_committing_or_far_jumps() {
        let text = "先检查录音设备状态。接下来点击 Export 按钮并导出完整视频。然后准备下一段内容。";
        let mut follower = ScriptFollower::new(text).unwrap();
        let mut partial = update(0, "接下来点击export按钮", false);
        assert!(matches!(
            follower.update(&partial),
            FollowDecision::Candidate(_)
        ));
        tokio::time::advance(Duration::from_millis(200)).await;
        assert!(matches!(
            follower.update(&partial),
            FollowDecision::Candidate(_)
        ));
        partial.end_ms += 200;
        partial.text.push_str("并导出");
        let FollowDecision::Follow(anchor) = follower.update(&partial) else {
            panic!("stable growing partial should follow");
        };
        assert_eq!(follower.current_sentence(), 0);
        assert_eq!(
            &text[anchor.read_offset..anchor.read_offset + '出'.len_utf8()],
            "出"
        );
        assert!(matches!(
            follower.update(&update(0, "忽然想聊一下完全不相干的话题", false)),
            FollowDecision::Stay(StayReason::Unmatched)
        ));
        moved(
            follower.update(&update(0, "接下来点击export按钮并导出完整视频", true)),
            1,
        );
        assert_eq!(follower.current_sentence(), 1);
        let mut far = ScriptFollower::new(script()).unwrap();
        let mut partial = update(0, "最后导出完整视频", false);
        assert!(matches!(far.update(&partial), FollowDecision::Candidate(_)));
        tokio::time::advance(Duration::from_millis(200)).await;
        partial.end_ms += 200;
        assert!(matches!(far.update(&partial), FollowDecision::Candidate(_)));
        assert_eq!(far.current_sentence(), 0);
    }

    #[test]
    fn reconnect_manual_takeover_and_script_changes_reject_late_results() {
        let mut follower = ScriptFollower::new(script()).unwrap();
        follower.begin_session(FollowGeneration(1)).unwrap();
        moved(
            follower.update_for(FollowGeneration(1), &update(5000, "首先打开录制窗口", true)),
            1,
        );
        follower.cancel_session();
        assert_eq!(
            follower.update_for(FollowGeneration(1), &update(6000, "然后选择屏幕区域", true)),
            FollowDecision::Stay(StayReason::StaleGeneration)
        );
        follower.begin_session(FollowGeneration(2)).unwrap();
        assert_eq!(
            follower.update_for(FollowGeneration(1), &update(6000, "然后选择屏幕区域", true)),
            FollowDecision::Stay(StayReason::StaleGeneration)
        );
        moved(
            follower.update_for(FollowGeneration(2), &update(0, "然后选择屏幕区域", true)),
            2,
        );
        follower.suspend();
        assert_eq!(
            follower.update_for(FollowGeneration(2), &update(1000, "首先打开录制窗口", true)),
            FollowDecision::Stay(StayReason::ManualHold)
        );
        follower.resume_at(0).unwrap();
        assert_eq!(
            follower.update_for(FollowGeneration(2), &update(1000, "首先打开录制窗口", true)),
            FollowDecision::Stay(StayReason::Duplicate)
        );
        assert!(
            follower
                .replace_script("新稿件介绍音频编辑。", FollowGeneration(2))
                .is_err()
        );
        assert!(follower.replace_script("", FollowGeneration(3)).is_err());
        follower
            .replace_script("新稿件介绍音频编辑。", FollowGeneration(3))
            .unwrap();
        assert_eq!(
            follower.update_for(FollowGeneration(2), &update(2000, "然后选择屏幕区域", true)),
            FollowDecision::Stay(StayReason::StaleGeneration)
        );
        moved(
            follower.update_for(FollowGeneration(3), &update(0, "新稿件介绍音频编辑", true)),
            0,
        );
    }

    #[test]
    fn older_utterance_cannot_override_a_newer_candidate() {
        let mut follower = ScriptFollower::new(script()).unwrap();
        assert!(matches!(
            follower.update(&update(2000, "然后选择屏幕区域", false)),
            FollowDecision::Candidate(_)
        ));
        assert_eq!(
            follower.update(&update(1000, "首先打开录制窗口", true)),
            FollowDecision::Stay(StayReason::Duplicate)
        );
        moved(follower.update(&update(2000, "然后选择屏幕区域", true)), 2);
    }

    #[test]
    fn numeric_normalization_keeps_ambiguous_shortened_chinese_numbers_distinct() {
        assert_eq!(integer("二十五").as_deref(), Some("25"));
        assert_eq!(integer("一百零二").as_deref(), Some("102"));
        assert_eq!(integer("一百二十").as_deref(), Some("120"));
        assert_eq!(integer("一百二"), None);
        assert_eq!(integer("二百五"), None);
        assert_eq!(
            number_at("负零点八").unwrap().0,
            number_at("-0.8").unwrap().0
        );
    }

    #[test]
    fn wire_error_details_never_escape_to_diagnostics() {
        let private = "synthetic-private-script api-key=not-real";
        let mut frame = vec![0x11, 0xf0, 0x10, 0];
        frame.extend_from_slice(&45_000_081u32.to_be_bytes());
        let body = serde_json::json!({"message":private}).to_string();
        frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
        frame.extend_from_slice(body.as_bytes());
        let error = decode_packet(&frame).unwrap_err();
        assert_eq!(
            error,
            FollowError::Server {
                code: 45_000_081,
                reason: ServerFailure::AudioTimeout
            }
        );
        assert!(!format!("{error:?}").contains(private));
        let frame = server_packet(
            0,
            None,
            serde_json::json!({"error":{"code":55_000_031,"message":private}}),
        );
        assert_eq!(
            decode_packet(&frame),
            Err(FollowError::Server {
                code: 55_000_031,
                reason: ServerFailure::Busy
            })
        );
        let error = network_error(tokio_tungstenite::tungstenite::Error::Io(
            std::io::Error::other(private),
        ));
        assert_eq!(error, FollowError::Network);
        for code in [0, 20_000_000] {
            assert!(
                decode_packet(&server_packet(0, None, serde_json::json!({"code":code}))).is_ok()
            );
        }
        assert!(decode_packet(&server_packet(1, Some(1), serde_json::json!({}))).is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn initialization_ack_precedes_audio_and_preserves_generation() {
        let socket = MockSocket {
            ready_after: Some(Box::pin(tokio::time::sleep(Duration::from_millis(600)))),
            after_finish: Some(Message::Binary(server_packet(
                3,
                Some(-2),
                serde_json::json!({"result":{}}),
            ))),
            ..Default::default()
        };
        let sent = socket.sent.clone();
        let (events, mut received) = event_channel();
        let task = tokio::spawn(pump(socket, finished_audio(), events));
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(300)).await;
        assert!(sent.lock().unwrap().is_empty());
        assert!(received.try_recv().is_err());
        tokio::time::advance(Duration::from_millis(300)).await;
        assert_eq!(
            received.recv().await,
            Some(SessionEvent {
                generation: GENERATION,
                event: StreamingEvent::Ready
            })
        );
        assert_eq!(task.await.unwrap(), Ok(()));
        assert!(
            sent.lock()
                .unwrap()
                .iter()
                .any(|message| matches!(message, Message::Binary(bytes) if bytes[1] == 0x20))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn silent_or_rejected_initialization_never_sends_pcm() {
        for incoming in [
            VecDeque::new(),
            VecDeque::from([Ok(Message::Binary(server_packet(
                0,
                None,
                serde_json::json!({"code":45_000_001,"message":"private"}),
            )))]),
        ] {
            let socket = MockSocket {
                incoming,
                ..Default::default()
            };
            let sent = socket.sent.clone();
            let (events, mut received) = event_channel();
            let result = pump(socket, finished_audio(), events).await;
            assert!(matches!(
                result,
                Err(FollowError::Timeout("initialization") | FollowError::Server { .. })
            ));
            assert!(
                !sent
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|message| matches!(message, Message::Binary(_)))
            );
            assert!(received.recv().await.is_none());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn original_capture_age_and_queue_overload_are_not_retimestamped() {
        let captured_at = Instant::now().into_std();
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(matches!(
            PcmChunk::captured_16khz_mono(vec![0; CHUNK_SAMPLES], captured_at),
            Err(FollowError::StaleAudio)
        ));
        assert!(matches!(
            PcmChunk::captured_16khz_mono(
                vec![0; CHUNK_SAMPLES],
                (Instant::now() + Duration::from_millis(1)).into_std()
            ),
            Err(FollowError::InvalidAudioClock)
        ));
        let (sender, mut receiver) = audio_channel();
        for _ in 0..4 {
            try_enqueue_audio(
                &sender,
                AudioInput::Pcm(pcm(vec![0; CHUNK_SAMPLES]).unwrap()),
            )
            .unwrap();
        }
        assert_eq!(
            try_enqueue_audio(&sender, AudioInput::Finish),
            Err(FollowError::AudioBackpressure)
        );
        let queued = receiver.recv().await.unwrap();
        tokio::time::advance(Duration::from_secs(2)).await;
        assert_eq!(
            try_enqueue_audio(&sender, queued),
            Err(FollowError::StaleAudio)
        );
        drop(receiver);
        assert_eq!(
            try_enqueue_audio(&sender, AudioInput::Finish),
            Err(FollowError::AudioClosed)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn duplicated_capture_clock_stops_before_resending_samples() {
        let (sender, receiver) = audio_channel();
        for _ in 0..2 {
            try_enqueue_audio(
                &sender,
                AudioInput::Pcm(pcm(vec![1; CHUNK_SAMPLES]).unwrap()),
            )
            .unwrap();
        }
        let socket = MockSocket::default();
        let sent = socket.sent.clone();
        let (events, _received) = event_channel();
        assert_eq!(
            pump(socket, receiver, events).await,
            Err(FollowError::InvalidAudioClock)
        );
        assert_eq!(
            sent.lock()
                .unwrap()
                .iter()
                .filter(|message| matches!(message, Message::Binary(_)))
                .count(),
            1
        );
    }

    #[tokio::test(start_paused = true)]
    async fn cancelling_blocked_audio_write_cannot_emit_late_results() {
        let socket = MockSocket {
            block_writes: true,
            ..Default::default()
        };
        let sent = socket.sent.clone();
        let (events, mut received) = event_channel();
        let (cancel, cancellation) = oneshot::channel();
        let task = tokio::spawn(cancellable(
            pump(socket, finished_audio(), events),
            cancellation,
        ));
        assert_eq!(
            received.recv().await.map(|event| event.event),
            Some(StreamingEvent::Ready)
        );
        cancel.send(()).unwrap();
        assert_eq!(task.await.unwrap(), Err(FollowError::Cancelled));
        assert!(sent.lock().unwrap().is_empty());
        assert!(received.recv().await.is_none());
    }
}
