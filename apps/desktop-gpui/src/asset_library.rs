use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};

use crate::stock_media::{Asset, MediaKind, Rendition};

mod generated;
pub(crate) use generated::{PendingGeneratedSource, read_generated_origin};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum AssetKind {
    Photo,
    Video,
    Audio,
}

impl From<MediaKind> for AssetKind {
    fn from(kind: MediaKind) -> Self {
        match kind {
            MediaKind::Photo => Self::Photo,
            MediaKind::Video => Self::Video,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct AssetRecord {
    pub path: String,
    pub name: String,
    pub kind: AssetKind,
    pub source: Option<Asset>,
    pub rendition: Option<usize>,
}

#[derive(Default)]
pub(crate) struct AssetInventory {
    pub records: Vec<AssetRecord>,
    pub invalid_metadata: bool,
    catalog_unavailable: bool,
}

const CATALOG_FILE: &str = "asset-catalog.json";
const CATALOG_LIMIT: u64 = 32 * 1024 * 1024;
static CATALOG_LOCK: Mutex<()> = Mutex::new(());

#[derive(Serialize, Deserialize)]
struct AssetCatalog {
    version: u32,
    records: BTreeMap<String, AssetRecord>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    generated: BTreeMap<String, generated::GeneratedRecord>,
}

fn metadata_error() -> String {
    crate::i18n::tr::editor::assets::metadata_error().into()
}

fn same_origin(left: &AssetRecord, right: &AssetRecord) -> bool {
    left.path == right.path
        && left.kind == right.kind
        && left.rendition == right.rendition
        && serde_json::to_value(&left.source).ok() == serde_json::to_value(&right.source).ok()
}

fn load_catalog(project: &Path) -> Result<Option<AssetCatalog>, String> {
    let path = project.join(CATALOG_FILE);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(metadata_error()),
    };
    if !metadata.is_file() || metadata.len() > CATALOG_LIMIT {
        return Err(metadata_error());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| metadata_error())?
        .take(CATALOG_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| metadata_error())?;
    if bytes.len() as u64 > CATALOG_LIMIT {
        return Err(metadata_error());
    }
    let catalog: AssetCatalog = serde_json::from_slice(&bytes).map_err(|_| metadata_error())?;
    if catalog.version != 1 || catalog.records.len() + catalog.generated.len() > 500 {
        return Err(metadata_error());
    }
    for (path, record) in &catalog.generated {
        if path != &record.record.path || catalog.records.contains_key(path) {
            return Err(metadata_error());
        }
        record.validate()?;
    }
    for (path, record) in &catalog.records {
        if path != &record.path
            || path.is_empty()
            || record.source.is_none()
            || Path::new(path)
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(metadata_error());
        }
        validate_record(record)?;
    }
    Ok(Some(catalog))
}

fn save_catalog(project: &Path, catalog: &AssetCatalog) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(catalog).map_err(|_| metadata_error())?;
    if bytes.len() as u64 > CATALOG_LIMIT || catalog.records.len() + catalog.generated.len() > 500 {
        return Err(metadata_error());
    }
    let temporary = project.join(format!(".asset-catalog-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| metadata_error())?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| metadata_error())?;
        drop(file);
        std::fs::rename(&temporary, project.join(CATALOG_FILE)).map_err(|_| metadata_error())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

fn register_source(project: &Path, record: &AssetRecord) -> Result<bool, String> {
    register_source_state(project, record).map(|(inserted, _)| inserted)
}

fn register_source_state(project: &Path, record: &AssetRecord) -> Result<(bool, bool), String> {
    if record.source.is_none() {
        return Ok((false, false));
    }
    validate_record(record)?;
    if project_asset_path(project, &record.path).is_none() {
        return Err(metadata_error());
    }
    let _lock = CATALOG_LOCK.lock().map_err(|_| metadata_error())?;
    let previous = load_catalog(project)?;
    let created = previous.is_none();
    let mut catalog = previous.unwrap_or(AssetCatalog {
        version: 1,
        records: BTreeMap::new(),
        generated: BTreeMap::new(),
    });
    if catalog.generated.contains_key(&record.path) {
        return Err(metadata_error());
    }
    if let Some(existing) = catalog.records.get(&record.path) {
        if !same_origin(existing, record) {
            return Err(metadata_error());
        }
        return Ok((false, false));
    }
    let _ = catalog.records.insert(record.path.clone(), record.clone());
    save_catalog(project, &catalog)?;
    Ok((true, created))
}

pub(crate) fn remove_source_if_matches(project: &Path, record: &AssetRecord) -> Result<(), String> {
    remove_source_if_matches_inner(project, record, false)
}

fn remove_source_if_matches_inner(
    project: &Path,
    record: &AssetRecord,
    remove_empty: bool,
) -> Result<(), String> {
    if record.source.is_none() {
        return Ok(());
    }
    let _lock = CATALOG_LOCK.lock().map_err(|_| metadata_error())?;
    let Some(mut catalog) = load_catalog(project)? else {
        return Ok(());
    };
    let Some(existing) = catalog.records.get(&record.path) else {
        return Ok(());
    };
    if serde_json::to_value(existing).ok() != serde_json::to_value(record).ok() {
        return Err(metadata_error());
    }
    let _ = catalog.records.remove(&record.path);
    if remove_empty && catalog.records.is_empty() && catalog.generated.is_empty() {
        std::fs::remove_file(project.join(CATALOG_FILE)).map_err(|_| metadata_error())
    } else {
        save_catalog(project, &catalog)
    }
}

pub(crate) struct PendingAssetSource {
    project: PathBuf,
    record: AssetRecord,
    inserted: bool,
    created_catalog: bool,
    directory: std::fs::Metadata,
}

impl PendingAssetSource {
    pub(crate) fn register(project: &Path, record: AssetRecord) -> Result<Self, String> {
        let project = project.canonicalize().map_err(|_| metadata_error())?;
        let directory = std::fs::symlink_metadata(&project).map_err(|_| metadata_error())?;
        if !directory.is_dir() || directory.file_type().is_symlink() {
            return Err(metadata_error());
        }
        let (inserted, created_catalog) = register_source_state(&project, &record)?;
        Ok(Self {
            project,
            record,
            inserted,
            created_catalog,
            directory,
        })
    }

    pub(crate) fn commit(mut self) {
        self.inserted = false;
    }

    pub(crate) fn rollback(&mut self) -> Result<(), String> {
        if !self.inserted {
            return Ok(());
        }
        let current = std::fs::symlink_metadata(&self.project).map_err(|_| metadata_error())?;
        if !current.is_dir() || !same_file_identity(&self.directory, &current) {
            return Err(metadata_error());
        }
        remove_source_if_matches_inner(&self.project, &self.record, self.created_catalog)?;
        self.inserted = false;
        Ok(())
    }
}

impl Drop for PendingAssetSource {
    fn drop(&mut self) {
        if let Err(error) = self.rollback() {
            tracing::warn!(%error, "Could not roll back an unpublished asset source");
        }
    }
}

pub(crate) fn same_file_identity(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        before.dev() == after.dev() && before.ino() == after.ino()
    }
    #[cfg(not(unix))]
    {
        before
            .created()
            .ok()
            .is_some_and(|created| after.created().ok() == Some(created))
    }
}

pub(crate) fn read_project_record(
    project: &Path,
    relative: &str,
) -> Result<Option<AssetRecord>, String> {
    let media = project_asset_path(project, relative).ok_or_else(metadata_error)?;
    let expected = load_catalog(project)?.and_then(|mut catalog| {
        catalog
            .records
            .remove(relative)
            .or_else(|| catalog.generated.remove(relative).map(|entry| entry.record))
    });
    read_generated_origin(project, relative)?;
    match std::fs::symlink_metadata(record_path(&media)) {
        Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
            return Err(metadata_error());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(metadata_error()),
    }
    let sidecar = read_record(&media, false)?;
    if let (Some(expected), Some(sidecar)) = (&expected, &sidecar)
        && !same_origin(expected, sidecar)
    {
        return Err(metadata_error());
    }
    let record = expected.or(sidecar);
    if record
        .as_ref()
        .is_some_and(|record| record.path != relative)
    {
        return Err(metadata_error());
    }
    Ok(record)
}

pub(crate) fn ensure_record(project: &Path, record: &AssetRecord) -> Result<AssetRecord, String> {
    ensure_record_inner(project, record, false)
}

pub(crate) fn ensure_reusable_record(
    project: &Path,
    record: &AssetRecord,
) -> Result<AssetRecord, String> {
    ensure_record_inner(project, record, true)
}

fn ensure_record_inner(
    project: &Path,
    record: &AssetRecord,
    require_persisted_source: bool,
) -> Result<AssetRecord, String> {
    let media = project_asset_path(project, &record.path)
        .ok_or_else(|| crate::i18n::tr::editor::assets::missing_media().to_string())?;
    let expected = load_catalog(project)?.and_then(|mut catalog| {
        catalog.records.remove(&record.path).or_else(|| {
            catalog
                .generated
                .remove(&record.path)
                .map(|entry| entry.record)
        })
    });
    generated::ensure(project, &record.path)?;
    let sidecar = read_record(&media, false)?;
    if require_persisted_source
        && record.source.is_some()
        && expected.is_none()
        && sidecar.is_none()
    {
        return Err(metadata_error());
    }
    let current = match (&expected, &sidecar) {
        (Some(expected), Some(sidecar)) if !same_origin(expected, sidecar) => {
            return Err(metadata_error());
        }
        (Some(expected), _) => expected.clone(),
        (None, Some(sidecar)) => sidecar.clone(),
        (None, None) => record.clone(),
    };
    if current.path != record.path
        || current.kind != record.kind
        || (record.source.is_some() && !same_origin(&current, record))
    {
        return Err(metadata_error());
    }
    if sidecar.is_none() {
        write_record(project, &current)?;
    } else {
        register_source(project, &current)?;
    }
    Ok(current)
}

pub(crate) fn prepare_image_reuse(
    project: &Path,
    record: &AssetRecord,
) -> Result<crate::import::ImportedEditorImage, String> {
    if record.kind != AssetKind::Photo {
        return Err(crate::i18n::tr::editor::assets::metadata_error().into());
    }
    let media = project_asset_path(project, &record.path)
        .ok_or_else(|| crate::i18n::tr::editor::assets::missing_media().to_string())?;
    let current = ensure_reusable_record(project, record)?;
    if current.path != record.path || current.kind != AssetKind::Photo {
        return Err(crate::i18n::tr::editor::assets::metadata_error().into());
    }
    validate_record(&current)?;
    const MAX_BYTES: u64 = 64 * 1024 * 1024;
    let file = std::fs::File::open(&media).map_err(|error| error.to_string())?;
    if file.metadata().map_err(|error| error.to_string())?.len() > MAX_BYTES {
        return Err(crate::i18n::tr::editor::assets::too_large().into());
    }
    let mut encoded = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut encoded)
        .map_err(|error| error.to_string())?;
    if encoded.len() as u64 > MAX_BYTES {
        return Err(crate::i18n::tr::editor::assets::too_large().into());
    }
    let (_, width, height) = cap_gpui_kernels::codec::inspect_overlay_image(&encoded)?;
    Ok(crate::import::ImportedEditorImage {
        path: current.path,
        name: current.name,
        width,
        height,
    })
}

pub(crate) fn project_asset_path(project: &Path, relative: &str) -> Option<PathBuf> {
    let path = Path::new(relative);
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return None;
    }
    let root = project.canonicalize().ok()?;
    let path = root.join(path).canonicalize().ok()?;
    (path.starts_with(&root) && path.is_file()).then_some(path)
}

