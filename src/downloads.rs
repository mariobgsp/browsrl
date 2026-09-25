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

/// Reserve a free destination inside `directory` for a download.
///
/// The name is chosen and *reserved* here rather than merely checked, so two
/// downloads of the same file name cannot both be handed the same path: the
/// reservation is an empty file that WebKit then writes over.
pub fn reserve(
    directory: &Path,
    suggested: &str,
    mut is_taken: impl FnMut(&str) -> bool,
) -> Result<PathBuf, String> {
    let leaf = safe_leaf_name(suggested).ok_or_else(|| {
        "the server supplied no usable download name, so the download was refused".to_string()
    })?;
    let (stem, extension) = split_extension(&leaf);
    for index in 0..1000u32 {
        let name = if index == 0 {
            leaf.clone()
        } else {
            match extension.as_deref() {
                Some(extension) => format!("{stem} ({index}).{extension}"),
                None => format!("{stem} ({index})"),
            }
        };
        if is_taken(&name) {
            continue;
        }
        let candidate = directory.join(&name);
        match reserve_file(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "cannot reserve a download file in {}: {error}",
                    directory.display()
                ));
            }
        }
    }
    Err("no free download name was found".to_string())
}

#[cfg(unix)]
fn reserve_file(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    // 0600: a download can be anything, so it is not world readable even
    // inside the profile.
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map(|_| ())
}

#[cfg(not(unix))]
fn reserve_file(path: &Path) -> std::io::Result<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map(|_| ())
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
