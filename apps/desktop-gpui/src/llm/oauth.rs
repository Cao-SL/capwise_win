//! Sign-ins that stand in for an API key, after qiaomu-clipper's
//! `oauth/accounts.ts`: ChatGPT and Codex accounts (OAuth with PKCE, tokens
//! refreshed before they expire and kept in the keychain like any key), and
//! 词元跳动 / OpenRouter, whose sign-in hands back an ordinary API key.

use std::{
    io::{Read as _, Write as _},
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Digest as _;

use crate::{
    ai_store::{self, Kind},
    i18n::tr,
};

const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const CODEX_AUTHORIZE: &str = "https://auth.openai.com/oauth/authorize";
const CODEX_TOKEN: &str = "https://auth.openai.com/oauth/token";
const CODEX_SCOPES: &str =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";
pub const CODEX_BASE: &str = "https://chatgpt.com/backend-api/codex";
const CODEX_VERSION: &str = "0.159.0";

const SIWC_AUTHORIZE: &str = "https://auth.openai.com/api/accounts/authorize";
const SIWC_TOKEN: &str = "https://auth.openai.com/api/accounts/oauth/token";
const SIWC_ISSUER: &str = "https://auth.openai.com";
const SIWC_RESOURCE: &str = "https://api.openai.com/v1";
const SIWC_DIRECT_SCOPE: &str = "chatgpt.tokens.use.direct";
pub const CHATGPT_BASE: &str = "https://api.openai.com/v1";
const SIWC_HOST_SETTING: &str = "chatgptAgentHostId";

const TOKENDANCE_AUTH: &str = "https://tokendance.space/auth";
const TOKENDANCE_EXCHANGE: &str = "https://tokendance.space/portal/api/v1/auth/keys";
const OPENROUTER_AUTH: &str = "https://openrouter.ai/auth";
const OPENROUTER_EXCHANGE: &str = "https://openrouter.ai/api/v1/auth/keys";
const APP_URL: &str = "https://github.com/joeseesun/capwise";

/// Both OpenAI flows only accept this return address.
const OPENAI_PORT: u16 = 1455;
const CALLBACK_PATH: &str = "/auth/callback";
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const REFRESH_MARGIN_MS: i64 = 3 * 60 * 1000;
const STORED_PREFIX: &str = "oauth:";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignIn {
    ChatGpt,
    Codex,
    TokenDance,
    OpenRouter,
}

impl SignIn {
    pub fn is_account(self) -> bool {
        matches!(self, Self::ChatGpt | Self::Codex)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountKind {
    ChatGpt,
    Codex,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    pub kind: AccountKind,
    pub client_id: String,
    pub access: String,
    pub refresh: String,
    /// Unix milliseconds.
    pub expires: i64,
    pub account_id: Option<String>,
    pub email: Option<String>,
    pub plan: Option<String>,
}

impl Credentials {
    /// "name@example.com · plus", or whatever of it the tokens carried.
    pub fn label(&self) -> String {
        match (&self.email, &self.plan) {
            (Some(email), Some(plan)) => format!("{email} · {plan}"),
            (Some(email), None) => email.clone(),
            (None, Some(plan)) => plan.clone(),
            (None, None) => String::new(),
        }
    }
}

/// The keychain holds one string per service; an account's tokens go in as
/// base64 JSON so they pass the key checks (no whitespace).
pub fn encode(credentials: &Credentials) -> String {
    let json = serde_json::to_vec(credentials).unwrap_or_default();
    format!("{STORED_PREFIX}{}", STANDARD.encode(json))
}

pub fn decode(stored: &str) -> Option<Credentials> {
    let bytes = STANDARD
        .decode(stored.trim().strip_prefix(STORED_PREFIX)?)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

fn random_string(length: usize) -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~";
    let mut out = String::with_capacity(length);
    while out.len() < length {
        for byte in uuid::Uuid::new_v4().as_bytes() {
            if out.len() < length {
                out.push(ALPHABET[*byte as usize % ALPHABET.len()] as char);
            }
        }
    }
    out
}

fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()))
}

