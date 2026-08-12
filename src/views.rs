//! Saved filter combinations ("views"), persisted separately from the
//! bookmark library so `drip import` / `drip sync` never touch them.

use crate::app::{LinkFilter, StatusFilter};
use crate::store;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedView {
    pub name: String,
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub year: Option<i32>,
    #[serde(default)]
    pub status: StatusFilter,
    #[serde(default)]
    pub link: LinkFilter,
    #[serde(default)]
    pub favorite: bool,
}

impl SavedView {
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if !self.query.is_empty() {
            parts.push(format!("\"{}\"", self.query));
        }
        if let Some(f) = &self.folder {
            parts.push(format!("folder:{f}"));
        }
        if let Some(d) = &self.domain {
            parts.push(format!("domain:{d}"));
        }
        if let Some(t) = &self.tag {
            parts.push(format!("tag:{t}"));
        }
        if let Some(y) = self.year {
            parts.push(format!("year:{y}"));
        }
        if self.status != StatusFilter::All {
            parts.push(format!("status:{}", self.status.label()));
        }
        if self.link != LinkFilter::All {
            parts.push(format!("links:{}", self.link.label()));
        }
        if self.favorite {
            parts.push("favorites".into());
        }
        if parts.is_empty() {
            "all bookmarks".into()
        } else {
            parts.join(" · ")
        }
    }
}

fn views_path() -> Result<PathBuf> {
    Ok(store::data_dir()?.join("views.json"))
}

pub fn load_views() -> Result<Vec<SavedView>> {
    load_views_from(&views_path()?)
}

pub fn save_views(views: &[SavedView]) -> Result<()> {
    save_views_to(&views_path()?, views)
}

fn load_views_from(path: &std::path::Path) -> Result<Vec<SavedView>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let views: Vec<SavedView> =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    Ok(views)
}

fn save_views_to(path: &std::path::Path, views: &[SavedView]) -> Result<()> {
    let raw = serde_json::to_string_pretty(views).context("serialize views")?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, raw).with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("rename to {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{LinkFilter, StatusFilter};

    #[test]
    fn round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!("drip-views-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("views.json");

        let views = vec![SavedView {
            name: "unread rust".into(),
            query: "rust".into(),
            folder: Some("dev".into()),
            domain: None,
            tag: Some("rust".into()),
            year: Some(2025),
            status: StatusFilter::Unread,
            link: LinkFilter::Alive,
            favorite: true,
        }];

        save_views_to(&path, &views).unwrap();
        let loaded = load_views_from(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "unread rust");
        assert_eq!(loaded[0].tag.as_deref(), Some("rust"));
        assert_eq!(loaded[0].status, StatusFilter::Unread);
        assert!(loaded[0].summary().contains("favorites"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_file_loads_empty() {
        let path = std::env::temp_dir().join("drip-views-test-missing-does-not-exist.json");
        assert!(load_views_from(&path).unwrap().is_empty());
    }
}