pub(crate) fn record_path(media: &Path) -> PathBuf {
    let mut name = media.as_os_str().to_os_string();
    name.push(".asset.json");
    PathBuf::from(name)
}

pub(crate) fn write_record(project: &Path, record: &AssetRecord) -> Result<(), String> {
    let media = project_asset_path(project, &record.path)
        .ok_or_else(|| "Imported media is no longer available".to_string())?;
    write_record_at(&media, record)?;
    if let Err(error) = register_source(project, record) {
        let _ = std::fs::remove_file(record_path(&media));
        return Err(error);
    }
    Ok(())
}

pub(crate) fn write_record_at(media: &Path, record: &AssetRecord) -> Result<(), String> {
    validate_record(record)?;
    let bytes = serde_json::to_vec_pretty(record).map_err(|error| error.to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(record_path(media))
        .map_err(|error| error.to_string())?;
    let result = file.write_all(&bytes).and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = result {
        let _ = std::fs::remove_file(record_path(media));
        return Err(error.to_string());
    }
    Ok(())
}

pub(crate) fn read_record(media: &Path, required: bool) -> Result<Option<AssetRecord>, String> {
    let path = record_path(media);
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !required => return Ok(None),
        Err(_) => return Err(crate::i18n::tr::editor::assets::metadata_error().into()),
    };
    if metadata.len() > 128 * 1024 {
        return Err(crate::i18n::tr::editor::assets::metadata_error().into());
    }
    let bytes = std::fs::read(path)
        .map_err(|_| crate::i18n::tr::editor::assets::metadata_error().to_string())?;
    let record = serde_json::from_slice(&bytes)
        .map_err(|_| crate::i18n::tr::editor::assets::metadata_error().to_string())?;
    validate_record(&record)?;
    Ok(Some(record))
}