fn jwt_claims(token: &str) -> Value {
    token
        .split('.')
        .nth(1)
        .and_then(|part| URL_SAFE_NO_PAD.decode(part.trim_end_matches('=')).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null)
}

fn claim(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn credentials(
    kind: AccountKind,
    token: &Value,
    base: Option<&Credentials>,
    client_id: &str,
) -> Credentials {
    let access = claim(token, "access_token").unwrap_or_default();
    let id_claims = token
        .get("id_token")
        .and_then(Value::as_str)
        .map(jwt_claims)
        .unwrap_or(Value::Null);
    let access_claims = jwt_claims(&access);
    let auth = |claims: &Value, key: &str| {
        claims
            .get("https://api.openai.com/auth")
            .and_then(|auth| claim(auth, key))
    };
    let lifetime_s = token
        .get("expires_in")
        .and_then(Value::as_f64)
        .or_else(|| {
            access_claims
                .get("exp")
                .and_then(Value::as_f64)
                .map(|exp| exp - now_ms() as f64 / 1000.0)
        })
        .unwrap_or(3600.0);
    Credentials {
        kind,
        client_id: client_id.to_string(),
        refresh: claim(token, "refresh_token")
            .or_else(|| base.map(|base| base.refresh.clone()))
            .unwrap_or_default(),
        expires: now_ms() + (lifetime_s * 1000.0) as i64,
        account_id: auth(&id_claims, "chatgpt_account_id")
            .or_else(|| auth(&access_claims, "chatgpt_account_id"))
            .or_else(|| base.and_then(|base| base.account_id.clone())),
        email: claim(&id_claims, "email").or_else(|| base.and_then(|base| base.email.clone())),
        plan: auth(&id_claims, "chatgpt_plan_type")
            .or_else(|| auth(&access_claims, "chatgpt_plan_type"))
            .or_else(|| base.and_then(|base| base.plan.clone())),
        access,
    }
}

/// A token endpoint's error, readable: `error_description`, an OpenAI-style
/// `error.message`, or the start of the body.
fn token_error(status: u16, body: &str) -> String {
    let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let reason = claim(&value, "error_description")
        .or_else(|| {
            value
                .pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| claim(&value, "error"))
        .unwrap_or_else(|| body.chars().take(160).collect());
    tr::ai::oauth_failed(status, reason)
}

enum Body {
    Form(Vec<(&'static str, String)>),
    Json(Value),
}

/// The answer's JSON, or the HTTP status (0 when nothing came back) with a
/// readable reason.
async fn post_token(url: &str, body: Body) -> Result<Value, (u16, String)> {
    let client = reqwest::Client::new();
    let request = client
        .post(url)
        .header("Accept", "application/json")
        .timeout(Duration::from_secs(30));
    let request = match body {
        Body::Form(fields) => request.form(&fields),
        Body::Json(value) => request.json(&value),
    };
    let response = request
        .send()
        .await
        .map_err(|error| (0, tr::ai::oauth_network(error.to_string())))?;
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err((status, token_error(status, &text)));
    }
    serde_json::from_str(&text).map_err(|_| (status, token_error(status, &text)))
}

fn message((_, message): (u16, String)) -> String {
    message
}

// ---- the return address ------------------------------------------------------

/// The browser's trip back, caught on a loopback port. Runs on a blocking
/// thread; `stop` ends the wait.
fn wait_for_callback(listener: TcpListener, stop: Arc<AtomicBool>) -> Result<reqwest::Url, String> {
    listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let started = Instant::now();
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err(tr::ai::oauth_cancelled().to_string());
        }
        if started.elapsed() > SIGN_IN_TIMEOUT {
            return Err(tr::ai::oauth_timeout().to_string());
        }
        let mut stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(error) => return Err(error.to_string()),
        };
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let mut buffer = [0u8; 8192];
        let read = stream.read(&mut buffer).unwrap_or(0);
        let request = String::from_utf8_lossy(&buffer[..read]);
        let target = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("/");
        if !target.starts_with(CALLBACK_PATH) {
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            continue;
        }
        let page = format!(
            "<!doctype html><meta charset=\"utf-8\"><title>Capwise</title><body style=\"font:15px -apple-system,sans-serif;display:grid;place-items:center;height:90vh;color:#333\"><p>{}</p></body>",
            tr::ai::oauth_done()
        );
        let _ = stream.write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
                page.len()
            )
            .as_bytes(),
        );
        return reqwest::Url::parse(&format!("http://127.0.0.1:{port}{target}"))
            .map_err(|error| error.to_string());
    }
}

