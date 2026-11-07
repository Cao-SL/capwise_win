use std::{
    fmt,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use reqwest::{Client, RequestBuilder, Response, Url, header};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

const JSON_LIMIT: usize = 4 * 1024 * 1024;
const PHOTO_LIMIT: usize = 64 * 1024 * 1024;
const VIDEO_LIMIT: usize = 256 * 1024 * 1024;
const PIXABAY_CACHE_SECONDS: u64 = 24 * 60 * 60;
const UNSPLASH_CACHE_SECONDS: u64 = 60 * 60;
const CACHE_VERSION: u32 = 1;
const DESCRIPTION_LIMIT: usize = 4096;
const AUTHOR_LIMIT: usize = 256;
const URL_LIMIT: usize = 4096;
const ASSET_METADATA_LIMIT: usize = 48 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Provider {
    Unsplash,
    Pixabay,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MediaKind {
    Photo,
    Video,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Orientation {
    #[default]
    Any,
    Landscape,
    Portrait,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchRequest {
    pub provider: Provider,
    pub kind: MediaKind,
    pub query: String,
    pub page: u32,
    pub per_page: u32,
    pub orientation: Orientation,
}

#[derive(Clone, Copy, Default)]
pub struct Credentials<'a> {
    pub unsplash: Option<&'a str>,
    pub pixabay: Option<&'a str>,
}

impl Credentials<'_> {
    fn key(&self, provider: Provider) -> Result<&str, StockError> {
        let key = match provider {
            Provider::Unsplash => self.unsplash,
            Provider::Pixabay => self.pixabay,
        }
        .map(str::trim)
        .filter(|key| !key.is_empty() && key.len() <= 512 && !key.chars().any(char::is_control))
        .ok_or(StockError::Credentials)?;
        Ok(key)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attribution {
    pub author: String,
    pub author_url: String,
    pub source_url: String,
    pub license_name: String,
    pub license_url: String,
    pub api_attribution_required: bool,
    pub search_source_credit_required: bool,
    pub additional_rights_may_apply: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rendition {
    pub label: String,
    pub url: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub approximate_bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Asset {
    pub provider: Provider,
    pub kind: MediaKind,
    pub id: String,
    pub description: String,
    pub thumbnail_url: Option<String>,
    pub renditions: Vec<Rendition>,
    pub duration_seconds: Option<u32>,
    pub attribution: Attribution,
    pub download_location: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RateLimit {
    pub limit: Option<u64>,
    pub remaining: Option<u64>,
    pub reset_seconds: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchPage {
    pub provider: Provider,
    pub kind: MediaKind,
    pub page: u32,
    pub per_page: u32,
    pub total: u64,
    pub available: u64,
    pub next_page: Option<u32>,
    pub fetched_at: u64,
    pub expires_at: u64,
    pub from_cache: bool,
    pub rate_limit: RateLimit,
    pub assets: Vec<Asset>,
}

pub struct DownloadedMedia {
    pub bytes: Vec<u8>,
    pub extension: &'static str,
    pub asset: Asset,
    pub unsplash_download_tracked: bool,
}

pub enum DownloadOutcome {
    Ready(DownloadedMedia),
    NeedsTracking {
        media: DownloadedMedia,
        error: StockError,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum StockError {
    Credentials,
    InvalidSearch,
    UnsupportedKind,
    InvalidResponse,
    UntrustedUrl,
    Network,
    Timeout,
    Http {
        status: u16,
        retry_after_seconds: Option<u64>,
    },
    TooLarge,
    UnsupportedMedia,
    Cache,
    Clock,
    Tracking(Box<StockError>),
}

impl fmt::Display for StockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Credentials => {
                formatter.write_str("Configure a valid API credential for this provider")
            }
            Self::InvalidSearch => formatter.write_str("Invalid search query or pagination"),
            Self::UnsupportedKind => {
                formatter.write_str("This provider does not support the requested media kind")
            }
            Self::InvalidResponse => {
                formatter.write_str("The media provider returned an invalid response")
            }
            Self::UntrustedUrl => {
                formatter.write_str("The media URL is outside the supported provider domains")
            }
            Self::Network => formatter.write_str("The media request failed"),
            Self::Timeout => formatter.write_str("The media request timed out"),
            Self::Http { status, .. } => {
                write!(formatter, "The media provider returned HTTP {status}")
            }
            Self::TooLarge => formatter.write_str("The media exceeds the download size limit"),
            Self::UnsupportedMedia => {
                formatter.write_str("The downloaded content is not a supported image or video")
            }
            Self::Cache => formatter.write_str("The media search cache could not be read or saved"),
            Self::Clock => {
                formatter.write_str("The system clock cannot date the media search cache")
            }
            Self::Tracking(error) => write!(
                formatter,
                "The required Unsplash download event failed: {error}"
            ),
        }
    }
}

impl std::error::Error for StockError {}

impl StockError {
    pub(crate) fn retry_wait_seconds(&self) -> Option<u64> {
        match self {
            Self::Http {
                status: 429,
                retry_after_seconds,
            } => Some(retry_after_seconds.unwrap_or(60).clamp(1, 86_400)),
            Self::Tracking(cause) => cause.retry_wait_seconds(),
            _ => None,
        }
    }
}

fn network_error(error: reqwest::Error) -> StockError {
    if error.is_timeout() {
        StockError::Timeout
    } else {
        StockError::Network
    }
}

pub struct StockMediaClient {
    http: Client,
    cache_dir: PathBuf,
    search_lock: tokio::sync::Mutex<()>,
    thumbnail_slots: tokio::sync::Semaphore,
}

impl StockMediaClient {
    pub fn new(cache_dir: impl Into<PathBuf>) -> Result<Self, StockError> {
        let cache_dir = cache_dir.into().join("stock-media-v1");
        std::fs::create_dir_all(&cache_dir).map_err(|_| StockError::Cache)?;
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(90))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("Capwise/stock-media")
            .build()
            .map_err(network_error)?;
        Ok(Self {
            http,
            cache_dir,
            search_lock: tokio::sync::Mutex::new(()),
            thumbnail_slots: tokio::sync::Semaphore::new(3),
        })
    }

    pub async fn search(
        &self,
        credentials: &Credentials<'_>,
        mut request: SearchRequest,
    ) -> Result<SearchPage, StockError> {
        request.query = request.query.trim().to_string();
        validate_search(&request)?;
        credentials.key(request.provider)?;
        let _guard = self.search_lock.lock().await;
        let now = unix_seconds()?;
        let cache_path = self.cache_dir.join(cache_filename(&request)?);
        if let Some(mut page) = read_cache(&cache_path, &request, now)? {
            page.from_cache = true;
            return Ok(page);
        }
        let response = self
            .search_request(credentials, &request)?
            .send()
            .await
            .map_err(network_error)?;
        validate_api_response_url(response.url(), request.provider)?;
        let rate_limit = rate_limit(response.headers(), request.provider);
        let bytes = api_bytes(response).await?;
        let mut page = parse_search(&request, &bytes, now)?;
        page.rate_limit = rate_limit;
        save_cache(&cache_path, &request, &page)?;
        Ok(page)
    }

    fn search_request(
        &self,
        credentials: &Credentials<'_>,
        request: &SearchRequest,
    ) -> Result<RequestBuilder, StockError> {
        let key = credentials.key(request.provider)?;
        let mut url = Url::parse(match (request.provider, request.kind) {
            (Provider::Unsplash, MediaKind::Photo) => "https://api.unsplash.com/search/photos",
            (Provider::Unsplash, MediaKind::Video) => return Err(StockError::UnsupportedKind),
            (Provider::Pixabay, MediaKind::Photo) => "https://pixabay.com/api/",
            (Provider::Pixabay, MediaKind::Video) => "https://pixabay.com/api/videos/",
        })
        .map_err(|_| StockError::UntrustedUrl)?;
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("page", &request.page.to_string())
                .append_pair("per_page", &request.per_page.to_string());
            match request.provider {
                Provider::Unsplash => {
                    query
                        .append_pair("query", request.query.trim())
                        .append_pair("content_filter", "high");
                    match request.orientation {
                        Orientation::Landscape => {
                            query.append_pair("orientation", "landscape");
                        }
                        Orientation::Portrait => {
                            query.append_pair("orientation", "portrait");
                        }
                        Orientation::Any => {}
                    }
                }
                Provider::Pixabay => {
                    query
                        .append_pair("key", key)
                        .append_pair("q", request.query.trim())
                        .append_pair("safesearch", "true");
                    if request.kind == MediaKind::Photo {
                        query.append_pair("image_type", "photo");
                        match request.orientation {
                            Orientation::Landscape => {
                                query.append_pair("orientation", "horizontal");
                            }
                            Orientation::Portrait => {
                                query.append_pair("orientation", "vertical");
                            }
                            Orientation::Any => {}
                        }
                    }
                }
            }
        }
        let builder = self
            .http
            .get(url)
            .timeout(Duration::from_secs(30))
            .header(header::ACCEPT, "application/json");
        if request.provider == Provider::Unsplash {
            authorize_unsplash(builder, key)
        } else {
            Ok(builder)
        }
    }

    pub async fn thumbnail_bytes(&self, asset: &Asset) -> Result<Vec<u8>, StockError> {
        let _slot = self
            .thumbnail_slots
            .acquire()
            .await
            .map_err(|_| StockError::Network)?;
        tokio::time::timeout(Duration::from_secs(12), self.thumbnail_bytes_inner(asset))
            .await
            .map_err(|_| StockError::Timeout)?
    }

    async fn thumbnail_bytes_inner(&self, asset: &Asset) -> Result<Vec<u8>, StockError> {
        validate_asset(asset)?;
        let url = asset
            .thumbnail_url
            .as_deref()
            .ok_or(StockError::InvalidResponse)?;
        let response = self.media_response(asset.provider, url).await?;
        ensure_success(&response)?;
        let declared = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let bytes = bounded_bytes(response, 2 * 1024 * 1024).await?;
        identify_media(MediaKind::Photo, &declared, &bytes)?;
        Ok(bytes)
    }

    pub async fn download_with_progress(
        &self,
        credentials: &Credentials<'_>,
        asset: &Asset,
        rendition: usize,
        byte_limit: usize,
        progress: impl FnMut(u64, Option<u64>) + Send,
    ) -> Result<DownloadOutcome, StockError> {
        validate_asset(asset)?;
        let rendition = asset
            .renditions
            .get(rendition)
            .ok_or(StockError::InvalidResponse)?;
        let limit = byte_limit.min(match asset.kind {
            MediaKind::Photo => PHOTO_LIMIT,
            MediaKind::Video => VIDEO_LIMIT,
        });
        if limit == 0
            || rendition
                .approximate_bytes
                .is_some_and(|size| size > limit as u64)
        {
            return Err(StockError::TooLarge);
        }
        if asset.provider == Provider::Unsplash {
            credentials.key(Provider::Unsplash)?;
        }
        let response = self.media_response(asset.provider, &rendition.url).await?;
        ensure_success(&response)?;
        let declared = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let bytes = bounded_bytes_with_progress(response, limit, progress).await?;
        let (_, extension) = identify_media(asset.kind, &declared, &bytes)?;
        let mut media = DownloadedMedia {
            bytes,
            extension,
            asset: asset.clone(),
            unsplash_download_tracked: false,
        };
        match self.retry_download_tracking(credentials, &mut media).await {
            Ok(()) => Ok(DownloadOutcome::Ready(media)),
            Err(error) => Ok(DownloadOutcome::NeedsTracking { media, error }),
        }
    }

    pub async fn retry_download_tracking(
        &self,
        credentials: &Credentials<'_>,
        media: &mut DownloadedMedia,
    ) -> Result<(), StockError> {
        if media.asset.provider != Provider::Unsplash || media.unsplash_download_tracked {
            return Ok(());
        }
        validate_asset(&media.asset)?;
        self.track_unsplash(credentials, &media.asset)
            .await
            .map_err(|error| StockError::Tracking(Box::new(error)))?;
        media.unsplash_download_tracked = true;
        Ok(())
    }

    async fn media_response(&self, provider: Provider, raw: &str) -> Result<Response, StockError> {
        let mut url = media_url(provider, raw)?;
        for _ in 0..=5 {
            let response = self
                .http
                .get(url.clone())
                .header(header::ACCEPT, "image/jpeg,image/png,image/webp,video/mp4")
                .send()
                .await
                .map_err(network_error)?;
            media_url(provider, response.url().as_str())?;
            if !response.status().is_redirection() {
                return Ok(response);
            }
            let location = response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or(StockError::UntrustedUrl)?;
            let next = url.join(location).map_err(|_| StockError::UntrustedUrl)?;
            url = media_url(provider, next.as_str())?;
        }
        Err(StockError::UntrustedUrl)
    }

    async fn track_unsplash(
        &self,
        credentials: &Credentials<'_>,
        asset: &Asset,
    ) -> Result<(), StockError> {
        let raw = asset
            .download_location
            .as_deref()
            .ok_or(StockError::InvalidResponse)?;
        let url = tracking_url(raw, &asset.id)?;
        let request = authorize_unsplash(
            self.http.get(url).timeout(Duration::from_secs(30)),
            credentials.key(Provider::Unsplash)?,
        )?;
        let response = request.send().await.map_err(network_error)?;
        validate_api_response_url(response.url(), Provider::Unsplash)?;
        validate_tracking_reply(&api_bytes(response).await?)
    }
}

fn authorize_unsplash(request: RequestBuilder, key: &str) -> Result<RequestBuilder, StockError> {
    let mut value = header::HeaderValue::from_str(&format!("Client-ID {key}"))
        .map_err(|_| StockError::Credentials)?;
    value.set_sensitive(true);
    Ok(request
        .header(header::AUTHORIZATION, value)
        .header("Accept-Version", "v1"))
}

fn validate_search(request: &SearchRequest) -> Result<(), StockError> {
    if request.provider == Provider::Unsplash && request.kind != MediaKind::Photo {
        return Err(StockError::UnsupportedKind);
    }
    let page_size = match request.provider {
        Provider::Unsplash => 1..=30,
        Provider::Pixabay => 3..=200,
    };
    if request.query.trim().is_empty()
        || request.query.chars().count() > 100
        || request.query.chars().any(char::is_control)
        || request.page == 0
        || request.page > 10000
        || !page_size.contains(&request.per_page)
        || (request.provider == Provider::Pixabay
            && request.kind == MediaKind::Video
            && request.orientation != Orientation::Any)
    {
        return Err(StockError::InvalidSearch);
    }
    Ok(())
}

fn checked_url(raw: &str, hosts: &[&str]) -> Result<Url, StockError> {
    if raw.len() > URL_LIMIT || raw.chars().any(char::is_control) {
        return Err(StockError::UntrustedUrl);
    }
    let url = Url::parse(raw).map_err(|_| StockError::UntrustedUrl)?;
    if url.as_str().len() > URL_LIMIT
        || url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port_or_known_default() != Some(443)
        || url.fragment().is_some()
        || !hosts.contains(&url.host_str().unwrap_or_default())
        || url
            .query_pairs()
            .any(|(key, _)| matches!(key.as_ref(), "key" | "client_id" | "access_token"))
    {
        return Err(StockError::UntrustedUrl);
    }
    Ok(url)
}

fn media_url(provider: Provider, raw: &str) -> Result<Url, StockError> {
    checked_url(
        raw,
        match provider {
            Provider::Unsplash => &["images.unsplash.com", "hd.unsplash.com"],
            Provider::Pixabay => &["cdn.pixabay.com", "pixabay.com"],
        },
    )
}

fn validate_api_response_url(url: &Url, provider: Provider) -> Result<(), StockError> {
    let (host, paths) = match provider {
        Provider::Unsplash => ("api.unsplash.com", &["/search/photos", "/photos/"][..]),
        Provider::Pixabay => ("pixabay.com", &["/api/", "/api/videos/"][..]),
    };
    if url.scheme() != "https"
        || url.host_str() != Some(host)
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || !paths.iter().any(|path| {
            url.path() == *path
                || (provider == Provider::Unsplash
                    && *path == "/photos/"
                    && url.path().starts_with(path)
                    && url.path().ends_with("/download"))
        })
    {
        return Err(StockError::UntrustedUrl);
    }
    Ok(())
}

fn tracking_url(raw: &str, id: &str) -> Result<Url, StockError> {
    let url = checked_url(raw, &["api.unsplash.com"])?;
    if id.is_empty()
        || id.len() > 100
        || !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        || url.path() != format!("/photos/{id}/download")
    {
        return Err(StockError::UntrustedUrl);
    }
    Ok(url)
}

fn validate_tracking_reply(bytes: &[u8]) -> Result<(), StockError> {
    #[derive(Deserialize)]
    struct Reply {
        url: String,
    }
    let reply: Reply = decode(bytes)?;
    checked_url(
        &reply.url,
        &[
            "images.unsplash.com",
            "image.unsplash.com",
            "hd.unsplash.com",
        ],
    )?;
    Ok(())
}

fn attribution_url(provider: Provider, raw: &str) -> Result<String, StockError> {
    if raw.len() > URL_LIMIT || raw.chars().any(char::is_control) {
        return Err(StockError::UntrustedUrl);
    }
    let hosts = match provider {
        Provider::Unsplash => &["unsplash.com"][..],
        Provider::Pixabay => &["pixabay.com"][..],
    };
    let mut url = Url::parse(raw).map_err(|_| StockError::UntrustedUrl)?;
    if url.scheme() == "http" {
        url.set_scheme("https")
            .map_err(|()| StockError::UntrustedUrl)?;
    }
    let mut url = checked_url(url.as_str(), hosts)?;
    if provider == Provider::Unsplash {
        let retained = url
            .query_pairs()
            .filter(|(key, _)| !matches!(key.as_ref(), "utm_source" | "utm_medium"))
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect::<Vec<_>>();
        url.set_query(None);
        url.query_pairs_mut()
            .extend_pairs(retained)
            .append_pair("utm_source", "capwise")
            .append_pair("utm_medium", "referral");
    }
    if url.as_str().len() > URL_LIMIT {
        return Err(StockError::UntrustedUrl);
    }
    Ok(url.into())
}

fn bounded_description(value: &str) -> String {
    let mut result = String::new();
    for character in value
        .chars()
        .filter(|character| !character.is_control() || *character == '\n')
    {
        if result.len() + character.len_utf8() > DESCRIPTION_LIMIT {
            break;
        }
        result.push(character);
    }
    result.trim().to_string()
}

fn normalize_asset(mut asset: Asset) -> Result<Asset, StockError> {
    asset.description = bounded_description(&asset.description);
    asset.attribution.author_url = attribution_url(asset.provider, &asset.attribution.author_url)?;
    asset.attribution.source_url = attribution_url(asset.provider, &asset.attribution.source_url)?;
    validate_asset(&asset)?;
    Ok(asset)
}

pub(crate) fn validate_asset(asset: &Asset) -> Result<(), StockError> {
    if asset.id.is_empty()
        || asset.id.len() > 100
        || asset.id.chars().any(char::is_control)
        || asset.renditions.is_empty()
        || asset.renditions.len() > 4
        || asset.description.len() > DESCRIPTION_LIMIT
        || asset
            .description
            .chars()
            .any(|character| character.is_control() && character != '\n')
        || asset.attribution.author.trim().is_empty()
        || asset.attribution.author.len() > AUTHOR_LIMIT
        || asset.attribution.author.chars().any(char::is_control)
        || !asset.attribution.search_source_credit_required
        || !asset.attribution.additional_rights_may_apply
        || (asset.provider == Provider::Unsplash && asset.kind != MediaKind::Photo)
    {
        return Err(StockError::InvalidResponse);
    }
    let (license_name, license_url, attribution_required) = match asset.provider {
        Provider::Unsplash => ("Unsplash License", "https://unsplash.com/license", true),
        Provider::Pixabay => (
            "Pixabay Content License",
            "https://pixabay.com/service/terms/",
            false,
        ),
    };
    if asset.attribution.license_name != license_name
        || asset.attribution.license_url != license_url
        || asset.attribution.api_attribution_required != attribution_required
        || (asset.provider == Provider::Pixabay
            && (asset.id.parse::<u64>().is_err() || asset.download_location.is_some()))
    {
        return Err(StockError::InvalidResponse);
    }
    for rendition in &asset.renditions {
        if rendition.label.is_empty()
            || rendition.label.len() > 32
            || !rendition
                .label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(StockError::InvalidResponse);
        }
        media_url(asset.provider, &rendition.url)?;
    }
    if let Some(thumbnail) = &asset.thumbnail_url {
        media_url(asset.provider, thumbnail)?;
    }
    attribution_url(asset.provider, &asset.attribution.author_url)?;
    attribution_url(asset.provider, &asset.attribution.source_url)?;
    if asset.provider == Provider::Unsplash {
        tracking_url(
            asset
                .download_location
                .as_deref()
                .ok_or(StockError::InvalidResponse)?,
            &asset.id,
        )?;
    }
    if serde_json::to_vec_pretty(asset)
        .map_err(|_| StockError::InvalidResponse)?
        .len()
        > ASSET_METADATA_LIMIT
    {
        return Err(StockError::InvalidResponse);
    }
    Ok(())
}

fn ensure_success(response: &Response) -> Result<(), StockError> {
    if response.status().is_success() {
        return Ok(());
    }
    Err(StockError::Http {
        status: response.status().as_u16(),
        retry_after_seconds: retry_after_seconds(
            response.headers(),
            SystemTime::now(),
            response.url().host_str() == Some("pixabay.com"),
        ),
    })
}

fn header_number(headers: &header::HeaderMap, name: &str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.parse().ok()
}

fn retry_after_seconds(headers: &header::HeaderMap, now: SystemTime, pixabay: bool) -> Option<u64> {
    let retry = headers
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value.trim().parse::<u64>().ok().or_else(|| {
                let date = chrono::DateTime::parse_from_rfc2822(value)
                    .ok()?
                    .timestamp();
                let now = now.duration_since(UNIX_EPOCH).ok()?.as_secs();
                Some(u64::try_from(date).unwrap_or(0).saturating_sub(now))
            })
        });
    retry.or_else(|| {
        pixabay
            .then(|| header_number(headers, "x-ratelimit-reset"))
            .flatten()
    })
}

fn rate_limit(headers: &header::HeaderMap, provider: Provider) -> RateLimit {
    RateLimit {
        limit: header_number(headers, "x-ratelimit-limit"),
        remaining: header_number(headers, "x-ratelimit-remaining"),
        reset_seconds: (provider == Provider::Pixabay)
            .then(|| header_number(headers, "x-ratelimit-reset"))
            .flatten(),
    }
}

async fn bounded_bytes(response: Response, limit: usize) -> Result<Vec<u8>, StockError> {
    bounded_bytes_with_progress(response, limit, |_, _| {}).await
}

async fn bounded_bytes_with_progress(
    mut response: Response,
    limit: usize,
    mut progress: impl FnMut(u64, Option<u64>) + Send,
) -> Result<Vec<u8>, StockError> {
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return Err(StockError::TooLarge);
    }
    let total = response.content_length();
    let mut bytes = Vec::new();
    progress(0, total);
    while let Some(chunk) = response.chunk().await.map_err(network_error)? {
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(StockError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
        progress(bytes.len() as u64, total);
    }
    Ok(bytes)
}

async fn api_bytes(response: Response) -> Result<Vec<u8>, StockError> {
    ensure_success(&response)?;
    let media_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !media_type
        .split(';')
        .next()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
    {
        return Err(StockError::InvalidResponse);
    }
    bounded_bytes(response, JSON_LIMIT).await
}

fn identify_media(
    kind: MediaKind,
    declared: &str,
    bytes: &[u8],
) -> Result<(&'static str, &'static str), StockError> {
    match (kind, declared) {
        (MediaKind::Photo, "image/jpeg") if bytes.starts_with(&[0xff, 0xd8, 0xff]) => {
            Ok(("image/jpeg", "jpg"))
        }
        (MediaKind::Photo, "image/png") if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => {
            Ok(("image/png", "png"))
        }
        (MediaKind::Photo, "image/webp")
            if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") =>
        {
            Ok(("image/webp", "webp"))
        }
        (MediaKind::Video, "video/mp4")
            if bytes.len() >= 12 && bytes.get(4..8) == Some(b"ftyp") =>
        {
            Ok(("video/mp4", "mp4"))
        }
        _ => Err(StockError::UnsupportedMedia),
    }
}

fn unix_seconds() -> Result<u64, StockError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| StockError::Clock)
}

