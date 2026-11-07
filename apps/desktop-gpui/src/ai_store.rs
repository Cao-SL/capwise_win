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
    Stock,
    Image,
}

impl Kind {
    fn slug(self) -> &'static str {
        match self {
            Self::Asr => "asr",
            Self::Llm => "llm",
            Self::Stock => "stock",
            Self::Image => "image",
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
    cache_key_if_unknown(account, value)
}

fn cache_key_if_unknown(account: String, value: Option<String>) -> Option<String> {
    keys().entry(account).or_insert(value).clone()
}

pub fn key_available_cached(kind: Kind, id: &str) -> Option<bool> {
    keys().get(&account(kind, id)).map(Option::is_some)
}

pub fn cached_key_tail(kind: Kind, id: &str) -> Option<Option<String>> {
    keys().get(&account(kind, id)).map(|value| {
        value.as_ref().map(|key| {
            let tail: String = key
                .chars()
                .rev()
                .take(4)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            format!("••••{tail}")
        })
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
    if kind == Kind::Stock && !key.trim().is_empty() && !stock_key_is_valid(key) {
        return Err(tr::asr::invalid_key().to_string());
    }
    let key = key.trim();
    if key.chars().any(char::is_whitespace) {
        return Err(tr::asr::invalid_key().to_string());
    }
    if key.is_empty() {
        return delete_key(kind, id);
    }
    set_key_uncached(kind, id, key)?;
    keys().insert(account(kind, id), Some(key.to_string()));
    Ok(())
}

pub fn stock_key_is_valid(key: &str) -> bool {
    let trimmed = key.trim();
    (8..=512).contains(&trimmed.len())
        && !key.chars().any(char::is_control)
        && trimmed.bytes().all(|byte| byte.is_ascii_graphic())
}

fn delete_key_with(
    kind: Kind,
    id: &str,
    delete: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    delete()?;
    keys().insert(account(kind, id), None);
    Ok(())
}

fn delete_key(kind: Kind, id: &str) -> Result<(), String> {
    delete_key_with(kind, id, || {
        #[cfg(target_os = "macos")]
        {
            match security_framework::passwords::delete_generic_password(
                KEYCHAIN_SERVICE,
                &account(kind, id),
            ) {
                Ok(()) => Ok(()),
                Err(error) if error.code() == -25300 => Ok(()),
                Err(_) => Err(tr::asr::keychain_failed().to_string()),
            }
        }
        #[cfg(not(target_os = "macos"))]
        Err(tr::asr::keychain_failed().to_string())
    })
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
    if let Err(error) = remove_checked(kind, id) {
        tracing::warn!(%error, "Could not remove service credentials");
    }
}

pub fn remove_checked(kind: Kind, id: &str) -> Result<(), String> {
    delete_key(kind, id)?;
    if kind == Kind::Stock {
        return Ok(());
    }
    let saved = store::update_store_section(SECTION, |section| {
        if let Some(entries) = section.get_mut(kind.slug()).and_then(Value::as_object_mut) {
            entries.remove(id);
        }
        if kind == Kind::Llm && section.get(LLM_DEFAULT).and_then(Value::as_str) == Some(id) {
            section.remove(LLM_DEFAULT);
        }
        true
    });
    if saved {
        Ok(())
    } else {
        Err(tr::asr::keychain_failed().to_string())
    }
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
    let local = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
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
    fn cached_key_tail_distinguishes_unknown_missing_and_masked_without_loading() {
        let id = "synthetic-image-cache-test";
        let account = account(Kind::Image, id);
        let _ = keys().remove(&account);
        assert_eq!(cached_key_tail(Kind::Image, id), None);
        let _ = keys().insert(account.clone(), None);
        assert_eq!(cached_key_tail(Kind::Image, id), Some(None));
        let _ = keys().insert(account.clone(), Some("synthetic-only-1234".into()));
        assert_eq!(
            cached_key_tail(Kind::Image, id),
            Some(Some("••••1234".into()))
        );
        assert_eq!(cached_key_tail(Kind::Llm, id), None);
        let _ = keys().remove(&account);
    }

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
        assert_eq!(account(Kind::Stock, "unsplash"), "stock.unsplash");
        assert_eq!(account(Kind::Stock, "pixabay"), "stock.pixabay");
        assert_ne!(
            account(Kind::Stock, "same-provider"),
            account(Kind::Llm, "same-provider")
        );
        assert_ne!(
            account(Kind::Stock, "same-provider"),
            account(Kind::Asr, "same-provider")
        );
    }

    #[test]
    fn stock_credentials_reject_empty_short_control_and_non_ascii_input() {
        assert!(stock_key_is_valid("  synthetic-stock-key_123  "));
        assert!(stock_key_is_valid("synthetic&reserved=value"));
        for key in [
            "",
            "short",
            "synthetic key",
            "synthetic\nkey",
            "\tsynthetic-key",
            "测试密钥12345678",
        ] {
            assert!(!stock_key_is_valid(key));
        }
        assert!(!stock_key_is_valid(&"x".repeat(513)));
    }

    #[test]
    fn stock_cached_key_availability_preserves_unknown_missing_and_available() {
        let id = "synthetic-cached-availability";
        let account = account(Kind::Stock, id);
        assert_eq!(key_available_cached(Kind::Stock, id), None);
        keys().insert(account.clone(), None);
        assert_eq!(key_available_cached(Kind::Stock, id), Some(false));
        keys().insert(account.clone(), Some("synthetic-credential".into()));
        assert_eq!(key_available_cached(Kind::Stock, id), Some(true));
        assert_eq!(key_available_cached(Kind::Llm, id), None);
        let _ = keys().remove(&account);
    }

    #[test]
    fn stock_late_keychain_read_preserves_newer_saved_or_removed_credential() {
        let account = account(Kind::Stock, "synthetic-late-credential");
        let other_account = super::account(Kind::Stock, "synthetic-unrelated-credential");
        keys().insert(
            other_account.clone(),
            Some("unrelated-synthetic-credential".into()),
        );
        keys().insert(account.clone(), Some("new-synthetic-credential".into()));
        assert_eq!(
            cache_key_if_unknown(account.clone(), Some("old-synthetic-credential".into())),
            Some("new-synthetic-credential".into())
        );
        keys().insert(account.clone(), None);
        assert_eq!(
            cache_key_if_unknown(account.clone(), Some("old-synthetic-credential".into())),
            None
        );
        assert_eq!(
            keys().get(&other_account),
            Some(&Some("unrelated-synthetic-credential".into()))
        );
        let _ = keys().remove(&account);
        let _ = keys().remove(&other_account);
    }

    #[test]
    fn rejected_keychain_deletion_keeps_the_cached_credential() {
        let id = "synthetic-delete-failure";
        keys().insert(
            account(Kind::Stock, id),
            Some("synthetic-credential".into()),
        );
        assert!(delete_key_with(Kind::Stock, id, || Err("synthetic failure".into())).is_err());
        assert_eq!(
            keys().get(&account(Kind::Stock, id)),
            Some(&Some("synthetic-credential".into()))
        );
        assert!(delete_key_with(Kind::Stock, id, || Ok(())).is_ok());
        assert_eq!(keys().get(&account(Kind::Stock, id)), Some(&None));
        let _ = keys().remove(&account(Kind::Stock, id));
    }
}