fn bind(port: u16) -> Result<TcpListener, String> {
    TcpListener::bind(("127.0.0.1", port)).map_err(|_| tr::ai::oauth_port_busy(port))
}

fn query(url: &reqwest::Url, key: &str) -> Option<String> {
    url.query_pairs()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// A sign-in waiting for the browser: open `url`, then `finish` it.
pub struct Pending {
    pub url: String,
    flow: SignIn,
    listener: TcpListener,
    redirect: String,
    verifier: String,
    state: String,
    nonce: String,
}

/// What a finished sign-in leaves in the keychain's place for the key: an
/// API key, or an account's encoded tokens.
pub struct SignedIn {
    pub key: String,
}

fn host_id() -> String {
    let section = crate::store::store_section(crate::transcription::GPUI_STORE_SECTION);
    if let Some(saved) = section
        .get(SIWC_HOST_SETTING)
        .and_then(Value::as_str)
        .filter(|saved| saved.starts_with("urn:uuid:"))
    {
        return saved.to_string();
    }
    let id = format!("urn:uuid:{}", uuid::Uuid::new_v4());
    crate::store::set_store_setting(
        crate::transcription::GPUI_STORE_SECTION,
        SIWC_HOST_SETTING,
        Value::String(id.clone()),
    );
    id
}

fn authorize_url(base: &str, params: &[(&str, &str)]) -> String {
    let mut url = reqwest::Url::parse(base).expect("valid sign-in address");
    url.query_pairs_mut().extend_pairs(params);
    url.to_string()
}

/// Binds the return address and builds the sign-in page's address.
pub fn begin(flow: SignIn) -> Result<Pending, String> {
    let verifier = random_string(64);
    let challenge = pkce_challenge(&verifier);
    let state = random_string(32);
    let nonce = random_string(24);
    let (listener, redirect, url) = match flow {
        SignIn::Codex => {
            let redirect = format!("http://localhost:{OPENAI_PORT}{CALLBACK_PATH}");
            let url = authorize_url(
                CODEX_AUTHORIZE,
                &[
                    ("response_type", "code"),
                    ("client_id", CODEX_CLIENT_ID),
                    ("redirect_uri", &redirect),
                    ("scope", CODEX_SCOPES),
                    ("code_challenge", &challenge),
                    ("code_challenge_method", "S256"),
                    ("id_token_add_organizations", "true"),
                    ("codex_cli_simplified_flow", "true"),
                    ("state", &state),
                    ("originator", "codex_cli_rs"),
                ],
            );
            (bind(OPENAI_PORT)?, redirect, url)
        }
        SignIn::ChatGpt => {
            let redirect = format!("http://127.0.0.1:{OPENAI_PORT}{CALLBACK_PATH}");
            let scopes =
                format!("openid profile email offline_access resource.invoke {SIWC_DIRECT_SCOPE}");
            let host = host_id();
            let url = authorize_url(
                SIWC_AUTHORIZE,
                &[
                    ("client_id", "dynamic_agent_client"),
                    ("agent_name_hint", "capwise"),
                    ("ext_agent_host_id", &host),
                    ("response_type", "code"),
                    ("redirect_uri", &redirect),
                    ("resource", SIWC_RESOURCE),
                    ("scope", &scopes),
                    ("state", &state),
                    ("nonce", &nonce),
                    ("code_challenge", &challenge),
                    ("code_challenge_method", "S256"),
                ],
            );
            (bind(OPENAI_PORT)?, redirect, url)
        }
        SignIn::TokenDance | SignIn::OpenRouter => {
            let listener = bind(0)?;
            let port = listener.local_addr().map_err(|e| e.to_string())?.port();
            let redirect = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
            let url = if flow == SignIn::OpenRouter {
                authorize_url(
                    OPENROUTER_AUTH,
                    &[
                        ("callback_url", &redirect),
                        ("code_challenge", &challenge),
                        ("code_challenge_method", "S256"),
                        ("key_label", "Capwise"),
                        ("state", &state),
                    ],
                )
            } else {
                authorize_url(
                    TOKENDANCE_AUTH,
                    &[
                        ("callback_url", &redirect),
                        ("code_challenge", &challenge),
                        ("code_challenge_method", "S256"),
                        ("app_url", APP_URL),
                        ("key_name", "Capwise"),
                    ],
                )
            };
            (listener, redirect, url)
        }
    };
    Ok(Pending {
        url,
        flow,
        listener,
        redirect,
        verifier,
        state,
        nonce,
    })
}

impl Pending {
    /// Waits for the browser and trades its code for a key or tokens.
    pub async fn finish(self, stop: Arc<AtomicBool>) -> Result<SignedIn, String> {
        let Pending {
            flow,
            listener,
            redirect,
            verifier,
            state,
            nonce,
            ..
        } = self;
        let back = tokio::task::spawn_blocking(move || wait_for_callback(listener, stop))
            .await
            .map_err(|error| error.to_string())??;
        if flow != SignIn::TokenDance && query(&back, "state").as_deref() != Some(state.as_str()) {
            return Err(tr::ai::oauth_state().to_string());
        }
        let Some(code) = query(&back, "code") else {
            return Err(query(&back, "error_description")
                .or_else(|| query(&back, "error"))
                .unwrap_or_else(|| tr::ai::oauth_incomplete().to_string()));
        };
        match flow {
            SignIn::Codex => {
                let token = post_token(
                    CODEX_TOKEN,
                    Body::Form(vec![
                        ("grant_type", "authorization_code".into()),
                        ("code", code),
                        ("redirect_uri", redirect),
                        ("client_id", CODEX_CLIENT_ID.into()),
                        ("code_verifier", verifier),
                    ]),
                )
                .await
                .map_err(message)?;
                if claim(&token, "access_token").is_none()
                    || claim(&token, "refresh_token").is_none()
                {
                    return Err(tr::ai::oauth_incomplete().to_string());
                }
                let credentials = credentials(AccountKind::Codex, &token, None, CODEX_CLIENT_ID);
                Ok(SignedIn {
                    key: encode(&credentials),
                })
            }
            SignIn::ChatGpt => {
                let client_id = query(&back, "client_id")
                    .ok_or_else(|| tr::ai::oauth_incomplete().to_string())?;
                let token = post_token(
                    SIWC_TOKEN,
                    Body::Form(vec![
                        ("grant_type", "authorization_code".into()),
                        ("client_id", client_id.clone()),
                        ("code", code),
                        ("code_verifier", verifier),
                        ("redirect_uri", redirect),
                        ("resource", SIWC_RESOURCE.into()),
                    ]),
                )
                .await
                .map_err(message)?;
                let id_token = claim(&token, "id_token").unwrap_or_default();
                if claim(&token, "access_token").is_none()
                    || claim(&token, "refresh_token").is_none()
                    || id_token.is_empty()
                {
                    return Err(tr::ai::oauth_incomplete().to_string());
                }
                let id = jwt_claims(&id_token);
                let audience_ok = match id.get("aud") {
                    Some(Value::String(aud)) => *aud == client_id,
                    Some(Value::Array(auds)) => {
                        auds.iter().any(|aud| aud.as_str() == Some(&client_id))
                    }
                    _ => false,
                };
                if claim(&id, "iss")
                    .map(|iss| iss.trim_end_matches('/').to_string())
                    .as_deref()
                    != Some(SIWC_ISSUER)
                    || !audience_ok
                    || claim(&id, "nonce").as_deref() != Some(nonce.as_str())
                {
                    return Err(tr::ai::oauth_state().to_string());
                }
                if !claim(&token, "scope")
                    .unwrap_or_default()
                    .split_whitespace()
                    .any(|scope| scope == SIWC_DIRECT_SCOPE)
                {
                    return Err(tr::ai::oauth_no_plan().to_string());
                }
                let credentials = credentials(AccountKind::ChatGpt, &token, None, &client_id);
                Ok(SignedIn {
                    key: encode(&credentials),
                })
            }
            SignIn::TokenDance | SignIn::OpenRouter => {
                let url = if flow == SignIn::OpenRouter {
                    OPENROUTER_EXCHANGE
                } else {
                    TOKENDANCE_EXCHANGE
                };
                let data = post_token(
                    url,
                    Body::Json(json!({
                        "code": code,
                        "code_verifier": verifier,
                        "code_challenge_method": "S256",
                    })),
                )
                .await
                .map_err(message)?;
                let key =
                    claim(&data, "key").ok_or_else(|| tr::ai::oauth_incomplete().to_string())?;
                Ok(SignedIn { key })
            }
        }
    }
}

// ---- using a signed-in account ------------------------------------------------

static REFRESHING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn refresh(old: &Credentials, name: &str) -> Result<Credentials, String> {
    let result = match old.kind {
        AccountKind::Codex => {
            post_token(
                CODEX_TOKEN,
                Body::Json(json!({
                    "client_id": if old.client_id.is_empty() { CODEX_CLIENT_ID } else { old.client_id.as_str() },
                    "grant_type": "refresh_token",
                    "refresh_token": old.refresh,
                    "scope": "openid profile email",
                })),
            )
            .await
        }
        AccountKind::ChatGpt => {
            post_token(
                SIWC_TOKEN,
                Body::Form(vec![
                    ("grant_type", "refresh_token".into()),
                    ("client_id", old.client_id.clone()),
                    ("refresh_token", old.refresh.clone()),
                    ("resource", SIWC_RESOURCE.into()),
                ]),
            )
            .await
        }
    };
    let token = result.map_err(|(status, error)| {
        if status == 400 || status == 401 {
            tr::ai::oauth_expired(name)
        } else {
            error
        }
    })?;
    if claim(&token, "access_token").is_none() {
        return Err(tr::ai::oauth_expired(name));
    }
    Ok(credentials(old.kind, &token, Some(old), &old.client_id))
}

/// The tokens a request should carry, renewed (and saved) shortly before
/// they expire. Refresh tokens rotate, so two requests must not both spend
/// the same one.
pub async fn fresh(provider_id: &str, stored: &str, name: &str) -> Result<Credentials, String> {
    let credentials = decode(stored).ok_or_else(|| tr::asr::no_key(name))?;
    if credentials.expires - now_ms() > REFRESH_MARGIN_MS {
        return Ok(credentials);
    }
    let _guard = REFRESHING.lock().await;
    if let Some(saved) = ai_store::key(Kind::Llm, provider_id).and_then(|key| decode(&key))
        && saved.expires - now_ms() > REFRESH_MARGIN_MS
    {
        return Ok(saved);
    }
    let next = refresh(&credentials, name).await?;
    let id = provider_id.to_string();
    let key = encode(&next);
    tokio::task::spawn_blocking(move || ai_store::set_key(Kind::Llm, &id, &key))
        .await
        .map_err(|error| error.to_string())??;
    Ok(next)
}

fn account_headers(
    request: reqwest::RequestBuilder,
    credentials: &Credentials,
) -> reqwest::RequestBuilder {
    let request = request.bearer_auth(&credentials.access);
    if credentials.kind != AccountKind::Codex {
        return request;
    }
    let request = request
        .header("originator", "codex_cli_rs")
        .header("version", CODEX_VERSION)
        .header("OpenAI-Beta", "responses=experimental");
    match &credentials.account_id {
        Some(id) => request.header("chatgpt-account-id", id),
        None => request,
    }
}

/// The text a Responses stream carries, from its `output_text` deltas.
fn stream_text(body: &str) -> Result<String, Option<String>> {
    let mut text = String::new();
    for line in body.lines() {
        let Some(raw) = line.trim().strip_prefix("data:") else {
            continue;
        };
        let Ok(event) = serde_json::from_str::<Value>(raw.trim()) else {
            continue;
        };
        match event.get("type").and_then(Value::as_str) {
            Some("response.output_text.delta") => {
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    text.push_str(delta);
                }
            }
            Some("response.failed" | "error") => {
                return Err(event
                    .pointer("/response/error/message")
                    .or_else(|| event.pointer("/error/message"))
                    .or_else(|| event.get("message"))
                    .and_then(Value::as_str)
                    .map(str::to_string));
            }
            _ => {}
        }
    }
    Ok(text)
}

