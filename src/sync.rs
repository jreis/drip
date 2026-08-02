//! High-level Raindrop pull sync.

use crate::config;
use crate::merge::{self, MergeStats};
use crate::model::Library;
use crate::raindrop::Client;
use crate::store;
use anyhow::Result;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Default)]
pub struct SyncOptions {
    /// Fetch every page (ignore last_synced_at watermark).
    pub full: bool,
    /// Drop local bookmarks missing from Raindrop (only meaningful with full).
    pub prune: bool,
    /// Don't write library to disk.
    pub dry_run: bool,
}

#[derive(Debug)]
pub struct SyncReport {
    pub merge: MergeStats,
    pub fetched: usize,
    pub mode: &'static str,
    pub since: Option<DateTime<Utc>>,
    pub library: Library,
}

/// Pull from Raindrop API and merge into local library.
pub fn pull(opts: SyncOptions) -> Result<SyncReport> {
    let token = config::require_access_token()?;
    let mut client = Client::new(token);

    let existing = store::load_library()?;
    // Prefer explicit API watermark; fall back to CSV import time so a prior
    // full CSV import does not force another full API crawl.
    let since = if opts.full {
        None
    } else {
        existing
            .as_ref()
            .and_then(|l| l.last_synced_at.or(l.imported_at))
    };

    // Empty library / no import history → full pull.
    let full = opts.full || since.is_none();
    let prune = opts.prune && full;

    let mode = if full {
        if prune { "full+prune" } else { "full" }
    } else if existing
        .as_ref()
        .is_some_and(|l| l.last_synced_at.is_none() && l.imported_at.is_some())
    {
        "incremental (from csv import time)"
    } else {
        "incremental"
    };

    eprint!("syncing from Raindrop ({mode})");
    if let Some(s) = since {
        eprint!(" since {}", s.format("%Y-%m-%d %H:%M UTC"));
    }
    eprintln!("…");
    if !full {
        eprintln!("  (only new/changed since watermark — not re-downloading everything)");
    }

    let remote = client.fetch_raindrops(since, full, |page, total| {
        eprint!("\r  page {page} · {total} items pulled");
        let _ = std::io::Write::flush(&mut std::io::stderr());
    })?;
    eprintln!();

    let fetched = remote.len();
    let (library, merge) =
        merge::merge_into_library(existing.as_ref(), remote, "raindrop-api".into(), prune);

    if !opts.dry_run {
        store::save_library(&library)?;
    }

    Ok(SyncReport {
        merge,
        fetched,
        mode,
        since,
        library,
    })
}

pub fn print_report(r: &SyncReport) {
    println!(
        "sync complete ({}) — fetched {} remote items",
        r.mode, r.fetched
    );
    if let Some(s) = r.since {
        println!("  watermark was {}", s.format("%Y-%m-%d %H:%M UTC"));
    }
    println!(
        "  added={} updated={} unchanged={} removed={} preserved_local={}",
        r.merge.added, r.merge.updated, r.merge.unchanged, r.merge.removed, r.merge.preserved_local
    );
    let s = r.library.stats();
    println!(
        "  library total={} unread={} dead={}",
        s.total, s.unread, s.link_dead
    );
    if let Ok(p) = store::library_path() {
        println!("  saved → {}", p.display());
    }
}