pub(crate) fn validate_record(record: &AssetRecord) -> Result<(), String> {
    if let Some(source) = &record.source {
        if AssetKind::from(source.kind) != record.kind
            || !record
                .rendition
                .is_some_and(|index| index < source.renditions.len())
            || crate::stock_media::validate_asset(source).is_err()
        {
            return Err(crate::i18n::tr::editor::assets::metadata_error().into());
        }
    } else if record.rendition.is_some() {
        return Err(crate::i18n::tr::editor::assets::metadata_error().into());
    }
    Ok(())
}

pub(crate) fn collect_records(project: &Path) -> AssetInventory {
    fn visit(root: &Path, directory: &Path, depth: usize, inventory: &mut AssetInventory) {
        if depth > 5 || inventory.records.len() >= 500 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                visit(root, &path, depth + 1, inventory);
            } else if kind.is_file() && path.to_string_lossy().ends_with(".asset.json") {
                let record = entry
                    .metadata()
                    .ok()
                    .filter(|meta| meta.len() <= 128 * 1024)
                    .and_then(|_| std::fs::read(&path).ok())
                    .and_then(|bytes| serde_json::from_slice::<AssetRecord>(&bytes).ok())
                    .filter(|record| validate_record(record).is_ok());
                if let Some(record) = record {
                    if let Some(existing) = inventory
                        .records
                        .iter()
                        .find(|existing| existing.path == record.path)
                    {
                        if !same_origin(existing, &record) {
                            inventory.invalid_metadata = true;
                        }
                    } else if project_asset_path(root, &record.path).is_some() {
                        inventory.records.push(record);
                    }
                } else {
                    inventory.invalid_metadata = true;
                }
            }
        }
    }
    let mut inventory = AssetInventory::default();
    match load_catalog(project) {
        Ok(Some(catalog)) => {
            inventory.records.extend(catalog.records.into_values());
            inventory
                .records
                .extend(catalog.generated.into_values().map(|entry| entry.record));
        }
        Ok(None) => {}
        Err(_) => {
            inventory.invalid_metadata = true;
            inventory.catalog_unavailable = true;
            return inventory;
        }
    }
    visit(project, &project.join("content"), 0, &mut inventory);
    visit(project, &project.join("assets/audio"), 0, &mut inventory);
    inventory
        .records
        .sort_by(|left, right| left.name.cmp(&right.name));
    inventory
}

