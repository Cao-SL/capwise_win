//! Large language models: the provider catalogue from qiaomu-clipper's
//! `provider-catalog.ts` and one chat call over the four request styles they
//! use. Settings live in [`crate::ai_store`].

use std::{collections::HashSet, time::Duration};

use serde_json::{Value, json};

use crate::{
    ai_store::{self, Endpoint, Kind},
    i18n::tr,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    /// `/chat/completions`, the OpenAI shape most services speak.
    OpenAi,
    Anthropic,
    Gemini,
    Ollama,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    /// One key for many models.
    Relay,
    Provider,
    /// Runs on this computer.
    Local,
}

pub struct LlmProvider {
    pub id: &'static str,
    pub name: fn() -> &'static str,
    pub group: Group,
    pub protocol: Protocol,
    /// The full request URL; Gemini's has a `{model-id}` slot.
    pub base_url: &'static str,
    pub key_url: Option<&'static str>,
    pub models: &'static [&'static str],
}

impl LlmProvider {
    pub fn needs_key(&self) -> bool {
        self.group != Group::Local
    }

    pub fn endpoint(&self) -> Endpoint {
        ai_store::endpoint(Kind::Llm, self.id).unwrap_or_else(|| Endpoint {
            base_url: self.base_url.to_string(),
            model: self.models.first().copied().unwrap_or_default().to_string(),
        })
    }

    pub fn ready(&self) -> bool {
        let endpoint = self.endpoint();
        ai_store::endpoint(Kind::Llm, self.id).is_some()
            && ai_store::address_allowed(&endpoint.base_url)
            && !endpoint.model.trim().is_empty()
            && (!self.needs_key() || ai_store::key(Kind::Llm, self.id).is_some())
    }
}

const fn openai(
    id: &'static str,
    name: fn() -> &'static str,
    group: Group,
    base_url: &'static str,
    key_url: Option<&'static str>,
    models: &'static [&'static str],
) -> LlmProvider {
    LlmProvider {
        id,
        name,
        group,
        protocol: Protocol::OpenAi,
        base_url,
        key_url,
        models,
    }
}

