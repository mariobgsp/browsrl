//! Browsing history: where the shell observed a completed page load.
//!
//! History is written by the shell when WebKit reports a finished load, not by
//! script injected into the page, so it records navigation the browser actually
//! performed. Consecutive visits to the same URL collapse into one entry, and
//! the table is capped so an unbounded profile cannot grow without limit.

use crate::navigation;
use rusqlite::{Connection, OptionalExtension, params};
use std::time::{SystemTime, UNIX_EPOCH};

/// Upper bound on stored history rows. Old rows are pruned on write.
pub const MAX_ENTRIES: i64 = 5_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryEntry {
    pub id: i64,
    pub url: String,
    pub title: String,
    pub visited_at: i64,
}

/// Record a completed load.
///
/// Returns `true` when a new entry was created and `false` when the visit
/// collapsed into the existing entry for that URL.
pub fn record(connection: &Connection, url: &str, title: &str) -> Result<bool, String> {
    // Blank and internal pages are navigation, not browsing.
    if url == "about:blank" {
        return Ok(false);
    }
    let url = navigation::validate_explicit_url(url)
        .map_err(|error| format!("history URL is not allowed: {error}"))?;
    let title = normalize_title(title);
    let visited_at = now_seconds();

    let previous: Option<i64> = connection
        .query_row(
            "SELECT id FROM history WHERE url = ?1 ORDER BY id DESC LIMIT 1",
            [&url],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("look up previous visit: {error}"))?;
    let created = match previous {
        Some(id) => {
            connection
                .execute(
                    "UPDATE history SET title = ?2, visited_at = ?3 WHERE id = ?1",
                    params![id, title, visited_at],
                )
                .map_err(|error| format!("refresh history entry: {error}"))?;
            false
        }
        None => {
            connection
                .execute(
                    "INSERT INTO history(url, title, visited_at) VALUES (?1, ?2, ?3)",
                    params![url, title, visited_at],
                )
                .map_err(|error| format!("insert history entry: {error}"))?;
            true
        }
    };
    prune(connection)?;
    Ok(created)
}

pub fn list(connection: &Connection, limit: i64) -> Result<Vec<HistoryEntry>, String> {
    let limit = limit.clamp(1, MAX_ENTRIES);
    let mut statement = connection
        .prepare("SELECT id, url, title, visited_at FROM history ORDER BY visited_at DESC, id DESC LIMIT ?1")
        .map_err(|error| format!("prepare history query: {error}"))?;
    let rows = statement
        .query_map([limit], |row| {
            Ok(HistoryEntry {
                id: row.get(0)?,
                url: row.get(1)?,
                title: row.get(2)?,
                visited_at: row.get(3)?,
            })
        })
        .map_err(|error| format!("read history: {error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("decode history: {error}"))
}

pub fn clear(connection: &Connection) -> Result<usize, String> {
    let removed = connection
        .execute("DELETE FROM history", [])
        .map_err(|error| format!("clear history: {error}"))?;
    Ok(removed)
}

pub fn count(connection: &Connection) -> Result<i64, String> {
    connection
        .query_row("SELECT COUNT(*) FROM history", [], |row| row.get(0))
        .map_err(|error| format!("count history: {error}"))
}

fn prune(connection: &Connection) -> Result<(), String> {
    let total = count(connection)?;
    if total <= MAX_ENTRIES {
        return Ok(());
    }
    connection
        .execute(
            "DELETE FROM history WHERE id NOT IN (
                 SELECT id FROM history ORDER BY visited_at DESC, id DESC LIMIT ?1
             )",
            [MAX_ENTRIES],
        )
        .map_err(|error| format!("prune history: {error}"))?;
    Ok(())
}

fn normalize_title(title: &str) -> String {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        "Untitled page".to_string()
    } else {
        trimmed.chars().take(200).collect()
    }
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}
