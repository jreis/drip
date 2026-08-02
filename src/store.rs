use crate::model::Library;
use anyhow::{Context, Result};
use directories::ProjectDirs;
use std::fs;
use std::path::{Path, PathBuf};

pub fn data_dir() -> Result<PathBuf> {
    let dirs =
        ProjectDirs::from("io", "drip", "drip").context("could not resolve data directory")?;
    let dir = dirs.data_dir().to_path_buf();
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    Ok(dir)
}

pub fn library_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("library.json"))
}

pub fn load_library() -> Result<Option<Library>> {
    let path = library_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let lib: Library =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    Ok(Some(lib))
}

pub fn save_library(lib: &Library) -> Result<()> {
    let path = library_path()?;
    save_library_to(&path, lib)
}

pub fn save_library_to(path: &Path, lib: &Library) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let raw = serde_json::to_string_pretty(lib).context("serialize library")?;
    // Atomic-ish write.
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, raw).with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("rename to {}", path.display()))?;
    Ok(())
}
