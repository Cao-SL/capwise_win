use std::{path::PathBuf, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const SETTINGS_SECTION: &str = "capwise_ai";
pub const SETTINGS_KEY: &str = "doubaoTts";
pub const KEYCHAIN_SERVICE: &str = "com.capwise.app.ai";
pub const KEYCHAIN_ACCOUNT: &str = "tts.doubao";
pub const RECOGNITION_ACCOUNT: &str = "asr.doubao";
pub const LEGACY_RECOGNITION_SERVICE: &str = "com.capwise.app.asr";
pub const RECOGNITION_ENDPOINT: &str = "https://openspeech.bytedance.com/api/v3";
pub const API_KEY_URL: &str =
    "https://console.volcengine.com/speech/new/setting/apikeys?projectName=default";
pub const MODEL: &str = "seed-tts-2.0";
pub const VOICE: &str = "zh_female_vv_uranus_bigtts";
pub const ENDPOINT: &str = "https://openspeech.bytedance.com/api/v3/tts/unidirectional";
const SAMPLE_RATE: u32 = 24_000;
const MAX_PCM_BYTES: usize = SAMPLE_RATE as usize * 2 * 600;

pub fn can_reuse_recognition_key(section: &serde_json::Map<String, serde_json::Value>) -> bool {
    section
        .get("asr")
        .and_then(|providers| providers.get("doubao"))
        .and_then(|provider| provider.get("baseUrl"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|address| address.trim().trim_end_matches('/') == RECOGNITION_ENDPOINT)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub resource_id: String,
    pub speaker: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            resource_id: MODEL.into(),
            speaker: VOICE.into(),
        }
    }
}

impl Settings {
    pub fn validate(&self) -> Result<(), &'static str> {
        if ![MODEL, "seed-icl-2.0"].contains(&self.resource_id.as_str()) {
            return Err("Select a supported Doubao speech synthesis model.");
        }
        if self.speaker.is_empty()
            || self.speaker.len() > 128
            || !self
                .speaker
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            return Err("Enter a valid Doubao voice ID.");
        }
        Ok(())
    }
}

pub fn app_store_path() -> PathBuf {
    if let Some(path) = std::env::var_os("CAP_GPUI_TAURI_STORE").filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    if let Ok(directory) = std::env::var("CAP_GPUI_APP_DATA_DIR")
        && !directory.trim().is_empty()
    {
        return PathBuf::from(directory).join("store");
    }
    #[cfg(target_os = "macos")]
    let base = home().join("Library/Application Support");
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("APPDATA").map_or_else(|| PathBuf::from("."), PathBuf::from);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .map_or_else(|| home().join(".local/share"), PathBuf::from);
    base.join("so.cap.desktop/store")
}

#[cfg(not(target_os = "windows"))]
fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from)
}

#[derive(Deserialize)]
struct SpeechChunk {
    code: i32,
    data: Option<String>,
    sentence: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct Sentence {
    pub text: String,
    #[serde(default)]
    pub words: Vec<Word>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Word {
    pub word: String,
    pub start_time: f64,
    pub end_time: f64,
}

pub struct Audio {
    pub wav: Vec<u8>,
    pub seconds: f64,
    pub sentences: Vec<Sentence>,
}

pub fn validate_text(text: &str) -> Result<(), &'static str> {
    if text.trim().is_empty() || text.chars().count() > 1_000 {
        return Err("Enter 1 to 1,000 characters for each narration request.");
    }
    Ok(())
}

pub async fn synthesize(
    settings: &Settings,
    key: &str,
    text: &str,
    captions: bool,
) -> Result<Audio, String> {
    synthesize_at(ENDPOINT, settings, key, text, captions).await
}

async fn synthesize_at(
    endpoint: &str,
    settings: &Settings,
    key: &str,
    text: &str,
    captions: bool,
) -> Result<Audio, String> {
    settings.validate()?;
    validate_text(text)?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|_| "Could not initialize the speech service.")?;
    let mut response = client
        .post(endpoint)
        .header("X-Api-Key", key)
        .header("X-Api-Resource-Id", &settings.resource_id)
        .header("X-Api-Request-Id", uuid::Uuid::new_v4().to_string())
        .json(&json!({
            "req_params": {
                "text": text.trim(),
                "speaker": settings.speaker,
                "audio_params": {
                    "format": "pcm", "sample_rate": SAMPLE_RATE, "enable_subtitle": captions
                }
            }
        }))
        .send()
        .await
        .map_err(|_| "Could not reach the Doubao speech service.")?;
    if !response.status().is_success() {
        return Err(format!(
            "Doubao speech synthesis returned HTTP {}.",
            response.status()
        ));
    }
    let mut pending = Vec::new();
    let mut pcm = Vec::new();
    let mut sentences = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "The Doubao audio stream was interrupted.")?
    {
        pending.extend_from_slice(&chunk);
        let mut consumed = 0;
        for line in pending.split_inclusive(|byte| *byte == b'\n') {
            if !line.ends_with(b"\n") {
                break;
            }
            append_chunk(line, &mut pcm, &mut sentences)?;
            consumed += line.len();
        }
        pending.drain(..consumed);
        if pending.len() > 2 * 1024 * 1024 {
            return Err("The Doubao audio response is too large.".into());
        }
    }
    append_chunk(&pending, &mut pcm, &mut sentences)?;
    let (wav, seconds) = pcm_to_wav(pcm)?;
    Ok(Audio {
        wav,
        seconds,
        sentences,
    })
}

