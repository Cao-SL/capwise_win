use super::*;
use crate::cover_generation::GeneratedOrigin;
use sha2::{Digest as _, Sha256};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct GeneratedRecord {
    #[serde(flatten)]
    pub(super) record: AssetRecord,
    generated: GeneratedOrigin,
    sha256: String,
}

impl GeneratedRecord {
    pub(super) fn validate(&self) -> Result<(), String> {
        if self.record.kind != AssetKind::Photo
            || self.record.source.is_some()
            || self.record.rendition.is_some()
            || !self.generated.is_valid()
            || self.record.path.is_empty()
            || Path::new(&self.record.path)
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
            || self.sha256.len() != 64
            || !self.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(metadata_error());
        }
        Ok(())
    }

    fn matches(&self, other: &Self) -> bool {
        same_origin(&self.record, &other.record)
            && self.generated == other.generated
            && self.sha256 == other.sha256
    }

    fn verify_media(&self, media: &Path) -> Result<(), String> {
        let (hash, dimensions) = media_metadata(media)?;
        if hash != self.sha256 || dimensions != self.generated.actual {
            return Err(metadata_error());
        }
        Ok(())
    }
}

fn media_metadata(path: &Path) -> Result<(String, crate::cover_generation::Dimensions), String> {
    let file = std::fs::File::open(path).map_err(|_| metadata_error())?;
    let limit = 32 * 1024 * 1024;
    if file.metadata().map_err(|_| metadata_error())?.len() > limit {
        return Err(metadata_error());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| metadata_error())?;
    if bytes.len() as u64 > limit {
        return Err(metadata_error());
    }
    let (_, dimensions) =
        crate::cover_generation::inspect_image(&bytes).map_err(|_| metadata_error())?;
    Ok((format!("{:x}", Sha256::digest(&bytes)), dimensions))
}

fn sidecar(media: &Path) -> Result<Option<GeneratedRecord>, String> {
    let path = record_path(media);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(metadata_error()),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 128 * 1024 {
        return Err(metadata_error());
    }
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).map_err(|_| metadata_error())?)
            .map_err(|_| metadata_error())?;
    if value.get("generated").is_none() {
        return Ok(None);
    }
    let record: GeneratedRecord = serde_json::from_value(value).map_err(|_| metadata_error())?;
    record.validate()?;
    Ok(Some(record))
}

fn read(project: &Path, relative: &str) -> Result<Option<GeneratedRecord>, String> {
    let media = project_asset_path(project, relative).ok_or_else(metadata_error)?;
    let catalog = load_catalog(project)?.and_then(|mut catalog| catalog.generated.remove(relative));
    let sidecar = sidecar(&media)?;
    if let (Some(catalog), Some(sidecar)) = (&catalog, &sidecar)
        && !catalog.matches(sidecar)
    {
        return Err(metadata_error());
    }
    let record = catalog.or(sidecar);
    if let Some(record) = &record {
        if record.record.path != relative {
            return Err(metadata_error());
        }
        record.verify_media(&media)?;
    }
    Ok(record)
}

pub(crate) fn read_generated_origin(
    project: &Path,
    relative: &str,
) -> Result<Option<GeneratedOrigin>, String> {
    Ok(read(project, relative)?.map(|record| record.generated))
}

fn write_sidecar(media: &Path, record: &GeneratedRecord) -> Result<std::fs::Metadata, String> {
    let mut file = std::fs::File::create_new(record_path(media)).map_err(|_| metadata_error())?;
    let identity = file.metadata().map_err(|_| metadata_error())?;
    let result = (|| {
        file.write_all(&serde_json::to_vec_pretty(record).map_err(|_| metadata_error())?)
            .map_err(|_| metadata_error())?;
        file.sync_all().map_err(|_| metadata_error())
    })();
    drop(file);
    if result.is_err() {
        let _ = std::fs::remove_file(record_path(media));
    }
    result?;
    Ok(identity)
}

pub(super) fn ensure(project: &Path, relative: &str) -> Result<(), String> {
    let Some(record) = read(project, relative)? else {
        return Ok(());
    };
    let media = project_asset_path(project, relative).ok_or_else(metadata_error)?;
    let _lock = CATALOG_LOCK.lock().map_err(|_| metadata_error())?;
    let mut catalog = load_catalog(project)?.unwrap_or(AssetCatalog {
        version: 1,
        records: BTreeMap::new(),
        generated: BTreeMap::new(),
    });
    if catalog.records.contains_key(relative) {
        return Err(metadata_error());
    }
    if let Some(existing) = catalog.generated.get(relative)
        && !existing.matches(&record)
    {
        return Err(metadata_error());
    }
    if sidecar(&media)?.is_none() {
        write_sidecar(&media, &record)?;
    }
    if !catalog.generated.contains_key(relative) {
        let _ = catalog.generated.insert(relative.into(), record);
        save_catalog(project, &catalog)?;
    }
    Ok(())
}

