//! The shared asset library: media kept outside any one project so it can be
//! reused in the next video. Files are copied into the app's data folder
//! (`library/`), so deleting a project never breaks the library, and a file
//! already in it is not stored twice (matched by content hash).
//!
//! The catalog is one small JSON file next to the media. Everything here is
//! blocking file I/O; callers run it on a background executor.

use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::asset_library::AssetKind;

const CATALOG: &str = "library.json";
/// Larger files stay where they are; a library is for reusable pieces, not
/// whole recordings.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
static LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct LibraryItem {
    pub id: String,
    pub name: String,
    pub kind: AssetKind,
    /// File name inside the library folder.
    pub file: String,
    pub hash: String,
    /// Milliseconds since the epoch; newest first in the panel.
    pub added: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct Catalog {
    version: u32,
    items: Vec<LibraryItem>,
}

pub(crate) fn dir() -> PathBuf {
    crate::store::app_data_dir().join("library")
}

pub(crate) fn path_of(item: &LibraryItem) -> PathBuf {
    dir().join(&item.file)
}

fn read_catalog(root: &Path) -> Catalog {
    std::fs::read(root.join(CATALOG))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_catalog(root: &Path, catalog: &Catalog) -> Result<(), String> {
    std::fs::create_dir_all(root).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec_pretty(catalog).map_err(|error| error.to_string())?;
    let temp = root.join(format!("{CATALOG}.tmp"));
    std::fs::write(&temp, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&temp, root.join(CATALOG)).map_err(|error| error.to_string())
}

/// Every item whose file is still there, newest first.
pub(crate) fn items() -> Vec<LibraryItem> {
    items_in(&dir())
}

fn items_in(root: &Path) -> Vec<LibraryItem> {
    let _guard = LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let mut items: Vec<_> = read_catalog(root)
        .items
        .into_iter()
        .filter(|item| root.join(&item.file).is_file())
        .collect();
    items.sort_by(|a, b| b.added.cmp(&a.added));
    items
}

/// The kind a file would be stored as, from its extension.
pub(crate) fn kind_for(path: &Path) -> Option<AssetKind> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    if crate::import::OVERLAY_IMAGE_EXTENSIONS
        .iter()
        .any(|known| known.eq_ignore_ascii_case(&extension))
    {
        Some(AssetKind::Photo)
    } else if ["mp4", "mov", "m4v"].contains(&extension.as_str()) {
        Some(AssetKind::Video)
    } else if ["mp3", "m4a", "wav", "aac", "flac", "ogg", "aif", "aiff"]
        .contains(&extension.as_str())
    {
        Some(AssetKind::Audio)
    } else {
        None
    }
}

fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Copies `source` into the library under `name`. Returns the stored item;
/// a file that is already there comes back as it is.
pub(crate) fn add(source: &Path, name: &str) -> Result<LibraryItem, String> {
    add_in(&dir(), source, name)
}

fn add_in(root: &Path, source: &Path, name: &str) -> Result<LibraryItem, String> {
    let kind = kind_for(source).ok_or_else(|| {
        crate::i18n::tr::editor::assets::library_unsupported().to_string()
    })?;
    let metadata = std::fs::metadata(source).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return Err(crate::i18n::tr::editor::assets::library_unsupported().to_string());
    }
    let hash = hash_file(source)?;
    let _guard = LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let mut catalog = read_catalog(root);
    if let Some(existing) = catalog
        .items
        .iter()
        .find(|item| item.hash == hash && root.join(&item.file).is_file())
    {
        return Ok(existing.clone());
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    let extension = source
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("bin")
        .to_ascii_lowercase();
    let file = format!("{id}.{extension}");
    std::fs::create_dir_all(root).map_err(|error| error.to_string())?;
    std::fs::copy(source, root.join(&file)).map_err(|error| error.to_string())?;
    let name = name.trim();
    let item = LibraryItem {
        id,
        name: if name.is_empty() {
            source
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_owned()
        } else {
            name.chars().take(120).collect()
        },
        kind,
        file,
        hash,
        added: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis() as u64),
    };
    catalog.version = 1;
    catalog.items.push(item.clone());
    if let Err(error) = write_catalog(root, &catalog) {
        let _ = std::fs::remove_file(root.join(&item.file));
        return Err(error);
    }
    Ok(item)
}

/// Removes an item and its file.
pub(crate) fn remove(id: &str) -> Result<(), String> {
    remove_in(&dir(), id)
}

fn remove_in(root: &Path, id: &str) -> Result<(), String> {
    let _guard = LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let mut catalog = read_catalog(root);
    let Some(position) = catalog.items.iter().position(|item| item.id == id) else {
        return Ok(());
    };
    let item = catalog.items.remove(position);
    write_catalog(root, &catalog)?;
    let _ = std::fs::remove_file(root.join(item.file));
    Ok(())
}

/// Whether a file with these bytes is already in the library.
pub(crate) fn contains(source: &Path) -> bool {
    let Ok(hash) = hash_file(source) else {
        return false;
    };
    items().iter().any(|item| item.hash == hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adding_copies_dedupes_and_removes() {
        let root = tempfile::tempdir().unwrap();
        let library = root.path().join("library");
        let source = root.path().join("cover.png");
        std::fs::write(&source, b"not really a png").unwrap();

        let first = add_in(&library, &source, "Cover").unwrap();
        assert_eq!(first.kind, AssetKind::Photo);
        assert!(library.join(&first.file).is_file());
        // The same bytes under another name are the same item.
        let again = add_in(&library, &source, "Other name").unwrap();
        assert_eq!(again.id, first.id);
        assert_eq!(items_in(&library).len(), 1);

        // The copy survives the original going away.
        std::fs::remove_file(&source).unwrap();
        assert_eq!(items_in(&library)[0].name, "Cover");

        remove_in(&library, &first.id).unwrap();
        assert!(items_in(&library).is_empty());
        assert!(!library.join(&first.file).exists());
    }

    #[test]
    fn unknown_types_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("notes.txt");
        std::fs::write(&source, b"text").unwrap();
        assert!(add_in(&root.path().join("library"), &source, "").is_err());
        assert_eq!(kind_for(Path::new("a.MOV")), Some(AssetKind::Video));
        assert_eq!(kind_for(Path::new("a.m4a")), Some(AssetKind::Audio));
    }
}