#[derive(Serialize, Deserialize)]
struct CacheEntry {
    version: u32,
    request: SearchRequest,
    page: SearchPage,
}

fn cache_filename(request: &SearchRequest) -> Result<String, StockError> {
    let bytes = serde_json::to_vec(request).map_err(|_| StockError::Cache)?;
    Ok(format!("{:x}.json", Sha256::digest(bytes)))
}

fn read_cache(
    path: &Path,
    request: &SearchRequest,
    now: u64,
) -> Result<Option<SearchPage>, StockError> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(StockError::Cache),
    };
    let mut bytes = Vec::new();
    file.take(JSON_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| StockError::Cache)?;
    let parsed = decode_cache(&bytes, request, now);
    match parsed {
        Ok(page) => Ok(page),
        Err(StockError::Cache) => {
            let isolated = path.with_extension(format!("invalid-{}.json", uuid::Uuid::new_v4()));
            match std::fs::rename(path, isolated) {
                Ok(()) => Ok(None),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(_) => Err(StockError::Cache),
            }
        }
        Err(error) => Err(error),
    }
}

fn decode_cache(
    bytes: &[u8],
    request: &SearchRequest,
    now: u64,
) -> Result<Option<SearchPage>, StockError> {
    if bytes.len() > JSON_LIMIT {
        return Err(StockError::Cache);
    }
    let mut entry: CacheEntry = serde_json::from_slice(bytes).map_err(|_| StockError::Cache)?;
    if entry.version != CACHE_VERSION || cache_filename(&entry.request)? != cache_filename(request)?
    {
        return Err(StockError::Cache);
    }
    let ttl = cache_ttl(request.provider);
    if now < entry.page.fetched_at || now.saturating_sub(entry.page.fetched_at) >= ttl {
        return Ok(None);
    }
    if entry.page.provider != request.provider
        || entry.page.kind != request.kind
        || entry.page.page != request.page
        || entry.page.per_page != request.per_page
        || entry.page.assets.len() > request.per_page as usize
        || entry
            .page
            .assets
            .iter()
            .any(|asset| asset.provider != request.provider || asset.kind != request.kind)
    {
        return Err(StockError::Cache);
    }
    entry.page.assets = entry
        .page
        .assets
        .into_iter()
        .map(normalize_asset)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| StockError::Cache)?;
    entry.page.expires_at = entry.page.fetched_at.saturating_add(ttl);
    Ok(Some(entry.page))
}

