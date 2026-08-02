use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Local reading status — independent of Raindrop folders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Unread,
    Reading,
    Done,
    Skipped,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Unread => "unread",
            Status::Reading => "reading",
            Status::Done => "done",
            Status::Skipped => "skipped",
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            Status::Unread => Status::Reading,
            Status::Reading => Status::Done,
            Status::Done => Status::Skipped,
            Status::Skipped => Status::Unread,
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Status::Unread => "·",
            Status::Reading => "▸",
            Status::Done => "✓",
            Status::Skipped => "–",
        }
    }
}

/// Result of a dead-link / URL health check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LinkHealth {
    #[default]
    Unknown,
    /// 2xx (and soft walls like 401/403/429)
    Alive,
    /// 3xx that didn't fully resolve
    Redirect,
    /// 404/410/host gone / connection refused
    Dead,
    /// Timeout, 5xx, TLS issues, etc.
    Error,
}

impl LinkHealth {
    pub fn as_str(self) -> &'static str {
        match self {
            LinkHealth::Unknown => "unknown",
            LinkHealth::Alive => "alive",
            LinkHealth::Redirect => "redirect",
            LinkHealth::Dead => "dead",
            LinkHealth::Error => "error",
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            LinkHealth::Unknown => "?",
            LinkHealth::Alive => "●",
            LinkHealth::Redirect => "↗",
            LinkHealth::Dead => "✗",
            LinkHealth::Error => "!",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bookmark {
    pub id: String,
    pub title: String,
    pub note: String,
    pub excerpt: String,
    pub url: String,
    pub folder: String,
    pub tags: Vec<String>,
    pub created: Option<DateTime<Utc>>,
    pub favorite: bool,
    /// Local-only fields (not from Raindrop CSV)
    #[serde(default)]
    pub status: Status,
    #[serde(default)]
    pub open_count: u32,
    #[serde(default)]
    pub last_opened: Option<DateTime<Utc>>,
    #[serde(default)]
    pub link_health: LinkHealth,
    #[serde(default)]
    pub link_status_code: Option<u16>,
    #[serde(default)]
    pub link_checked_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub link_error: Option<String>,
}

impl Bookmark {
    pub fn domain(&self) -> String {
        url::host_from_url(&self.url).unwrap_or_else(|| "—".into())
    }

    pub fn search_blob(&self) -> String {
        let tags = self.tags.join(" ");
        format!(
            "{} {} {} {} {} {}",
            self.title, self.url, self.excerpt, self.note, self.folder, tags
        )
        .to_lowercase()
    }

    pub fn mark_opened(&mut self) {
        self.open_count = self.open_count.saturating_add(1);
        self.last_opened = Some(Utc::now());
        if self.status == Status::Unread {
            self.status = Status::Reading;
        }
    }

    pub fn apply_check(
        &mut self,
        health: LinkHealth,
        status_code: Option<u16>,
        error: Option<String>,
        checked_at: DateTime<Utc>,
    ) {
        self.link_health = health;
        self.link_status_code = status_code;
        self.link_error = error;
        self.link_checked_at = Some(checked_at);
    }

    pub fn link_summary(&self) -> String {
        match self.link_health {
            LinkHealth::Unknown => "not checked".into(),
            h => {
                let code = self
                    .link_status_code
                    .map(|c| format!(" HTTP {c}"))
                    .unwrap_or_default();
                let when = self
                    .link_checked_at
                    .map(|t| t.format(" · %Y-%m-%d").to_string())
                    .unwrap_or_default();
                let err = self
                    .link_error
                    .as_ref()
                    .map(|e| format!(" ({e})"))
                    .unwrap_or_default();
                format!("{}{code}{when}{err}", h.as_str())
            }
        }
    }
}

/// Tiny URL host helper so we don't pull in the full `url` crate yet.
mod url {
    pub fn host_from_url(raw: &str) -> Option<String> {
        let without_scheme = raw
            .strip_prefix("https://")
            .or_else(|| raw.strip_prefix("http://"))
            .unwrap_or(raw);
        let host = without_scheme.split('/').next()?.split('?').next()?;
        let host = host.strip_prefix("www.").unwrap_or(host);
        if host.is_empty() {
            None
        } else {
            Some(host.to_string())
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Library {
    pub bookmarks: Vec<Bookmark>,
    pub imported_at: Option<DateTime<Utc>>,
    pub source_path: Option<String>,
    /// Watermark for incremental Raindrop API sync (`drip sync`).
    #[serde(default)]
    pub last_synced_at: Option<DateTime<Utc>>,
}

impl Library {
    pub fn folders(&self) -> Vec<String> {
        let mut set: HashSet<&str> = HashSet::new();
        for b in &self.bookmarks {
            set.insert(b.folder.as_str());
        }
        let mut v: Vec<String> = set.into_iter().map(str::to_string).collect();
        v.sort();
        v
    }

    pub fn stats(&self) -> LibraryStats {
        let mut s = LibraryStats {
            total: self.bookmarks.len(),
            ..Default::default()
        };
        for b in &self.bookmarks {
            match b.status {
                Status::Unread => s.unread += 1,
                Status::Reading => s.reading += 1,
                Status::Done => s.done += 1,
                Status::Skipped => s.skipped += 1,
            }
            if b.open_count > 0 {
                s.opened_once += 1;
            }
            match b.link_health {
                LinkHealth::Unknown => s.link_unknown += 1,
                LinkHealth::Alive => s.link_alive += 1,
                LinkHealth::Redirect => s.link_redirect += 1,
                LinkHealth::Dead => s.link_dead += 1,
                LinkHealth::Error => s.link_error += 1,
            }
        }
        s
    }

    pub fn apply_check_result(&mut self, result: &crate::health::CheckResult) -> bool {
        if let Some(b) = self.bookmarks.iter_mut().find(|b| b.id == result.id) {
            b.apply_check(
                result.health,
                result.status_code,
                result.error.clone(),
                result.checked_at,
            );
            true
        } else {
            false
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct LibraryStats {
    pub total: usize,
    pub unread: usize,
    pub reading: usize,
    pub done: usize,
    pub skipped: usize,
    pub opened_once: usize,
    pub link_unknown: usize,
    pub link_alive: usize,
    pub link_redirect: usize,
    pub link_dead: usize,
    pub link_error: usize,
}
