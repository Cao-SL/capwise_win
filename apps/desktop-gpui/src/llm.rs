//! Large language models: the provider catalogue from qiaomu-clipper's
//! `provider-catalog.ts` and one chat call over the four request styles they
//! use. Settings live in [`crate::ai_store`].

use std::time::Duration;

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
    openai("tokendance", tr::ai::llm::tokendance, Group::Relay, "https://tokendance.space/gateway/v1/chat/completions", Some("https://tokendance.space/keys"), &["deepseek-v4-flash", "qwen3.8-flash", "kimi-k3"]),
    openai("siliconflow", tr::ai::llm::siliconflow, Group::Relay, "https://api.siliconflow.cn/v1/chat/completions", Some("https://cloud.siliconflow.cn/account/ak"), &["deepseek-ai/deepseek-v4-flash", "deepseek-ai/deepseek-v4-pro"]),
    openai("openrouter", || "OpenRouter", Group::Relay, "https://openrouter.ai/api/v1/chat/completions", Some("https://openrouter.ai/settings/keys"), &["meta-llama/llama-4-scout", "meta-llama/llama-4-maverick"]),
    openai("opencode-go", || "OpenCode Go", Group::Relay, "https://opencode.ai/zen/go/v1/chat/completions", Some("https://opencode.ai/auth"), &["deepseek-v4.1-flash", "glm-5.3-flash", "kimi-k3", "longcat-2.0", "mimo-v2.6-flash"]),
    openai("openai", || "OpenAI", Group::Provider, "https://api.openai.com/v1/chat/completions", Some("https://platform.openai.com/api-keys"), &["gpt-5.6-luna", "gpt-5.6-terra", "gpt-5.6-sol"]),
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
        models: &["gemini-3.1-flash-lite", "gemini-3.5-flash", "gemini-3.1-pro-preview"],
    },
    openai("deepseek", || "DeepSeek", Group::Provider, "https://api.deepseek.com/v1/chat/completions", Some("https://platform.deepseek.com/api_keys"), &["deepseek-flash", "deepseek-v4-pro"]),
    openai("moonshot-cn", || "Kimi", Group::Provider, "https://api.moonshot.cn/v1/chat/completions", Some("https://platform.moonshot.cn/console/api-keys"), &["kimi-k3", "kimi-k2.6"]),
    openai("zhipu", tr::ai::llm::zhipu, Group::Provider, "https://open.bigmodel.cn/api/paas/v4/chat/completions", Some("https://open.bigmodel.cn/usercenter/apikeys"), &[]),
    openai("qwen-cn", tr::ai::llm::qwen, Group::Provider, "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions", Some("https://bailian.console.aliyun.com/?apiKey=1"), &[]),
    openai("doubao", tr::ai::llm::doubao, Group::Provider, "https://ark.cn-beijing.volces.com/api/v3/chat/completions", Some("https://console.volcengine.com/ark/region:ark+cn-beijing/apiKey"), &[]),
    openai("minimax-cn", || "MiniMax", Group::Provider, "https://api.minimaxi.com/v1/chat/completions", Some("https://platform.minimaxi.com/user-center/basic-information/interface-key"), &[]),
    openai("stepfun", tr::ai::llm::stepfun, Group::Provider, "https://api.stepfun.com/v1/chat/completions", Some("https://platform.stepfun.com/interface-key"), &[]),
    openai("qianfan", tr::ai::llm::qianfan, Group::Provider, "https://qianfan.baidubce.com/v2/chat/completions", Some("https://console.bce.baidu.com/iam/#/iam/apikey/list"), &[]),
    openai("hunyuan", tr::ai::llm::hunyuan, Group::Provider, "https://api.hunyuan.cloud.tencent.com/v1/chat/completions", Some("https://console.cloud.tencent.com/hunyuan/api-key"), &[]),
    openai("mimo", tr::ai::llm::mimo, Group::Provider, "https://api.xiaomimimo.com/v1/chat/completions", Some("https://platform.xiaomimimo.com/"), &[]),
    openai("xai", || "xAI", Group::Provider, "https://api.x.ai/v1/chat/completions", Some("https://console.x.ai"), &["grok-4.5", "grok-4.3"]),
    openai("mistral", || "Mistral", Group::Provider, "https://api.mistral.ai/v1/chat/completions", Some("https://console.mistral.ai/api-keys"), &[]),
    openai("groq", || "Groq", Group::Provider, "https://api.groq.com/openai/v1/chat/completions", Some("https://console.groq.com/keys"), &[]),
    openai("perplexity", || "Perplexity", Group::Provider, "https://api.perplexity.ai/chat/completions", Some("https://www.perplexity.ai/settings/api"), &["sonar", "sonar-pro"]),
    openai("nvidia", || "NVIDIA", Group::Provider, "https://integrate.api.nvidia.com/v1/chat/completions", Some("https://build.nvidia.com/settings/api-keys"), &[]),
    openai("together", || "Together AI", Group::Provider, "https://api.together.xyz/v1/chat/completions", Some("https://api.together.xyz/settings/api-keys"), &[]),
    openai("fireworks", || "Fireworks AI", Group::Provider, "https://api.fireworks.ai/inference/v1/chat/completions", Some("https://fireworks.ai/account/api-keys"), &[]),
    openai("cerebras", || "Cerebras", Group::Provider, "https://api.cerebras.ai/v1/chat/completions", Some("https://cloud.cerebras.ai/"), &[]),
    LlmProvider {
        id: "ollama",
        name: || "Ollama",
        group: Group::Local,
        protocol: Protocol::Ollama,
        base_url: "http://127.0.0.1:11434/api/chat",
        key_url: None,
        models: &["gemma4", "qwen3.5", "gpt-oss:20b", "llama3.3"],
    },
    openai("lmstudio", || "LM Studio", Group::Local, "http://127.0.0.1:1234/v1/chat/completions", None, &[]),
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
    let url = endpoint.base_url.trim().replace("{model-id}", endpoint.model.trim());
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
    value.pointer(pointer).and_then(Value::as_str).map(str::to_string)
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
            return reply_text(provider.protocol, &value)
                .ok_or_else(|| tr::asr::unreadable(name));
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
            (Protocol::OpenAi, json!({"choices":[{"message":{"content":"OK"}}]})),
            (Protocol::Ollama, json!({"message":{"content":"OK"}})),
            (Protocol::Anthropic, json!({"content":[{"type":"text","text":"OK"}]})),
            (Protocol::Gemini, json!({"candidates":[{"content":{"parts":[{"text":"OK"}]}}]})),
        ];
        for (protocol, value) in cases {
            assert_eq!(reply_text(protocol, &value).as_deref(), Some("OK"));
        }
        assert_eq!(error_message(r#"{"error":{"message":"bad key"}}"#), "bad key");
        assert_eq!(error_message("plain"), "plain");
    }
}
