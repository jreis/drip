//! Raindrop.io REST API client (pull-only).

use crate::merge::RemoteBookmark;
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;
use std::thread;
use std::time::{Duration, Instant};
use ureq::Agent;

const API_BASE: &str = "https://api.raindrop.io/rest/v1";
const PER_PAGE: u32 = 50; // API max
const USER_AGENT: &str = "drip/0.1 (https://github.com/jreis/drip)";

/// Raindrop documents ~120 req/min for OAuth. Stay under that.
const MIN_INTERVAL: Duration = Duration::from_millis(550);
const MAX_RETRIES: u32 = 8;
const DEFAULT_RETRY_WAIT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct Client {
    agent: Agent,
    token: String,
    /// Serialize and pace requests.
    last_request: Option<Instant>,
}

impl Client {
    pub fn new(token: impl Into<String>) -> Self {
        let agent: Agent = Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .max_redirects(5)
            .user_agent(USER_AGENT)
            .build()
            .into();
        Self {
            agent,
            token: token.into(),
            last_request: None,
        }
    }

    fn pace(&mut self) {
        if let Some(last) = self.last_request {
            let elapsed = last.elapsed();
            if elapsed < MIN_INTERVAL {
                thread::sleep(MIN_INTERVAL - elapsed);
            }
        }
        self.last_request = Some(Instant::now());
    }

