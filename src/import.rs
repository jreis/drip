use crate::merge::{self, RemoteBookmark};
use crate::model::Library;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::fs::File;
use std::path::Path;

/// Raindrop.io CSV export row.
#[derive(Debug, Deserialize)]
struct RaindropRow {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    note: String,
    #[serde(default)]
    excerpt: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    folder: String,
    #[serde(default)]
    tags: String,
    #[serde(default)]
    created: String,
    #[serde(default)]
    favorite: String,
}

fn parse_tags(raw: &str) -> Vec<String> {
    raw.split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect()
}

fn parse_created(raw: &str) -> Option<DateTime<Utc>> {
    if raw.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn parse_favorite(raw: &str) -> bool {
    matches!(raw.to_ascii_lowercase().as_str(), "true" | "1" | "yes")
}

/// Import a Raindrop CSV. Preserves local status / open history / link health for matching ids.
pub fn import_csv(path: &Path, existing: Option<&Library>) -> Result<Library> {
    let file = File::open(path).with_context(|| format!("open CSV {}", path.display()))?;
    let mut rdr = csv::ReaderBuilder::new()
        .flexible(true)
        .trim(csv::Trim::Headers)
        .from_reader(file);

    let mut remote = Vec::new();
    for (i, result) in rdr.deserialize::<RaindropRow>().enumerate() {
        let row = result.with_context(|| format!("parse CSV row {}", i + 2))?;
        if row.url.is_empty() && row.title.is_empty() {
            continue;
        }
        remote.push(RemoteBookmark {
            id: row.id,
            title: row.title,
            note: row.note,
            excerpt: row.excerpt,
            url: row.url,
            folder: row.folder,
            tags: parse_tags(&row.tags),
            created: parse_created(&row.created),
            favorite: parse_favorite(&row.favorite),
            broken: false,
        });
    }

    // CSV is a full snapshot — prune ids missing from the file.
    let (mut lib, _stats) =
        merge::merge_into_library(existing, remote, path.display().to_string(), true);
    // Treat CSV import as a sync watermark so the next `drip sync` can be incremental.
    lib.last_synced_at = Some(Utc::now());
    Ok(lib)
}