pub async fn chat(
    provider_id: &str,
    name: &str,
    stored: &str,
    model: &str,
    system: &str,
    user: &str,
) -> Result<String, String> {
    let credentials = fresh(provider_id, stored, name).await?;
    let url = match credentials.kind {
        AccountKind::Codex => format!("{CODEX_BASE}/responses"),
        AccountKind::ChatGpt => format!("{CHATGPT_BASE}/responses"),
    };
    let request = reqwest::Client::new()
        .post(url)
        .header("Accept", "text/event-stream")
        .timeout(Duration::from_secs(180))
        .json(&json!({
            "model": model.trim(),
            "instructions": system,
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": user }],
            }],
            "store": false,
            "stream": true,
        }));
    let response = account_headers(request, &credentials)
        .send()
        .await
        .map_err(|error| tr::asr::connection_failed(name, error.to_string()))?;
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    if status == 401 || status == 403 {
        return Err(tr::ai::oauth_expired(name));
    }
    if !(200..300).contains(&status) {
        return Err(tr::asr::failed(name, status, super::error_message(&body)));
    }
    match stream_text(&body) {
        Ok(text) if !text.is_empty() => Ok(text),
        Ok(_) | Err(None) => Err(tr::asr::unreadable(name)),
        Err(Some(message)) => Err(tr::asr::failed(name, status, &message)),
    }
}