fn cache_ttl(provider: Provider) -> u64 {
    match provider {
        Provider::Unsplash => UNSPLASH_CACHE_SECONDS,
        Provider::Pixabay => PIXABAY_CACHE_SECONDS,
    }
}

fn save_cache(path: &Path, request: &SearchRequest, page: &SearchPage) -> Result<(), StockError> {
    let bytes = serde_json::to_vec(&CacheEntry {
        version: CACHE_VERSION,
        request: request.clone(),
        page: page.clone(),
    })
    .map_err(|_| StockError::Cache)?;
    if bytes.len() > JSON_LIMIT {
        return Err(StockError::Cache);
    }
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| StockError::Cache)?;
        file.write_all(&bytes).map_err(|_| StockError::Cache)?;
        std::fs::rename(&temporary, path).map_err(|_| StockError::Cache)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, StockError> {
    serde_json::from_slice(bytes).map_err(|_| StockError::InvalidResponse)
}

#[derive(Deserialize)]
struct UnsplashSearch {
    total: u64,
    total_pages: u32,
    results: Vec<serde_json::Value>,
}
#[derive(Deserialize)]
struct UnsplashPhoto {
    id: String,
    width: u32,
    height: u32,
    alt_description: Option<String>,
    description: Option<String>,
    urls: UnsplashUrls,
    links: UnsplashLinks,
    user: UnsplashUser,
}
#[derive(Deserialize)]
struct UnsplashUrls {
    small: String,
    regular: String,
    full: String,
}
#[derive(Deserialize)]
struct UnsplashLinks {
    html: String,
    download_location: String,
}
#[derive(Deserialize)]
struct UnsplashUser {
    name: String,
    links: HtmlLink,
}
#[derive(Deserialize)]
struct HtmlLink {
    html: String,
}