pub(crate) fn collect_project_records(
    project: &Path,
    configuration: &cap_project::ProjectConfiguration,
) -> AssetInventory {
    let mut inventory = collect_records(project);
    if inventory.catalog_unavailable {
        return inventory;
    }
    let mut candidates = Vec::new();
    if let Some(timeline) = &configuration.timeline {
        candidates.extend(timeline.image_segments.iter().map(|image| AssetRecord {
            path: image.path.clone(),
            name: image.name.clone(),
            kind: AssetKind::Photo,
            source: None,
            rendition: None,
        }));
        candidates.extend(timeline.audio_segments.iter().map(|audio| {
            AssetRecord {
                path: audio.path.clone(),
                name: audio
                    .name
                    .as_deref()
                    .filter(|name| !name.trim().is_empty())
                    .map_or_else(
                        || {
                            Path::new(&audio.path).file_name().map_or_else(
                                || audio.path.clone(),
                                |name| name.to_string_lossy().into_owned(),
                            )
                        },
                        str::to_string,
                    ),
                kind: AssetKind::Audio,
                source: None,
                rendition: None,
            }
        }));
    }
    if let Ok(meta) = cap_project::RecordingMeta::load_for_project(project)
        && let cap_project::RecordingMetaInner::Studio(studio) = &meta.inner
    {
        let paths: Vec<_> = match studio.as_ref() {
            cap_project::StudioRecordingMeta::SingleSegment { segment } => {
                vec![segment.display.path.as_str().to_string()]
            }
            cap_project::StudioRecordingMeta::MultipleSegments { inner } => inner
                .segments
                .iter()
                .map(|segment| segment.display.path.as_str().to_string())
                .collect(),
        };
        for (index, path) in paths.into_iter().enumerate() {
            let name = configuration
                .timeline
                .as_ref()
                .and_then(|timeline| {
                    timeline
                        .segments
                        .iter()
                        .find(|clip| clip.recording_clip as usize == index)
                })
                .and_then(|clip| clip.name.clone())
                .unwrap_or_else(|| crate::i18n::tr::editor::clips::clip(index + 1));
            candidates.push(AssetRecord {
                path,
                name,
                kind: AssetKind::Video,
                source: None,
                rendition: None,
            });
        }
    }
    for candidate in candidates {
        if inventory
            .records
            .iter()
            .any(|record| record.path == candidate.path)
        {
            continue;
        }
        let Some(media) = project_asset_path(project, &candidate.path) else {
            continue;
        };
        match read_record(&media, false) {
            Ok(None) => inventory.records.push(candidate),
            Ok(Some(record)) if record.path == candidate.path && record.kind == candidate.kind => {
                inventory.records.push(record)
            }
            Ok(Some(_)) | Err(_) => inventory.invalid_metadata = true,
        }
    }
    inventory
        .records
        .sort_by(|left, right| left.name.cmp(&right.name));
    inventory
}

pub(crate) fn image_placement(
    timeline: &cap_project::TimelineConfiguration,
    requested_lane: u32,
    time: f64,
    total: f64,
) -> Option<(u32, f64, f64)> {
    if !time.is_finite() || !total.is_finite() || total <= 0.0 {
        return None;
    }
    let start = crate::editor_edits::overlay_insertion_time(
        timeline,
        crate::editor_timeline::TrackKind::Image,
        time.max(0.0),
    );
    let end = crate::editor_edits::overlay_blocked_spans(
        timeline,
        crate::editor_timeline::TrackKind::Image,
    )
    .iter()
    .filter(|span| span.start > start)
    .map(|span| span.start)
    .fold(start + 3.0, f64::min);
    if !end.is_finite() || end <= start {
        return None;
    }
    let lane = if crate::editor_edits::overlay_overlaps(timeline, requested_lane, start, end, None)
    {
        crate::editor_edits::overlay_front_layer(timeline, start, end)
    } else {
        requested_lane
    };
    Some((lane, start, end))
}

pub(crate) fn image_reveal_time(segment: &cap_project::ImageSegment) -> f64 {
    let animation_in = if segment.animation_in == cap_project::ImageMotion::None {
        0.
    } else {
        segment.animation_in_duration
    };
    let animation_out = if segment.animation_out == cap_project::ImageMotion::None {
        0.
    } else {
        segment.animation_out_duration
    };
    crate::editor_shapes::reveal_time(
        segment.start,
        segment.start,
        segment.end,
        animation_in,
        animation_out,
    )
}

pub(crate) fn recording_index(project: &Path, relative: &str) -> Option<u32> {
    let source = project_asset_path(project, relative)?;
    let meta = cap_project::RecordingMeta::load_for_project(project).ok()?;
    let cap_project::RecordingMetaInner::Studio(studio) = &meta.inner else {
        return None;
    };
    match studio.as_ref() {
        cap_project::StudioRecordingMeta::SingleSegment { segment } => {
            (meta.path(&segment.display.path).canonicalize().ok()? == source).then_some(0)
        }
        cap_project::StudioRecordingMeta::MultipleSegments { inner } => inner
            .segments
            .iter()
            .position(|segment| {
                meta.path(&segment.display.path)
                    .canonicalize()
                    .is_ok_and(|path| path == source)
            })
            .and_then(|index| u32::try_from(index).ok()),
    }
}

