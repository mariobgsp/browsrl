use rusqlite::{Connection, OptionalExtension, params};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use url::Url;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use crate::{bookmarks, history};

/// Bumped whenever the tables below change; `migrate_schema` refuses to open a
/// database written by a newer build.
const SCHEMA_VERSION: i64 = 2;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionTab {
    pub id: i64,
    pub position: i64,
    pub url: String,
    pub title: String,
    pub selected: bool,
}

#[derive(Debug)]
pub struct SessionStore {
    connection: Connection,
}

impl SessionStore {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent()
            && !parent.exists()
        {
            fs::create_dir_all(parent)
                .map_err(|error| format!("create profile directory: {error}"))?;
            // Only directories this process created are hardened; an existing
            // one may legitimately be shared with the user.
            ensure_private_directory(parent)?;
        }
        create_private_file(path)?;
        let mut connection = Connection::open(path)
            .map_err(|error| describe_open_error(path, &error.to_string()))?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(|error| format!("configure session database: {error}"))?;
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA foreign_keys = ON;",
            )
            .map_err(|error| describe_open_error(path, &error.to_string()))?;
        migrate_schema(&mut connection)?;
        protect_database_files(path)?;
        Ok(Self { connection })
    }

    pub fn load(&self) -> Result<Vec<SessionTab>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, position, url, title, selected
                 FROM session_tabs
                 ORDER BY position ASC, id ASC",
            )
            .map_err(|error| format!("prepare session query: {error}"))?;
        let rows = statement
            .query_map([], |row| {
                Ok(SessionTab {
                    id: row.get(0)?,
                    position: row.get(1)?,
                    url: row.get(2)?,
                    title: row.get(3)?,
                    selected: row.get::<_, i64>(4)? != 0,
                })
            })
            .map_err(|error| format!("read session: {error}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("decode session: {error}"))
    }

    pub fn save_normal(&mut self, records: &[SessionTab]) -> Result<(), String> {
        let mut records = records.to_vec();
        records.sort_by_key(|record| record.position);
        for record in &records {
            validate_record(record)?;
        }
        // A session always has a selected tab, so fall back to the first one
        // rather than indexing (which could panic on an empty slice).
        let has_selection = records.iter().any(|record| record.selected);
        if let (false, Some(first)) = (
            has_selection,
            records.iter_mut().find(|record| !record.selected),
        ) {
            first.selected = true;
        }

        let transaction = self
            .connection
            .transaction()
            .map_err(|error| format!("begin session transaction: {error}"))?;
        transaction
            .execute("DELETE FROM session_tabs", [])
            .map_err(|error| format!("clear session transaction: {error}"))?;
        {
            let mut statement = transaction
                .prepare(
                    "INSERT INTO session_tabs(id, position, url, title, selected)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                )
                .map_err(|error| format!("prepare session write: {error}"))?;
            for (position, record) in records.iter().enumerate() {
                statement
                    .execute(params![
                        record.id,
                        position as i64,
                        record.url,
                        record.title,
                        i64::from(record.selected),
                    ])
                    .map_err(|error| format!("write session row: {error}"))?;
            }
        }
        transaction
            .commit()
            .map_err(|error| format!("commit session transaction: {error}"))
    }

    // ---- Bookmarks ---------------------------------------------------------

    /// Bookmark a URL. Returns `false` when it was already bookmarked, in
    /// which case the stored title is refreshed instead.
    pub fn add_bookmark(&self, url: &str, title: &str) -> Result<bool, String> {
        bookmarks::add(&self.connection, url, title)
    }

    pub fn remove_bookmark(&self, url: &str) -> Result<bool, String> {
        bookmarks::remove(&self.connection, url)
    }

    pub fn is_bookmarked(&self, url: &str) -> Result<bool, String> {
        bookmarks::contains(&self.connection, url)
    }

    pub fn bookmarks(&self) -> Result<Vec<bookmarks::Bookmark>, String> {
        bookmarks::list(&self.connection)
    }

    // ---- History -----------------------------------------------------------

    /// Record a completed page load. Returns `true` for a new entry and
    /// `false` when the visit collapsed into an existing one.
    pub fn record_visit(&self, url: &str, title: &str) -> Result<bool, String> {
        history::record(&self.connection, url, title)
    }

    pub fn history(&self, limit: i64) -> Result<Vec<history::HistoryEntry>, String> {
        history::list(&self.connection, limit)
    }

    pub fn history_len(&self) -> Result<i64, String> {
        history::count(&self.connection)
    }

    pub fn clear_history(&self) -> Result<usize, String> {
        history::clear(&self.connection)
    }

    pub fn schema_version(&self) -> Result<i64, String> {
        self.connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|error| format!("read schema version: {error}"))
    }
}

