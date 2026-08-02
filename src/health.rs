//! Concurrent URL health checking (dead-link detection).

use crate::model::LinkHealth;
use chrono::{DateTime, Utc};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use ureq::Agent;

const USER_AGENT: &str = "drip/0.1 (+local raindrop link checker; contact: local)";
const TIMEOUT_SECS: u64 = 8;
const DEFAULT_CONCURRENCY: usize = 12;

#[derive(Debug, Clone)]
pub struct CheckJob {
    pub id: String,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct CheckResult {
    pub id: String,
    pub health: LinkHealth,
    pub status_code: Option<u16>,
    pub error: Option<String>,
    pub checked_at: DateTime<Utc>,
}

/// Shared progress for in-flight batch checks.
#[derive(Debug, Default)]
pub struct CheckProgress {
    pub total: AtomicUsize,
    pub done: AtomicUsize,
    pub alive: AtomicUsize,
    pub dead: AtomicUsize,
    pub errors: AtomicUsize,
    pub redirects: AtomicUsize,
    pub cancel: AtomicBool,
}

impl CheckProgress {
    pub fn snapshot(&self) -> (usize, usize, usize, usize, usize, usize) {
        (
            self.done.load(Ordering::Relaxed),
            self.total.load(Ordering::Relaxed),
            self.alive.load(Ordering::Relaxed),
            self.dead.load(Ordering::Relaxed),
            self.errors.load(Ordering::Relaxed),
            self.redirects.load(Ordering::Relaxed),
        )
    }

    pub fn is_running(&self) -> bool {
        let done = self.done.load(Ordering::Relaxed);
        let total = self.total.load(Ordering::Relaxed);
        total > 0 && done < total && !self.cancel.load(Ordering::Relaxed)
    }
}

fn build_agent() -> Agent {
    Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(TIMEOUT_SECS)))
        .http_status_as_error(false)
        .max_redirects(5)
        .max_redirects_will_error(false)
        .user_agent(USER_AGENT)
        .build()
        .into()
}

/// Classify a single URL. Prefer HEAD; fall back to GET when HEAD is refused.
pub fn check_url(agent: &Agent, url: &str) -> (LinkHealth, Option<u16>, Option<String>) {
    if url.trim().is_empty() {
        return (LinkHealth::Error, None, Some("empty url".into()));
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return (LinkHealth::Error, None, Some("non-http(s) scheme".into()));
    }

    match do_request(agent, "HEAD", url) {
        Ok((code, health)) => {
            // Some servers hate HEAD — retry with GET.
            if matches!(code, 405 | 501) {
                return match do_request(agent, "GET", url) {
                    Ok((c, h)) => (h, Some(c), None),
                    Err(e) => classify_transport_error(&e),
                };
            }
            (health, Some(code), None)
        }
        Err(e) => {
            // Transport-level HEAD failure: try GET once.
            let head_err = e.to_string();
            match do_request(agent, "GET", url) {
                Ok((c, h)) => (h, Some(c), None),
                Err(e2) => {
                    let (_, code, msg) = classify_transport_error(&e2);
                    (LinkHealth::Error, code, Some(msg.unwrap_or(head_err)))
                }
            }
        }
    }
}

fn do_request(agent: &Agent, method: &str, url: &str) -> Result<(u16, LinkHealth), ureq::Error> {
    let resp = match method {
        "HEAD" => agent.head(url).call(),
        _ => agent.get(url).header("Range", "bytes=0-0").call(),
    }?;

    let code = resp.status().as_u16();
    Ok((code, classify_status(code)))
}

fn classify_status(code: u16) -> LinkHealth {
    match code {
        200..=299 => LinkHealth::Alive,
        // Redirects that weren't followed (or final after partial follow)
        300..=399 => LinkHealth::Redirect,
        // Gone / not found — clearly dead
        404 | 410 | 451 => LinkHealth::Dead,
        // Auth walls / rate limits still mean the host is alive
        401 | 403 | 429 => LinkHealth::Alive,
        // Method issues shouldn't happen after fallback, treat as error
        405 | 501 => LinkHealth::Error,
        400..=499 => LinkHealth::Dead,
        500..=599 => LinkHealth::Error,
        _ => LinkHealth::Error,
    }
}

