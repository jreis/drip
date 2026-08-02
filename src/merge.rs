//! Shared merge of remote bookmarks into the local library.

use crate::model::{Bookmark, Library, Status};
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};

/// Normalized remote bookmark (CSV or API).
#[derive(Debug, Clone)]
pub struct RemoteBookmark {
    pub id: String,
    pub title: String,
    pub note: String,
    pub excerpt: String,
    pub url: String,
    pub folder: String,
    pub tags: Vec<String>,
    pub created: Option<DateTime<Utc>>,
    pub favorite: bool,
    /// When Raindrop says the link is broken.
    pub broken: bool,
}

#[derive(Debug, Default, Clone)]
pub struct MergeStats {
    pub total: usize,
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub removed: usize,
    pub preserved_local: usize,
}

/// Merge remote items into a new library, preserving local status / health / opens.
///
/// - If `prune` is true, local bookmarks whose ids are not in `remote` are dropped.
/// - If `prune` is false, remote-only updates are applied; orphans stay (incremental sync).
pub fn merge_into_library(
    existing: Option<&Library>,
    remote: Vec<RemoteBookmark>,
    source: String,
    prune: bool,
) -> (Library, MergeStats) {
    let mut prior: HashMap<String, Bookmark> = HashMap::new();
    if let Some(lib) = existing {
        for b in &lib.bookmarks {
            prior.insert(b.id.clone(), b.clone());
        }
    }

    let mut stats = MergeStats::default();
    let remote_ids: HashSet<String> = remote.iter().map(|r| r.id.clone()).collect();
    let mut bookmarks = Vec::with_capacity(remote.len() + if prune { 0 } else { prior.len() });

    for r in remote {
        stats.total += 1;
        let (local_fields, is_new) = match prior.remove(&r.id) {
            Some(old) => {
                let meta_changed = old.title != r.title
                    || old.url != r.url
                    || old.folder != r.folder
                    || old.note != r.note
                    || old.excerpt != r.excerpt
                    || old.tags != r.tags
                    || old.favorite != r.favorite;
                if meta_changed {
                    stats.updated += 1;
                } else {
                    stats.unchanged += 1;
                }
                stats.preserved_local += 1;
                (
                    LocalFields {
                        status: old.status,
                        open_count: old.open_count,
                        last_opened: old.last_opened,
                        link_health: old.link_health,
                        link_status_code: old.link_status_code,
                        link_checked_at: old.link_checked_at,
                        link_error: old.link_error,
                    },
                    false,
                )
            }
            None => {
                stats.added += 1;
                (LocalFields::default(), true)
            }
        };

        let status = seed_status(
            local_fields.status,
            local_fields.open_count,
            &r.folder,
            is_new,
        );

        let mut b = Bookmark {
            id: r.id,
            title: r.title,
            note: r.note,
            excerpt: r.excerpt,
            url: r.url,
            folder: if r.folder.is_empty() {
                "Unsorted".into()
            } else {
                r.folder
            },
            tags: r.tags,
            created: r.created,
            favorite: r.favorite,
            status,
            open_count: local_fields.open_count,
            last_opened: local_fields.last_opened,
            link_health: local_fields.link_health,
            link_status_code: local_fields.link_status_code,
            link_checked_at: local_fields.link_checked_at,
            link_error: local_fields.link_error,
        };

        // Raindrop-side broken flag can seed dead-link state once.
        if r.broken && b.link_health == crate::model::LinkHealth::Unknown {
            b.link_health = crate::model::LinkHealth::Dead;
            b.link_error = Some("marked broken in Raindrop".into());
        }

        bookmarks.push(b);
    }

    if prune {
        stats.removed = prior.len();
    } else {
        // Keep orphans (present locally, not in this remote batch).
        for (_, old) in prior {
            bookmarks.push(old);
        }
    }

    bookmarks.sort_by_key(|b| std::cmp::Reverse(b.created));

    // Silence unused if prune path didn't use remote_ids for anything else.
    let _ = remote_ids;

    let mut lib = existing.cloned().unwrap_or_default();
    lib.bookmarks = bookmarks;
    lib.imported_at = Some(Utc::now());
    lib.source_path = Some(source);
    lib.last_synced_at = Some(Utc::now());

    (lib, stats)
}

