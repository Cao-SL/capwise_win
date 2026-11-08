//! Stand-in for `src/auth.rs` when the `cap-cloud` feature is off: there is no
//! account, so every authenticated request fails as signed out.

#![allow(dead_code)]

use serde_json::Value;

pub const PRICING_URL: &str = "https://cap.so/pricing?ref=desktop";

#[derive(Debug)]
pub enum AuthApiError {
    InvalidAuthentication,
    UpgradeRequired,
    Timeout,
    Status(reqwest::StatusCode, String),
    Other(String),
}

impl AuthApiError {
    pub fn message(&self) -> String {
        match self {
            Self::InvalidAuthentication => {
                "User is not authenticated or credentials have expired!".into()
            }
            Self::UpgradeRequired => {
                "User needs to upgrade their account to use this feature!".into()
            }
            Self::Timeout => "The request has timed out".into(),
            Self::Status(_, message) | Self::Other(message) => message.clone(),
        }
    }
}

impl std::fmt::Display for AuthApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

pub fn submit_deep_link(_url: &str) {}

pub fn sign_out() -> bool {
    crate::store::set_auth(None)
}

pub fn server_url() -> String {
    let url = crate::store::GeneralSettings::load().server_url;
    if url.trim().is_empty() {
        crate::store::DEFAULT_SERVER_URL.to_string()
    } else {
        url.trim_end_matches('/').to_string()
    }
}

pub async fn authed_request(
    _method: reqwest::Method,
    _path: &str,
    _body: Option<Value>,
) -> Result<reqwest::Response, AuthApiError> {
    Err(AuthApiError::InvalidAuthentication)
}

pub async fn update_auth_plan() -> Result<(), AuthApiError> {
    Err(AuthApiError::InvalidAuthentication)
}