/// Clipper's catalogue minus the ChatGPT and Codex account sign-ins, in the
/// order its picker shows them.
pub static PROVIDERS: &[LlmProvider] = &[
    openai(
        "tokendance",
        tr::ai::llm::tokendance,
        Group::Relay,
        "https://tokendance.space/gateway/v1/chat/completions",
        Some("https://tokendance.space/keys"),
        &["deepseek-v4-flash", "qwen3.8-flash", "kimi-k3"],
    ),
    openai(
        "siliconflow",
        tr::ai::llm::siliconflow,
        Group::Relay,
        "https://api.siliconflow.cn/v1/chat/completions",
        Some("https://cloud.siliconflow.cn/account/ak"),
        &[
            "deepseek-ai/deepseek-v4-flash",
            "deepseek-ai/deepseek-v4-pro",
        ],
    ),
    openai(
        "openrouter",
        || "OpenRouter",
        Group::Relay,
        "https://openrouter.ai/api/v1/chat/completions",
        Some("https://openrouter.ai/settings/keys"),
        &["meta-llama/llama-4-scout", "meta-llama/llama-4-maverick"],
    ),
    openai(
        "opencode-go",
        || "OpenCode Go",
        Group::Relay,
        "https://opencode.ai/zen/go/v1/chat/completions",
        Some("https://opencode.ai/auth"),
        &[
            "deepseek-v4.1-flash",
            "glm-5.3-flash",
            "kimi-k3",
            "longcat-2.0",
            "mimo-v2.6-flash",
        ],
    ),
    openai(
        "openai",
        || "OpenAI",
        Group::Provider,
        "https://api.openai.com/v1/chat/completions",
        Some("https://platform.openai.com/api-keys"),
        &["gpt-5.6-luna", "gpt-5.6-terra", "gpt-5.6-sol"],
    ),
    LlmProvider {
        id: "anthropic",
        name: || "Anthropic",
        group: Group::Provider,
        protocol: Protocol::Anthropic,
        base_url: "https://api.anthropic.com/v1/messages",
        key_url: Some("https://console.anthropic.com/settings/keys"),
        models: &["claude-haiku-4-5", "claude-sonnet-5", "claude-opus-5"],
    },
    LlmProvider {
        id: "google-gemini",
        name: || "Google Gemini",
        group: Group::Provider,
        protocol: Protocol::Gemini,
        base_url: "https://generativelanguage.googleapis.com/v1beta/models/{model-id}:generateContent",
        key_url: Some("https://aistudio.google.com/app/apikey"),
        models: &[
            "gemini-3.1-flash-lite",
            "gemini-3.5-flash",
            "gemini-3.1-pro-preview",
        ],
    },
    openai(
        "deepseek",
        || "DeepSeek",
        Group::Provider,
        "https://api.deepseek.com/v1/chat/completions",
        Some("https://platform.deepseek.com/api_keys"),
        &["deepseek-flash", "deepseek-v4-pro"],
    ),
    openai(
        "moonshot-cn",
        || "Kimi",
        Group::Provider,
        "https://api.moonshot.cn/v1/chat/completions",
        Some("https://platform.moonshot.cn/console/api-keys"),
        &["kimi-k3", "kimi-k2.6"],
    ),
    openai(
        "zhipu",
        tr::ai::llm::zhipu,
        Group::Provider,
        "https://open.bigmodel.cn/api/paas/v4/chat/completions",
        Some("https://open.bigmodel.cn/usercenter/apikeys"),
        &[],
    ),
    openai(
        "qwen-cn",
        tr::ai::llm::qwen,
        Group::Provider,
        "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions",
        Some("https://bailian.console.aliyun.com/?apiKey=1"),
        &[],
    ),
    openai(
        "doubao",
        tr::ai::llm::doubao,
        Group::Provider,
        "https://ark.cn-beijing.volces.com/api/v3/chat/completions",
        Some("https://console.volcengine.com/ark/region:ark+cn-beijing/apiKey"),
        &[],
    ),
    openai(
        "minimax-cn",
        || "MiniMax",
        Group::Provider,
        "https://api.minimaxi.com/v1/chat/completions",
        Some("https://platform.minimaxi.com/user-center/basic-information/interface-key"),
        &[],
    ),
    openai(
        "stepfun",
        tr::ai::llm::stepfun,
        Group::Provider,
        "https://api.stepfun.com/v1/chat/completions",
        Some("https://platform.stepfun.com/interface-key"),
        &[],
    ),
    openai(
        "qianfan",
        tr::ai::llm::qianfan,
        Group::Provider,
        "https://qianfan.baidubce.com/v2/chat/completions",
        Some("https://console.bce.baidu.com/iam/#/iam/apikey/list"),
        &[],
    ),
    openai(
        "hunyuan",
        tr::ai::llm::hunyuan,
        Group::Provider,
        "https://api.hunyuan.cloud.tencent.com/v1/chat/completions",
        Some("https://console.cloud.tencent.com/hunyuan/api-key"),
        &[],
    ),
    openai(
        "mimo",
        tr::ai::llm::mimo,
        Group::Provider,
        "https://api.xiaomimimo.com/v1/chat/completions",
        Some("https://platform.xiaomimimo.com/"),
        &[],
    ),
    openai(
        "xai",
        || "xAI",
        Group::Provider,
        "https://api.x.ai/v1/chat/completions",
        Some("https://console.x.ai"),
        &["grok-4.5", "grok-4.3"],
    ),
    openai(
        "mistral",
        || "Mistral",
        Group::Provider,
        "https://api.mistral.ai/v1/chat/completions",
        Some("https://console.mistral.ai/api-keys"),
        &[],
    ),
    openai(
        "groq",
        || "Groq",
        Group::Provider,
        "https://api.groq.com/openai/v1/chat/completions",
        Some("https://console.groq.com/keys"),
        &[],
    ),
    openai(
        "perplexity",
        || "Perplexity",
        Group::Provider,
        "https://api.perplexity.ai/chat/completions",
        Some("https://www.perplexity.ai/settings/api"),
        &["sonar", "sonar-pro"],
    ),
    openai(
        "nvidia",
        || "NVIDIA",
        Group::Provider,
        "https://integrate.api.nvidia.com/v1/chat/completions",
        Some("https://build.nvidia.com/settings/api-keys"),
        &[],
    ),
    openai(
        "together",
        || "Together AI",
        Group::Provider,
        "https://api.together.xyz/v1/chat/completions",
        Some("https://api.together.xyz/settings/api-keys"),
        &[],
    ),
    openai(
        "fireworks",
        || "Fireworks AI",
        Group::Provider,
        "https://api.fireworks.ai/inference/v1/chat/completions",
        Some("https://fireworks.ai/account/api-keys"),
        &[],
    ),
    openai(
        "cerebras",
        || "Cerebras",
        Group::Provider,
        "https://api.cerebras.ai/v1/chat/completions",
        Some("https://cloud.cerebras.ai/"),
        &[],
    ),
    LlmProvider {
        id: "ollama",
        name: || "Ollama",
        group: Group::Local,
        protocol: Protocol::Ollama,
        base_url: "http://127.0.0.1:11434/api/chat",
        key_url: None,
        models: &["gemma4", "qwen3.5", "gpt-oss:20b", "llama3.3"],
    },
    openai(
        "lmstudio",
        || "LM Studio",
        Group::Local,
        "http://127.0.0.1:1234/v1/chat/completions",
        None,
        &[],
    ),
];