#[derive(Default)]
struct LocalFields {
    status: Status,
    open_count: u32,
    last_opened: Option<DateTime<Utc>>,
    link_health: crate::model::LinkHealth,
    link_status_code: Option<u16>,
    link_checked_at: Option<DateTime<Utc>>,
    link_error: Option<String>,
}

fn seed_status(status: Status, open_count: u32, folder: &str, is_new: bool) -> Status {
    if !is_new || open_count > 0 || status != Status::Unread {
        return status;
    }
    match folder.to_ascii_lowercase().as_str() {
        "archive" | "archived" | "done" | "read" => Status::Done,
        _ => status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Bookmark, LinkHealth, Status};

    fn remote(id: &str, title: &str, folder: &str) -> RemoteBookmark {
        RemoteBookmark {
            id: id.into(),
            title: title.into(),
            note: String::new(),
            excerpt: String::new(),
            url: format!("https://example.com/{id}"),
            folder: folder.into(),
            tags: vec![],
            created: None,
            favorite: false,
            broken: false,
        }
    }

    fn local_bm(id: &str, status: Status, opens: u32) -> Bookmark {
        Bookmark {
            id: id.into(),
            title: "old title".into(),
            note: String::new(),
            excerpt: String::new(),
            url: format!("https://example.com/{id}"),
            folder: "unread".into(),
            tags: vec![],
            created: None,
            favorite: false,
            status,
            open_count: opens,
            last_opened: None,
            link_health: LinkHealth::Alive,
            link_status_code: Some(200),
            link_checked_at: None,
            link_error: None,
        }
    }

    #[test]
    fn merge_preserves_local_status_and_health() {
        let existing = Library {
            bookmarks: vec![local_bm("1", Status::Reading, 3)],
            ..Default::default()
        };
        let remote = vec![remote("1", "new title", "unread")];
        let (lib, stats) = merge_into_library(Some(&existing), remote, "test".into(), true);
        assert_eq!(stats.updated, 1);
        assert_eq!(lib.bookmarks.len(), 1);
        let b = &lib.bookmarks[0];
        assert_eq!(b.title, "new title");
        assert_eq!(b.status, Status::Reading);
        assert_eq!(b.open_count, 3);
        assert_eq!(b.link_health, LinkHealth::Alive);
        assert_eq!(b.link_status_code, Some(200));
    }

    #[test]
    fn merge_adds_new_and_seeds_archive_as_done() {
        let remote = vec![remote("1", "a", "unread"), remote("2", "b", "archive")];
        let (lib, stats) = merge_into_library(None, remote, "test".into(), true);
        assert_eq!(stats.added, 2);
        assert_eq!(lib.bookmarks.len(), 2);
        let archive = lib.bookmarks.iter().find(|b| b.id == "2").unwrap();
        assert_eq!(archive.status, Status::Done);
        let unread = lib.bookmarks.iter().find(|b| b.id == "1").unwrap();
        assert_eq!(unread.status, Status::Unread);
    }

    #[test]
    fn merge_without_prune_keeps_orphans() {
        let existing = Library {
            bookmarks: vec![local_bm("old", Status::Done, 1)],
            ..Default::default()
        };
        let remote = vec![remote("new", "n", "unread")];
        let (lib, stats) = merge_into_library(Some(&existing), remote, "test".into(), false);
        assert_eq!(stats.added, 1);
        assert_eq!(stats.removed, 0);
        assert_eq!(lib.bookmarks.len(), 2);
        assert!(lib.bookmarks.iter().any(|b| b.id == "old"));
        assert!(lib.bookmarks.iter().any(|b| b.id == "new"));
    }

    #[test]
    fn merge_with_prune_drops_orphans() {
        let existing = Library {
            bookmarks: vec![local_bm("old", Status::Done, 1)],
            ..Default::default()
        };
        let remote = vec![remote("new", "n", "unread")];
        let (lib, stats) = merge_into_library(Some(&existing), remote, "test".into(), true);
        assert_eq!(stats.removed, 1);
        assert_eq!(lib.bookmarks.len(), 1);
        assert_eq!(lib.bookmarks[0].id, "new");
    }
}