pub(crate) struct PendingGeneratedSource {
    project: PathBuf,
    directory: std::fs::Metadata,
    record: GeneratedRecord,
    sidecar: std::fs::Metadata,
    active: bool,
}

impl PendingGeneratedSource {
    pub(crate) fn register(
        project: &Path,
        record: AssetRecord,
        generated: GeneratedOrigin,
    ) -> Result<Self, String> {
        let project = project.canonicalize().map_err(|_| metadata_error())?;
        let directory = std::fs::symlink_metadata(&project).map_err(|_| metadata_error())?;
        let media = project_asset_path(&project, &record.path).ok_or_else(metadata_error)?;
        let (sha256, dimensions) = media_metadata(&media)?;
        if dimensions != generated.actual {
            return Err(metadata_error());
        }
        let entry = GeneratedRecord {
            record,
            generated,
            sha256,
        };
        entry.validate()?;
        let _lock = CATALOG_LOCK.lock().map_err(|_| metadata_error())?;
        let mut catalog = load_catalog(&project)?.unwrap_or(AssetCatalog {
            version: 1,
            records: BTreeMap::new(),
            generated: BTreeMap::new(),
        });
        if catalog.records.contains_key(&entry.record.path)
            || catalog.generated.contains_key(&entry.record.path)
        {
            return Err(metadata_error());
        }
        let sidecar = write_sidecar(&media, &entry)?;
        let _ = catalog
            .generated
            .insert(entry.record.path.clone(), entry.clone());
        if let Err(error) = save_catalog(&project, &catalog) {
            let _ = std::fs::remove_file(record_path(&media));
            return Err(error);
        }
        Ok(Self {
            project,
            directory,
            record: entry,
            sidecar,
            active: true,
        })
    }

    pub(crate) fn commit(mut self) {
        self.active = false;
    }

    pub(crate) fn rollback(&mut self) -> Result<(), String> {
        if !self.active {
            return Ok(());
        }
        let current = std::fs::symlink_metadata(&self.project).map_err(|_| metadata_error())?;
        if !current.is_dir() || !same_file_identity(&self.directory, &current) {
            return Err(metadata_error());
        }
        let _lock = CATALOG_LOCK.lock().map_err(|_| metadata_error())?;
        let mut catalog = load_catalog(&self.project)?.ok_or_else(metadata_error)?;
        if !catalog
            .generated
            .get(&self.record.record.path)
            .is_some_and(|record| record.matches(&self.record))
        {
            return Err(metadata_error());
        }
        let _ = catalog.generated.remove(&self.record.record.path);
        save_catalog(&self.project, &catalog)?;
        let path = record_path(&self.project.join(&self.record.record.path));
        if std::fs::symlink_metadata(&path)
            .is_ok_and(|metadata| same_file_identity(&self.sidecar, &metadata))
        {
            std::fs::remove_file(path).map_err(|_| metadata_error())?;
        }
        self.active = false;
        Ok(())
    }
}