#[derive(Deserialize)]
struct PixabaySearch {
    total: u64,
    #[serde(rename = "totalHits")]
    total_hits: u64,
    hits: Vec<serde_json::Value>,
}
#[derive(Deserialize)]
struct PixabayPhoto {
    id: u64,
    #[serde(rename = "pageURL")]
    page_url: String,
    tags: String,
    #[serde(rename = "previewURL")]
    preview_url: String,
    #[serde(rename = "largeImageURL")]
    large_image_url: String,
    #[serde(rename = "imageURL")]
    image_url: Option<String>,
    #[serde(rename = "imageWidth")]
    image_width: u32,
    #[serde(rename = "imageHeight")]
    image_height: u32,
    #[serde(rename = "imageSize")]
    image_size: Option<u64>,
    user: String,
    user_id: u64,
}
#[derive(Deserialize)]
struct PixabayVideo {
    id: u64,
    #[serde(rename = "pageURL")]
    page_url: String,
    tags: String,
    duration: u32,
    user: String,
    user_id: u64,
    videos: std::collections::BTreeMap<String, PixabayVideoSize>,
}
#[derive(Deserialize)]
struct PixabayVideoSize {
    url: String,
    width: u32,
    height: u32,
    size: u64,
    #[serde(default)]
    thumbnail: Option<String>,
}

