use crate::digest;
use crate::health::{self, CheckJob, CheckProgress, CheckResult};
use crate::model::{Bookmark, Library, LinkHealth, Status};
use anyhow::Result;
use chrono::Datelike;
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Search,
    Help,
    DomainBrowser,
    YearBrowser,
    Digest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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
    pub year_filter: Option<i32>,
    pub status_filter: StatusFilter,
    pub link_filter: LinkFilter,
    pub message: String,
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
            year_filter: None,
            status_filter: StatusFilter::All,
            link_filter: LinkFilter::All,
            message: if show_digest {
                "today's dig — enter open · j/k · esc dismiss · z reshuffle".into()
            } else {
                String::new()
            },
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
        let year = self.year_filter;
        let status = self.status_filter;
        let link = self.link_filter;

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
                if let Some(y) = year {
                    match b.created {
                        Some(c) if c.year() == y => {}
                        _ => return false,
                    }
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
        if let Some(b) = self.selected_bookmark_mut() {
            b.status = b.status.cycle();
            let s = b.status.as_str().to_string();
            self.dirty = true;
            self.message = format!("status → {s}");
        }
        self.refilter();
    }

    pub fn mark_done(&mut self) {
        if let Some(b) = self.selected_bookmark_mut() {
            b.status = Status::Done;
            self.dirty = true;
            self.message = "marked done".into();
        }
        self.refilter();
    }

    pub fn mark_skipped(&mut self) {
        if let Some(b) = self.selected_bookmark_mut() {
            b.status = Status::Skipped;
            self.dirty = true;
            self.message = "skipped".into();
        }
        self.refilter();
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

    pub fn clear_filters(&mut self) {
        self.folder_filter = None;
        self.domain_filter = None;
        self.year_filter = None;
        self.status_filter = StatusFilter::All;
        self.link_filter = LinkFilter::All;
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