pub fn provider(id: &str) -> Option<&'static LlmProvider> {
    PROVIDERS.iter().find(|provider| provider.id == id)
}

/// The configured providers, in catalogue order.
pub fn configured() -> Vec<&'static LlmProvider> {
    let saved = ai_store::saved_ids(Kind::Llm);
    PROVIDERS
        .iter()
        .filter(|provider| saved.iter().any(|id| id == provider.id))
        .collect()
}

/// The model the app uses: the chosen default, else the first ready one.
pub fn default_provider() -> Option<&'static LlmProvider> {
    let ready: Vec<_> = configured().into_iter().filter(|p| p.ready()).collect();
    ai_store::llm_default()
        .and_then(|id| ready.iter().copied().find(|p| p.id == id))
        .or_else(|| ready.first().copied())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredModel {
    pub id: String,
    pub display_name: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiscoveryError {
    Address,
    Unsupported,
    MissingKey,
    Authentication,
    Http(u16),
    Connection,
    Timeout,
    Response,
    Limit,
}

impl DiscoveryError {
    pub fn message(self) -> String {
        match self {
            Self::Address => tr::ai::address_invalid().to_string(),
            Self::Unsupported => tr::ai::discovery_unsupported().to_string(),
            Self::MissingKey => tr::ai::discovery_missing_key().to_string(),
            Self::Authentication => tr::ai::discovery_authentication().to_string(),
            Self::Http(status) => tr::ai::discovery_http(status),
            Self::Connection => tr::ai::discovery_connection().to_string(),
            Self::Timeout => tr::ai::discovery_timeout().to_string(),
            Self::Response => tr::ai::discovery_response().to_string(),
            Self::Limit => tr::ai::discovery_limit().to_string(),
        }
    }
}

const DISCOVERY_PAGE_BYTES: usize = 1024 * 1024;
const DISCOVERY_MAX_MODELS: usize = 4096;
const DISCOVERY_MAX_PAGES: usize = 16;

fn discovery_url(protocol: Protocol, endpoint: &str) -> Result<reqwest::Url, DiscoveryError> {
    let endpoint = endpoint.trim().replace("{model-id}", "model-placeholder");
    if !ai_store::address_allowed(&endpoint) {
        return Err(DiscoveryError::Address);
    }
    let mut url = reqwest::Url::parse(&endpoint).map_err(|_| DiscoveryError::Address)?;
    if url.fragment().is_some() {
        return Err(DiscoveryError::Address);
    }
    let path = url.path().trim_end_matches('/');
    let next_path = match protocol {
        Protocol::OpenAi => path
            .strip_suffix("/chat/completions")
            .map(|prefix| format!("{prefix}/models")),
        Protocol::Anthropic => path
            .strip_suffix("/messages")
            .map(|prefix| format!("{prefix}/models")),
        Protocol::Ollama => path
            .strip_suffix("/api/chat")
            .map(|prefix| format!("{prefix}/api/tags")),
        Protocol::Gemini => path.rsplit_once("/models/").and_then(|(prefix, model)| {
            model
                .strip_suffix(":generateContent")
                .filter(|id| !id.is_empty() && !id.contains('/'))
                .map(|_| format!("{prefix}/models"))
        }),
    }
    .ok_or(DiscoveryError::Unsupported)?;
    url.set_path(&next_path);
    Ok(url)
}

fn model_id_valid(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 && id.bytes().all(|byte| byte.is_ascii_graphic())
}

fn discovery_page(
    protocol: Protocol,
    value: &Value,
) -> Result<(Vec<DiscoveredModel>, Option<String>), DiscoveryError> {
    let rows = value
        .get(match protocol {
            Protocol::OpenAi | Protocol::Anthropic => "data",
            Protocol::Gemini | Protocol::Ollama => "models",
        })
        .and_then(Value::as_array)
        .ok_or(DiscoveryError::Response)?;
    if rows.len() > DISCOVERY_MAX_MODELS {
        return Err(DiscoveryError::Limit);
    }
    let mut models = Vec::new();
    for row in rows {
        if protocol == Protocol::Gemini
            && !row
                .get("supportedGenerationMethods")
                .and_then(Value::as_array)
                .is_some_and(|methods| {
                    methods
                        .iter()
                        .any(|method| method.as_str() == Some("generateContent"))
                })
        {
            continue;
        }
        let id = match protocol {
            Protocol::OpenAi | Protocol::Anthropic => row.get("id").and_then(Value::as_str),
            Protocol::Gemini => row
                .get("name")
                .and_then(Value::as_str)
                .and_then(|name| name.strip_prefix("models/")),
            Protocol::Ollama => row
                .get("model")
                .or_else(|| row.get("name"))
                .and_then(Value::as_str),
        }
        .filter(|id| model_id_valid(id));
        let Some(id) = id else {
            return Err(DiscoveryError::Response);
        };
        if protocol == Protocol::Gemini
            && !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
        {
            return Err(DiscoveryError::Response);
        }
        let display_name = row
            .get(match protocol {
                Protocol::Anthropic => "display_name",
                Protocol::Gemini => "displayName",
                _ => "name",
            })
            .and_then(Value::as_str)
            .filter(|name| {
                !name.is_empty()
                    && name.len() <= 512
                    && !name.chars().any(char::is_control)
                    && *name != id
            })
            .map(str::to_string);
        models.push(DiscoveredModel {
            id: id.to_string(),
            display_name,
        });
    }
    let cursor = match protocol {
        Protocol::Anthropic => match value.get("has_more").and_then(Value::as_bool) {
            Some(true) => Some(
                value
                    .get("last_id")
                    .and_then(Value::as_str)
                    .ok_or(DiscoveryError::Response)?,
            ),
            Some(false) => None,
            None => return Err(DiscoveryError::Response),
        },
        Protocol::Gemini => match value.get("nextPageToken") {
            None | Some(Value::Null) => None,
            Some(Value::String(token)) if token.is_empty() => None,
            Some(Value::String(token)) => Some(token.as_str()),
            Some(_) => return Err(DiscoveryError::Response),
        },
        _ => None,
    };
    if cursor.is_some_and(|cursor| {
        cursor.is_empty() || cursor.len() > 2048 || cursor.chars().any(char::is_control)
    }) {
        return Err(DiscoveryError::Response);
    }
    Ok((models, cursor.map(str::to_string)))
}

async fn discovery_body(mut response: reqwest::Response) -> Result<Value, DiscoveryError> {
    let status = response.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(DiscoveryError::Authentication);
    }
    if !status.is_success() {
        return Err(DiscoveryError::Http(status.as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > DISCOVERY_PAGE_BYTES as u64)
    {
        return Err(DiscoveryError::Limit);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| DiscoveryError::Connection)?
    {
        if chunk.len() > DISCOVERY_PAGE_BYTES.saturating_sub(bytes.len()) {
            return Err(DiscoveryError::Limit);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| DiscoveryError::Response)
}

async fn discover_with(
    protocol: Protocol,
    url: reqwest::Url,
    key: &str,
) -> Result<Vec<DiscoveredModel>, DiscoveryError> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| DiscoveryError::Connection)?;
    let mut result = Vec::new();
    let mut ids = HashSet::new();
    let mut cursors = HashSet::new();
    let mut cursor: Option<String> = None;
    for _ in 0..DISCOVERY_MAX_PAGES {
        let mut request = client.get(url.clone());
        request = match protocol {
            Protocol::Anthropic => request
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01")
                .query(&[("limit", "100")]),
            Protocol::Gemini => request
                .header("x-goog-api-key", key)
                .query(&[("pageSize", "100")]),
            _ if !key.is_empty() => request.bearer_auth(key),
            _ => request,
        };
        if let Some(cursor) = &cursor {
            request = request.query(&[(
                if protocol == Protocol::Anthropic {
                    "after_id"
                } else {
                    "pageToken"
                },
                cursor,
            )]);
        }
        let response = request.send().await.map_err(|error| {
            if error.is_timeout() {
                DiscoveryError::Timeout
            } else {
                DiscoveryError::Connection
            }
        })?;
        let value = discovery_body(response).await?;
        let (models, next) = discovery_page(protocol, &value)?;
        for model in models {
            if ids.insert(model.id.clone()) {
                result.push(model);
                if result.len() > DISCOVERY_MAX_MODELS {
                    return Err(DiscoveryError::Limit);
                }
            }
        }
        let Some(next) = next else {
            return Ok(result);
        };
        if !cursors.insert(next.clone()) {
            return Err(DiscoveryError::Response);
        }
        cursor = Some(next);
    }
    Err(DiscoveryError::Limit)
}

pub async fn discover_models(
    id: &str,
    endpoint: &str,
    key: &str,
) -> Result<Vec<DiscoveredModel>, DiscoveryError> {
    let provider = provider(id).ok_or(DiscoveryError::Unsupported)?;
    let url = discovery_url(provider.protocol, endpoint)?;
    if provider.needs_key() && key.trim().is_empty() {
        return Err(DiscoveryError::MissingKey);
    }
    tokio::time::timeout(
        Duration::from_secs(25),
        discover_with(provider.protocol, url, key.trim()),
    )
    .await
    .map_err(|_| DiscoveryError::Timeout)?
}

const RETRIES: u32 = 3;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

fn request(
    client: &reqwest::Client,
    provider: &LlmProvider,
    endpoint: &Endpoint,
    key: &str,
    system: &str,
    user: &str,
) -> reqwest::RequestBuilder {
    let url = endpoint
        .base_url
        .trim()
        .replace("{model-id}", endpoint.model.trim());
    let model = endpoint.model.trim();
    let builder = match provider.protocol {
        Protocol::OpenAi => client.post(url).json(&json!({
            "model": model,
            "messages": [{ "role": "system", "content": system }, { "role": "user", "content": user }],
            "stream": false,
        })),
        Protocol::Ollama => client.post(url).json(&json!({
            "model": model,
            "messages": [{ "role": "system", "content": system }, { "role": "user", "content": user }],
            "stream": false,
        })),
        Protocol::Anthropic => {
            return client
                .post(url)
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01")
                .json(&json!({
                    "model": model,
                    "max_tokens": 4096,
                    "system": system,
                    "messages": [{ "role": "user", "content": user }],
                }));
        }
        Protocol::Gemini => {
            return client.post(url).header("x-goog-api-key", key).json(&json!({
                "systemInstruction": { "parts": [{ "text": system }] },
                "contents": [{ "role": "user", "parts": [{ "text": user }] }],
            }));
        }
    };
    if key.is_empty() {
        builder
    } else {
        builder.bearer_auth(key)
    }
}

fn reply_text(protocol: Protocol, value: &Value) -> Option<String> {
    let pointer = match protocol {
        Protocol::OpenAi => "/choices/0/message/content",
        Protocol::Ollama => "/message/content",
        Protocol::Anthropic => "/content/0/text",
        Protocol::Gemini => "/candidates/0/content/parts/0/text",
    };
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn error_message(body: &str) -> String {
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

async fn chat_with(
    provider: &LlmProvider,
    endpoint: &Endpoint,
    key: &str,
    system: &str,
    user: &str,
) -> Result<String, String> {
    let name = (provider.name)();
    let client = reqwest::Client::new();
    let mut last = String::new();
    for attempt in 0..=RETRIES {
        let sent = request(&client, provider, endpoint, key, system, user)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await;
        let response = match sent {
            Ok(response) => response,
            Err(error) => {
                last = error.to_string();
                if attempt < RETRIES {
                    tokio::time::sleep(crate::asr_cloud::backoff(attempt, None)).await;
                }
                continue;
            }
        };
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("Retry-After")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<f64>().ok());
        let body = response.text().await.unwrap_or_default();
        if (200..300).contains(&status) {
            let value: Value =
                serde_json::from_str(&body).map_err(|_| tr::asr::unreadable(name))?;
            return reply_text(provider.protocol, &value).ok_or_else(|| tr::asr::unreadable(name));
        }
        let detail = error_message(&body);
        if status == 401 || status == 403 {
            return Err(tr::asr::auth_failed(name));
        }
        if status == 402 || detail.to_lowercase().contains("insufficient") {
            return Err(tr::asr::quota(name, &detail));
        }
        last = tr::asr::failed(name, status, &detail);
        if (status == 429 || status >= 500) && attempt < RETRIES {
            tokio::time::sleep(crate::asr_cloud::backoff(attempt, retry_after)).await;
            continue;
        }
        return Err(last);
    }
    Err(tr::asr::connection_failed(name, &last))
}

pub async fn chat(id: &str, system: &str, user: &str) -> Result<String, String> {
    let provider = provider(id).ok_or_else(|| format!("Unknown model provider {id}"))?;
    let endpoint = provider.endpoint();
    if !provider.ready() {
        return Err(tr::ai::model_missing().to_string());
    }
    let key = ai_store::key(Kind::Llm, id).unwrap_or_default();
    chat_with(provider, &endpoint, &key, system, user).await
}

/// Checks the address, key and model with a one-word question. Returns the
/// round trip in milliseconds.
pub fn test(id: &str, key: &str, endpoint: &Endpoint) -> Result<u128, String> {
    let provider = provider(id).ok_or_else(|| format!("Unknown model provider {id}"))?;
    if !ai_store::address_allowed(&endpoint.base_url.replace("{model-id}", "m")) {
        return Err(tr::ai::address_invalid().to_string());
    }
    if endpoint.model.trim().is_empty() {
        return Err(tr::ai::model_missing().to_string());
    }
    if provider.needs_key() && key.trim().is_empty() {
        return Err(tr::asr::no_key((provider.name)()));
    }
    let started = std::time::Instant::now();
    tokio::runtime::Handle::current().block_on(chat_with(
        provider,
        endpoint,
        key.trim(),
        "Reply with the single word OK.",
        "ping",
    ))?;
    Ok(started.elapsed().as_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};

    fn model_server(responses: Vec<String>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for response in responses {
                let started = std::time::Instant::now();
                let (mut stream, _) = loop {
                    match listener.accept() {
                        Ok(connection) => break connection,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                && started.elapsed() < Duration::from_secs(5) =>
                        {
                            std::thread::sleep(Duration::from_millis(5))
                        }
                        Err(error) => panic!("mock model request missing: {error}"),
                    }
                };
                // macOS inherits the listener's nonblocking mode on accepted sockets.
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buf = [0u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let count = stream.read(&mut buf).unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&buf[..count]);
                }
                requests.push(String::from_utf8(request).unwrap());
                stream.write_all(response.as_bytes()).unwrap();
            }
            requests
        });
        (format!("http://{address}"), server)
    }

    fn model_json(value: Value) -> String {
        let body = value.to_string();
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn request_header<'a>(request: &'a str, name: &str) -> Option<&'a str> {
        request
            .lines()
            .skip(1)
            .take_while(|line| !line.is_empty())
            .filter_map(|line| line.split_once(':'))
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.trim())
    }

    #[test]
    fn discovery_keeps_proxy_prefixes_and_rejects_ambiguous_addresses() {
        for (protocol, input, expected) in [
            (
                Protocol::OpenAi,
                "https://host.test/gateway/v1/chat/completions",
                "https://host.test/gateway/v1/models",
            ),
            (
                Protocol::Anthropic,
                "https://host.test/custom/v1/messages/",
                "https://host.test/custom/v1/models",
            ),
            (
                Protocol::Gemini,
                "https://host.test/v1beta/models/{model-id}:generateContent",
                "https://host.test/v1beta/models",
            ),
            (
                Protocol::Ollama,
                "http://127.0.0.1:11434/api/chat",
                "http://127.0.0.1:11434/api/tags",
            ),
        ] {
            assert_eq!(discovery_url(protocol, input).unwrap().as_str(), expected);
        }
        for input in [
            "https://host.test/v1?key=secret",
            "https://user:secret@host.test/v1/chat/completions",
            "https://host.test/v1/chat/completions#secret",
            "http://external.test/v1/chat/completions",
        ] {
            assert_eq!(
                discovery_url(Protocol::OpenAi, input),
                Err(DiscoveryError::Address)
            );
        }
        assert_eq!(
            discovery_url(Protocol::OpenAi, "https://host.test/custom"),
            Err(DiscoveryError::Unsupported)
        );
    }

    #[test]
    fn discovery_preserves_callable_ids_and_filters_gemini_embeddings() {
        let value = json!({"models":[
            {"name":"models/text-embedding-004", "supportedGenerationMethods":["embedContent"]},
            {"name":"models/gemini-version-001", "displayName":"Friendly name", "supportedGenerationMethods":["generateContent"]}
        ]});
        let (models, cursor) = discovery_page(Protocol::Gemini, &value).unwrap();
        assert_eq!(
            models,
            vec![DiscoveredModel {
                id: "gemini-version-001".into(),
                display_name: Some("Friendly name".into())
            }]
        );
        assert!(cursor.is_none());
        assert_eq!(
            discovery_page(
                Protocol::OpenAi,
                &json!({"data":[{"id":"organization/new-model:free"}]})
            )
            .unwrap()
            .0[0]
                .id,
            "organization/new-model:free"
        );
        assert_eq!(
            discovery_page(
                Protocol::Ollama,
                &json!({"models":[{"name":"installed:8b"}]})
            )
            .unwrap()
            .0[0]
                .id,
            "installed:8b"
        );
        assert!(discovery_page(Protocol::Gemini, &json!({"models":[{"name":"models/model?key=bad", "supportedGenerationMethods":["generateContent"]}]})).is_err());
    }

    #[tokio::test]
    async fn discovery_paginates_with_encoded_cursor_and_deduplicates_ids() {
        let (base, server) = model_server(vec![
            model_json(
                json!({"models":[{"name":"models/first", "supportedGenerationMethods":["generateContent"]}], "nextPageToken":"token+/=&value"}),
            ),
            model_json(
                json!({"models":[{"name":"models/first", "supportedGenerationMethods":["generateContent"]},{"name":"models/second", "supportedGenerationMethods":["generateContent"]}]}),
            ),
        ]);
        let models = discover_models(
            "google-gemini",
            &format!("{base}/proxy/v1beta/models/{{model-id}}:generateContent"),
            "synthetic-secret",
        )
        .await
        .unwrap();
        assert_eq!(
            models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
        let requests = server.join().unwrap();
        assert!(requests[0].starts_with("GET /proxy/v1beta/models?pageSize=100 "));
        assert_eq!(
            request_header(&requests[0], "x-goog-api-key"),
            Some("synthetic-secret")
        );
        assert!(requests[1].contains("pageToken=token%2B%2F%3D%26value"));
        assert!(
            !requests[1]
                .lines()
                .next()
                .unwrap()
                .contains("synthetic-secret")
        );
    }

    #[tokio::test]
    async fn discovery_anthropic_pagination_stops_repeated_cursor() {
        let page = json!({"data":[{"id":"claude-new","display_name":"Claude New"}],"has_more":true,"last_id":"claude-new"});
        let (base, server) = model_server(vec![model_json(page.clone()), model_json(page)]);
        let result = discover_models(
            "anthropic",
            &format!("{base}/v1/messages"),
            "synthetic-secret",
        )
        .await;
        assert_eq!(result, Err(DiscoveryError::Response));
        let requests = server.join().unwrap();
        assert_eq!(
            request_header(&requests[0], "anthropic-version"),
            Some("2023-06-01")
        );
        assert_eq!(
            request_header(&requests[0], "x-api-key"),
            Some("synthetic-secret")
        );
        assert!(requests[1].contains("after_id=claude-new"));
    }

    #[tokio::test]
    async fn discovery_redirects_and_error_bodies_cannot_forward_or_echo_credentials() {
        let destination = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        destination.set_nonblocking(true).unwrap();
        let body = "synthetic-secret";
        let (base, server) = model_server(vec![format!(
            "HTTP/1.1 302 Found\r\nLocation: http://{}/secret\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            destination.local_addr().unwrap(),
            body.len()
        )]);
        let error = discover_models("openai", &format!("{base}/v1/chat/completions"), body)
            .await
            .unwrap_err();
        assert_eq!(error, DiscoveryError::Http(302));
        assert!(!format!("{error:?}").contains(body));
        assert!(
            matches!(destination.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
        let requests = server.join().unwrap();
        assert_eq!(
            request_header(&requests[0], "authorization"),
            Some("Bearer synthetic-secret")
        );
        let (base, server) = model_server(vec![format!(
            "HTTP/1.1 401 Unauthorized\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )]);
        assert_eq!(
            discover_models("openai", &format!("{base}/chat/completions"), body).await,
            Err(DiscoveryError::Authentication)
        );
        server.join().unwrap();
    }

    #[tokio::test]
    async fn discovery_bounds_declared_and_streamed_response_sizes() {
        let (base, server) = model_server(vec![format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            DISCOVERY_PAGE_BYTES + 1
        )]);
        assert_eq!(
            discover_models("openai", &format!("{base}/chat/completions"), "synthetic").await,
            Err(DiscoveryError::Limit)
        );
        server.join().unwrap();
        let chunk = " ".repeat(DISCOVERY_PAGE_BYTES + 1);
        let (base, server) = model_server(vec![format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{chunk}\r\n0\r\n\r\n",
            chunk.len()
        )]);
        assert_eq!(
            discover_models("openai", &format!("{base}/chat/completions"), "synthetic").await,
            Err(DiscoveryError::Limit)
        );
        server.join().unwrap();
    }

    #[tokio::test]
    async fn dropping_discovery_cancels_a_pending_http_request() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (accepted, request_started) = tokio::sync::oneshot::channel();
        let (closed, socket_closed) = tokio::sync::oneshot::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 1024];
            while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                let count = stream.read(&mut buffer).unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&buffer[..count]);
            }
            accepted.send(()).unwrap();
            let result = stream.read(&mut buffer).map_err(|error| error.kind());
            closed.send(result).unwrap();
        });
        let endpoint = format!("http://{address}/chat/completions");
        let mut request = Box::pin(discover_models("openai", &endpoint, "synthetic-secret"));
        tokio::select! {
            started = request_started => started.unwrap(),
            result = &mut request => panic!("discovery ended before cancellation: {result:?}"),
        }
        drop(request);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(6), socket_closed)
                .await
                .unwrap()
                .unwrap(),
            Ok(0)
        );
        server.join().unwrap();
    }

    #[test]
    fn catalogue_ids_are_unique_and_addresses_are_allowed() {
        let mut ids: Vec<&str> = PROVIDERS.iter().map(|p| p.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), PROVIDERS.len());
        for provider in PROVIDERS {
            assert!(
                ai_store::address_allowed(&provider.base_url.replace("{model-id}", "m")),
                "{}",
                provider.id
            );
        }
        assert!(!provider("ollama").unwrap().needs_key());
        assert!(provider("deepseek").unwrap().needs_key());
    }

    #[test]
    fn each_protocol_reads_its_own_reply_shape() {
        let cases = [
            (
                Protocol::OpenAi,
                json!({"choices":[{"message":{"content":"OK"}}]}),
            ),
            (Protocol::Ollama, json!({"message":{"content":"OK"}})),
            (
                Protocol::Anthropic,
                json!({"content":[{"type":"text","text":"OK"}]}),
            ),
            (
                Protocol::Gemini,
                json!({"candidates":[{"content":{"parts":[{"text":"OK"}]}}]}),
            ),
        ];
        for (protocol, value) in cases {
            assert_eq!(reply_text(protocol, &value).as_deref(), Some("OK"));
        }
        assert_eq!(
            error_message(r#"{"error":{"message":"bad key"}}"#),
            "bad key"
        );
        assert_eq!(error_message("plain"), "plain");
    }
}
