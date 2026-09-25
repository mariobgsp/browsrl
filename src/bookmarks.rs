//! Bookmark storage: the URLs a person asked the browser to remember.
//!
//! Bookmarks are explicit, deduplicated by URL, and never created implicitly by
//! page loads. Only URLs the navigation layer accepts may be stored, so a
//! bookmark can never reintroduce a scheme that was rejected on the way in.

use crate::navigation;
use rusqlite::{Connection, params};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bookmark {
    pub url: String,
    pub title: String,
    pub created_at: i64,
}

pub fn validate(url: &str) -> Result<String, String> {
    navigation::validate_explicit_url(url)
        .map_err(|error| format!("bookmark URL is not allowed: {error}"))
}

pub fn add(connection: &Connection, url: &str, title: &str) -> Result<bool, String> {
    let url = validate(url)?;
    let title = normalize_title(title);
    let existing: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM bookmarks WHERE url = ?1",
            [&url],
            |row| row.get(0),
        )
        .map_err(|error| format!("look up bookmark: {error}"))?;
    if existing > 0 {
        // Keep the newest title rather than duplicating the row.
        connection
            .execute(
                "UPDATE bookmarks SET title = ?2 WHERE url = ?1",
                params![url, title],
            )
            .map_err(|error| format!("refresh bookmark title: {error}"))?;
        return Ok(false);
    }
    connection
        .execute(
            "INSERT INTO bookmarks(url, title, created_at) VALUES (?1, ?2, ?3)",
            params![url, title, now_seconds()],
        )
        .map_err(|error| format!("insert bookmark: {error}"))?;
    Ok(true)
}

pub fn remove(connection: &Connection, url: &str) -> Result<bool, String> {
    let url = validate(url)?;
    let removed = connection
        .execute("DELETE FROM bookmarks WHERE url = ?1", [&url])
        .map_err(|error| format!("delete bookmark: {error}"))?;
    Ok(removed > 0)
}

pub fn contains(connection: &Connection, url: &str) -> Result<bool, String> {
    let url = validate(url)?;
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM bookmarks WHERE url = ?1",
            [&url],
            |row| row.get(0),
        )
        .map_err(|error| format!("look up bookmark: {error}"))?;
    Ok(count > 0)
}

pub fn list(connection: &Connection) -> Result<Vec<Bookmark>, String> {
    let mut statement = connection
        .prepare("SELECT url, title, created_at FROM bookmarks ORDER BY title COLLATE NOCASE, url")
        .map_err(|error| format!("prepare bookmark query: {error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok(Bookmark {
                url: row.get(0)?,
                title: row.get(1)?,
                created_at: row.get(2)?,
            })
        })
        .map_err(|error| format!("read bookmarks: {error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("decode bookmarks: {error}"))
}

fn normalize_title(title: &str) -> String {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        "Untitled bookmark".to_string()
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