/// The models a signed-in account may use, from its own catalogue.
pub async fn models(
    provider_id: &str,
    name: &str,
    stored: &str,
) -> Result<Vec<super::DiscoveredModel>, super::DiscoveryError> {
    use super::DiscoveryError;
    let credentials = fresh(provider_id, stored, name)
        .await
        .map_err(|_| DiscoveryError::Authentication)?;
    let url = match credentials.kind {
        AccountKind::Codex => format!("{CODEX_BASE}/models?client_version={CODEX_VERSION}"),
        AccountKind::ChatGpt => format!("{CHATGPT_BASE}/models"),
    };
    let request = reqwest::Client::new()
        .get(url)
        .header("Accept", "application/json")
        .timeout(Duration::from_secs(20));
    let response = account_headers(request, &credentials)
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                DiscoveryError::Timeout
            } else {
                DiscoveryError::Connection
            }
        })?;
    let status = response.status().as_u16();
    if status == 401 || status == 403 {
        return Err(DiscoveryError::Authentication);
    }
    if !(200..300).contains(&status) {
        return Err(DiscoveryError::Http(status));
    }
    let value: Value = response
        .json()
        .await
        .map_err(|_| DiscoveryError::Response)?;
    account_models(&value).ok_or(DiscoveryError::Response)
}

