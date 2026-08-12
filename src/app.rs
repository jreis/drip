use crate::digest;
use crate::health::{self, CheckJob, CheckProgress, CheckResult};
use crate::model::{Bookmark, Library, LinkHealth, Status};
use crate::views::{self, SavedView};
use anyhow::Result;
use chrono::Datelike;
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::Receiver;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Search,
    Help,
    DomainBrowser,
    TagBrowser,
    DuplicatesBrowser,
    YearBrowser,
    Digest,
    ViewBrowser,
    ViewSave,
    ConfirmDelete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusFilter {
    #[default]
    All,
    Unread,
    Reading,
    Done,
    Skipped,
    NeverOpened,
}

impl StatusFilter {
    pub fn label(self) -> &'static str {
        match self {
            StatusFilter::All => "all",
            StatusFilter::Unread => "unread",
            StatusFilter::Reading => "reading",
            StatusFilter::Done => "done",
            StatusFilter::Skipped => "skipped",
            StatusFilter::NeverOpened => "never opened",
        }
    }

    pub fn next(self) -> Self {
        match self {
            StatusFilter::All => StatusFilter::Unread,
            StatusFilter::Unread => StatusFilter::Reading,
            StatusFilter::Reading => StatusFilter::Done,
            StatusFilter::Done => StatusFilter::Skipped,
            StatusFilter::Skipped => StatusFilter::NeverOpened,
            StatusFilter::NeverOpened => StatusFilter::All,
        }
    }

    fn matches(self, b: &Bookmark) -> bool {
        match self {
            StatusFilter::All => true,
            StatusFilter::Unread => b.status == Status::Unread,
            StatusFilter::Reading => b.status == Status::Reading,
            StatusFilter::Done => b.status == Status::Done,
            StatusFilter::Skipped => b.status == Status::Skipped,
            StatusFilter::NeverOpened => b.open_count == 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkFilter {
    #[default]
    All,
    Unknown,
    Dead,
    Alive,
    Error,
    Redirect,
}

impl LinkFilter {
    pub fn label(self) -> &'static str {
        match self {
            LinkFilter::All => "all links",
            LinkFilter::Unknown => "unchecked",
            LinkFilter::Dead => "dead",
            LinkFilter::Alive => "alive",
            LinkFilter::Error => "errors",
            LinkFilter::Redirect => "redirects",
        }
    }

    pub fn next(self) -> Self {
        match self {
            LinkFilter::All => LinkFilter::Unknown,
            LinkFilter::Unknown => LinkFilter::Dead,
            LinkFilter::Dead => LinkFilter::Alive,
            LinkFilter::Alive => LinkFilter::Error,
            LinkFilter::Error => LinkFilter::Redirect,
            LinkFilter::Redirect => LinkFilter::All,
        }
    }

    fn matches(self, b: &Bookmark) -> bool {
        match self {
            LinkFilter::All => true,
            LinkFilter::Unknown => b.link_health == LinkHealth::Unknown,
            LinkFilter::Dead => b.link_health == LinkHealth::Dead,
            LinkFilter::Alive => b.link_health == LinkHealth::Alive,
            LinkFilter::Error => b.link_health == LinkHealth::Error,
            LinkFilter::Redirect => b.link_health == LinkHealth::Redirect,
        }
    }
}

pub struct App {
    pub library: Library,
    pub filtered: Vec<usize>,
    pub selected: usize,
    pub list_offset: usize,
    pub mode: InputMode,
    pub query: String,
    pub folder_filter: Option<String>,
    pub domain_filter: Option<String>,
    pub tag_filter: Option<String>,
    pub url_filter: Option<String>,
    pub year_filter: Option<i32>,
    pub status_filter: StatusFilter,
    pub link_filter: LinkFilter,
    pub favorite_filter: bool,
    pub message: String,
    pub bulk_selected: HashSet<usize>,
    pub pending_delete_ids: Vec<String>,
    pub dirty: bool,
    pub should_quit: bool,
    matcher: SkimMatcherV2,
    check_rx: Option<Receiver<CheckResult>>,
    pub check_progress: Option<Arc<CheckProgress>>,
    pending_results: usize,

    // Domain browser
    pub domain_list: Vec<(String, usize)>,
    pub domain_selected: usize,
    pub domain_offset: usize,
    pub domain_query: String,

    // Tag browser
    pub tag_list: Vec<(String, usize)>,
    pub tag_selected: usize,
    pub tag_offset: usize,
    pub tag_query: String,

    // Duplicates browser
    pub dup_list: Vec<(String, Vec<usize>)>,
    pub dup_selected: usize,
    pub dup_offset: usize,

    // Saved views
    pub views: Vec<SavedView>,
    pub view_selected: usize,
    pub view_offset: usize,
    pub view_name_input: String,

    // Year browser
    pub year_list: Vec<(i32, usize)>,
    pub year_selected: usize,

    // Launch digest
    pub digest_indices: Vec<usize>,
    pub digest_selected: usize,
    digest_salt: u64,
}

impl App {
    pub fn new(library: Library) -> Self {
        let digest_indices = digest::build_digest(&library, digest::default_size());
        let show_digest = !digest_indices.is_empty();

        let mut app = Self {
            library,
            filtered: Vec::new(),
            selected: 0,
            list_offset: 0,
            mode: if show_digest {
                InputMode::Digest
            } else {
                InputMode::Normal
            },
            query: String::new(),
            folder_filter: None,
            domain_filter: None,
            tag_filter: None,
            url_filter: None,
            year_filter: None,
            status_filter: StatusFilter::All,
            link_filter: LinkFilter::All,
            favorite_filter: false,
            message: if show_digest {
                "today's dig — enter open · j/k · esc dismiss · z reshuffle".into()
            } else {
                String::new()
            },
            bulk_selected: HashSet::new(),
            pending_delete_ids: Vec::new(),
            dirty: false,
            should_quit: false,
            matcher: SkimMatcherV2::default(),
            check_rx: None,
            check_progress: None,
            pending_results: 0,
            domain_list: Vec::new(),
            domain_selected: 0,
            domain_offset: 0,
            domain_query: String::new(),
            tag_list: Vec::new(),
            tag_selected: 0,
            tag_offset: 0,
            tag_query: String::new(),
            dup_list: Vec::new(),
            dup_selected: 0,
            dup_offset: 0,
            views: views::load_views().unwrap_or_default(),
            view_selected: 0,
            view_offset: 0,
            view_name_input: String::new(),
            year_list: Vec::new(),
            year_selected: 0,
            digest_indices,
            digest_selected: 0,
            digest_salt: 1,
        };
        app.refilter();
        app
    }

    pub fn refilter(&mut self) {
        let q = self.query.trim().to_lowercase();
        let folder = self.folder_filter.clone();
        let domain = self.domain_filter.clone();
        let tag = self.tag_filter.clone();
        let url = self.url_filter.clone();
        let year = self.year_filter;
        let status = self.status_filter;
        let link = self.link_filter;
        let favorite_only = self.favorite_filter;

        let mut scored: Vec<(i64, usize)> = self
            .library
            .bookmarks
            .iter()
            .enumerate()
            .filter(|(_, b)| {
                if let Some(ref f) = folder
                    && &b.folder != f
                {
                    return false;
                }
                if let Some(ref d) = domain
                    && &b.domain() != d
                {
                    return false;
                }
                if let Some(ref t) = tag
                    && !b.tags.iter().any(|x| x == t)
                {
                    return false;
                }
                if let Some(ref u) = url
                    && &b.url != u
                {
                    return false;
                }
                if let Some(y) = year {
                    match b.created {
                        Some(c) if c.year() == y => {}
                        _ => return false,
                    }
                }
                if favorite_only && !b.favorite {
                    return false;
                }
                status.matches(b) && link.matches(b)
            })
            .filter_map(|(i, b)| {
                if q.is_empty() {
                    return Some((0, i));
                }
                let blob = b.search_blob();
                self.matcher.fuzzy_match(&blob, &q).map(|score| (score, i))
            })
            .collect();

        if !q.is_empty() {
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        }

        self.filtered = scored.into_iter().map(|(_, i)| i).collect();
        if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len().saturating_sub(1);
        }
        self.clamp_offset(0);
    }

    pub fn selected_bookmark(&self) -> Option<&Bookmark> {
        self.filtered
            .get(self.selected)
            .and_then(|&i| self.library.bookmarks.get(i))
    }

    pub fn selected_bookmark_mut(&mut self) -> Option<&mut Bookmark> {
        let idx = *self.filtered.get(self.selected)?;
        self.library.bookmarks.get_mut(idx)
    }

    pub fn move_sel(&mut self, delta: isize, page: usize) {
        if self.filtered.is_empty() {
            return;
        }
        let len = self.filtered.len() as isize;
        let mut next = self.selected as isize + delta;
        if next < 0 {
            next = 0;
        } else if next >= len {
            next = len - 1;
        }
        self.selected = next as usize;
        self.clamp_offset(page);
    }

    pub fn clamp_offset(&mut self, visible: usize) {
        if visible == 0 {
            return;
        }
        if self.selected < self.list_offset {
            self.list_offset = self.selected;
        } else if self.selected >= self.list_offset + visible {
            self.list_offset = self.selected + 1 - visible;
        }
    }

    pub fn open_selected(&mut self) -> Result<()> {
        let url = match self.selected_bookmark() {
            Some(b) => b.url.clone(),
            None => {
                self.message = "nothing selected".into();
                return Ok(());
            }
        };
        open::that(&url)?;
        if let Some(b) = self.selected_bookmark_mut() {
            b.mark_opened();
        }
        self.dirty = true;
        self.message = format!("opened {url}");
        Ok(())
    }

    pub fn open_digest_selected(&mut self) -> Result<()> {
        let idx = match self.digest_indices.get(self.digest_selected).copied() {
            Some(i) => i,
            None => {
                self.message = "empty digest".into();
                return Ok(());
            }
        };
        let url = self.library.bookmarks[idx].url.clone();
        open::that(&url)?;
        self.library.bookmarks[idx].mark_opened();
        self.dirty = true;
        self.message = format!("opened {url}");
        // Jump main list to this item after dismiss-friendly open
        if let Some(pos) = self.filtered.iter().position(|&i| i == idx) {
            self.selected = pos;
        }
        Ok(())
    }

    pub fn copy_url(&mut self) -> Result<()> {
        let url = match self.selected_bookmark() {
            Some(b) => b.url.clone(),
            None => {
                self.message = "nothing selected".into();
                return Ok(());
            }
        };
        let mut cb = arboard::Clipboard::new()?;
        cb.set_text(url.clone())?;
        self.message = format!("copied {url}");
        Ok(())
    }

    pub fn cycle_status(&mut self) {
        if !self.bulk_selected.is_empty() {
            let n = self.bulk_selected.len();
            for &idx in &self.bulk_selected {
                if let Some(b) = self.library.bookmarks.get_mut(idx) {
                    b.status = b.status.cycle();
                }
            }
            self.dirty = true;
            self.message = format!("cycled status on {n} selected");
            self.bulk_selected.clear();
            self.refilter();
            return;
        }
        if let Some(b) = self.selected_bookmark_mut() {
            b.status = b.status.cycle();
            let s = b.status.as_str().to_string();
            self.dirty = true;
            self.message = format!("status → {s}");
        }
        self.refilter();
    }

    pub fn mark_done(&mut self) {
        if !self.bulk_selected.is_empty() {
            let n = self.bulk_selected.len();
            for &idx in &self.bulk_selected {
                if let Some(b) = self.library.bookmarks.get_mut(idx) {
                    b.status = Status::Done;
                }
            }
            self.dirty = true;
            self.message = format!("marked {n} done");
            self.bulk_selected.clear();
            self.refilter();
            return;
        }
        if let Some(b) = self.selected_bookmark_mut() {
            b.status = Status::Done;
            self.dirty = true;
            self.message = "marked done".into();
        }
        self.refilter();
    }

    pub fn mark_skipped(&mut self) {
        if !self.bulk_selected.is_empty() {
            let n = self.bulk_selected.len();
            for &idx in &self.bulk_selected {
                if let Some(b) = self.library.bookmarks.get_mut(idx) {
                    b.status = Status::Skipped;
                }
            }
            self.dirty = true;
            self.message = format!("skipped {n} selected");
            self.bulk_selected.clear();
            self.refilter();
            return;
        }
        if let Some(b) = self.selected_bookmark_mut() {
            b.status = Status::Skipped;
            self.dirty = true;
            self.message = "skipped".into();
        }
        self.refilter();
    }

    // ── Bulk selection ──────────────────────────────────────────────

    pub fn toggle_bulk_selected(&mut self) {
        let Some(&idx) = self.filtered.get(self.selected) else {
            return;
        };
        if !self.bulk_selected.remove(&idx) {
            self.bulk_selected.insert(idx);
        }
        let n = self.bulk_selected.len();
        self.message = if n == 0 {
            "selection cleared".into()
        } else {
            format!("{n} selected")
        };
    }

    pub fn toggle_select_all_in_view(&mut self) {
        let all_selected = !self.filtered.is_empty()
            && self
                .filtered
                .iter()
                .all(|i| self.bulk_selected.contains(i));
        if all_selected {
            for i in &self.filtered {
                self.bulk_selected.remove(i);
            }
            self.message = "selection cleared".into();
        } else {
            for &i in &self.filtered {
                self.bulk_selected.insert(i);
            }
            self.message = format!("{} selected", self.bulk_selected.len());
        }
    }

    pub fn clear_bulk_selection(&mut self) {
        self.bulk_selected.clear();
        self.message = "selection cleared".into();
    }

    // ── Delete ───────────────────────────────────────────────────────

    /// Stages the selected bookmark(s) for deletion and asks for confirmation.
    /// Deletion is local-only: it does not touch Raindrop, so a bookmark
    /// still present there returns on the next pull sync.
    pub fn request_delete(&mut self) {
        if !self.bulk_selected.is_empty() {
            self.pending_delete_ids = self
                .bulk_selected
                .iter()
                .filter_map(|&i| self.library.bookmarks.get(i))
                .map(|b| b.id.clone())
                .collect();
        } else if let Some(b) = self.selected_bookmark() {
            self.pending_delete_ids = vec![b.id.clone()];
        } else {
            return;
        }
        let n = self.pending_delete_ids.len();
        self.mode = InputMode::ConfirmDelete;
        self.message = format!(
            "delete {n} bookmark{}? y confirm · esc cancel",
            if n == 1 { "" } else { "s" }
        );
    }

    pub fn confirm_delete(&mut self) {
        let ids: HashSet<String> = self.pending_delete_ids.drain(..).collect();
        let n = ids.len();
        self.library.bookmarks.retain(|b| !ids.contains(&b.id));
        self.bulk_selected.clear();
        self.dirty = true;
        self.mode = InputMode::Normal;
        self.message = format!("deleted {n} bookmark{}", if n == 1 { "" } else { "s" });
        self.refilter();
    }

    pub fn cancel_delete(&mut self) {
        self.pending_delete_ids.clear();
        self.mode = InputMode::Normal;
        self.message = "delete cancelled".into();
    }

    pub fn random_pick(&mut self) {
        let candidates: Vec<usize> = self
            .filtered
            .iter()
            .copied()
            .filter(|&i| {
                let b = &self.library.bookmarks[i];
                b.open_count == 0 && b.status == Status::Unread && b.link_health != LinkHealth::Dead
            })
            .collect();

        let pool = if candidates.is_empty() {
            &self.filtered
        } else {
            &candidates
        };
        if pool.is_empty() {
            self.message = "no bookmarks to pick".into();
            return;
        }

        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as usize)
            .unwrap_or(0);
        let pick = pool[seed % pool.len()];
        if let Some(pos) = self.filtered.iter().position(|&i| i == pick) {
            self.selected = pos;
            self.message = "🎲 serendipity".into();
        }
    }

    pub fn toggle_folder_filter(&mut self) {
        let folders = self.library.folders();
        if folders.is_empty() {
            return;
        }
        match &self.folder_filter {
            None => self.folder_filter = Some(folders[0].clone()),
            Some(cur) => {
                if let Some(i) = folders.iter().position(|f| f == cur) {
                    if i + 1 < folders.len() {
                        self.folder_filter = Some(folders[i + 1].clone());
                    } else {
                        self.folder_filter = None;
                    }
                } else {
                    self.folder_filter = None;
                }
            }
        }
        self.selected = 0;
        self.list_offset = 0;
        self.refilter();
    }

    pub fn cycle_status_filter(&mut self) {
        self.status_filter = self.status_filter.next();
        self.selected = 0;
        self.list_offset = 0;
        self.refilter();
        self.message = format!("filter: {}", self.status_filter.label());
    }

    pub fn cycle_link_filter(&mut self) {
        self.link_filter = self.link_filter.next();
        self.selected = 0;
        self.list_offset = 0;
        self.refilter();
        self.message = format!("links: {}", self.link_filter.label());
    }

    pub fn toggle_favorite_filter(&mut self) {
        self.favorite_filter = !self.favorite_filter;
        self.selected = 0;
        self.list_offset = 0;
        self.refilter();
        self.message = if self.favorite_filter {
            "favorites only".into()
        } else {
            "favorites: off".into()
        };
    }

    pub fn clear_filters(&mut self) {
        self.folder_filter = None;
        self.domain_filter = None;
        self.tag_filter = None;
        self.url_filter = None;
        self.year_filter = None;
        self.status_filter = StatusFilter::All;
        self.link_filter = LinkFilter::All;
        self.favorite_filter = false;
        self.query.clear();
        self.selected = 0;
        self.list_offset = 0;
        self.refilter();
        self.message = "filters cleared".into();
    }

    // ── Domain browser ──────────────────────────────────────────────

    pub fn open_domain_browser(&mut self) {
        self.rebuild_domain_list();
        self.domain_selected = 0;
        self.domain_offset = 0;
        self.domain_query.clear();
        // Pre-select current domain filter if any
        if let Some(ref d) = self.domain_filter
            && let Some(i) = self.domain_list.iter().position(|(name, _)| name == d)
        {
            self.domain_selected = i;
        }
        self.mode = InputMode::DomainBrowser;
        self.message = "domain browser — enter filter · / search · esc close".into();
    }

    pub fn rebuild_domain_list(&mut self) {
        let q = self.domain_query.trim().to_lowercase();
        let mut counts: HashMap<String, usize> = HashMap::new();
        for b in &self.library.bookmarks {
            let d = b.domain();
            *counts.entry(d).or_default() += 1;
        }
        let mut list: Vec<(String, usize)> = counts.into_iter().collect();
        if !q.is_empty() {
            list.retain(|(name, _)| {
                self.matcher.fuzzy_match(&name.to_lowercase(), &q).is_some()
                    || name.to_lowercase().contains(&q)
            });
        }
        list.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        self.domain_list = list;
        if self.domain_selected >= self.domain_list.len() {
            self.domain_selected = self.domain_list.len().saturating_sub(1);
        }
    }

    pub fn domain_move(&mut self, delta: isize, visible: usize) {
        if self.domain_list.is_empty() {
            return;
        }
        let len = self.domain_list.len() as isize;
        let mut next = self.domain_selected as isize + delta;
        if next < 0 {
            next = 0;
        } else if next >= len {
            next = len - 1;
        }
        self.domain_selected = next as usize;
        if visible > 0 {
            if self.domain_selected < self.domain_offset {
                self.domain_offset = self.domain_selected;
            } else if self.domain_selected >= self.domain_offset + visible {
                self.domain_offset = self.domain_selected + 1 - visible;
            }
        }
    }

    pub fn apply_domain_selection(&mut self) {
        if let Some((name, count)) = self.domain_list.get(self.domain_selected) {
            self.domain_filter = Some(name.clone());
            self.message = format!("domain: {name} ({count})");
        }
        self.mode = InputMode::Normal;
        self.selected = 0;
        self.list_offset = 0;
        self.refilter();
    }

    /// Quick filter: set domain from currently selected bookmark.
    pub fn filter_domain_from_selected(&mut self) {
        let domain = match self.selected_bookmark() {
            Some(b) => b.domain(),
            None => {
                self.message = "nothing selected".into();
                return;
            }
        };
        self.domain_filter = Some(domain.clone());
        self.selected = 0;
        self.list_offset = 0;
        self.refilter();
        self.message = format!("domain: {domain}");
    }

    // ── Tag browser ─────────────────────────────────────────────────

    pub fn open_tag_browser(&mut self) {
        self.rebuild_tag_list();
        self.tag_selected = 0;
        self.tag_offset = 0;
        self.tag_query.clear();
        // Pre-select current tag filter if any
        if let Some(ref t) = self.tag_filter
            && let Some(i) = self.tag_list.iter().position(|(name, _)| name == t)
        {
            self.tag_selected = i;
        }
        self.mode = InputMode::TagBrowser;
        self.message = "tag browser — enter filter · / search · esc close".into();
    }

    pub fn rebuild_tag_list(&mut self) {
        let q = self.tag_query.trim().to_lowercase();
        let mut counts: HashMap<String, usize> = HashMap::new();
        for b in &self.library.bookmarks {
            for t in &b.tags {
                *counts.entry(t.clone()).or_default() += 1;
            }
        }
        let mut list: Vec<(String, usize)> = counts.into_iter().collect();
        if !q.is_empty() {
            list.retain(|(name, _)| {
                self.matcher.fuzzy_match(&name.to_lowercase(), &q).is_some()
                    || name.to_lowercase().contains(&q)
            });
        }
        list.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        self.tag_list = list;
        if self.tag_selected >= self.tag_list.len() {
            self.tag_selected = self.tag_list.len().saturating_sub(1);
        }
    }

    pub fn tag_move(&mut self, delta: isize, visible: usize) {
        if self.tag_list.is_empty() {
            return;
        }
        let len = self.tag_list.len() as isize;
        let mut next = self.tag_selected as isize + delta;
        if next < 0 {
            next = 0;
        } else if next >= len {
            next = len - 1;
        }
        self.tag_selected = next as usize;
        if visible > 0 {
            if self.tag_selected < self.tag_offset {
                self.tag_offset = self.tag_selected;
            } else if self.tag_selected >= self.tag_offset + visible {
                self.tag_offset = self.tag_selected + 1 - visible;
            }
        }
    }

    pub fn apply_tag_selection(&mut self) {
        if let Some((name, count)) = self.tag_list.get(self.tag_selected) {
            self.tag_filter = Some(name.clone());
            self.message = format!("tag: {name} ({count})");
        }
        self.mode = InputMode::Normal;
        self.selected = 0;
        self.list_offset = 0;
        self.refilter();
    }

    // ── Duplicates browser ──────────────────────────────────────────

    pub fn open_duplicates_browser(&mut self) {
        self.dup_list = self.library.duplicate_groups();
        self.dup_selected = 0;
        self.dup_offset = 0;
        if self.dup_list.is_empty() {
            self.message = "no duplicate URLs found".into();
            return;
        }
        self.mode = InputMode::DuplicatesBrowser;
        self.message = "duplicates — enter view group · esc close".into();
    }

    pub fn dup_move(&mut self, delta: isize, visible: usize) {
        if self.dup_list.is_empty() {
            return;
        }
        let len = self.dup_list.len() as isize;
        let mut next = self.dup_selected as isize + delta;
        if next < 0 {
            next = 0;
        } else if next >= len {
            next = len - 1;
        }
        self.dup_selected = next as usize;
        if visible > 0 {
            if self.dup_selected < self.dup_offset {
                self.dup_offset = self.dup_selected;
            } else if self.dup_selected >= self.dup_offset + visible {
                self.dup_offset = self.dup_selected + 1 - visible;
            }
        }
    }

    pub fn apply_dup_selection(&mut self) {
        if let Some((url, indices)) = self.dup_list.get(self.dup_selected) {
            self.url_filter = Some(url.clone());
            self.message = format!("duplicates of {url} ({} copies)", indices.len());
        }
        self.mode = InputMode::Normal;
        self.selected = 0;
        self.list_offset = 0;
        self.refilter();
    }

    // ── Saved views ──────────────────────────────────────────────────

    pub fn open_view_browser(&mut self) {
        self.view_selected = 0;
        self.view_offset = 0;
        if self.views.is_empty() {
            self.message = "no saved views yet — O saves the current filters".into();
            return;
        }
        self.mode = InputMode::ViewBrowser;
        self.message = "saved views — enter load · x delete · esc close".into();
    }

    pub fn view_move(&mut self, delta: isize, visible: usize) {
        if self.views.is_empty() {
            return;
        }
        let len = self.views.len() as isize;
        let mut next = self.view_selected as isize + delta;
        if next < 0 {
            next = 0;
        } else if next >= len {
            next = len - 1;
        }
        self.view_selected = next as usize;
        if visible > 0 {
            if self.view_selected < self.view_offset {
                self.view_offset = self.view_selected;
            } else if self.view_selected >= self.view_offset + visible {
                self.view_offset = self.view_selected + 1 - visible;
            }
        }
    }

    pub fn apply_view_selection(&mut self) {
        if let Some(v) = self.views.get(self.view_selected).cloned() {
            self.query = v.query;
            self.folder_filter = v.folder;
            self.domain_filter = v.domain;
            self.tag_filter = v.tag;
            self.url_filter = None;
            self.year_filter = v.year;
            self.status_filter = v.status;
            self.link_filter = v.link;
            self.favorite_filter = v.favorite;
            self.message = format!("view: {}", v.name);
        }
        self.mode = InputMode::Normal;
        self.selected = 0;
        self.list_offset = 0;
        self.refilter();
    }

    pub fn delete_selected_view(&mut self) {
        if self.view_selected >= self.views.len() {
            return;
        }
        let removed = self.views.remove(self.view_selected);
        if self.view_selected >= self.views.len() {
            self.view_selected = self.views.len().saturating_sub(1);
        }
        let _ = views::save_views(&self.views);
        self.message = format!("deleted view: {}", removed.name);
        if self.views.is_empty() {
            self.mode = InputMode::Normal;
        }
    }

    pub fn begin_save_view(&mut self) {
        self.view_name_input.clear();
        self.mode = InputMode::ViewSave;
        self.message = "name this view — enter save · esc cancel".into();
    }

    pub fn confirm_save_view(&mut self) {
        let name = self.view_name_input.trim().to_string();
        if name.is_empty() {
            self.message = "view name can't be empty".into();
            return;
        }
        let view = SavedView {
            name: name.clone(),
            query: self.query.clone(),
            folder: self.folder_filter.clone(),
            domain: self.domain_filter.clone(),
            tag: self.tag_filter.clone(),
            year: self.year_filter,
            status: self.status_filter,
            link: self.link_filter,
            favorite: self.favorite_filter,
        };
        if let Some(existing) = self.views.iter_mut().find(|v| v.name == name) {
            *existing = view;
        } else {
            self.views.push(view);
            self.views.sort_by(|a, b| a.name.cmp(&b.name));
        }
        let _ = views::save_views(&self.views);
        self.mode = InputMode::Normal;
        self.message = format!("saved view: {name}");
    }

    // ── Year browser / scrub ────────────────────────────────────────

    pub fn open_year_browser(&mut self) {
        self.rebuild_year_list();
        self.year_selected = 0;
        if let Some(y) = self.year_filter
            && let Some(i) = self.year_list.iter().position(|(yr, _)| *yr == y)
        {
            self.year_selected = i;
        }
        self.mode = InputMode::YearBrowser;
        self.message = "year scrub — enter focus year · esc close · 0 clear".into();
    }

    pub fn rebuild_year_list(&mut self) {
        let mut counts: HashMap<i32, usize> = HashMap::new();
        for b in &self.library.bookmarks {
            if let Some(c) = b.created {
                *counts.entry(c.year()).or_default() += 1;
            }
        }
        let mut list: Vec<(i32, usize)> = counts.into_iter().collect();
        list.sort_by_key(|a| a.0); // oldest first for scrubbing
        self.year_list = list;
        if self.year_selected >= self.year_list.len() {
            self.year_selected = self.year_list.len().saturating_sub(1);
        }
    }

    pub fn year_move(&mut self, delta: isize) {
        if self.year_list.is_empty() {
            return;
        }
        let len = self.year_list.len() as isize;
        let mut next = self.year_selected as isize + delta;
        if next < 0 {
            next = 0;
        } else if next >= len {
            next = len - 1;
        }
        self.year_selected = next as usize;
    }

    pub fn apply_year_selection(&mut self) {
        if let Some((year, count)) = self.year_list.get(self.year_selected) {
            self.year_filter = Some(*year);
            // Year scrub defaults: never-opened unread pile for that year
            self.status_filter = StatusFilter::NeverOpened;
            self.message = format!(
                "year scrub {year} ({count}) · never-opened · n skip · d done · esc clear filters"
            );
        }
        self.mode = InputMode::Normal;
        self.selected = 0;
        self.list_offset = 0;
        self.refilter();
    }

    pub fn clear_year_filter(&mut self) {
        self.year_filter = None;
        self.mode = InputMode::Normal;
        self.selected = 0;
        self.list_offset = 0;
        self.refilter();
        self.message = "year filter cleared".into();
    }

    // ── Digest ──────────────────────────────────────────────────────

    pub fn show_digest(&mut self) {
        self.digest_indices = digest::build_digest_reshuffled(
            &self.library,
            digest::default_size(),
            self.digest_salt,
        );
        self.digest_selected = 0;
        if self.digest_indices.is_empty() {
            self.message = "digest empty — everything opened or dead?".into();
            self.mode = InputMode::Normal;
            return;
        }
        self.mode = InputMode::Digest;
        self.message = "today's dig — enter open · j/k · n skip · z reshuffle · esc dismiss".into();
    }

    pub fn reshuffle_digest(&mut self) {
        self.digest_salt = self.digest_salt.wrapping_add(1);
        self.digest_indices = digest::build_digest_reshuffled(
            &self.library,
            digest::default_size(),
            self.digest_salt,
        );
        self.digest_selected = 0;
        if self.digest_indices.is_empty() {
            self.message = "digest empty".into();
            self.mode = InputMode::Normal;
        } else {
            self.message = "digest reshuffled".into();
        }
    }

    pub fn digest_move(&mut self, delta: isize) {
        if self.digest_indices.is_empty() {
            return;
        }
        let len = self.digest_indices.len() as isize;
        let mut next = self.digest_selected as isize + delta;
        if next < 0 {
            next = 0;
        } else if next >= len {
            next = len - 1;
        }
        self.digest_selected = next as usize;
    }

    pub fn digest_skip_selected(&mut self) {
        if let Some(&idx) = self.digest_indices.get(self.digest_selected) {
            self.library.bookmarks[idx].status = Status::Skipped;
            self.dirty = true;
            self.digest_indices.remove(self.digest_selected);
            if self.digest_selected >= self.digest_indices.len() {
                self.digest_selected = self.digest_indices.len().saturating_sub(1);
            }
            self.message = "skipped from digest".into();
            if self.digest_indices.is_empty() {
                self.mode = InputMode::Normal;
                self.message = "digest done — nice".into();
            }
        }
    }

    pub fn dismiss_digest(&mut self) {
        self.mode = InputMode::Normal;
        self.message.clear();
    }

    // ── Health checks ───────────────────────────────────────────────

    pub fn is_checking(&self) -> bool {
        self.check_progress
            .as_ref()
            .map(|p| p.is_running())
            .unwrap_or(false)
            || self.check_rx.is_some()
    }

    pub fn check_selected(&mut self) {
        let Some(b) = self.selected_bookmark() else {
            self.message = "nothing selected".into();
            return;
        };
        let job = CheckJob {
            id: b.id.clone(),
            url: b.url.clone(),
        };
        self.start_jobs(vec![job]);
        self.message = "checking link…".into();
    }

    pub fn check_filtered(&mut self, force: bool) {
        if self.is_checking() {
            self.message = "check already running (x to cancel)".into();
            return;
        }
        let jobs: Vec<CheckJob> = self
            .filtered
            .iter()
            .filter_map(|&i| {
                let b = &self.library.bookmarks[i];
                if !force && b.link_health != LinkHealth::Unknown {
                    return None;
                }
                Some(CheckJob {
                    id: b.id.clone(),
                    url: b.url.clone(),
                })
            })
            .collect();

        if jobs.is_empty() {
            self.message = if force {
                "nothing in view to check".into()
            } else {
                "no unchecked links in view (Shift-C to recheck all)".into()
            };
            return;
        }

        let n = jobs.len();
        self.start_jobs(jobs);
        self.message = format!("checking {n} links…");
    }

    fn start_jobs(&mut self, jobs: Vec<CheckJob>) {
        if let Some(p) = &self.check_progress {
            p.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if let Some(rx) = self.check_rx.take() {
            while rx.try_recv().is_ok() {}
        }

        let (rx, progress) = health::spawn_batch(jobs, health::default_concurrency());
        self.check_rx = Some(rx);
        self.check_progress = Some(progress);
        self.pending_results = 0;
    }

    pub fn cancel_checks(&mut self) {
        if let Some(p) = &self.check_progress {
            p.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.message = "checks cancelled".into();
    }

    pub fn poll_checks(&mut self) -> bool {
        let Some(rx) = &self.check_rx else {
            return false;
        };

        let mut applied = 0usize;
        loop {
            match rx.try_recv() {
                Ok(result) => {
                    if self.library.apply_check_result(&result) {
                        applied += 1;
                        self.dirty = true;
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.check_rx = None;
                    break;
                }
            }
        }

        if applied > 0 {
            self.pending_results += applied;
            if self.pending_results >= 8
                || self
                    .check_progress
                    .as_ref()
                    .map(|p| !p.is_running())
                    .unwrap_or(true)
            {
                self.refilter();
                self.pending_results = 0;
            }
        }

        if let Some(p) = &self.check_progress {
            let (done, total, alive, dead, errors, redirects) = p.snapshot();
            if total > 0 {
                if done >= total || self.check_rx.is_none() {
                    self.message = format!(
                        "check done: {done}/{total} · alive={alive} dead={dead} err={errors} redir={redirects}"
                    );
                    self.check_progress = None;
                    self.refilter();
                } else {
                    self.message = format!(
                        "checking {done}/{total} · alive={alive} dead={dead} err={errors}  [x cancel]"
                    );
                }
            }
        }

        applied > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bm(id: &str) -> Bookmark {
        Bookmark {
            id: id.into(),
            title: String::new(),
            note: String::new(),
            excerpt: String::new(),
            url: format!("https://example.com/{id}"),
            folder: String::new(),
            tags: vec![],
            created: None,
            favorite: false,
            status: Status::Unread,
            open_count: 0,
            last_opened: None,
            link_health: LinkHealth::Unknown,
            link_status_code: None,
            link_checked_at: None,
            link_error: None,
        }
    }

    fn test_app() -> App {
        let library = Library {
            bookmarks: vec![bm("1"), bm("2"), bm("3")],
            ..Default::default()
        };
        let mut app = App::new(library);
        app.mode = InputMode::Normal;
        app
    }

    #[test]
    fn delete_selected_removes_bookmark_after_confirm() {
        let mut app = test_app();
        app.selected = 1; // "2"
        app.request_delete();
        assert_eq!(app.mode, InputMode::ConfirmDelete);
        assert_eq!(app.pending_delete_ids, vec!["2".to_string()]);

        app.confirm_delete();
        assert_eq!(app.mode, InputMode::Normal);
        assert!(app.pending_delete_ids.is_empty());
        assert_eq!(app.library.bookmarks.len(), 2);
        assert!(app.library.bookmarks.iter().all(|b| b.id != "2"));
        assert!(app.dirty);
    }

    #[test]
    fn cancel_delete_leaves_library_untouched() {
        let mut app = test_app();
        app.selected = 0;
        app.request_delete();
        app.cancel_delete();
        assert_eq!(app.mode, InputMode::Normal);
        assert_eq!(app.library.bookmarks.len(), 3);
        assert!(!app.dirty);
    }

    #[test]
    fn bulk_delete_removes_all_selected() {
        let mut app = test_app();
        app.bulk_selected.insert(0);
        app.bulk_selected.insert(2);
        app.request_delete();
        assert_eq!(app.pending_delete_ids.len(), 2);

        app.confirm_delete();
        assert_eq!(app.library.bookmarks.len(), 1);
        assert_eq!(app.library.bookmarks[0].id, "2");
        assert!(app.bulk_selected.is_empty());
    }
}