pub(crate) fn recommended_rendition(renditions: &[Rendition], target: [u32; 2]) -> usize {
    let needed = target[0].max(target[1]);
    renditions
        .iter()
        .enumerate()
        .filter_map(|(index, item)| Some((index, item.width?.max(item.height?))))
        .min_by_key(|(_, dimension)| {
            if *dimension >= needed {
                (0, *dimension)
            } else {
                (1, u32::MAX - dimension)
            }
        })
        .map_or(0, |(index, _)| index)
}

pub(crate) struct StagedMedia {
    pub path: PathBuf,
    directory: PathBuf,
}

impl StagedMedia {
    pub fn write(bytes: &[u8], extension: &str, record: &AssetRecord) -> Result<Self, String> {
        if !matches!(extension, "jpg" | "jpeg" | "png" | "webp" | "mp4") {
            return Err("Unsupported downloaded media".into());
        }
        let directory =
            std::env::temp_dir().join(format!("capwise-asset-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).map_err(|error| error.to_string())?;
        let staged = Self {
            path: directory.join(format!("asset.{extension}")),
            directory,
        };
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged.path)
            .map_err(|error| error.to_string())?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| error.to_string())?;
        write_record_at(&staged.path, record)?;
        Ok(staged)
    }
}

impl Drop for StagedMedia {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_kind_keeps_existing_photo_video_records_and_adds_audio() {
        for (value, kind) in [
            ("photo", AssetKind::Photo),
            ("video", AssetKind::Video),
            ("audio", AssetKind::Audio),
        ] {
            let record: AssetRecord = serde_json::from_value(serde_json::json!({ "path": "content/source", "name": "Source", "kind": value, "source": null, "rendition": null })).unwrap();
            assert_eq!(record.kind, kind);
            assert_eq!(serde_json::to_value(record).unwrap()["kind"], value);
        }
    }

    #[test]
    fn image_and_sticker_insertion_reveals_content_after_the_title_card_and_entrance() {
        let mut timeline: cap_project::TimelineConfiguration =
            serde_json::from_value(serde_json::json!({ "segments": [], "zoomSegments": [] }))
                .unwrap();
        let mut intro =
            cap_project::TextSegment::new_title_card(cap_project::TitleCardRole::Intro, "Intro");
        intro.end = 1.234;
        timeline.text_segments.push(intro);
        let (lane, start, end) = image_placement(&timeline, 0, 0., 12.).unwrap();
        assert_eq!(start, 1.234);
        assert_eq!(end, 4.234);
        let mut image = cap_project::ImageSegment {
            start,
            end,
            track: lane,
            ..Default::default()
        };
        let preview = image_reveal_time(&image);
        let fps = f64::from(crate::editor_window::EDITOR_PREVIEW_FPS);
        assert!((preview * fps).round() / fps >= start);
        image.animation_in = cap_project::ImageMotion::Pop;
        assert!(image_reveal_time(&image) >= start + image.animation_in_duration);
        image.end = start + 0.1;
        let preview = image_reveal_time(&image);
        assert!(preview > start && preview < image.end);
        let mut outro =
            cap_project::TextSegment::new_title_card(cap_project::TitleCardRole::Outro, "Outro");
        outro.start = 1.434;
        outro.end = 3.434;
        timeline.text_segments.push(outro);
        let (_, start, end) = image_placement(&timeline, 0, 2., 3.434).unwrap();
        assert_eq!(start, 1.234);
        assert_eq!(end, 1.434);
        image.start = start;
        image.end = end;
        let preview = image_reveal_time(&image);
        assert!(preview > start && preview < end);
    }

    #[test]
    fn image_placement_preserves_default_duration_at_video_end() {
        let empty: cap_project::TimelineConfiguration =
            serde_json::from_value(serde_json::json!({"segments": [], "zoomSegments": []}))
                .unwrap();
        assert_eq!(image_placement(&empty, 0, 12., 30.), Some((0, 12., 15.)));
        let mut occupied = empty.clone();
        occupied.image_segments = vec![cap_project::ImageSegment {
            start: 11.,
            end: 15.,
            track: 0,
            ..Default::default()
        }];
        occupied.text_segments = vec![crate::editor_edits::default_text_segment(12., 13., 2)];
        assert_eq!(image_placement(&occupied, 0, 12., 30.), Some((3, 12., 15.)));
        assert_eq!(image_placement(&occupied, 1, 12., 30.), Some((1, 12., 15.)));
        assert_eq!(image_placement(&occupied, 0, 15., 30.), Some((0, 15., 18.)));
        assert_eq!(image_placement(&empty, 0, 29., 30.), Some((0, 29., 32.)));
        assert_eq!(image_placement(&empty, 0, 30., 30.), Some((0, 30., 33.)));
        assert_eq!(image_placement(&empty, 0, f64::NAN, 30.), None);
    }