    fn get_json<T: for<'de> Deserialize<'de>>(&mut self, path: &str) -> Result<T> {
        let url = format!("{API_BASE}{path}");
        let mut attempt = 0u32;

        loop {
            self.pace();

            let mut resp = self
                .agent
                .get(&url)
                .header("Authorization", &format!("Bearer {}", self.token))
                .call()
                .with_context(|| format!("GET {url}"))?;

            let status = resp.status().as_u16();

            // Capture rate-limit headers before consuming the body.
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .map(Duration::from_secs);

            let remaining = resp
                .headers()
                .get("x-ratelimit-remaining")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u32>().ok());

            let body = resp
                .body_mut()
                .read_to_string()
                .with_context(|| format!("read body {url}"))?;

            if status == 401 || status == 403 {
                bail!("Raindrop auth failed (HTTP {status}). Re-run: drip auth");
            }

            if status == 429 {
                attempt += 1;
                if attempt > MAX_RETRIES {
                    bail!(
                        "Raindrop rate limit (HTTP 429) after {MAX_RETRIES} retries. \
                         Wait a few minutes and re-run: drip sync"
                    );
                }
                let wait = retry_after
                    .unwrap_or(DEFAULT_RETRY_WAIT)
                    .max(Duration::from_secs(5));
                eprintln!(
                    "\n  rate limited (429) — sleeping {}s then retry {attempt}/{MAX_RETRIES}…",
                    wait.as_secs()
                );
                thread::sleep(wait);
                continue;
            }

            if !(200..300).contains(&status) {
                bail!("Raindrop API HTTP {status}: {}", truncate(&body, 200));
            }

            // If the server is almost out of budget, pause proactively.
            if remaining == Some(0) {
                let wait = retry_after.unwrap_or(DEFAULT_RETRY_WAIT);
                eprintln!(
                    "\n  rate limit budget empty — sleeping {}s…",
                    wait.as_secs()
                );
                thread::sleep(wait.max(Duration::from_secs(5)));
            }

            return serde_json::from_str(&body).with_context(|| format!("parse JSON from {url}"));
        }
    }

    /// Verify token; returns email or display name if available.
    pub fn whoami(&mut self) -> Result<String> {
        #[derive(Deserialize)]
        struct UserResp {
            result: bool,
            user: Option<User>,
        }
        #[derive(Deserialize)]
        struct User {
            email: Option<String>,
            #[serde(default, rename = "fullName")]
            full_name: Option<String>,
            #[serde(default)]
            name: Option<String>,
        }

        let resp: UserResp = self.get_json("/user")?;
        if !resp.result {
            bail!("Raindrop /user returned result=false");
        }
        let u = resp.user.context("no user in response")?;
        Ok(u.email
            .or(u.full_name)
            .or(u.name)
            .unwrap_or_else(|| "(authenticated)".into()))
    }

    /// Map collection id → title (includes Unsorted system name).
    pub fn collection_names(&mut self) -> Result<HashMap<i64, String>> {
        #[derive(Deserialize)]
        struct CollResp {
            #[serde(default)]
            items: Vec<Collection>,
        }
        #[derive(Deserialize)]
        struct Collection {
            #[serde(rename = "_id")]
            id: i64,
            #[serde(default)]
            title: String,
        }

        let mut map = HashMap::new();
        map.insert(-1, "Unsorted".into());
        map.insert(-99, "Trash".into());

        let root: CollResp = self.get_json("/collections")?;
        for c in root.items {
            map.insert(c.id, c.title);
        }

        if let Ok(children) = self.get_json::<CollResp>("/collections/childrens") {
            for c in children.items {
                map.insert(c.id, c.title);
            }
        }

        Ok(map)
    }

    /// Pull raindrops. `since` enables incremental mode (sort by lastUpdate, stop early).
    /// `full` ignores since and pages everything (except trash).
    pub fn fetch_raindrops(
        &mut self,
        since: Option<DateTime<Utc>>,
        full: bool,
        on_page: impl Fn(usize, usize),
    ) -> Result<Vec<RemoteBookmark>> {
        let names = self.collection_names()?;
        let mut out = Vec::new();
        let mut page = 0u32;
        let since = since.map(|t| t - chrono::Duration::minutes(2));

        loop {
            let path = format!("/raindrops/0?perpage={PER_PAGE}&page={page}&sort=-lastUpdate");
            let resp: RaindropsPage = self.get_json(&path)?;
            if !resp.result {
                bail!("raindrops list result=false");
            }

            let count = resp.items.len();
            if count == 0 {
                break;
            }

            let mut stop_incremental = false;
            for item in resp.items {
                if !full
                    && let (Some(since), Some(lu)) = (since, parse_dt(&item.last_update))
                    && lu < since
                {
                    stop_incremental = true;
                    break;
                }

                let coll_id = item.collection.as_ref().map(|c| c.id).unwrap_or(-1);
                if coll_id == -99 {
                    continue;
                }
                let folder = names
                    .get(&coll_id)
                    .cloned()
                    .unwrap_or_else(|| match coll_id {
                        -1 => "Unsorted".into(),
                        _ => format!("collection:{coll_id}"),
                    });

                out.push(RemoteBookmark {
                    id: item.id.to_string(),
                    title: item.title.unwrap_or_default(),
                    note: item.note.unwrap_or_default(),
                    excerpt: item.excerpt.unwrap_or_default(),
                    url: item.link.unwrap_or_default(),
                    folder,
                    tags: item.tags.unwrap_or_default(),
                    created: parse_dt(&item.created),
                    favorite: item.important.unwrap_or(false),
                    broken: item.broken.unwrap_or(false),
                });
            }

            on_page(page as usize + 1, out.len());

            if stop_incremental || count < PER_PAGE as usize {
                break;
            }
            page += 1;
        }

        Ok(out)
    }
}

#[derive(Debug, Deserialize)]
struct RaindropsPage {
    result: bool,
    #[serde(default)]
    items: Vec<ApiRaindrop>,
}

#[derive(Debug, Deserialize)]
struct ApiRaindrop {
    #[serde(rename = "_id")]
    id: i64,
    title: Option<String>,
    note: Option<String>,
    excerpt: Option<String>,
    link: Option<String>,
    tags: Option<Vec<String>>,
    created: Option<String>,
    #[serde(rename = "lastUpdate")]
    last_update: Option<String>,
    important: Option<bool>,
    broken: Option<bool>,
    collection: Option<CollRef>,
}

#[derive(Debug, Deserialize)]
struct CollRef {
    #[serde(rename = "$id")]
    id: i64,
}

fn parse_dt(raw: &Option<String>) -> Option<DateTime<Utc>> {
    let raw = raw.as_ref()?;
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}…")
    }
}
