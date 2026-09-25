//! Download destinations.
//!
//! Everything a page asks to be saved lands inside the profile's `downloads`
//! directory, never wherever the page's `Content-Disposition` header points.
//! The suggested filename is server-controlled, so it is reduced to a single
//! safe leaf name before it is joined to the destination: no path separators,
//! no parent references, and no leading dot that would hide the file.

use std::path::{Path, PathBuf};

/// Refuse anything that could escape the download directory or overwrite
/// configuration, and cap the length.
pub fn safe_leaf_name(suggested: &str) -> Option<String> {
    let leaf = suggested
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(suggested)
        .trim();
    // Control characters, path separators and parent references all mean the
    // server is trying to steer the write somewhere else.
    if leaf.is_empty()
        || leaf == "."
        || leaf == ".."
        || leaf.starts_with('.')
        || leaf
            .chars()
            .any(|character| character.is_control() || character == '/')
    {
        return None;
    }
    let name: String = leaf.chars().take(120).collect();
    let name = name.trim().to_string();
    if name.is_empty() { None } else { Some(name) }
}

/// Pick a destination inside `directory`, avoiding collisions with `taken`.
///
/// Returns `None` when the server supplied nothing usable, which the caller
/// reports as a refused download rather than guessing a name.
pub fn destination_for(
    directory: &Path,
    suggested: &str,
    taken: impl Fn(&str) -> bool,
) -> Option<PathBuf> {
    let leaf = safe_leaf_name(suggested)?;
    let candidate = directory.join(&leaf);
    if !taken(leaf.as_str()) && !candidate.exists() {
        return Some(candidate);
    }
    // Keep the extension when adding a counter so the file stays openable.
    let (stem, extension) = split_extension(&leaf);
    for index in 1..1000 {
        let numbered = match extension.as_deref() {
            Some(extension) => format!("{stem} ({index}).{extension}"),
            None => format!("{stem} ({index})"),
        };
        let candidate = directory.join(&numbered);
        if !taken(numbered.as_str()) && !candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn split_extension(leaf: &str) -> (String, Option<String>) {
    match leaf.rsplit_once('.') {
        // A leading dot was already rejected, so this is a real extension.
        Some((stem, extension)) if !stem.is_empty() && !extension.is_empty() => {
            (stem.to_string(), Some(extension.to_string()))
        }
        _ => (leaf.to_string(), None),
    }
}