    #[test]
    fn legacy_inventory_finds_audio_and_images_but_does_not_disguise_broken_provenance() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("content/images")).unwrap();
        std::fs::create_dir_all(root.path().join("assets/audio")).unwrap();
        std::fs::write(root.path().join("content/images/photo.png"), b"fixture").unwrap();
        std::fs::write(root.path().join("assets/audio/speech.wav"), b"fixture").unwrap();
        let mut timeline = serde_json::json!({"segments": [], "zoomSegments": []});
        timeline["imageSegments"] = serde_json::json!([
            {"path":"content/images/photo.png", "name":"Photo", "start":0,"end":3}
        ]);
        timeline["audioSegments"] = serde_json::json!([
            {"path":"assets/audio/speech.wav", "start":0,"end":3}
        ]);
        let config = cap_project::ProjectConfiguration {
            timeline: Some(serde_json::from_value(timeline).unwrap()),
            ..Default::default()
        };
        let inventory = collect_project_records(root.path(), &config);
        assert_eq!(inventory.records.len(), 2);
        assert!(!inventory.invalid_metadata);
        assert!(
            inventory
                .records
                .iter()
                .any(|record| record.kind == AssetKind::Audio)
        );
        std::fs::write(
            record_path(&root.path().join("content/images/photo.png")),
            b"broken",
        )
        .unwrap();
        let inventory = collect_project_records(root.path(), &config);
        assert_eq!(inventory.records.len(), 1);
        assert!(inventory.invalid_metadata);
        assert_eq!(inventory.records[0].kind, AssetKind::Audio);
        assert_eq!(inventory.records[0].name, "speech.wav");
    }

    fn online_record() -> AssetRecord {
        AssetRecord {
            path: "content/images/photo.jpg".into(),
            name: "Photo".into(),
            kind: AssetKind::Photo,
            rendition: Some(0),
            source: Some(Asset {
                provider: crate::stock_media::Provider::Pixabay,
                kind: MediaKind::Photo,
                id: "123".into(),
                description: "Photo".into(),
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
                attribution: crate::stock_media::Attribution {
                    author: "Author".into(),
                    author_url: "https://pixabay.com/users/author-1/".into(),
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

    fn online_png(root: &Path, name: &str) -> AssetRecord {
        let mut record = online_record();
        record.path = format!("content/images/{name}.png");
        let media = root.join(&record.path);
        std::fs::create_dir_all(media.parent().unwrap()).unwrap();
        image::RgbImage::from_pixel(3, 2, image::Rgb([12, 34, 56]))
            .save(media)
            .unwrap();
        record
    }

    #[test]
    fn reopening_without_sidecar_keeps_online_source_and_reuse_restores_it() {
        let root = tempfile::tempdir().unwrap();
        let record = online_png(root.path(), "photo");
        write_record(root.path(), &record).unwrap();
        let sidecar = record_path(&root.path().join(&record.path));
        let original = std::fs::read(&sidecar).unwrap();
        std::fs::remove_file(&sidecar).unwrap();
        let inventory =
            collect_project_records(root.path(), &cap_project::ProjectConfiguration::default());
        assert_eq!(inventory.records.len(), 1);
        assert!(same_origin(&inventory.records[0], &record));
        assert!(!inventory.invalid_metadata);
        assert!(!sidecar.exists());
        prepare_image_reuse(root.path(), &inventory.records[0]).unwrap();
        assert_eq!(std::fs::read(sidecar).unwrap(), original);
    }

    #[test]
    fn corrupted_catalog_cannot_reclassify_online_media_as_local() {
        let root = tempfile::tempdir().unwrap();
        let record = online_png(root.path(), "photo");
        write_record(root.path(), &record).unwrap();
        std::fs::remove_file(record_path(&root.path().join(&record.path))).unwrap();
        std::fs::write(root.path().join(CATALOG_FILE), b"{truncated").unwrap();
        let project: cap_project::ProjectConfiguration =
            serde_json::from_value(serde_json::json!({
                "timeline": { "segments": [], "zoomSegments": [], "imageSegments": [{
                    "path": record.path, "name": "Photo", "start": 0.0, "end": 3.0
                }] }
            }))
            .unwrap();
        let inventory = collect_project_records(root.path(), &project);
        assert!(inventory.invalid_metadata);
        assert!(inventory.records.is_empty());
        assert!(prepare_image_reuse(root.path(), &record).is_err());
        assert_eq!(
            std::fs::read(root.path().join(CATALOG_FILE)).unwrap(),
            b"{truncated"
        );
    }

    #[test]
    fn conflicting_sidecar_never_replaces_catalog_origin() {
        let root = tempfile::tempdir().unwrap();
        let record = online_png(root.path(), "photo");
        write_record(root.path(), &record).unwrap();
        let mut changed = record.clone();
        changed.source.as_mut().unwrap().attribution.author = "Different author".into();
        std::fs::write(
            record_path(&root.path().join(&record.path)),
            serde_json::to_vec(&changed).unwrap(),
        )
        .unwrap();
        let inventory = collect_records(root.path());
        assert!(inventory.invalid_metadata);
        assert!(same_origin(&inventory.records[0], &record));
        assert!(prepare_image_reuse(root.path(), &inventory.records[0]).is_err());
    }

    #[test]
    fn catalog_write_failure_rolls_back_only_the_new_sidecar() {
        let root = tempfile::tempdir().unwrap();
        let record = online_png(root.path(), "photo");
        let media = root.path().join(&record.path);
        let bytes = std::fs::read(&media).unwrap();
        std::fs::create_dir(root.path().join(CATALOG_FILE)).unwrap();
        assert!(write_record(root.path(), &record).is_err());
        assert!(!record_path(&media).exists());
        assert_eq!(std::fs::read(media).unwrap(), bytes);
        assert!(root.path().join(CATALOG_FILE).is_dir());
    }

    #[test]
    fn parallel_source_registration_keeps_both_records() {
        let root = tempfile::tempdir().unwrap();
        let first = online_png(root.path(), "first");
        let second = online_png(root.path(), "second");
        std::thread::scope(|scope| {
            let first = scope.spawn(|| write_record(root.path(), &first));
            let second = scope.spawn(|| write_record(root.path(), &second));
            first.join().unwrap().unwrap();
            second.join().unwrap().unwrap();
        });
        let catalog = load_catalog(root.path()).unwrap().unwrap();
        assert_eq!(catalog.records.len(), 2);
        assert!(catalog.records.contains_key(&first.path));
        assert!(catalog.records.contains_key(&second.path));
    }

    #[test]
    fn failed_publication_removes_only_its_own_pending_source() {
        let root = tempfile::tempdir().unwrap();
        let first = online_png(root.path(), "first");
        let second = online_png(root.path(), "second");
        let pending = PendingAssetSource::register(root.path(), first.clone()).unwrap();
        write_record(root.path(), &second).unwrap();
        drop(pending);
        let catalog = load_catalog(root.path()).unwrap().unwrap();
        assert!(!catalog.records.contains_key(&first.path));
        assert!(catalog.records.contains_key(&second.path));
        PendingAssetSource::register(root.path(), first.clone())
            .unwrap()
            .commit();
        assert!(
            load_catalog(root.path())
                .unwrap()
                .unwrap()
                .records
                .contains_key(&first.path)
        );
    }

    #[test]
    fn legacy_sources_migrate_only_on_reuse_and_local_assets_stay_local() {
        let root = tempfile::tempdir().unwrap();
        let online = online_png(root.path(), "online");
        write_record_at(&root.path().join(&online.path), &online).unwrap();
        assert_eq!(collect_records(root.path()).records.len(), 1);
        assert!(!root.path().join(CATALOG_FILE).exists());
        prepare_image_reuse(root.path(), &online).unwrap();
        assert!(root.path().join(CATALOG_FILE).is_file());
        let mut local = online_png(root.path(), "local");
        local.source = None;
        local.rendition = None;
        assert!(ensure_record(root.path(), &local).unwrap().source.is_none());
        assert!(
            !load_catalog(root.path())
                .unwrap()
                .unwrap()
                .records
                .contains_key(&local.path)
        );
    }

    #[test]
    fn missing_media_keeps_its_known_source_visible_after_reopening() {
        let root = tempfile::tempdir().unwrap();
        let record = online_png(root.path(), "photo");
        write_record(root.path(), &record).unwrap();
        std::fs::remove_file(root.path().join(&record.path)).unwrap();
        let inventory = collect_records(root.path());
        assert_eq!(inventory.records.len(), 1);
        assert!(same_origin(&inventory.records[0], &record));
        assert!(prepare_image_reuse(root.path(), &inventory.records[0]).is_err());
    }

    #[test]
    fn image_reuse_keeps_one_media_and_source_record_across_lanes_and_undo() {
        let root = tempfile::tempdir().unwrap();
        let mut record = online_record();
        record.path = "content/images/photo.png".into();
        let media = root.path().join(&record.path);
        std::fs::create_dir_all(media.parent().unwrap()).unwrap();
        image::RgbImage::from_pixel(3, 2, image::Rgb([255, 30, 20]))
            .save(&media)
            .unwrap();
        write_record(root.path(), &record).unwrap();
        let media_before = std::fs::read(&media).unwrap();
        let record_before = std::fs::read(record_path(&media)).unwrap();
        let mut project = cap_project::ProjectConfiguration {
            timeline: Some(
                serde_json::from_value(serde_json::json!({
                    "segments": [{"start":0,"end":8,"timescale":1}], "zoomSegments":[]
                }))
                .unwrap(),
            ),
            ..Default::default()
        };
        let mut history = crate::editor_edits::ProjectHistory::new(project.clone());
        for expected_lane in 0..2 {
            let imported = prepare_image_reuse(root.path(), &record).unwrap();
            assert_eq!(imported.path, record.path);
            assert_eq!((imported.width, imported.height), (3, 2));
            let timeline = project.timeline.as_mut().unwrap();
            let (lane, start, end) = image_placement(timeline, 0, 3., 8.).unwrap();
            assert_eq!(lane, expected_lane);
            crate::editor_edits::insert_image_segment(
                timeline,
                cap_project::ImageSegment {
                    path: imported.path,
                    name: imported.name,
                    track: lane,
                    start,
                    end,
                    ..Default::default()
                },
            );
            ensure_record(root.path(), &record).unwrap();
            history.record(&project);
        }
        let inventory = collect_project_records(root.path(), &project);
        assert_eq!(inventory.records.len(), 1);
        assert!(inventory.records[0].source.is_some());
        assert_eq!(
            std::fs::read_dir(media.parent().unwrap()).unwrap().count(),
            2
        );
        assert_eq!(std::fs::read(&media).unwrap(), media_before);
        assert_eq!(std::fs::read(record_path(&media)).unwrap(), record_before);
        assert_eq!(
            history
                .undo()
                .unwrap()
                .timeline
                .as_ref()
                .unwrap()
                .image_segments
                .len(),
            1
        );
        let redone = &history
            .redo()
            .unwrap()
            .timeline
            .as_ref()
            .unwrap()
            .image_segments;
        assert_eq!(redone.len(), 2);
        assert!(redone.iter().all(|segment| segment.path == record.path));
    }

    #[test]
    fn new_import_can_register_source_but_reuse_requires_its_persisted_evidence() {
        let root = tempfile::tempdir().unwrap();
        let existing = online_png(root.path(), "existing");
        ensure_record(root.path(), &existing).unwrap();
        let pending = online_png(root.path(), "pending");
        let catalog = std::fs::read(root.path().join(CATALOG_FILE)).unwrap();
        let media = root.path().join(&pending.path);
        let original = std::fs::read(&media).unwrap();
        assert!(ensure_reusable_record(root.path(), &pending).is_err());
        assert!(prepare_image_reuse(root.path(), &pending).is_err());
        assert!(!record_path(&media).exists());
        assert_eq!(std::fs::read(&media).unwrap(), original);
        assert_eq!(
            std::fs::read(root.path().join(CATALOG_FILE)).unwrap(),
            catalog
        );
        ensure_record(root.path(), &pending).unwrap();
        assert!(same_origin(
            &ensure_reusable_record(root.path(), &pending).unwrap(),
            &pending
        ));
    }

    #[test]
    fn image_reuse_revalidates_existing_provenance_without_writing_or_removing_media() {
        let root = tempfile::tempdir().unwrap();
        let mut record = online_record();
        record.path = "photo.png".into();
        let media = root.path().join(&record.path);
        image::RgbImage::from_pixel(2, 2, image::Rgb([1, 2, 3]))
            .save(&media)
            .unwrap();
        let original = std::fs::read(&media).unwrap();
        assert!(prepare_image_reuse(root.path(), &record).is_err());
        assert!(!record_path(&media).exists());
        std::fs::write(record_path(&media), b"broken").unwrap();
        assert!(prepare_image_reuse(root.path(), &record).is_err());
        assert_eq!(std::fs::read(record_path(&media)).unwrap(), b"broken");
        assert_eq!(std::fs::read(&media).unwrap(), original);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 2);
    }

    #[test]
    fn foreign_asset_records_revalidate_urls_kind_and_rendition_on_read() {
        let root = tempfile::tempdir().unwrap();
        let media = root.path().join("photo.jpg");
        std::fs::write(&media, b"fixture").unwrap();
        let original = online_record();
        write_record_at(&media, &original).unwrap();
        assert!(read_record(&media, true).unwrap().is_some());
        for invalid in 0..5 {
            let mut record = original.clone();
            match invalid {
                0 => {
                    record.source.as_mut().unwrap().attribution.author_url =
                        "file:///tmp/local".into()
                }
                1 => {
                    record.source.as_mut().unwrap().attribution.source_url =
                        "capwise://unexpected".into()
                }
                2 => {
                    record.source.as_mut().unwrap().attribution.license_url =
                        "https://untrusted.invalid/".into()
                }
                3 => record.kind = AssetKind::Audio,
                _ => record.rendition = Some(4),
            }
            std::fs::write(record_path(&media), serde_json::to_vec(&record).unwrap()).unwrap();
            assert!(
                read_record(&media, true).is_err(),
                "accepted invalid source case {invalid}"
            );
        }
    }

    #[test]
    fn recommendation_uses_real_renditions_without_upscaling_claims() {
        let renditions: Vec<_> = [640, 1280, 1920]
            .into_iter()
            .map(|width| Rendition {
                label: width.to_string(),
                url: String::new(),
                width: Some(width),
                height: Some(width / 2),
                approximate_bytes: None,
            })
            .collect();
        assert_eq!(recommended_rendition(&renditions, [1080, 720]), 1);
        assert_eq!(recommended_rendition(&renditions, [3840, 2160]), 2);
    }

    #[test]
    fn staged_media_and_sidecar_are_cleaned_when_cancelled() {
        let record = AssetRecord {
            path: String::new(),
            name: "Photo".into(),
            kind: AssetKind::Photo,
            source: None,
            rendition: None,
        };
        let staged = StagedMedia::write(b"fixture", "jpg", &record).unwrap();
        let directory = staged.directory.clone();
        assert!(record_path(&staged.path).is_file());
        drop(staged);
        assert!(!directory.exists());
    }

    #[test]
    fn project_assets_reject_parent_and_absolute_paths() {
        assert!(project_asset_path(Path::new("/tmp"), "../outside.jpg").is_none());
        assert!(project_asset_path(Path::new("/tmp"), "/tmp/photo.jpg").is_none());
    }

    #[test]
    fn missing_corrupt_or_oversized_required_provenance_never_falls_back_to_local() {
        let record = AssetRecord {
            path: String::new(),
            name: "Photo".into(),
            kind: AssetKind::Photo,
            source: None,
            rendition: None,
        };
        let staged = StagedMedia::write(b"fixture", "jpg", &record).unwrap();
        assert!(read_record(&staged.path, true).unwrap().is_some());
        std::fs::write(record_path(&staged.path), b"broken").unwrap();
        assert!(read_record(&staged.path, false).is_err());
        std::fs::write(record_path(&staged.path), vec![b' '; 128 * 1024 + 1]).unwrap();
        assert!(read_record(&staged.path, true).is_err());
        std::fs::remove_file(record_path(&staged.path)).unwrap();
        assert!(read_record(&staged.path, true).is_err());
        assert!(read_record(&staged.path, false).unwrap().is_none());
    }
}