impl Drop for PendingGeneratedSource {
    fn drop(&mut self) {
        if self.rollback().is_err() {
            tracing::warn!("Could not roll back generated image metadata");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cover_generation::{DimensionStatus, Dimensions};

    fn origin() -> GeneratedOrigin {
        let size = Dimensions {
            width: 4,
            height: 3,
        };
        GeneratedOrigin {
            version: 1,
            provider: "volcengineArk".into(),
            requested_model: "synthetic-model".into(),
            reported_model: None,
            target: size,
            requested: size,
            actual: size,
            dimension_status: DimensionStatus::RequestedSize,
            created: None,
        }
    }

    fn fixture(root: &Path, name: &str) -> AssetRecord {
        let directory = root.join("content/images");
        fs::create_dir_all(&directory).unwrap();
        image::RgbImage::from_pixel(4, 3, image::Rgb([12, 40, 90]))
            .save(directory.join(name))
            .unwrap();
        AssetRecord {
            path: format!("content/images/{name}"),
            name: name.into(),
            kind: AssetKind::Photo,
            source: None,
            rendition: None,
        }
    }

    use std::fs;

    #[test]
    fn generated_reopen_recovers_missing_sidecar_from_catalog_on_explicit_reuse() {
        let root = tempfile::tempdir().unwrap();
        let record = fixture(root.path(), "first.png");
        PendingGeneratedSource::register(root.path(), record.clone(), origin())
            .unwrap()
            .commit();
        fs::remove_file(record_path(&root.path().join(&record.path))).unwrap();
        assert_eq!(
            read_generated_origin(root.path(), &record.path).unwrap(),
            Some(origin())
        );
        assert!(!record_path(&root.path().join(&record.path)).exists());
        ensure_reusable_record(root.path(), &record).unwrap();
        assert!(record_path(&root.path().join(&record.path)).exists());
        assert_eq!(
            read_generated_origin(root.path(), &record.path).unwrap(),
            Some(origin())
        );
    }

    #[test]
    fn generated_sidecar_restores_missing_catalog_but_corruption_never_becomes_local() {
        let root = tempfile::tempdir().unwrap();
        let record = fixture(root.path(), "first.png");
        PendingGeneratedSource::register(root.path(), record.clone(), origin())
            .unwrap()
            .commit();
        fs::remove_file(root.path().join(CATALOG_FILE)).unwrap();
        ensure_reusable_record(root.path(), &record).unwrap();
        assert_eq!(
            load_catalog(root.path()).unwrap().unwrap().generated.len(),
            1
        );
        fs::write(root.path().join(CATALOG_FILE), b"broken").unwrap();
        assert!(read_project_record(root.path(), &record.path).is_err());
        assert!(prepare_image_reuse(root.path(), &record).is_err());
        assert!(root.path().join(&record.path).is_file());
        assert_eq!(fs::read(root.path().join(CATALOG_FILE)).unwrap(), b"broken");
    }

    #[test]
    fn generated_registration_failure_and_rollback_preserve_unrelated_assets() {
        let root = tempfile::tempdir().unwrap();
        let first = fixture(root.path(), "first.png");
        fs::create_dir(root.path().join(CATALOG_FILE)).unwrap();
        assert!(PendingGeneratedSource::register(root.path(), first.clone(), origin()).is_err());
        assert!(!record_path(&root.path().join(&first.path)).exists());
        fs::remove_dir(root.path().join(CATALOG_FILE)).unwrap();
        let pending =
            PendingGeneratedSource::register(root.path(), first.clone(), origin()).unwrap();
        let second = fixture(root.path(), "second.png");
        PendingGeneratedSource::register(root.path(), second.clone(), origin())
            .unwrap()
            .commit();
        drop(pending);
        let catalog = load_catalog(root.path()).unwrap().unwrap();
        assert!(!catalog.generated.contains_key(&first.path));
        assert!(catalog.generated.contains_key(&second.path));
        assert!(!record_path(&root.path().join(&first.path)).exists());
        assert!(record_path(&root.path().join(&second.path)).exists());
    }

    #[test]
    fn generated_concurrent_publication_does_not_lose_either_record() {
        let root = tempfile::tempdir().unwrap();
        let records = [
            fixture(root.path(), "one.png"),
            fixture(root.path(), "two.png"),
        ];
        let path = root.path();
        std::thread::scope(|scope| {
            let handles: Vec<_> = records
                .into_iter()
                .map(|record| {
                    scope.spawn(move || {
                        PendingGeneratedSource::register(path, record, origin())
                            .unwrap()
                            .commit()
                    })
                })
                .collect();
            for handle in handles {
                handle.join().unwrap();
            }
        });
        assert_eq!(
            load_catalog(root.path()).unwrap().unwrap().generated.len(),
            2
        );
    }

    #[test]
    fn generated_changed_media_and_conflicting_provenance_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let record = fixture(root.path(), "first.png");
        PendingGeneratedSource::register(root.path(), record.clone(), origin())
            .unwrap()
            .commit();
        let sidecar_path = record_path(&root.path().join(&record.path));
        let mut json: serde_json::Value =
            serde_json::from_slice(&fs::read(&sidecar_path).unwrap()).unwrap();
        json["generated"]["requestedModel"] = serde_json::json!("different-model");
        fs::write(&sidecar_path, serde_json::to_vec(&json).unwrap()).unwrap();
        assert!(read_generated_origin(root.path(), &record.path).is_err());
        fs::remove_file(sidecar_path).unwrap();
        image::RgbImage::from_pixel(4, 3, image::Rgb([255, 10, 90]))
            .save(root.path().join(&record.path))
            .unwrap();
        assert!(read_generated_origin(root.path(), &record.path).is_err());
    }
}