fn append_chunk(
    line: &[u8],
    pcm: &mut Vec<u8>,
    sentences: &mut Vec<Sentence>,
) -> Result<(), String> {
    if line.iter().all(u8::is_ascii_whitespace) {
        return Ok(());
    }
    let chunk: SpeechChunk =
        serde_json::from_slice(line).map_err(|_| "Doubao sent an invalid audio response.")?;
    if !matches!(chunk.code, 0 | 20_000_000) {
        return Err(format!(
            "Doubao rejected speech synthesis (code {}). Check the key, model and voice.",
            chunk.code
        ));
    }
    if let Some(data) = chunk.data {
        STANDARD
            .decode_vec(data, pcm)
            .map_err(|_| "Doubao sent invalid audio data.")?;
    }
    if let Some(sentence) = chunk
        .sentence
        .and_then(|value| serde_json::from_value(value).ok())
    {
        sentences.push(sentence);
    }
    if pcm.len() > MAX_PCM_BYTES || sentences.len() > 1_000 {
        return Err("The Doubao audio response is too large.".into());
    }
    Ok(())
}

fn pcm_to_wav(pcm: Vec<u8>) -> Result<(Vec<u8>, f64), String> {
    if pcm.is_empty()
        || !pcm.len().is_multiple_of(2)
        || !pcm.chunks_exact(2).any(|sample| sample != [0, 0])
    {
        return Err("Doubao returned no usable speech audio.".into());
    }
    let bytes = pcm.len() as u32;
    let mut wav = Vec::with_capacity(pcm.len() + 44);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(bytes + 36).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&[16, 0, 0, 0, 1, 0, 1, 0]);
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&[2, 0, 16, 0]);
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&bytes.to_le_bytes());
    wav.extend_from_slice(&pcm);
    Ok((wav, f64::from(bytes) / f64::from(SAMPLE_RATE * 2)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
        net::TcpListener,
    };

    #[tokio::test]
    async fn doubao_stream_uses_configured_voice_and_keeps_pcm_and_word_times() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/tts", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = BufReader::new(&mut stream);
            let mut line = String::new();
            request.read_line(&mut line).await.unwrap();
            assert_eq!(line, "POST /tts HTTP/1.1\r\n");
            let mut headers = std::collections::HashMap::new();
            loop {
                line.clear();
                request.read_line(&mut line).await.unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                let (name, value) = line.split_once(':').unwrap();
                headers.insert(name.to_ascii_lowercase(), value.trim().to_string());
            }
            assert_eq!(headers["x-api-key"], "synthetic-api-key");
            assert_eq!(headers["x-api-resource-id"], MODEL);
            assert!(uuid::Uuid::parse_str(&headers["x-api-request-id"]).is_ok());
            let mut body = vec![0; headers["content-length"].parse().unwrap()];
            request.read_exact(&mut body).await.unwrap();
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(json["req_params"]["speaker"], "test_voice");
            assert_eq!(json["req_params"]["text"], "你好");
            assert_eq!(json["req_params"]["audio_params"]["enable_subtitle"], true);
            let response = concat!(
                "{\"code\":0,\"data\":\"AQACAA==\"}\n",
                "{\"code\":0,\"data\":\"AwAEAA==\",\"sentence\":{\"text\":\"你好\",\"words\":[",
                "{\"word\":\"你好\",\"startTime\":0.0,\"endTime\":0.0001}]}}\n",
                "{\"code\":20000000,\"data\":null}"
            );
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response.len()
            );
            stream.write_all(headers.as_bytes()).await.unwrap();
            let (first, second) = response.as_bytes().split_at(42);
            stream.write_all(first).await.unwrap();
            tokio::task::yield_now().await;
            stream.write_all(second).await.unwrap();
        });
        let settings = Settings {
            speaker: "test_voice".into(),
            ..Default::default()
        };
        let audio = synthesize_at(&endpoint, &settings, "synthetic-api-key", "你好", true)
            .await
            .unwrap();
        server.await.unwrap();
        assert_eq!(&audio.wav[..4], b"RIFF");
        assert_eq!(&audio.wav[44..], &[1, 0, 2, 0, 3, 0, 4, 0]);
        assert!((audio.seconds - 4.0 / 24_000.0).abs() < f64::EPSILON);
        assert_eq!(audio.sentences[0].words[0].end_time, 0.0001);
    }

    #[test]
    fn partial_and_silent_responses_are_not_audio() {
        assert!(append_chunk(br#"{"code":45000000}"#, &mut Vec::new(), &mut Vec::new()).is_err());
        assert!(pcm_to_wav(vec![1, 0, 2]).is_err());
        assert!(pcm_to_wav(vec![0, 0]).is_err());
    }

    #[test]
    fn only_official_doubao_recognition_settings_allow_key_reuse() {
        for (address, expected) in [
            (RECOGNITION_ENDPOINT, true),
            ("https://openspeech.bytedance.com/api/v3/", true),
            ("http://openspeech.bytedance.com/api/v3", false),
            ("https://openspeech.bytedance.com.example.com/api/v3", false),
            ("https://custom.example.com/api/v3", false),
        ] {
            let section = json!({"asr":{"doubao":{"baseUrl":address}}});
            assert_eq!(
                can_reuse_recognition_key(section.as_object().unwrap()),
                expected
            );
        }
        assert!(!can_reuse_recognition_key(&serde_json::Map::new()));
    }
}