fn migrate_schema(connection: &mut Connection) -> Result<(), String> {
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| format!("read schema version: {error}"))?;
    if version > SCHEMA_VERSION {
        return Err(format!(
            "session database schema version {version} is newer than supported version {SCHEMA_VERSION}"
        ));
    }

    let transaction = connection
        .transaction()
        .map_err(|error| format!("begin schema migration: {error}"))?;
    transaction
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS session_tabs (
                 id INTEGER PRIMARY KEY,
                 position INTEGER NOT NULL CHECK (position >= 0),
                 url TEXT NOT NULL,
                 title TEXT NOT NULL,
                 selected INTEGER NOT NULL CHECK (selected IN (0, 1))
             );
             CREATE INDEX IF NOT EXISTS session_tabs_position
                 ON session_tabs(position);
             CREATE TABLE IF NOT EXISTS bookmarks (
                 url TEXT PRIMARY KEY,
                 title TEXT NOT NULL,
                 created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS history (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 url TEXT NOT NULL,
                 title TEXT NOT NULL,
                 visited_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS history_visited_at
                 ON history(visited_at DESC);
             CREATE INDEX IF NOT EXISTS history_url
                 ON history(url);",
        )
        .map_err(|error| format!("create session schema: {error}"))?;

    let legacy_exists: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'session')",
            [],
            |row| Ok(row.get::<_, i64>(0)? != 0),
        )
        .map_err(|error| format!("inspect legacy session schema: {error}"))?;
    if legacy_exists {
        let existing_rows: i64 = transaction
            .query_row("SELECT COUNT(*) FROM session_tabs", [], |row| row.get(0))
            .map_err(|error| format!("count migrated session rows: {error}"))?;
        if existing_rows == 0 {
            let legacy_url: Option<String> = transaction
                .query_row("SELECT url FROM session WHERE id = 1", [], |row| row.get(0))
                .optional()
                .map_err(|error| format!("read legacy session: {error}"))?;
            if let Some(url) = legacy_url {
                let url = if is_allowed_url(&url) {
                    url
                } else {
                    "about:blank".to_string()
                };
                transaction
                    .execute(
                        "INSERT INTO session_tabs(id, position, url, title, selected)
                         VALUES (1, 0, ?1, 'Restored Tab', 1)",
                        [&url],
                    )
                    .map_err(|error| format!("migrate legacy session: {error}"))?;
            }
        }
        transaction
            .execute("DROP TABLE session", [])
            .map_err(|error| format!("remove legacy session table: {error}"))?;
    }
    transaction
        .pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(|error| format!("set schema version: {error}"))?;
    transaction
        .commit()
        .map_err(|error| format!("commit schema migration: {error}"))
}

fn validate_record(record: &SessionTab) -> Result<(), String> {
    if record.id <= 0 {
        return Err("session tab id must be positive".to_string());
    }
    if record.position < 0 {
        return Err("session tab position cannot be negative".to_string());
    }
    if !is_allowed_url(&record.url) {
        return Err(format!("session tab URL is not allowed: {}", record.url));
    }
    Ok(())
}

fn is_allowed_url(value: &str) -> bool {
    if value == "about:blank" {
        return true;
    }
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    !host.is_empty()
        && !host.chars().any(|character| {
            character.is_whitespace()
                || character.is_control()
                || matches!(character, '/' | '\\' | '?' | '#' | '@')
        })
}

/// Turn a low-level SQLite failure into something a person can act on. The
/// common one is a second process writing the same profile, which Browsrl
/// does not support: the GUI is single-instance through GApplication.
fn describe_open_error(path: &Path, message: &str) -> String {
    if message.contains("locked") || message.contains("busy") {
        return format!(
            "session database {} is in use by another Browsrl process; \
             close it or use a different --profile-dir",
            path.display()
        );
    }
    format!("open session database {}: {message}", path.display())
}

/// Create the database file with owner-only permissions before SQLite opens
/// it, so it is never briefly readable by other local users.
#[cfg(unix)]
fn create_private_file(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;

    if path.exists() {
        return Ok(());
    }
    std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .map(|_| ())
        .map_err(|error| format!("create session database {}: {error}", path.display()))
}

#[cfg(not(unix))]
fn create_private_file(path: &Path) -> Result<(), String> {
    if !path.exists() {
        std::fs::File::create(path)
            .map(|_| ())
            .map_err(|error| format!("create session database {}: {error}", path.display()))?;
    }
    Ok(())
}

#[cfg(unix)]
fn ensure_private_directory(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("protect directory {}: {error}", path.display()))
}

#[cfg(not(unix))]
fn ensure_private_directory(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// Keep the database and its write-ahead log readable only by the owner.
///
/// The `-wal` and `-shm` sidecars hold the same URLs and titles as the
/// database, and SQLite creates them with the process umask rather than with
/// the database file's mode, so they are hardened explicitly.
#[cfg(unix)]
fn protect_database_files(path: &Path) -> Result<(), String> {
    for name in [
        path.to_path_buf(),
        sidecar(path, "-wal"),
        sidecar(path, "-shm"),
    ] {
        if !name.exists() {
            continue;
        }
        fs::set_permissions(&name, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("protect session database {}: {error}", name.display()))?;
    }
    Ok(())
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

#[cfg(not(unix))]
fn protect_database_files(_path: &Path) -> Result<(), String> {
    Ok(())
}
