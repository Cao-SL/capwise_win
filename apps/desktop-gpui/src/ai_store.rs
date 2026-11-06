//! Where AI service settings live: each service's address and model in the
//! app store, its API key in the login keychain.

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex, PoisonError},
};

use serde_json::{Map, Value, json};

use crate::{i18n::tr, store};

const SECTION: &str = "capwise_ai";
const LLM_DEFAULT: &str = "llmDefault";
const KEYCHAIN_SERVICE: &str = "com.capwise.app.ai";
const LEGACY_KEYCHAIN_SERVICE: &str = "com.capwise.app.asr";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Asr,
    Llm,
}

impl Kind {
    fn slug(self) -> &'static str {
        match self {
            Self::Asr => "asr",
            Self::Llm => "llm",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Endpoint {
    pub base_url: String,
    pub model: String,
}

fn account(kind: Kind, id: &str) -> String {
    format!("{}.{id}", kind.slug())
}

static KEYS: LazyLock<Mutex<HashMap<String, Option<String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn keys() -> std::sync::MutexGuard<'static, HashMap<String, Option<String>>> {
    KEYS.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(target_os = "macos")]
fn keychain_get(service: &str, account: &str) -> Option<String> {
    security_framework::passwords::get_generic_password(service, account)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
}

#[cfg(not(target_os = "macos"))]
fn keychain_get(_service: &str, _account: &str) -> Option<String> {
    None
}

fn read_key(kind: Kind, id: &str) -> Option<String> {
    let found = keychain_get(KEYCHAIN_SERVICE, &account(kind, id));
    // The first Doubao build kept its key under its own keychain service.
    #[cfg(target_os = "macos")]
    if found.is_none()
        && kind == Kind::Asr
        && id == "doubao"
        && let Some(legacy) = keychain_get(LEGACY_KEYCHAIN_SERVICE, "doubao")
    {
        if set_key_uncached(kind, id, &legacy).is_ok() {
            let _ = security_framework::passwords::delete_generic_password(
                LEGACY_KEYCHAIN_SERVICE,
                "doubao",
            );
        }
        return Some(legacy);
    }
    found
}

/// Cached, so the settings and editor can ask on every frame.
pub fn key(kind: Kind, id: &str) -> Option<String> {
    let account = account(kind, id);
    if let Some(cached) = keys().get(&account) {
        return cached.clone();
    }
    let value = read_key(kind, id);
    keys().insert(account, value.clone());
    value
}

/// The saved key's last four characters, for showing that one is saved.
pub fn key_tail(kind: Kind, id: &str) -> Option<String> {
    key(kind, id).map(|key| {
        let chars: Vec<char> = key.chars().collect();
        let tail: String = chars[chars.len().saturating_sub(4)..].iter().collect();
        format!("••••{tail}")
    })
}

fn set_key_uncached(kind: Kind, id: &str, key: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        security_framework::passwords::set_generic_password(
            KEYCHAIN_SERVICE,
            &account(kind, id),
            key.as_bytes(),
        )
        .map_err(|error| format!("{}: {error}", tr::asr::keychain_failed()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (kind, id, key);
        Err(tr::asr::keychain_failed().to_string())
    }
}

pub fn set_key(kind: Kind, id: &str, key: &str) -> Result<(), String> {
    let key = key.trim();
    if key.chars().any(char::is_whitespace) {
        return Err(tr::asr::invalid_key().to_string());
    }
    if key.is_empty() {
        delete_key(kind, id);
        return Ok(());
    }
    set_key_uncached(kind, id, key)?;
    keys().insert(account(kind, id), Some(key.to_string()));
    Ok(())
}

fn delete_key(kind: Kind, id: &str) {
    #[cfg(target_os = "macos")]
    let _ = security_framework::passwords::delete_generic_password(
        KEYCHAIN_SERVICE,
        &account(kind, id),
    );
    keys().insert(account(kind, id), None);
}

fn section() -> Map<String, Value> {
    store::store_section(SECTION)
}

pub fn endpoint(kind: Kind, id: &str) -> Option<Endpoint> {
    let entry = section().get(kind.slug())?.get(id)?.clone();
    Some(Endpoint {
        base_url: entry.get("baseUrl")?.as_str()?.to_string(),
        model: entry.get("model")?.as_str()?.to_string(),
    })
}

/// The services of `kind` that have saved settings, in the order saved.
pub fn saved_ids(kind: Kind) -> Vec<String> {
    section()
        .get(kind.slug())
        .and_then(Value::as_object)
        .map(|entries| entries.keys().cloned().collect())
        .unwrap_or_default()
}

pub fn set_endpoint(kind: Kind, id: &str, endpoint: &Endpoint) -> bool {
    store::update_store_section(SECTION, |section| {
        let entries = section
            .entry(kind.slug().to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !entries.is_object() {
            *entries = Value::Object(Map::new());
        }
        entries.as_object_mut().map(|entries| {
            entries.insert(
                id.to_string(),
                json!({ "baseUrl": endpoint.base_url.trim(), "model": endpoint.model.trim() }),
            )
        });
        true
    })
}

/// Forgets a service: its settings and its key.
pub fn remove(kind: Kind, id: &str) {
    delete_key(kind, id);
    store::update_store_section(SECTION, |section| {
        if let Some(entries) = section.get_mut(kind.slug()).and_then(Value::as_object_mut) {
            entries.remove(id);
        }
        if kind == Kind::Llm && section.get(LLM_DEFAULT).and_then(Value::as_str) == Some(id) {
            section.remove(LLM_DEFAULT);
        }
        true
    });
}

pub fn llm_default() -> Option<String> {
    section()
        .get(LLM_DEFAULT)
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub fn set_llm_default(id: &str) -> bool {
    store::set_store_setting(SECTION, LLM_DEFAULT, Value::String(id.to_string()))
}

/// An address the app may send audio or text to: https, or plain http to
/// this computer only.
pub fn address_allowed(base_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url.trim()) else {
        return false;
    };
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]" | "::1"));
    url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.host_str().is_some()
        && (url.scheme() == "https" || (url.scheme() == "http" && local))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_or_this_computer_is_allowed() {
        assert!(address_allowed("https://api.siliconflow.cn/v1"));
        assert!(address_allowed("http://127.0.0.1:8765/v1"));
        assert!(address_allowed("http://localhost:11434/api/chat"));
        assert!(!address_allowed("http://example.com/v1"));
        assert!(!address_allowed("https://user:pw@example.com"));
        assert!(!address_allowed("https://example.com/v1?key=1"));
        assert!(!address_allowed("not a url"));
    }

    #[test]
    fn accounts_are_namespaced_by_kind() {
        assert_eq!(account(Kind::Asr, "doubao"), "asr.doubao");
        assert_eq!(account(Kind::Llm, "deepseek"), "llm.deepseek");
    }
}