/// Either an OpenAI-style `data` list or ChatGPT's `models` catalogue.
fn account_models(value: &Value) -> Option<Vec<super::DiscoveredModel>> {
    let rows = value
        .get("models")
        .or_else(|| value.get("data"))
        .and_then(Value::as_array)?;
    let mut seen = std::collections::HashSet::new();
    Some(
        rows.iter()
            .filter(|row| {
                row.get("visibility")
                    .and_then(Value::as_str)
                    .is_none_or(|visibility| visibility == "list")
            })
            .filter_map(|row| {
                let id = claim(row, "slug").or_else(|| claim(row, "id"))?;
                seen.insert(id.clone()).then(|| super::DiscoveredModel {
                    display_name: claim(row, "display_name")
                        .or_else(|| claim(row, "name"))
                        .filter(|name| *name != id),
                    id,
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt(claims: Value) -> String {
        format!(
            "e30.{}.sig",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap())
        )
    }

    #[test]
    fn stored_credentials_round_trip_and_pass_the_key_checks() {
        let credentials = Credentials {
            kind: AccountKind::Codex,
            client_id: CODEX_CLIENT_ID.into(),
            access: "a.b.c".into(),
            refresh: "r".into(),
            expires: 1,
            account_id: Some("acct".into()),
            email: Some("me@example.com".into()),
            plan: Some("plus".into()),
        };
        let stored = encode(&credentials);
        assert!(!stored.chars().any(char::is_whitespace));
        assert_eq!(decode(&stored), Some(credentials));
        assert_eq!(decode("sk-plain-api-key"), None);
    }

    #[test]
    fn tokens_yield_the_account_email_plan_and_expiry() {
        let token = json!({
            "access_token": jwt(json!({ "https://api.openai.com/auth": { "chatgpt_account_id": "acct_1" } })),
            "refresh_token": "refresh",
            "id_token": jwt(json!({
                "email": "me@example.com",
                "https://api.openai.com/auth": { "chatgpt_plan_type": "pro" },
            })),
            "expires_in": 3600,
        });
        let credentials = credentials(AccountKind::Codex, &token, None, CODEX_CLIENT_ID);
        assert_eq!(credentials.account_id.as_deref(), Some("acct_1"));
        assert_eq!(credentials.label(), "me@example.com · pro");
        assert!(credentials.expires > now_ms() + 3_500_000);
    }

    #[test]
    fn pkce_challenge_matches_the_rfc_example() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert_eq!(random_string(64).len(), 64);
    }

    #[test]
    fn responses_stream_collects_text_and_surfaces_failures() {
        let body = "event: x\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"O\"}\n\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"K\"}\ndata: [DONE]\n";
        assert_eq!(stream_text(body), Ok("OK".to_string()));
        let failed = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"quota\"}}}\n";
        assert_eq!(stream_text(failed), Err(Some("quota".to_string())));
    }

    #[test]
    fn account_catalogues_skip_hidden_models() {
        let value = json!({ "models": [
            { "slug": "gpt-a", "display_name": "GPT A" },
            { "slug": "gpt-hidden", "visibility": "hide" },
            { "id": "gpt-b" },
        ]});
        let ids: Vec<_> = account_models(&value)
            .unwrap()
            .into_iter()
            .map(|model| model.id)
            .collect();
        assert_eq!(ids, ["gpt-a", "gpt-b"]);
    }

    #[test]
    fn the_callback_listener_returns_the_browser_address() {
        let listener = bind(0).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let waiter = std::thread::spawn({
            let stop = stop.clone();
            move || wait_for_callback(listener, stop)
        });
        let mut stray = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stray
            .write_all(b"GET /favicon.ico HTTP/1.1\r\n\r\n")
            .unwrap();
        let mut answer = String::new();
        stray.read_to_string(&mut answer).unwrap();
        assert!(answer.starts_with("HTTP/1.1 404"));
        let mut browser = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        browser
            .write_all(b"GET /auth/callback?code=abc&state=xyz HTTP/1.1\r\n\r\n")
            .unwrap();
        let url = waiter.join().unwrap().unwrap();
        assert_eq!(query(&url, "code").as_deref(), Some("abc"));
        assert_eq!(query(&url, "state").as_deref(), Some("xyz"));
    }
}
