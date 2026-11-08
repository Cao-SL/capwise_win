use std::{
    fmt, fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::{Mutex, PoisonError},
};

use cap_project::{
    MAX_TITLE_CARD_DURATION, MIN_TITLE_CARD_DURATION, ProjectConfiguration,
    TITLE_CARD_TEMPLATE_VERSION, TextAlign, TextAnimation, TextBackgroundStyle, TextSegment,
    TitleCardBackground, TitleCardChange, TitleCardError, TitleCardRole, TitleCardTemplate, XY,
};
use serde::{Deserialize, Serialize};

use crate::asset_library::{self, AssetKind, AssetRecord, PendingAssetSource};

const VERSION: u32 = 1;
const MAX_JSON: u64 = 128 * 1024;
const MAX_IMAGE: u64 = 64 * 1024 * 1024;
const MANIFEST: &str = "template.json";
static STORE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug)]
pub(crate) enum PresetError {
    Invalid(&'static str),
    Io(std::io::Error),
    Json(serde_json::Error),
    Image(String),
}

impl fmt::Display for PresetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(reason) => write!(f, "Invalid title card template: {reason}"),
            Self::Io(error) => write!(f, "Cannot access title card template: {error}"),
            Self::Json(error) => write!(f, "Cannot read title card template: {error}"),
            Self::Image(error) => write!(f, "Cannot use title card background: {error}"),
        }
    }
}

impl std::error::Error for PresetError {}

impl From<std::io::Error> for PresetError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for PresetError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Appearance {
    center: XY<f64>,
    size: XY<f64>,
    font_family: String,
    font_size: f32,
    font_weight: f32,
    italic: bool,
    uppercase: bool,
    align: TextAlign,
    letter_spacing: f32,
    line_height: f32,
    opacity: f32,
    shadow: f32,
    glow: f32,
    stroke_width: f32,
    stroke_color: String,
    background_style: TextBackgroundStyle,
    background_color: Option<String>,
    color: String,
    gradient_color: Option<String>,
    animation_in: TextAnimation,
    animation_out: TextAnimation,
    animation_in_duration: f64,
    animation_out_duration: f64,
    fade_duration: f64,
}

impl Appearance {
    fn from_segment(s: &TextSegment) -> Self {
        Self {
            center: s.center,
            size: s.size,
            font_family: s.font_family.clone(),
            font_size: s.font_size,
            font_weight: s.font_weight,
            italic: s.italic,
            uppercase: s.uppercase,
            align: s.align,
            letter_spacing: s.letter_spacing,
            line_height: s.line_height,
            opacity: s.opacity,
            shadow: s.shadow,
            glow: s.glow,
            stroke_width: s.stroke_width,
            stroke_color: s.stroke_color.clone(),
            background_style: s.background_style,
            background_color: s.background_color.clone(),
            color: s.color.clone(),
            gradient_color: s.gradient_color.clone(),
            animation_in: s.animation_in,
            animation_out: s.animation_out,
            animation_in_duration: s.animation_in_duration,
            animation_out_duration: s.animation_out_duration,
            fade_duration: s.fade_duration,
        }
    }

