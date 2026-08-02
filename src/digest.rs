//! Launch digest — resurface a few forgotten bookmarks on open.

use crate::model::{Library, LinkHealth, Status};
use chrono::{Datelike, Utc};
use std::collections::HashSet;

const DIGEST_SIZE: usize = 3;

/// Build a small set of bookmarks worth revisiting.
///
/// Prefers: never opened, unread, not dead, older saves, diverse domains.
pub fn build_digest(library: &Library, n: usize) -> Vec<usize> {
    let n = if n == 0 { DIGEST_SIZE } else { n };
    let now = Utc::now();

    let mut scored: Vec<(i64, usize, String)> = library
        .bookmarks
        .iter()
        .enumerate()
        .filter(|(_, b)| {
            b.open_count == 0
                && b.status == Status::Unread
                && b.link_health != LinkHealth::Dead
                && !b.url.is_empty()
        })
        .map(|(i, b)| {
            let age_days = b.created.map(|c| (now - c).num_days().max(0)).unwrap_or(0);
            // Slight jitter from id so picks rotate day-to-day without a RNG crate.
            let jitter = id_jitter(&b.id, day_seed());
            // Older + unread wins; tiny jitter breaks ties.
            let score = age_days * 10 + jitter;
            (score, i, b.domain())
        })
        .collect();

    // Highest score first.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));

    let mut picked = Vec::with_capacity(n);
    let mut seen_domains = HashSet::new();

    // Pass 1: unique domains
    for (_, idx, domain) in &scored {
        if picked.len() >= n {
            break;
        }
        if seen_domains.insert(domain.clone()) {
            picked.push(*idx);
        }
    }

    // Pass 2: fill remaining
    if picked.len() < n {
        for (_, idx, _) in &scored {
            if picked.len() >= n {
                break;
            }
            if !picked.contains(idx) {
                picked.push(*idx);
            }
        }
    }

    picked
}

pub fn default_size() -> usize {
    DIGEST_SIZE
}

fn day_seed() -> u64 {
    let now = Utc::now();
    // Changes daily so digest reshuffles naturally.
    (now.year() as u64) * 1000 + (now.ordinal() as u64)
}

fn id_jitter(id: &str, seed: u64) -> i64 {
    let mut h: u64 = seed ^ 0x9e37_79b9_7f4a_7c15;
    for b in id.bytes() {
        h = h.wrapping_mul(0x100_0000_01b3).wrapping_add(b as u64);
    }
    (h % 97) as i64
}

/// Alternate seed for manual reshuffle (`z`).
pub fn build_digest_reshuffled(library: &Library, n: usize, salt: u64) -> Vec<usize> {
    let n = if n == 0 { DIGEST_SIZE } else { n };
    let now = Utc::now();

    let mut scored: Vec<(i64, usize, String)> = library
        .bookmarks
        .iter()
        .enumerate()
        .filter(|(_, b)| {
            b.open_count == 0
                && b.status == Status::Unread
                && b.link_health != LinkHealth::Dead
                && !b.url.is_empty()
        })
        .map(|(i, b)| {
            let age_days = b.created.map(|c| (now - c).num_days().max(0)).unwrap_or(0);
            let jitter = id_jitter(&b.id, day_seed() ^ salt.wrapping_mul(0x517c_c1b7));
            (age_days * 10 + jitter, i, b.domain())
        })
        .collect();

    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));

    let mut picked = Vec::with_capacity(n);
    let mut seen_domains = HashSet::new();
    for (_, idx, domain) in &scored {
        if picked.len() >= n {
            break;
        }
        if seen_domains.insert(domain.clone()) {
            picked.push(*idx);
        }
    }
    if picked.len() < n {
        for (_, idx, _) in &scored {
            if picked.len() >= n {
                break;
            }
            if !picked.contains(idx) {
                picked.push(*idx);
            }
        }
    }
    picked
}