fn pixabay_attribution(
    user: &str,
    user_id: u64,
    page_url: &str,
) -> Result<Attribution, StockError> {
    if user.trim().is_empty() || user.len() > AUTHOR_LIMIT || user.chars().any(char::is_control) {
        return Err(StockError::InvalidResponse);
    }
    let mut author_url =
        Url::parse("https://pixabay.com/users/").map_err(|_| StockError::UntrustedUrl)?;
    author_url
        .path_segments_mut()
        .map_err(|()| StockError::UntrustedUrl)?
        .push(&format!("{user}-{user_id}"))
        .push("");
    Ok(Attribution {
        author: user.to_string(),
        author_url: author_url.into(),
        source_url: attribution_url(Provider::Pixabay, page_url)?,
        license_name: "Pixabay Content License".into(),
        license_url: "https://pixabay.com/service/terms/".into(),
        api_attribution_required: false,
        search_source_credit_required: true,
        additional_rights_may_apply: true,
    })
}

fn parse_rows<T: DeserializeOwned>(
    rows: Vec<serde_json::Value>,
    per_page: u32,
    convert: impl Fn(T) -> Result<Asset, StockError>,
) -> Result<Vec<Asset>, StockError> {
    if rows.len() > per_page as usize {
        return Err(StockError::InvalidResponse);
    }
    let had_results = !rows.is_empty();
    let assets: Vec<_> = rows
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .filter_map(|item| convert(item).and_then(normalize_asset).ok())
        .collect();
    if had_results && assets.is_empty() {
        return Err(StockError::InvalidResponse);
    }
    Ok(assets)
}

fn parse_search(request: &SearchRequest, bytes: &[u8], now: u64) -> Result<SearchPage, StockError> {
    let (total, available, next_page, assets) = match (request.provider, request.kind) {
        (Provider::Unsplash, MediaKind::Photo) => {
            let response: UnsplashSearch = decode(bytes)?;
            let assets =
                parse_rows::<UnsplashPhoto>(response.results, request.per_page, |photo| {
                    Ok(Asset {
                        provider: Provider::Unsplash,
                        kind: MediaKind::Photo,
                        id: photo.id,
                        description: photo
                            .alt_description
                            .or(photo.description)
                            .unwrap_or_default(),
                        thumbnail_url: Some(photo.urls.small),
                        renditions: vec![
                            Rendition {
                                label: "regular".into(),
                                url: photo.urls.regular,
                                width: None,
                                height: None,
                                approximate_bytes: None,
                            },
                            Rendition {
                                label: "full".into(),
                                url: photo.urls.full,
                                width: Some(photo.width),
                                height: Some(photo.height),
                                approximate_bytes: None,
                            },
                        ],
                        duration_seconds: None,
                        attribution: Attribution {
                            author: photo.user.name,
                            author_url: attribution_url(
                                Provider::Unsplash,
                                &photo.user.links.html,
                            )?,
                            source_url: attribution_url(Provider::Unsplash, &photo.links.html)?,
                            license_name: "Unsplash License".into(),
                            license_url: "https://unsplash.com/license".into(),
                            api_attribution_required: true,
                            search_source_credit_required: true,
                            additional_rights_may_apply: true,
                        },
                        download_location: Some(photo.links.download_location),
                    })
                })?;
            (
                response.total,
                response.total,
                (request.page < response.total_pages).then_some(request.page + 1),
                assets,
            )
        }
        (Provider::Pixabay, MediaKind::Photo) => {
            let response: PixabaySearch = decode(bytes)?;
            let assets = parse_rows::<PixabayPhoto>(response.hits, request.per_page, |photo| {
                let mut renditions = vec![Rendition {
                    label: "1280".into(),
                    url: photo.large_image_url,
                    width: None,
                    height: None,
                    approximate_bytes: None,
                }];
                if let Some(url) = photo.image_url.filter(|url| !url.is_empty()) {
                    renditions.push(Rendition {
                        label: "original".into(),
                        url,
                        width: Some(photo.image_width),
                        height: Some(photo.image_height),
                        approximate_bytes: photo.image_size,
                    });
                }
                Ok(Asset {
                    provider: Provider::Pixabay,
                    kind: MediaKind::Photo,
                    id: photo.id.to_string(),
                    description: photo.tags,
                    thumbnail_url: Some(photo.preview_url),
                    renditions,
                    duration_seconds: None,
                    attribution: pixabay_attribution(&photo.user, photo.user_id, &photo.page_url)?,
                    download_location: None,
                })
            })?;
            (
                response.total,
                response.total_hits,
                next_pixabay_page(request, response.total_hits),
                assets,
            )
        }
        (Provider::Pixabay, MediaKind::Video) => {
            let response: PixabaySearch = decode(bytes)?;
            let assets = parse_rows::<PixabayVideo>(response.hits, request.per_page, |video| {
                let thumbnail_url =
                    ["tiny", "small", "medium", "large"]
                        .into_iter()
                        .find_map(|name| {
                            video
                                .videos
                                .get(name)
                                .and_then(|size| size.thumbnail.as_ref())
                                .filter(|url| !url.is_empty())
                                .cloned()
                        });
                let renditions = ["medium", "large", "small", "tiny"]
                    .into_iter()
                    .filter_map(|name| {
                        video
                            .videos
                            .get(name)
                            .filter(|size| !size.url.is_empty() && size.size > 0)
                            .map(|size| Rendition {
                                label: name.into(),
                                url: size.url.clone(),
                                width: Some(size.width),
                                height: Some(size.height),
                                approximate_bytes: Some(size.size),
                            })
                    })
                    .collect();
                Ok(Asset {
                    provider: Provider::Pixabay,
                    kind: MediaKind::Video,
                    id: video.id.to_string(),
                    description: video.tags,
                    thumbnail_url,
                    renditions,
                    duration_seconds: Some(video.duration),
                    attribution: pixabay_attribution(&video.user, video.user_id, &video.page_url)?,
                    download_location: None,
                })
            })?;
            (
                response.total,
                response.total_hits,
                next_pixabay_page(request, response.total_hits),
                assets,
            )
        }
        _ => return Err(StockError::UnsupportedKind),
    };
    if assets.len() > request.per_page as usize {
        return Err(StockError::InvalidResponse);
    }
    for asset in &assets {
        validate_asset(asset)?;
    }
    Ok(SearchPage {
        provider: request.provider,
        kind: request.kind,
        page: request.page,
        per_page: request.per_page,
        total,
        available,
        next_page,
        fetched_at: now,
        expires_at: now.saturating_add(cache_ttl(request.provider)),
        from_cache: false,
        rate_limit: RateLimit::default(),
        assets,
    })
}