    fn validate(&self) -> Result<(), PresetError> {
        let numbers = [
            (f64::from(self.font_size), 8.0, 400.0),
            (f64::from(self.font_weight), 100.0, 900.0),
            (f64::from(self.letter_spacing), -3.0, 20.0),
            (f64::from(self.line_height), 0.8, 2.0),
            (f64::from(self.opacity), 0.0, 1.0),
            (f64::from(self.shadow), 0.0, 1.0),
            (f64::from(self.glow), 0.0, 1.0),
            (f64::from(self.stroke_width), 0.0, 12.0),
            (self.animation_in_duration, 0.0, 3.0),
            (self.animation_out_duration, 0.0, 3.0),
            (self.fade_duration, 0.0, 3.0),
            (self.center.x, 0.0, 1.0),
            (self.center.y, 0.0, 1.0),
            (self.size.x, 0.001, 2.0),
            (self.size.y, 0.001, 2.0),
        ];
        if numbers
            .iter()
            .any(|&(v, min, max)| !v.is_finite() || !(min..=max).contains(&v))
        {
            return Err(PresetError::Invalid("appearance range"));
        }
        if self.font_family.trim().is_empty()
            || self.font_family.chars().count() > 128
            || self.font_family.chars().any(char::is_control)
        {
            return Err(PresetError::Invalid("font family"));
        }
        if [&self.color, &self.stroke_color]
            .into_iter()
            .chain(self.background_color.iter())
            .chain(self.gradient_color.iter())
            .any(|color| !valid_color(color))
        {
            return Err(PresetError::Invalid("text color"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TitleCardPreset {
    version: u32,
    pub name: String,
    pub duration: f64,
    pub template: TitleCardTemplate,
    template_version: u32,
    pub content: String,
    pub kicker: String,
    pub subtitle: String,
    pub background: TitleCardBackground,
    appearance: Appearance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    background_asset: Option<BackgroundAsset>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BackgroundAsset {
    name: String,
    source: Option<crate::stock_media::Asset>,
    rendition: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    generated: Option<crate::cover_generation::GeneratedOrigin>,
}

impl BackgroundAsset {
    fn record(&self, path: String) -> AssetRecord {
        AssetRecord {
            path,
            name: self.name.clone(),
            kind: AssetKind::Photo,
            source: self.source.clone(),
            rendition: self.rendition,
        }
    }
}

impl TitleCardPreset {
    fn capture(name: &str, segment: &TextSegment) -> Result<Self, PresetError> {
        let card = segment
            .title_card
            .as_ref()
            .ok_or(PresetError::Invalid("not a title card"))?;
        let preset = Self {
            version: VERSION,
            name: checked_name(name)?,
            duration: segment.end - segment.start,
            template: card.template,
            template_version: card.template_version,
            content: segment.content.clone(),
            kicker: card.kicker.clone(),
            subtitle: card.subtitle.clone(),
            background: card.background.clone(),
            appearance: Appearance::from_segment(segment),
            background_asset: None,
        };
        preset.validate(false)?;
        Ok(preset)
    }

    fn validate(&self, stored: bool) -> Result<(), PresetError> {
        if self.version != VERSION || self.template_version != TITLE_CARD_TEMPLATE_VERSION {
            return Err(PresetError::Invalid("unsupported version"));
        }
        if checked_name(&self.name)? != self.name {
            return Err(PresetError::Invalid("name"));
        }
        if !self.duration.is_finite()
            || !(MIN_TITLE_CARD_DURATION..=MAX_TITLE_CARD_DURATION).contains(&self.duration)
        {
            return Err(PresetError::Invalid("duration"));
        }
        if self.content.len() > 8192 || self.content.contains('\0') {
            return Err(PresetError::Invalid("title"));
        }
        self.appearance.validate()?;
        if let Some(asset) = &self.background_asset {
            if asset
                .generated
                .as_ref()
                .is_some_and(|origin| !origin.is_valid() || asset.source.is_some())
            {
                return Err(PresetError::Invalid("generated background origin"));
            }
            if !matches!(self.background, TitleCardBackground::ProjectImage { .. })
                || asset.name.len() > 4096
                || asset.name.contains('\0')
            {
                return Err(PresetError::Invalid("background source"));
            }
            asset_library::validate_record(&asset.record("background.png".into()))
                .map_err(PresetError::Image)?;
        }
        let segment = self.segment(TitleCardRole::Intro)?;
        segment
            .title_card
            .as_ref()
            .ok_or(PresetError::Invalid("title card"))?
            .validate()
            .map_err(|_| PresetError::Invalid("title card metadata"))?;
        if stored
            && let TitleCardBackground::ProjectImage { path, .. } = &self.background
            && !matches!(
                path.as_str(),
                "background.png"
                    | "background.jpg"
                    | "background.webp"
                    | "background.gif"
                    | "background.bmp"
            )
        {
            return Err(PresetError::Invalid("background resource path"));
        }
        Ok(())
    }

    pub(crate) fn segment(&self, role: TitleCardRole) -> Result<TextSegment, PresetError> {
        let mut segment = TextSegment::new_title_card(role, &self.content);
        segment.end = self.duration;
        let mut value = serde_json::to_value(&segment)?;
        let style = serde_json::to_value(&self.appearance)?;
        let target = value
            .as_object_mut()
            .ok_or(PresetError::Invalid("appearance"))?;
        for (key, field) in style
            .as_object()
            .ok_or(PresetError::Invalid("appearance"))?
        {
            target.insert(key.clone(), field.clone());
        }
        segment = serde_json::from_value(value)?;
        let card = segment
            .title_card
            .as_mut()
            .ok_or(PresetError::Invalid("title card"))?;
        card.template = self.template;
        card.template_version = self.template_version;
        card.kicker = self.kicker.clone();
        card.subtitle = self.subtitle.clone();
        card.background = self.background.clone();
        Ok(segment)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PresetEntry {
    pub id: String,
    pub preset: TitleCardPreset,
}

#[derive(Debug, Default)]
pub(crate) struct PresetListing {
    pub entries: Vec<PresetEntry>,
    pub issues: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub(crate) struct TitleCardPresetStore {
    root: PathBuf,
}

impl Default for TitleCardPresetStore {
    fn default() -> Self {
        Self::new(
            crate::store::app_data_dir()
                .join("title-card-presets")
                .join("v1"),
        )
    }
}

impl TitleCardPresetStore {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(crate) fn list(&self) -> Result<PresetListing, PresetError> {
        let _lock = STORE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(root) = checked_root(&self.root, false)? else {
            return Ok(PresetListing::default());
        };
        let mut listing = PresetListing::default();
        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            let id = entry.file_name().to_string_lossy().into_owned();
            if id.starts_with('.') {
                continue;
            }
            match self.read_manifest(&root, &id) {
                Ok(preset) => listing.entries.push(PresetEntry { id, preset }),
                Err(error) => listing.issues.push((id, error.to_string())),
            }
        }
        listing
            .entries
            .sort_by(|a, b| a.preset.name.cmp(&b.preset.name).then(a.id.cmp(&b.id)));
        listing.issues.sort();
        Ok(listing)
    }

    pub(crate) fn save(
        &self,
        name: &str,
        project_path: &Path,
        segment: &TextSegment,
    ) -> Result<PresetEntry, PresetError> {
        let _lock = STORE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let mut preset = TitleCardPreset::capture(name, segment)?;
        let image = match &preset.background {
            TitleCardBackground::ProjectImage { path, .. } => {
                let project = checked_root(project_path, false)?
                    .ok_or(PresetError::Invalid("missing project"))?;
                let image = read_image(&checked_descendant(&project, path, false)?)?;
                let record = asset_library::read_project_record(&project, path)
                    .map_err(PresetError::Image)?;
                if let Some(record) = &record
                    && record.kind != AssetKind::Photo
                {
                    return Err(PresetError::Invalid("background source kind"));
                }
                preset.background_asset = Some(record.map_or_else(
                    || BackgroundAsset {
                        name: preset.name.clone(),
                        source: None,
                        rendition: None,
                        generated: None,
                    },
                    |record| BackgroundAsset {
                        name: record.name,
                        source: record.source,
                        rendition: record.rendition,
                        generated: None,
                    },
                ));
                if let Some(asset) = &mut preset.background_asset {
                    asset.generated = asset_library::read_generated_origin(&project, path)
                        .map_err(PresetError::Image)?;
                }
                Some(image)
            }
            TitleCardBackground::Solid { .. } => None,
        };
        if let (Some(image), TitleCardBackground::ProjectImage { path, .. }) =
            (&image, &mut preset.background)
        {
            *path = format!("background.{}", image.extension);
        }
        preset.validate(true)?;
        let manifest = encoded_preset(&preset)?;
        let root =
            checked_root(&self.root, true)?.ok_or(PresetError::Invalid("store directory"))?;
        let id = uuid::Uuid::new_v4().simple().to_string();
        let destination = root.join(&id);
        if destination.exists() {
            return Err(PresetError::Invalid("template identity collision"));
        }
        let staging = root.join(format!(".{id}.staging"));
        fs::create_dir(&staging)?;
        let mut cleanup = OwnedPath::directory(staging.clone());
        if let Some(image) = image {
            write_new(
                &staging.join(format!("background.{}", image.extension)),
                &image.bytes,
            )?;
        }
        write_new(&staging.join(MANIFEST), &manifest)?;
        fs::rename(&staging, &destination)?;
        cleanup.keep = true;
        Ok(PresetEntry { id, preset })
    }

    pub(crate) fn rename(&self, id: &str, name: &str) -> Result<(), PresetError> {
        let _lock = STORE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let root = checked_root(&self.root, false)?.ok_or(PresetError::Invalid("missing store"))?;
        let mut preset = self.read_manifest(&root, id)?;
        preset.name = checked_name(name)?;
        let directory = checked_descendant(&root, id, true)?;
        let temporary = directory.join(format!(".{}.json", uuid::Uuid::new_v4().simple()));
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        let mut cleanup = OwnedPath::file(temporary.clone());
        write_open(file, &encoded_preset(&preset)?)?;
        let destination = checked_descendant(&directory, MANIFEST, false)?;
        fs::rename(&temporary, destination)?;
        cleanup.keep = true;
        Ok(())
    }

    pub(crate) fn remove(&self, id: &str) -> Result<(), PresetError> {
        let _lock = STORE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        checked_id(id)?;
        let root = checked_root(&self.root, false)?.ok_or(PresetError::Invalid("missing store"))?;
        let directory = checked_descendant(&root, id, true)?;
        for entry in fs::read_dir(&directory)? {
            let kind = entry?.file_type()?;
            if !kind.is_file() || kind.is_symlink() {
                return Err(PresetError::Invalid("unexpected template resource"));
            }
        }
        let trash = root.join(format!(".{id}.{}.removed", uuid::Uuid::new_v4().simple()));
        fs::rename(&directory, &trash)?;
        if let Err(error) = fs::remove_dir_all(&trash) {
            let _ = fs::rename(&trash, &directory);
            return Err(error.into());
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn load(&self, id: &str) -> Result<TitleCardPreset, PresetError> {
        let _lock = STORE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        self.load_inner(id).map(|(preset, _)| preset)
    }

    pub(crate) fn materialize(
        &self,
        id: &str,
        target_project: &Path,
        role: TitleCardRole,
    ) -> Result<MaterializedTitleCard, PresetError> {
        let _lock = STORE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let (preset, image) = self.load_inner(id)?;
        let project = checked_root(target_project, false)?
            .ok_or(PresetError::Invalid("missing target project"))?;
        let mut segment = preset.segment(role)?;
        let mut asset = ProjectAsset {
            root: Some((project.clone(), fs::metadata(&project)?)),
            files: Vec::new(),
            source: None,
            generated: None,
            directories: Vec::new(),
            keep: false,
        };
        if let Some(image) = image {
            let content = ensure_child_directory(&project, "content", &mut asset.directories)?;
            let images = ensure_child_directory(&content, "images", &mut asset.directories)?;
            let filename = format!(
                "template-{}.{}",
                uuid::Uuid::new_v4().simple(),
                image.extension
            );
            let target = images.join(&filename);
            let file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)?;
            asset.files.push((target.clone(), file.metadata()?));
            write_open(file, &image.bytes)?;
            let relative = format!("content/images/{filename}");
            let record = preset.background_asset.as_ref().map_or_else(
                || AssetRecord {
                    path: relative.clone(),
                    name: preset.name.clone(),
                    kind: AssetKind::Photo,
                    source: None,
                    rendition: None,
                },
                |origin| origin.record(relative.clone()),
            );
            if let Some(origin) = preset
                .background_asset
                .as_ref()
                .and_then(|asset| asset.generated.clone())
            {
                asset.generated = Some(
                    asset_library::PendingGeneratedSource::register(
                        &project,
                        record.clone(),
                        origin,
                    )
                    .map_err(PresetError::Image)?,
                );
            } else {
                asset_library::write_record_at(&target, &record).map_err(PresetError::Image)?;
            }
            let sidecar = asset_library::record_path(&target);
            asset
                .files
                .push((sidecar.clone(), fs::symlink_metadata(sidecar)?));
            if asset.generated.is_none() {
                asset.source = Some(
                    PendingAssetSource::register(&project, record).map_err(PresetError::Image)?,
                );
            }
            if let Some(card) = segment.title_card.as_mut()
                && let TitleCardBackground::ProjectImage { path, .. } = &mut card.background
            {
                *path = relative;
            }
        }
        Ok(MaterializedTitleCard { segment, asset })
    }

    fn read_manifest(&self, root: &Path, id: &str) -> Result<TitleCardPreset, PresetError> {
        checked_id(id)?;
        let directory = checked_descendant(root, id, true)?;
        let path = checked_descendant(&directory, MANIFEST, false)?;
        let preset: TitleCardPreset = serde_json::from_slice(&read_bounded(&path, MAX_JSON)?)?;
        preset.validate(true)?;
        if let TitleCardBackground::ProjectImage { path, .. } = &preset.background {
            checked_descendant(&directory, path, false)?;
        }
        Ok(preset)
    }

    fn load_inner(
        &self,
        id: &str,
    ) -> Result<(TitleCardPreset, Option<ImageResource>), PresetError> {
        let root = checked_root(&self.root, false)?.ok_or(PresetError::Invalid("missing store"))?;
        let preset = self.read_manifest(&root, id)?;
        let image = match &preset.background {
            TitleCardBackground::ProjectImage { path, .. } => {
                let directory = checked_descendant(&root, id, true)?;
                Some(read_image(&checked_descendant(&directory, path, false)?)?)
            }
            TitleCardBackground::Solid { .. } => None,
        };
        Ok((preset, image))
    }
}

pub(crate) struct MaterializedTitleCard {
    segment: TextSegment,
    asset: ProjectAsset,
}

impl MaterializedTitleCard {
    #[cfg(test)]
    pub(crate) fn segment(&self) -> &TextSegment {
        &self.segment
    }

    pub(crate) fn set_track(&mut self, track: u32) {
        self.segment.track = track;
    }

    pub(crate) fn apply(
        &self,
        project: &mut ProjectConfiguration,
    ) -> Result<TitleCardChange, TitleCardError> {
        project.upsert_title_card(self.segment.clone())
    }

    pub(crate) fn is_referenced(&self, project: &ProjectConfiguration) -> bool {
        let Some(expected) = self.segment.title_card.as_ref() else {
            return false;
        };
        project.title_card(expected.role).is_some_and(|segment| {
            segment
                .title_card
                .as_ref()
                .is_some_and(|card| card.background == expected.background)
        })
    }

    pub(crate) fn retain_if_referenced(mut self, project: &ProjectConfiguration) -> bool {
        let referenced = self.is_referenced(project);
        self.asset.keep = referenced;
        if referenced && let Some(source) = self.asset.source.take() {
            source.commit();
        }
        if referenced && let Some(source) = self.asset.generated.take() {
            source.commit();
        }
        referenced
    }
}

#[derive(Default)]
struct ProjectAsset {
    root: Option<(PathBuf, fs::Metadata)>,
    files: Vec<(PathBuf, fs::Metadata)>,
    source: Option<PendingAssetSource>,
    generated: Option<asset_library::PendingGeneratedSource>,
    directories: Vec<PathBuf>,
    keep: bool,
}

impl Drop for ProjectAsset {
    fn drop(&mut self) {
        if !self.keep {
            if let Some(mut source) = self.generated.take()
                && source.rollback().is_err()
            {
                tracing::warn!(
                    "Retained template background because generated metadata rollback failed"
                );
                return;
            }
            if let Some(mut source) = self.source.take()
                && let Err(error) = source.rollback()
            {
                tracing::warn!(%error, "Retained template background because its source rollback failed");
                return;
            }
            if self.root.as_ref().is_some_and(|(root, before)| {
                !fs::symlink_metadata(root).is_ok_and(|after| {
                    after.is_dir() && asset_library::same_file_identity(before, &after)
                })
            }) {
                return;
            }
            for (file, before) in self.files.iter().rev() {
                if fs::symlink_metadata(file).is_ok_and(|after| {
                    after.is_file() && asset_library::same_file_identity(before, &after)
                }) {
                    let _ = fs::remove_file(file);
                }
            }
            for directory in self.directories.iter().rev() {
                let _ = fs::remove_dir(directory);
            }
        }
    }
}

struct OwnedPath {
    path: PathBuf,
    directory: bool,
    keep: bool,
}

impl OwnedPath {
    fn file(path: PathBuf) -> Self {
        Self {
            path,
            directory: false,
            keep: false,
        }
    }

    fn directory(path: PathBuf) -> Self {
        Self {
            path,
            directory: true,
            keep: false,
        }
    }
}

impl Drop for OwnedPath {
    fn drop(&mut self) {
        if !self.keep {
            if self.directory {
                let _ = fs::remove_dir_all(&self.path);
            } else {
                let _ = fs::remove_file(&self.path);
            }
        }
    }
}

struct ImageResource {
    bytes: Vec<u8>,
    extension: &'static str,
}

fn read_image(path: &Path) -> Result<ImageResource, PresetError> {
    let bytes = read_bounded(path, MAX_IMAGE)?;
    let (extension, _, _) =
        cap_gpui_kernels::codec::inspect_overlay_image(&bytes).map_err(PresetError::Image)?;
    Ok(ImageResource { bytes, extension })
}

fn checked_name(name: &str) -> Result<String, PresetError> {
    let name = name.trim();
    if name.is_empty()
        || name.chars().count() > 64
        || name.starts_with('.')
        || name.chars().any(|c| {
            c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
    {
        return Err(PresetError::Invalid("name"));
    }
    Ok(name.to_string())
}

fn checked_id(id: &str) -> Result<(), PresetError> {
    if id.len() != 32
        || !id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(PresetError::Invalid("template identity"));
    }
    Ok(())
}

fn valid_color(color: &str) -> bool {
    color.len() == 7
        && color.starts_with('#')
        && color.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
}

fn checked_root(path: &Path, create: bool) -> Result<Option<PathBuf>, PresetError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(PresetError::Invalid("directory is not an ordinary folder"))
        }
        Ok(_) => Ok(Some(fs::canonicalize(path)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !create => Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path)?;
            checked_root(path, false)
        }
        Err(error) => Err(error.into()),
    }
}

fn checked_descendant(
    root: &Path,
    relative: &str,
    directory: bool,
) -> Result<PathBuf, PresetError> {
    if relative.is_empty()
        || relative.contains(['\\', ':'])
        || relative.split('/').any(|p| matches!(p, "" | "." | ".."))
    {
        return Err(PresetError::Invalid("resource path"));
    }
    let mut path = root.to_path_buf();
    let components: Vec<_> = Path::new(relative).components().collect();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Err(PresetError::Invalid("resource path"));
        };
        path.push(name);
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink()
            || ((index + 1 < components.len() || directory) && !metadata.is_dir())
            || (index + 1 == components.len() && !directory && !metadata.is_file())
        {
            return Err(PresetError::Invalid(
                "resource is not an ordinary file or folder",
            ));
        }
    }
    if !fs::canonicalize(&path)?.starts_with(root) {
        return Err(PresetError::Invalid("resource escaped its folder"));
    }
    Ok(path)
}

fn ensure_child_directory(
    parent: &Path,
    name: &str,
    created: &mut Vec<PathBuf>,
) -> Result<PathBuf, PresetError> {
    let path = parent.join(name);
    match fs::create_dir(&path) {
        Ok(()) => created.push(path.clone()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    checked_descendant(parent, name, true)
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, PresetError> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(PresetError::Invalid("resource size"));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(PresetError::Invalid("resource size"));
    }
    Ok(bytes)
}

fn write_open(mut file: fs::File, bytes: &[u8]) -> Result<(), PresetError> {
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), PresetError> {
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    write_open(file, bytes)
}

fn encoded_preset(preset: &TitleCardPreset) -> Result<Vec<u8>, PresetError> {
    let bytes = serde_json::to_vec_pretty(preset)?;
    if bytes.len() as u64 > MAX_JSON {
        return Err(PresetError::Invalid("metadata size"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_card_generated_origin_survives_template_copy_and_rejected_copy_rolls_back() {
        use crate::cover_generation::{DimensionStatus, Dimensions, GeneratedOrigin};
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.cap");
        let target = directory.path().join("target.cap");
        fs::create_dir(&target).unwrap();
        let segment = image_card(&source);
        let dimensions = Dimensions {
            width: 4,
            height: 3,
        };
        let origin = GeneratedOrigin {
            version: 1,
            provider: "volcengineArk".into(),
            requested_model: "synthetic-image-model".into(),
            reported_model: None,
            target: dimensions,
            requested: dimensions,
            actual: dimensions,
            dimension_status: DimensionStatus::RequestedSize,
            created: None,
        };
        let record = AssetRecord {
            path: background_path(&segment).into(),
            name: "Generated background".into(),
            kind: AssetKind::Photo,
            source: None,
            rendition: None,
        };
        asset_library::PendingGeneratedSource::register(&source, record, origin.clone())
            .unwrap()
            .commit();
        let store = TitleCardPresetStore::new(directory.path().join("templates"));
        let entry = store.save("Generated cover", &source, &segment).unwrap();
        let rejected = store
            .materialize(&entry.id, &target, TitleCardRole::Intro)
            .unwrap();
        let rejected_path = background_path(rejected.segment()).to_string();
        assert_eq!(
            asset_library::read_generated_origin(&target, &rejected_path).unwrap(),
            Some(origin.clone())
        );
        drop(rejected);
        assert!(!target.join(rejected_path).exists());
        let accepted = store
            .materialize(&entry.id, &target, TitleCardRole::Intro)
            .unwrap();
        let accepted_path = background_path(accepted.segment()).to_string();
        let mut configuration = project();
        accepted.apply(&mut configuration).unwrap();
        assert!(accepted.retain_if_referenced(&configuration));
        assert_eq!(
            asset_library::read_generated_origin(&target, &accepted_path).unwrap(),
            Some(origin)
        );
        assert!(
            asset_library::read_project_record(&target, &accepted_path)
                .unwrap()
                .unwrap()
                .source
                .is_none()
        );
    }

    fn project() -> ProjectConfiguration {
        serde_json::from_value(serde_json::json!({
            "timeline": {"segments": [{"start": 0.0, "end": 5.0, "timescale": 1.0}], "zoomSegments": []}
        })).unwrap()
    }

    fn image_card(project: &Path) -> TextSegment {
        fs::create_dir_all(project.join("content/images")).unwrap();
        image::RgbaImage::from_pixel(4, 3, image::Rgba([20, 30, 40, 255]))
            .save(project.join("content/images/source.png"))
            .unwrap();
        let mut segment = TextSegment::new_title_card(TitleCardRole::Outro, "精品片尾");
        segment.start = 12.0;
        segment.end = 15.0;
        segment.track = 7;
        segment.enabled = false;
        let card = segment.title_card.as_mut().unwrap();
        card.kicker = "CAPWISE".into();
        card.subtitle = "下次见".into();
        card.background = TitleCardBackground::ProjectImage {
            path: "content/images/source.png".into(),
            fit: cap_project::TitleCardImageFit::Cover,
            focus: XY::new(0.3, 0.6),
            dim: 0.2,
        };
        segment
    }

    fn background_path(segment: &TextSegment) -> &str {
        match &segment.title_card.as_ref().unwrap().background {
            TitleCardBackground::ProjectImage { path, .. } => path,
            _ => panic!("expected image background"),
        }
    }

    fn online_record(path: &str) -> AssetRecord {
        use crate::stock_media::{Asset, Attribution, MediaKind, Provider, Rendition};
        AssetRecord {
            path: path.into(),
            name: "Photographer landscape".into(),
            kind: AssetKind::Photo,
            rendition: Some(0),
            source: Some(Asset {
                provider: Provider::Pixabay,
                kind: MediaKind::Photo,
                id: "123".into(),
                description: "Landscape".into(),
                thumbnail_url: Some("https://cdn.pixabay.com/photo/2024/01/01/photo.jpg".into()),
                renditions: vec![Rendition {
                    label: "large".into(),
                    url: "https://cdn.pixabay.com/photo/2024/01/01/photo.jpg".into(),
                    width: Some(1280),
                    height: Some(720),
                    approximate_bytes: None,
                }],
                duration_seconds: None,
                download_location: None,
                attribution: Attribution {
                    author: "Photographer".into(),
                    author_url: "https://pixabay.com/users/photographer-1/".into(),
                    source_url: "https://pixabay.com/photos/id-123/".into(),
                    license_name: "Pixabay Content License".into(),
                    license_url: "https://pixabay.com/service/terms/".into(),
                    api_attribution_required: false,
                    search_source_credit_required: true,
                    additional_rights_may_apply: true,
                },
            }),
        }
    }

    #[test]
    fn title_card_presets_preserve_online_source_across_projects_and_missing_sidecar() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.cap");
        let target = directory.path().join("target.cap");
        fs::create_dir(&target).unwrap();
        let segment = image_card(&source);
        let original = online_record(background_path(&segment));
        asset_library::write_record(&source, &original).unwrap();
        fs::remove_file(asset_library::record_path(&source.join(&original.path))).unwrap();
        let store = TitleCardPresetStore::new(directory.path().join("templates"));
        let entry = store.save("Source", &source, &segment).unwrap();
        assert!(!asset_library::record_path(&source.join(&original.path)).exists());
        let stored = serde_json::to_string(&entry.preset).unwrap();
        assert!(!stored.contains(&original.path));
        fs::remove_dir_all(&source).unwrap();
        let pending = store
            .materialize(&entry.id, &target, TitleCardRole::Intro)
            .unwrap();
        let relative = background_path(pending.segment()).to_string();
        let mut config = project();
        pending.apply(&mut config).unwrap();
        assert!(pending.retain_if_referenced(&config));
        let recovered = asset_library::read_project_record(&target, &relative)
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(recovered.source).unwrap(),
            serde_json::to_value(original.source).unwrap()
        );
        assert_eq!(recovered.rendition, original.rendition);
        fs::remove_file(asset_library::record_path(&target.join(&relative))).unwrap();
        assert!(
            asset_library::read_project_record(&target, &relative)
                .unwrap()
                .unwrap()
                .source
                .is_some()
        );
        store.remove(&entry.id).unwrap();
        assert!(target.join(relative).is_file());
    }

    #[test]
    fn title_card_presets_cancel_source_registration_keeps_other_concurrent_assets() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.cap");
        let target = directory.path().join("target.cap");
        fs::create_dir(&target).unwrap();
        let segment = image_card(&source);
        asset_library::write_record(&source, &online_record(background_path(&segment))).unwrap();
        let store = TitleCardPresetStore::new(directory.path().join("templates"));
        let entry = store.save("Source", &source, &segment).unwrap();
        drop(
            store
                .materialize(&entry.id, &target, TitleCardRole::Intro)
                .unwrap(),
        );
        assert_eq!(fs::read_dir(&target).unwrap().count(), 0);
        let pending = store
            .materialize(&entry.id, &target, TitleCardRole::Intro)
            .unwrap();
        let pending_path = background_path(pending.segment()).to_string();
        let other = image_card(&target);
        let other_record = online_record(background_path(&other));
        asset_library::write_record(&target, &other_record).unwrap();
        drop(pending);
        assert!(!target.join(pending_path).exists());
        let records = asset_library::collect_records(&target);
        assert!(!records.invalid_metadata);
        assert_eq!(records.records.len(), 1);
        assert_eq!(records.records[0].path, other_record.path);
    }

    #[cfg(unix)]
    #[test]
    fn title_card_presets_cancel_does_not_touch_replaced_project_directory() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.cap");
        let target = directory.path().join("target.cap");
        let moved = directory.path().join("moved.cap");
        fs::create_dir(&target).unwrap();
        let segment = image_card(&source);
        asset_library::write_record(&source, &online_record(background_path(&segment))).unwrap();
        let store = TitleCardPresetStore::new(directory.path().join("templates"));
        let entry = store.save("Source", &source, &segment).unwrap();
        let pending = store
            .materialize(&entry.id, &target, TitleCardRole::Intro)
            .unwrap();
        let relative = background_path(pending.segment()).to_string();
        fs::rename(&target, &moved).unwrap();
        fs::create_dir_all(target.join("content/images")).unwrap();
        fs::write(target.join(&relative), b"replacement project file").unwrap();
        fs::write(target.join("asset-catalog.json"), b"replacement catalog").unwrap();
        drop(pending);
        assert_eq!(
            fs::read(target.join(&relative)).unwrap(),
            b"replacement project file"
        );
        assert_eq!(
            fs::read(target.join("asset-catalog.json")).unwrap(),
            b"replacement catalog"
        );
        assert!(moved.join(relative).is_file());
    }

    #[test]
    fn title_card_failed_registration_cleans_sidecar_and_preserves_existing_content() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.cap");
        let target = directory.path().join("target.cap");
        let segment = image_card(&source);
        let record = online_record(background_path(&segment));
        asset_library::write_record(&source, &record).unwrap();
        let store = TitleCardPresetStore::new(directory.path().join("templates"));
        let entry = store.save("Synthetic source", &source, &segment).unwrap();
        let images = target.join("content/images");
        fs::create_dir_all(&images).unwrap();
        let existing = images.join("existing.png");
        fs::write(&existing, b"unrelated user content").unwrap();
        fs::write(target.join("asset-catalog.json"), b"broken catalog").unwrap();
        assert!(
            store
                .materialize(&entry.id, &target, TitleCardRole::Intro)
                .is_err()
        );
        assert_eq!(fs::read(&existing).unwrap(), b"unrelated user content");
        assert_eq!(fs::read_dir(&images).unwrap().count(), 1);
        assert_eq!(
            fs::read(target.join("asset-catalog.json")).unwrap(),
            b"broken catalog"
        );
    }

    #[test]
    fn title_card_presets_reject_corrupt_source_at_save_load_and_registration() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.cap");
        let target = directory.path().join("target.cap");
        fs::create_dir(&target).unwrap();
        let segment = image_card(&source);
        let record = online_record(background_path(&segment));
        asset_library::write_record(&source, &record).unwrap();
        let root = directory.path().join("templates");
        let store = TitleCardPresetStore::new(root.clone());
        let entry = store.save("Source", &source, &segment).unwrap();
        fs::write(
            asset_library::record_path(&source.join(&record.path)),
            b"broken",
        )
        .unwrap();
        assert!(store.save("Reject", &source, &segment).is_err());
        fs::write(target.join("asset-catalog.json"), b"broken catalog").unwrap();
        assert!(
            store
                .materialize(&entry.id, &target, TitleCardRole::Intro)
                .is_err()
        );
        assert!(!target.join("content").exists());
        assert_eq!(
            fs::read(target.join("asset-catalog.json")).unwrap(),
            b"broken catalog"
        );
        let manifest = root.join(&entry.id).join(MANIFEST);
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
        value["backgroundAsset"]["source"]["attribution"]["author_url"] =
            serde_json::json!("file:///private/secret");
        fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(store.load(&entry.id).is_err());
        assert_eq!(store.list().unwrap().issues.len(), 1);
    }

    #[test]
    fn title_card_presets_cross_project_reopen_owns_images_and_excludes_project_state() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.cap");
        let target = directory.path().join("target.cap");
        fs::create_dir(&target).unwrap();
        let segment = image_card(&source);
        let store_path = directory.path().join("templates");
        let entry = TitleCardPresetStore::new(store_path.clone())
            .save("片尾", &source, &segment)
            .unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&fs::read(store_path.join(&entry.id).join(MANIFEST)).unwrap())
                .unwrap();
        for forbidden in [
            "id",
            "role",
            "start",
            "end",
            "track",
            "enabled",
            "captions",
            "timeline",
            "titleCard",
        ] {
            assert!(json.get(forbidden).is_none());
            assert!(json["appearance"].get(forbidden).is_none());
        }
        fs::remove_dir_all(&source).unwrap();
        let reopened = TitleCardPresetStore::new(store_path);
        let loaded = reopened.load(&entry.id).unwrap();
        assert_eq!(loaded.duration, 3.0);
        let materialized = reopened
            .materialize(&entry.id, &target, TitleCardRole::Intro)
            .unwrap();
        assert!(materialized.segment().enabled);
        assert_eq!(materialized.segment().track, 0);
        assert_ne!(
            materialized.segment().title_card.as_ref().unwrap().id,
            segment.title_card.as_ref().unwrap().id
        );
        let copied = background_path(materialized.segment()).to_string();
        let mut project = project();
        let before = project.clone();
        materialized.apply(&mut project).unwrap();
        assert!(materialized.retain_if_referenced(&project));
        assert!(target.join(&copied).is_file());
        assert_eq!(
            project.title_card(TitleCardRole::Intro).unwrap().content,
            segment.content
        );
        let decoded: ProjectConfiguration =
            serde_json::from_slice(&serde_json::to_vec(&project).unwrap()).unwrap();
        assert_eq!(
            background_path(decoded.title_card(TitleCardRole::Intro).unwrap()),
            copied
        );
        project = before;
        assert!(project.title_card(TitleCardRole::Intro).is_none());
        assert!(target.join(copied).is_file());
        reopened.remove(&entry.id).unwrap();
        assert!(reopened.list().unwrap().entries.is_empty());
    }

    #[test]
    fn title_card_presets_drop_and_failed_upsert_leave_target_unchanged() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.cap");
        let target = directory.path().join("target.cap");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("project-config.json"), b"original").unwrap();
        let store = TitleCardPresetStore::new(directory.path().join("templates"));
        let entry = store.save("可复用", &source, &image_card(&source)).unwrap();
        let pending = store
            .materialize(&entry.id, &target, TitleCardRole::Intro)
            .unwrap();
        drop(pending);
        assert!(!target.join("content").exists());
        let pending = store
            .materialize(&entry.id, &target, TitleCardRole::Intro)
            .unwrap();
        let mut invalid = ProjectConfiguration::default();
        let original = invalid.clone();
        assert!(pending.apply(&mut invalid).is_err());
        drop(pending);
        assert_eq!(
            serde_json::to_value(&invalid).unwrap(),
            serde_json::to_value(&original).unwrap()
        );
        assert!(!target.join("content").exists());
        assert_eq!(
            fs::read(target.join("project-config.json")).unwrap(),
            b"original"
        );
    }

    #[test]
    fn title_card_presets_rejected_edit_cleans_materialized_image_and_preserves_card_lane() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.cap");
        let target = directory.path().join("target.cap");
        fs::create_dir(&target).unwrap();
        let store = TitleCardPresetStore::new(directory.path().join("templates"));
        let entry = store.save("Image", &source, &image_card(&source)).unwrap();
        let actual = project();
        let mut pending = store
            .materialize(&entry.id, &target, TitleCardRole::Intro)
            .unwrap();
        pending.set_track(3);
        let mut candidate = actual.clone();
        pending.apply(&mut candidate).unwrap();
        assert_eq!(candidate.title_card(TitleCardRole::Intro).unwrap().track, 3);
        assert!(!pending.retain_if_referenced(&actual));
        assert!(!target.join("content").exists());
        let mut existing = TextSegment::new_title_card(TitleCardRole::Intro, "Existing");
        existing.track = 7;
        let old_id = existing.title_card.as_ref().unwrap().id.clone();
        let mut actual = actual;
        actual.upsert_title_card(existing).unwrap();
        let mut pending = store
            .materialize(&entry.id, &target, TitleCardRole::Intro)
            .unwrap();
        pending.set_track(3);
        pending.apply(&mut actual).unwrap();
        assert!(pending.retain_if_referenced(&actual));
        let result = actual.title_card(TitleCardRole::Intro).unwrap();
        assert_eq!(result.track, 7);
        assert_eq!(result.title_card.as_ref().unwrap().id, old_id);
    }

    #[test]
    fn title_card_presets_rename_keeps_image_and_remove_reclaims_only_template() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.cap");
        let root = directory.path().join("templates");
        let store = TitleCardPresetStore::new(root.clone());
        let entry = store.save("原名", &source, &image_card(&source)).unwrap();
        let other = store.save("另一份", &source, &image_card(&source)).unwrap();
        let bytes = fs::read(root.join(&entry.id).join("background.png")).unwrap();
        store.rename(&entry.id, "  新名称  ").unwrap();
        assert_eq!(store.load(&entry.id).unwrap().name, "新名称");
        assert_eq!(
            fs::read(root.join(&entry.id).join("background.png")).unwrap(),
            bytes
        );
        store.remove(&entry.id).unwrap();
        assert!(!root.join(&entry.id).exists());
        assert!(store.load(&other.id).is_ok());
        assert!(source.join("content/images/source.png").exists());
    }

    #[test]
    fn title_card_presets_corruption_and_traversal_are_reported_without_target_writes() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.cap");
        let root = directory.path().join("templates");
        let target = directory.path().join("target.cap");
        fs::create_dir(&target).unwrap();
        let store = TitleCardPresetStore::new(root.clone());
        let entry = store.save("模板", &source, &image_card(&source)).unwrap();
        let manifest = root.join(&entry.id).join(MANIFEST);
        let original = fs::read(&manifest).unwrap();
        fs::write(&manifest, b"{broken").unwrap();
        assert_eq!(store.list().unwrap().issues.len(), 1);
        assert!(
            store
                .materialize(&entry.id, &target, TitleCardRole::Intro)
                .is_err()
        );
        let mut json: serde_json::Value = serde_json::from_slice(&original).unwrap();
        json["background"]["path"] = serde_json::json!("../source.cap/content/images/source.png");
        fs::write(&manifest, serde_json::to_vec(&json).unwrap()).unwrap();
        assert!(store.load(&entry.id).is_err());
        for id in ["../source.cap", "/tmp", "", "a/../../b"] {
            assert!(store.load(id).is_err());
            assert!(store.remove(id).is_err());
        }
        assert_eq!(fs::read_dir(&target).unwrap().count(), 0);
        assert!(source.join("content/images/source.png").exists());
    }

    #[test]
    fn title_card_presets_invalid_input_never_creates_a_store() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("templates");
        let store = TitleCardPresetStore::new(root.clone());
        let mut segment = TextSegment::new_title_card(TitleCardRole::Intro, "Title");
        for name in ["", "../bad", "a/b", "a\\b", "a\nb", "a:b"] {
            assert!(store.save(name, directory.path(), &segment).is_err());
        }
        segment.font_size = f32::NAN;
        assert!(store.save("bad", directory.path(), &segment).is_err());
        assert!(!root.exists());
    }

    #[test]
    fn title_card_presets_corrupt_image_and_blocked_destination_preserve_existing_files() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.cap");
        let root = directory.path().join("templates");
        let target = directory.path().join("target.cap");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("content"), b"keep").unwrap();
        let store = TitleCardPresetStore::new(root.clone());
        let entry = store.save("Image", &source, &image_card(&source)).unwrap();
        assert!(
            store
                .materialize(&entry.id, &target, TitleCardRole::Outro)
                .is_err()
        );
        assert_eq!(fs::read(target.join("content")).unwrap(), b"keep");
        fs::write(root.join(&entry.id).join("background.png"), b"not an image").unwrap();
        assert!(store.load(&entry.id).is_err());
        assert_eq!(fs::read_dir(&target).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn title_card_presets_reject_source_template_and_target_symlinks() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.cap");
        let target = directory.path().join("target.cap");
        let outside = directory.path().join("outside");
        fs::create_dir(&target).unwrap();
        fs::create_dir(&outside).unwrap();
        let root = directory.path().join("templates");
        let store = TitleCardPresetStore::new(root.clone());
        let segment = image_card(&source);
        let entry = store.save("Safe", &source, &segment).unwrap();
        symlink(&outside, target.join("content")).unwrap();
        assert!(
            store
                .materialize(&entry.id, &target, TitleCardRole::Intro)
                .is_err()
        );
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
        let image = root.join(&entry.id).join("background.png");
        fs::remove_file(&image).unwrap();
        symlink(source.join("content/images/source.png"), &image).unwrap();
        assert!(store.load(&entry.id).is_err());
        assert!(store.remove(&entry.id).is_err());
        assert!(source.join("content/images/source.png").exists());
        let original = source.join("content/images/source.png");
        let moved = outside.join("real.png");
        fs::rename(&original, &moved).unwrap();
        symlink(&moved, &original).unwrap();
        assert!(store.save("Reject", &source, &segment).is_err());
        symlink(&root, directory.path().join("store-link")).unwrap();
        assert!(
            TitleCardPresetStore::new(directory.path().join("store-link"))
                .list()
                .is_err()
        );
    }
}