fn classify_transport_error(err: &ureq::Error) -> (LinkHealth, Option<u16>, Option<String>) {
    let msg = err.to_string();
    let lower = msg.to_ascii_lowercase();

    // DNS / connection refused / no route → treat as dead host
    if lower.contains("dns")
        || lower.contains("name or service not known")
        || lower.contains("nodename nor servname")
        || lower.contains("no such host")
        || lower.contains("connection refused")
        || lower.contains("network is unreachable")
        || lower.contains("host unreachable")
        || lower.contains("failed to lookup")
        || lower.contains("nxdomain")
    {
        return (LinkHealth::Dead, None, Some(msg));
    }

    // TLS cert issues: host exists, but broken — flag as error
    if lower.contains("certificate") || lower.contains("tls") || lower.contains("ssl") {
        return (LinkHealth::Error, None, Some(msg));
    }

    // Timeouts
    if lower.contains("timed out") || lower.contains("timeout") {
        return (LinkHealth::Error, None, Some(msg));
    }

    if let ureq::Error::StatusCode(code) = err {
        let health = classify_status(*code);
        return (health, Some(*code), None);
    }

    (LinkHealth::Error, None, Some(msg))
}

/// Spawn a worker pool that checks `jobs` and sends results on the returned receiver.
/// Progress is updated as work completes. Cancel via `progress.cancel`.
pub fn spawn_batch(
    jobs: Vec<CheckJob>,
    concurrency: usize,
) -> (Receiver<CheckResult>, Arc<CheckProgress>) {
    let progress = Arc::new(CheckProgress {
        total: AtomicUsize::new(jobs.len()),
        done: AtomicUsize::new(0),
        alive: AtomicUsize::new(0),
        dead: AtomicUsize::new(0),
        errors: AtomicUsize::new(0),
        redirects: AtomicUsize::new(0),
        cancel: AtomicBool::new(false),
    });

    let (tx, rx) = mpsc::channel::<CheckResult>();
    if jobs.is_empty() {
        return (rx, progress);
    }

    let concurrency = concurrency.clamp(1, 64);
    let queue = Arc::new(Mutex::new(
        jobs.into_iter().collect::<std::collections::VecDeque<_>>(),
    ));
    let prog = Arc::clone(&progress);

    for _ in 0..concurrency {
        let queue = Arc::clone(&queue);
        let tx = tx.clone();
        let prog = Arc::clone(&prog);
        thread::spawn(move || {
            let agent = build_agent();
            loop {
                if prog.cancel.load(Ordering::Relaxed) {
                    break;
                }
                let job = {
                    let mut q = queue.lock().unwrap();
                    q.pop_front()
                };
                let Some(job) = job else { break };

                let (health, status_code, error) = check_url(&agent, &job.url);
                match health {
                    LinkHealth::Alive => {
                        prog.alive.fetch_add(1, Ordering::Relaxed);
                    }
                    LinkHealth::Dead => {
                        prog.dead.fetch_add(1, Ordering::Relaxed);
                    }
                    LinkHealth::Error => {
                        prog.errors.fetch_add(1, Ordering::Relaxed);
                    }
                    LinkHealth::Redirect => {
                        prog.redirects.fetch_add(1, Ordering::Relaxed);
                    }
                    LinkHealth::Unknown => {}
                }
                prog.done.fetch_add(1, Ordering::Relaxed);

                let _ = tx.send(CheckResult {
                    id: job.id,
                    health,
                    status_code,
                    error,
                    checked_at: Utc::now(),
                });
            }
        });
    }
    // Drop the original sender so rx closes when workers finish.
    drop(tx);

    (rx, progress)
}

pub fn default_concurrency() -> usize {
    DEFAULT_CONCURRENCY
}

/// Blocking batch check for CLI use. Returns results in completion order.
pub fn check_blocking(
    jobs: Vec<CheckJob>,
    concurrency: usize,
    on_progress: impl Fn(usize, usize, &CheckResult),
) -> Vec<CheckResult> {
    let total = jobs.len();
    let (rx, _prog) = spawn_batch(jobs, concurrency);
    let mut out = Vec::with_capacity(total);
    let mut done = 0usize;
    for result in rx {
        done += 1;
        on_progress(done, total, &result);
        out.push(result);
    }
    out
}
