use crate::model::Library;
use anyhow::{Context, Result};
use serde::Serialize;
use std::fs::File;
use std::path::Path;

/// Raindrop-compatible row — same columns `import::import_csv` reads back in.
#[derive(Debug, Serialize)]
struct ExportRow<'a> {
    id: &'a str,
    title: &'a str,
    note: &'a str,
    excerpt: &'a str,
    url: &'a str,
    folder: &'a str,
    tags: String,
    created: String,
    favorite: bool,
}

/// Write the library as a Raindrop-compatible CSV. Local-only fields (status,
/// open history, link health) are not part of this format — use the JSON
/// export for a full-fidelity backup.
pub fn export_csv(lib: &Library, path: &Path) -> Result<()> {
    let file = File::create(path).with_context(|| format!("create {}", path.display()))?;
    let mut wtr = csv::Writer::from_writer(file);
    for b in &lib.bookmarks {
        wtr.serialize(ExportRow {
            id: &b.id,
            title: &b.title,
            note: &b.note,
            excerpt: &b.excerpt,
            url: &b.url,
            folder: &b.folder,
            tags: b.tags.join(", "),
            created: b.created.map(|c| c.to_rfc3339()).unwrap_or_default(),
            favorite: b.favorite,
        })
        .with_context(|| format!("write row for {}", b.id))?;
    }
    wtr.flush()
        .with_context(|| format!("flush {}", path.display()))?;
    Ok(())
}