fn next_pixabay_page(request: &SearchRequest, available: u64) -> Option<u32> {
    (u64::from(request.page) * u64::from(request.per_page) < available).then_some(request.page + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(provider: Provider, kind: MediaKind) -> SearchRequest {
        SearchRequest {
            provider,
            kind,
            query: "海边 & key=not-a-credential".into(),
            page: 1,
            per_page: 24,
            orientation: Orientation::Any,
        }
    }

    fn unsplash_fixture() -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "total": 25, "total_pages": 2,
            "results": [{"id": "photo_123", "width": 3200, "height": 1800,
                "alt_description": "Synthetic coastline", "description": null,
                "urls": {"small": "https://images.unsplash.com/photo-test?w=400&ixid=keep-me", "regular": "https://images.unsplash.com/photo-test?w=1080&ixid=keep-me", "full": "https://hd.unsplash.com/photo-test?ixid=keep-me"},
                "links": {"html": "https://unsplash.com/photos/photo_123", "download_location": "https://api.unsplash.com/photos/photo_123/download?ixid=event-token"},
                "user": {"name": "Synthetic Creator", "links": {"html": "https://unsplash.com/@synthetic"}}
            }]
        })).unwrap()
    }

    fn pixabay_photo_fixture() -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"total": 1000, "totalHits": 500, "hits": [{
            "id": 42, "pageURL": "https://pixabay.com/photos/example-42/", "tags": "sea, blue",
            "previewURL": "https://cdn.pixabay.com/photo/example_150.jpg", "largeImageURL": "https://pixabay.com/get/example_1280.jpg",
            "imageWidth": 4000, "imageHeight": 2000, "imageSize": 4000000, "user": "Example Creator", "user_id": 7
        }]})).unwrap()
    }

    #[test]
    fn authentication_and_queries_are_encoded_without_spreading_credentials() {
        let directory = tempfile::tempdir().unwrap();
        let client = StockMediaClient::new(directory.path()).unwrap();
        let credentials = Credentials {
            unsplash: Some("synthetic-access"),
            pixabay: Some("synthetic&reserved=key"),
        };
        let input = request(Provider::Pixabay, MediaKind::Photo);
        let built = client
            .search_request(&credentials, &input)
            .unwrap()
            .build()
            .unwrap();
        let pairs: std::collections::HashMap<_, _> = built.url().query_pairs().collect();
        assert_eq!(pairs.get("q").unwrap(), &input.query);
        assert_eq!(pairs.get("key").unwrap(), "synthetic&reserved=key");
        assert_eq!(pairs.keys().filter(|key| key.as_ref() == "key").count(), 1);
        let unsplash = client
            .search_request(&credentials, &request(Provider::Unsplash, MediaKind::Photo))
            .unwrap()
            .build()
            .unwrap();
        assert!(!unsplash.url().as_str().contains("synthetic-access"));
        assert!(unsplash.headers()[header::AUTHORIZATION].is_sensitive());
        assert!(!cache_filename(&input).unwrap().contains("synthetic"));
    }

    #[test]
    fn unsplash_metadata_preserves_hotlinks_attribution_and_exact_download_event() {
        let page = parse_search(
            &request(Provider::Unsplash, MediaKind::Photo),
            &unsplash_fixture(),
            100,
        )
        .unwrap();
        let asset = &page.assets[0];
        assert_eq!(page.next_page, Some(2));
        assert_eq!(
            asset.renditions[0].url,
            "https://images.unsplash.com/photo-test?w=1080&ixid=keep-me"
        );
        assert_eq!(
            asset.download_location.as_deref(),
            Some("https://api.unsplash.com/photos/photo_123/download?ixid=event-token")
        );
        assert!(asset.attribution.author_url.contains("utm_source=capwise"));
        assert!(asset.attribution.api_attribution_required);
        let restored: Asset = serde_json::from_slice(&serde_json::to_vec(asset).unwrap()).unwrap();
        assert_eq!(
            restored.attribution.license_url,
            "https://unsplash.com/license"
        );
        assert!(validate_asset(&restored).is_ok());
    }

    #[test]
    fn pixabay_uses_available_image_access_and_video_renditions() {
        let page = parse_search(
            &request(Provider::Pixabay, MediaKind::Photo),
            &pixabay_photo_fixture(),
            100,
        )
        .unwrap();
        assert_eq!(page.total, 1000);
        assert_eq!(page.available, 500);
        assert_eq!(page.assets[0].renditions.len(), 1);
        assert!(
            page.assets[0]
                .attribution
                .author_url
                .contains("Example%20Creator-7")
        );
        let bytes = serde_json::to_vec(&serde_json::json!({"total": 1, "totalHits": 1, "hits": [{
            "id": 7, "pageURL": "https://pixabay.com/videos/id-7/", "tags": "sea", "duration": 12, "user": "Maker", "user_id": 9,
            "videos": {"large": {"url": "", "size": 0, "width": 0, "height": 0}, "medium": {"url": "https://cdn.pixabay.com/video/7_medium.mp4", "thumbnail": "https://cdn.pixabay.com/video/7_medium.jpg", "size": 1200, "width": 1920, "height": 1080}}
        }]})).unwrap();
        let video =
            parse_search(&request(Provider::Pixabay, MediaKind::Video), &bytes, 100).unwrap();
        assert_eq!(video.next_page, None);
        assert_eq!(video.assets[0].renditions.len(), 1);
        assert_eq!(video.assets[0].renditions[0].label, "medium");
        assert_eq!(
            video.assets[0].thumbnail_url.as_deref(),
            Some("https://cdn.pixabay.com/video/7_medium.jpg")
        );
    }

    #[test]
    fn corrupted_cache_is_isolated_and_the_next_success_can_replace_it() {
        let directory = tempfile::tempdir().unwrap();
        let input = request(Provider::Pixabay, MediaKind::Photo);
        let path = directory.path().join(cache_filename(&input).unwrap());
        let unrelated = directory.path().join("other-query.json");
        std::fs::write(&unrelated, b"untouched").unwrap();
        for bytes in [b"{truncated".as_slice(), br#"{"version":999}"#.as_slice()] {
            std::fs::write(&path, bytes).unwrap();
            assert!(read_cache(&path, &input, 100).unwrap().is_none());
            assert!(!path.exists());
            assert_eq!(std::fs::read(&unrelated).unwrap(), b"untouched");
            let page = parse_search(&input, &pixabay_photo_fixture(), 100).unwrap();
            save_cache(&path, &input, &page).unwrap();
            assert_eq!(
                read_cache(&path, &input, 101).unwrap().unwrap().assets[0].id,
                page.assets[0].id
            );
        }
        assert_eq!(
            std::fs::read_dir(directory.path())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().contains("invalid-"))
                .count(),
            2
        );
    }

    #[test]
    fn cache_io_failure_is_not_mistaken_for_corrupt_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let input = request(Provider::Pixabay, MediaKind::Photo);
        let path = directory.path().join(cache_filename(&input).unwrap());
        std::fs::create_dir(&path).unwrap();
        assert!(matches!(
            read_cache(&path, &input, 100),
            Err(StockError::Cache)
        ));
        assert!(path.is_dir());
    }

    #[test]
    fn pixabay_disk_cache_lasts_exactly_twenty_four_hours_and_contains_no_key() {
        let directory = tempfile::tempdir().unwrap();
        let input = request(Provider::Pixabay, MediaKind::Photo);
        let page = parse_search(&input, &pixabay_photo_fixture(), 100).unwrap();
        let path = directory.path().join(cache_filename(&input).unwrap());
        save_cache(&path, &input, &page).unwrap();
        assert!(
            read_cache(&path, &input, 100 + PIXABAY_CACHE_SECONDS - 1)
                .unwrap()
                .is_some()
        );
        assert!(
            read_cache(&path, &input, 100 + PIXABAY_CACHE_SECONDS)
                .unwrap()
                .is_none()
        );
        let saved = std::fs::read_to_string(path).unwrap();
        assert!(!saved.contains("synthetic&reserved=key"));
        assert!(!saved.contains("Authorization"));
        assert_eq!(page.expires_at, 100 + PIXABAY_CACHE_SECONDS);
    }

    #[test]
    fn huge_descriptions_fit_project_sidecars_without_discarding_usable_results() {
        let mut response: serde_json::Value = serde_json::from_slice(&unsplash_fixture()).unwrap();
        let ordinary = response["results"][0].clone();
        response["results"][0]["alt_description"] =
            serde_json::Value::String("海边🌊\n\u{0000}".repeat(30_000));
        let mut second = ordinary.clone();
        second["id"] = "photo_456".into();
        second["links"]["html"] = "https://unsplash.com/photos/photo_456".into();
        second["links"]["download_location"] =
            "https://api.unsplash.com/photos/photo_456/download?ixid=retained".into();
        let mut invalid = ordinary;
        invalid["user"]["name"] = "x".repeat(AUTHOR_LIMIT + 1).into();
        response["results"].as_array_mut().unwrap().extend([
            second,
            invalid,
            serde_json::json!({"id": false}),
        ]);
        let page = parse_search(
            &request(Provider::Unsplash, MediaKind::Photo),
            &serde_json::to_vec(&response).unwrap(),
            100,
        )
        .unwrap();
        assert_eq!(page.assets.len(), 2);
        assert_eq!(page.assets[0].id, "photo_123");
        assert_eq!(page.assets[1].id, "photo_456");
        for asset in page.assets {
            assert!(asset.description.len() <= DESCRIPTION_LIMIT);
            assert!(!asset.description.contains('\0'));
            assert!(serde_json::to_vec_pretty(&asset).unwrap().len() <= ASSET_METADATA_LIMIT);
            let sidecar = serde_json::json!({"path": "content/images/synthetic-photo.jpg", "name": asset.description,
                "kind": "photo", "source": asset, "rendition": 0});
            assert!(serde_json::to_vec_pretty(&sidecar).unwrap().len() < 128 * 1024);
            assert_eq!(
                sidecar["source"]["attribution"]["author"],
                "Synthetic Creator"
            );
            assert_eq!(
                sidecar["source"]["attribution"]["license_url"],
                "https://unsplash.com/license"
            );
        }
    }

    #[test]
    fn metadata_validation_rejects_long_urls_and_forged_license_instead_of_truncating_them() {
        let page = parse_search(
            &request(Provider::Unsplash, MediaKind::Photo),
            &unsplash_fixture(),
            100,
        )
        .unwrap();
        let mut asset = page.assets[0].clone();
        asset.renditions[0].url = format!("https://images.unsplash.com/{}", "x".repeat(URL_LIMIT));
        assert!(normalize_asset(asset).is_err());
        let mut asset = page.assets[0].clone();
        asset.attribution.license_url = "https://untrusted.test/license".into();
        assert!(normalize_asset(asset).is_err());
        let mut asset = page.assets[0].clone();
        asset.attribution.api_attribution_required = false;
        assert!(normalize_asset(asset).is_err());
        let mut response: serde_json::Value =
            serde_json::from_slice(&pixabay_photo_fixture()).unwrap();
        let mut invalid = response["hits"][0].clone();
        invalid["largeImageURL"] = "https://untrusted.test/media.jpg".into();
        response["hits"].as_array_mut().unwrap().push(invalid);
        let kept = parse_search(
            &request(Provider::Pixabay, MediaKind::Photo),
            &serde_json::to_vec(&response).unwrap(),
            100,
        )
        .unwrap();
        assert_eq!(kept.assets.len(), 1);
        assert_eq!(
            kept.assets[0].renditions[0].url,
            "https://pixabay.com/get/example_1280.jpg"
        );
    }

    #[test]
    fn existing_cache_normalizes_description_and_attribution_without_rewriting_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let input = request(Provider::Unsplash, MediaKind::Photo);
        let mut page = parse_search(&input, &unsplash_fixture(), 100).unwrap();
        page.assets[0].description = "🌊".repeat(40_000);
        let path = directory.path().join(cache_filename(&input).unwrap());
        save_cache(&path, &input, &page).unwrap();
        let original_bytes = std::fs::read(&path).unwrap();
        let cached = read_cache(&path, &input, 101).unwrap().unwrap();
        assert!(cached.assets[0].description.len() <= DESCRIPTION_LIMIT);
        assert_eq!(
            cached.assets[0]
                .attribution
                .author_url
                .matches("utm_source=")
                .count(),
            1
        );
        assert_eq!(
            cached.assets[0]
                .attribution
                .author_url
                .matches("utm_medium=")
                .count(),
            1
        );
        assert!(
            serde_json::to_vec_pretty(&cached.assets[0]).unwrap().len() <= ASSET_METADATA_LIMIT
        );
        assert_eq!(std::fs::read(&path).unwrap(), original_bytes);
    }

    #[test]
    fn damaged_cache_is_isolated_without_bypassing_url_or_license_checks() {
        let directory = tempfile::tempdir().unwrap();
        let input = request(Provider::Pixabay, MediaKind::Photo);
        let page = parse_search(&input, &pixabay_photo_fixture(), 100).unwrap();
        let path = directory.path().join(cache_filename(&input).unwrap());
        let mutations: [fn(&mut Asset); 3] = [
            |asset: &mut Asset| {
                asset.thumbnail_url = Some("https://untrusted.test/image.jpg".into());
            },
            |asset: &mut Asset| {
                asset.attribution.license_url = "https://untrusted.test/license".into();
            },
            |asset: &mut Asset| {
                asset.attribution.author = "x".repeat(AUTHOR_LIMIT + 1);
            },
        ];
        for mutate in mutations {
            let mut damaged = page.clone();
            mutate(&mut damaged.assets[0]);
            save_cache(&path, &input, &damaged).unwrap();
            let original_bytes = std::fs::read(&path).unwrap();
            assert!(read_cache(&path, &input, 101).unwrap().is_none());
            assert!(!path.exists());
            assert!(
                std::fs::read_dir(directory.path())
                    .unwrap()
                    .filter_map(Result::ok)
                    .any(|entry| std::fs::read(entry.path())
                        .is_ok_and(|bytes| bytes == original_bytes))
            );
        }
        std::fs::write(&path, b"{broken-json").unwrap();
        assert!(read_cache(&path, &input, 101).unwrap().is_none());
        assert!(!path.exists());
        assert!(
            std::fs::read_dir(directory.path())
                .unwrap()
                .filter_map(Result::ok)
                .any(
                    |entry| std::fs::read(entry.path()).is_ok_and(|bytes| bytes == b"{broken-json")
                )
        );
    }

    #[test]
    fn urls_reject_foreign_hosts_credentials_ports_and_wrong_tracking_identity() {
        for url in [
            "https://images.unsplash.com.attacker.test/p.jpg",
            "http://images.unsplash.com/p.jpg",
            "https://user@images.unsplash.com/p.jpg",
            "https://images.unsplash.com:444/p.jpg",
            "https://127.0.0.1/p.jpg",
            "https://images.unsplash.com/p.jpg?client_id=secret",
        ] {
            assert!(media_url(Provider::Unsplash, url).is_err());
        }
        assert!(media_url(Provider::Pixabay, "https://cdn.pixabay.com/video/a.mp4").is_ok());
        assert!(
            tracking_url(
                "https://api.unsplash.com/photos/different/download",
                "expected"
            )
            .is_err()
        );
        assert!(
            tracking_url(
                "https://api.unsplash.com/photos/expected/download?ixid=keep",
                "expected"
            )
            .is_ok()
        );
        assert!(
            validate_api_response_url(
                &Url::parse("https://evil.test/api/?key=synthetic").unwrap(),
                Provider::Pixabay
            )
            .is_err()
        );
    }

    #[test]
    fn media_validation_rejects_html_svg_and_mismatched_signatures() {
        assert_eq!(
            identify_media(MediaKind::Photo, "image/jpeg", b"<html>error</html>"),
            Err(StockError::UnsupportedMedia)
        );
        assert_eq!(
            identify_media(MediaKind::Photo, "image/svg+xml", b"<svg/>"),
            Err(StockError::UnsupportedMedia)
        );
        assert!(identify_media(MediaKind::Photo, "image/png", b"\x89PNG\r\n\x1a\n").is_ok());
        assert!(identify_media(MediaKind::Video, "video/mp4", b"\0\0\0\x18ftypisom").is_ok());
        assert!(identify_media(MediaKind::Photo, "video/mp4", b"\0\0\0\x18ftypisom").is_err());
    }

    #[test]
    fn download_tracking_requires_a_valid_event_reply_instead_of_silent_success() {
        for bytes in [
            b"null".as_slice(),
            br#"{"errors":["failed"]}"#,
            br#"{"url":"https://untrusted.test/file.jpg"}"#,
        ] {
            assert!(validate_tracking_reply(bytes).is_err());
        }
        assert!(
            validate_tracking_reply(
                br#"{"url":"https://images.unsplash.com/photo-test?ixid=preserved"}"#
            )
            .is_ok()
        );
        assert!(
            validate_tracking_reply(br#"{"url":"https://image.unsplash.com/example"}"#).is_ok()
        );
    }

    #[test]
    fn rate_limit_retry_accepts_seconds_dates_and_pixabay_reset_without_guessing_unsplash() {
        let now = UNIX_EPOCH + Duration::from_secs(1_445_412_480);
        let mut headers = header::HeaderMap::new();
        headers.insert("retry-after", "120".parse().unwrap());
        assert_eq!(retry_after_seconds(&headers, now, false), Some(120));
        headers.insert(
            "retry-after",
            "Wed, 21 Oct 2015 07:30:00 GMT".parse().unwrap(),
        );
        assert_eq!(retry_after_seconds(&headers, now, false), Some(120));
        headers.insert(
            "retry-after",
            "Wed, 21 Oct 2015 07:27:00 GMT".parse().unwrap(),
        );
        assert_eq!(retry_after_seconds(&headers, now, false), Some(0));
        headers.insert("retry-after", "invalid".parse().unwrap());
        headers.insert("x-ratelimit-reset", "42".parse().unwrap());
        assert_eq!(retry_after_seconds(&headers, now, false), None);
        assert_eq!(retry_after_seconds(&headers, now, true), Some(42));
        assert_eq!(rate_limit(&headers, Provider::Unsplash).reset_seconds, None);
        assert_eq!(
            rate_limit(&headers, Provider::Pixabay).reset_seconds,
            Some(42)
        );
        let _ = headers.remove("x-ratelimit-reset");
        assert_eq!(retry_after_seconds(&headers, now, true), None);
        assert_eq!(
            StockError::Http {
                status: 429,
                retry_after_seconds: None
            }
            .retry_wait_seconds(),
            Some(60)
        );
        assert_eq!(
            StockError::Http {
                status: 401,
                retry_after_seconds: Some(60)
            }
            .retry_wait_seconds(),
            None
        );
        assert_eq!(
            StockError::Tracking(Box::new(StockError::Http {
                status: 429,
                retry_after_seconds: Some(42)
            }))
            .retry_wait_seconds(),
            Some(42)
        );
    }

    async fn mock_response(raw: &'static [u8]) -> Response {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request).unwrap();
            stream.write_all(raw).unwrap();
        });
        let response = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
            .get(format!("http://{address}/synthetic"))
            .send()
            .await
            .unwrap();
        server.join().unwrap();
        response
    }

    #[tokio::test]
    async fn bounded_download_rejects_declared_and_chunked_overflow() {
        let declared = mock_response(
            b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\nabcdefghijkl",
        )
        .await;
        assert_eq!(bounded_bytes(declared, 10).await, Err(StockError::TooLarge));
        let chunked = mock_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n8\r\nabcdefgh\r\n8\r\nijklmnop\r\n0\r\n\r\n").await;
        assert_eq!(bounded_bytes(chunked, 10).await, Err(StockError::TooLarge));
    }

    #[tokio::test]
    async fn download_progress_is_actual_bytes_and_http_errors_never_include_body_or_url() {
        let response =
            mock_response(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\ndata")
                .await;
        let mut progress = Vec::new();
        assert_eq!(
            bounded_bytes_with_progress(response, 10, |bytes, total| progress.push((bytes, total)))
                .await
                .unwrap(),
            b"data"
        );
        assert_eq!(progress.first(), Some(&(0, Some(4))));
        assert_eq!(progress.last(), Some(&(4, Some(4))));
        let response = mock_response(b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 18\r\nRetry-After: 60\r\nConnection: close\r\n\r\nsynthetic-key-body").await;
        let error = api_bytes(response).await.unwrap_err();
        assert_eq!(
            error,
            StockError::Http {
                status: 429,
                retry_after_seconds: Some(60)
            }
        );
        assert!(!format!("{error:?} {error}").contains("synthetic-key"));
    }

    #[tokio::test]
    async fn confirmed_tracking_is_not_repeated_or_confused_with_project_import() {
        let directory = tempfile::tempdir().unwrap();
        let client = StockMediaClient::new(directory.path()).unwrap();
        let asset = parse_search(
            &request(Provider::Unsplash, MediaKind::Photo),
            &unsplash_fixture(),
            100,
        )
        .unwrap()
        .assets
        .remove(0);
        let mut media = DownloadedMedia {
            bytes: vec![0xff, 0xd8, 0xff],
            extension: "jpg",
            asset,
            unsplash_download_tracked: true,
        };
        assert!(
            client
                .retry_download_tracking(&Credentials::default(), &mut media)
                .await
                .is_ok()
        );
        media.unsplash_download_tracked = false;
        assert_eq!(
            client
                .retry_download_tracking(&Credentials::default(), &mut media)
                .await,
            Err(StockError::Tracking(Box::new(StockError::Credentials)))
        );
        assert_eq!(media.bytes, [0xff, 0xd8, 0xff]);
        assert!(!media.unsplash_download_tracked);
    }
}
