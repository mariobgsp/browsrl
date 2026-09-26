//! Reading bookmarks from another browser's export file.
//!
//! Every mainstream browser exports the same format, called a Netscape
//! bookmark file: nested `<DL>` lists, bookmarks as `<DT><A HREF="...">`, and
//! folders as `<DT><H3>`. It is written by machines but read by people, so this
//! parser is deliberately tolerant - it scans for the tags it cares about rather
//! than trusting the line layout - and deliberately bounded, because the file is
//! untrusted input: a size cap and a bookmark cap mean a hostile or corrupt file
//! cannot make the process eat memory.
//!
//! Only web URLs survive. A bookmark file is a good way to smuggle a `file:`
//! or `javascript:` URL into a browser, so the same rule the store already
//! applies is applied here before anything is written.

use std::fmt;

/// Most export files are a few hundred kilobytes; anything past this is either
/// not a bookmark file or an attempt to exhaust memory.
const MAX_BYTES: usize = 32 * 1024 * 1024;
/// Enough for any real bookmark collection, and a hard stop regardless.
const MAX_BOOKMARKS: usize = 50_000;
/// Folder names are kept for grouping; a path deeper than this is not real.
const MAX_FOLDER_DEPTH: usize = 16;

/// One bookmark as it appeared in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Imported {
    pub url: String,
    pub title: String,
    /// Enclosing folder names, outermost first, empty for a top-level bookmark.
    pub folders: Vec<String>,
}

/// What an import did, so the caller can report it instead of guessing.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    /// Written to the store.
    pub added: usize,
    /// Already present, by URL.
    pub duplicates: usize,
    /// Refused: not http, not https, or unusable.
    pub refused: usize,
    /// Could not be written for a reason that was not a refusal, such as a full
    /// disk. Kept apart from `refused` so a storage fault is never reported as a
    /// policy decision.
    pub failed: usize,
    /// Found in the file but nested too deeply to keep.
    pub too_deep: usize,
    /// Folders seen, for reporting. Folders are not stored yet.
    pub folders: usize,
}

impl fmt::Display for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} bookmark(s) added, {} already present, {} refused, {} nested too deep, in {} folder(s)",
            self.added, self.duplicates, self.refused, self.too_deep, self.folders
        )
    }
}

/// Why a file could not be read at all.
#[derive(Debug)]
pub enum Error {
    TooLarge { bytes: usize, limit: usize },
    Unreadable { path: String, reason: String },
    NoBookmarks,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::TooLarge { bytes, limit } => write!(
                f,
                "the file is {bytes} bytes, over the {limit} byte limit for an import"
            ),
            Error::Unreadable { path, reason } => {
                write!(f, "could not read {path}: {reason}")
            }
            Error::NoBookmarks => write!(
                f,
                "no bookmarks were found; is this a browser's exported bookmarks file?"
            ),
        }
    }
}

/// What an import did, in enough detail to be reported to a person.
#[derive(Debug, Default)]
pub struct Report {
    pub summary: Summary,
    /// Folder paths for the first few entries, so the caller can show that the
    /// folder structure was read rather than flattened away.
    pub sample: Vec<(String, Vec<String>)>,
}

/// Write parsed bookmarks into a store, counting what happened to each.
///
/// Duplicates and refusals are counted rather than hidden, and one bad entry
/// never stops the rest: a person's bookmark file is the only copy they have of
/// that list, so a single `file:` URL in it must not cost them the other four
/// hundred.
pub fn apply(store: &crate::storage::SessionStore, entries: &[Imported]) -> Report {
    let mut report = Report::default();
    let mut folder_names: Vec<String> = Vec::new();

    for entry in entries {
        for folder in &entry.folders {
            if !folder.is_empty() && !folder_names.contains(folder) {
                folder_names.push(folder.clone());
            }
        }
        if entry.folders.len() > MAX_FOLDER_DEPTH {
            report.summary.too_deep += 1;
            continue;
        }
        if report.sample.len() < 8 {
            report
                .sample
                .push((entry.url.clone(), entry.folders.clone()));
        }
        // The store validates the URL, so the rule that refuses a bookmark typed
        // by hand refuses one that arrived in a file; there is deliberately no
        // second check here to drift out of step with it. A failure that is not
        // a refusal - a full disk, a locked database - is reported rather than
        // quietly counted as a refused bookmark.
        match store.add_bookmark(&entry.url, &entry.title) {
            Ok(true) => report.summary.added += 1,
            // `false` means the store already had this URL.
            Ok(false) => report.summary.duplicates += 1,
            Err(error) => {
                if crate::bookmarks::validate(&entry.url).is_ok() {
                    eprintln!("could not store bookmark {}: {error}", entry.url);
                    report.summary.failed += 1;
                } else {
                    report.summary.refused += 1;
                }
            }
        }
    }
    report.summary.folders = folder_names.len();
    report
}

/// Read a bookmark file from disk.
pub fn read_file(path: &std::path::Path) -> Result<Vec<Imported>, Error> {
    let bytes = std::fs::metadata(path)
        .map(|meta| meta.len() as usize)
        .unwrap_or(0);
    if bytes > MAX_BYTES {
        return Err(Error::TooLarge {
            bytes,
            limit: MAX_BYTES,
        });
    }
    // A path that cannot be read is reported as such rather than as "no
    // bookmarks", which would send a person looking for the wrong problem.
    let raw = std::fs::read(path).map_err(|error| Error::Unreadable {
        path: path.display().to_string(),
        reason: error.to_string(),
    })?;
    parse(&String::from_utf8_lossy(&raw))
}

/// Parse bookmark-file HTML into bookmarks, keeping folder paths.
pub fn parse(input: &str) -> Result<Vec<Imported>, Error> {
    if input.len() > MAX_BYTES {
        return Err(Error::TooLarge {
            bytes: input.len(),
            limit: MAX_BYTES,
        });
    }
    let mut found: Vec<Imported> = Vec::new();
    // Folder stack: the innermost `<DL>` is the last entry.
    let mut folders: Vec<String> = Vec::new();
    let mut depth_of_folder: Vec<usize> = Vec::new();
    let mut list_depth = 0usize;
    let mut rest = input;

    while let Some(open) = rest.find('<') {
        rest = &rest[open + 1..];
        let Some(close) = rest.find('>') else {
            // A tag that never closes: the file is truncated, so stop cleanly
            // with whatever was understood rather than discarding it.
            break;
        };
        let tag = &rest[..close];
        rest = &rest[close + 1..];
        let name = tag_name(tag);
        let closing = tag.starts_with('/');
        let self_closing = tag.ends_with('/');

        match (name.as_str(), closing) {
            ("dl", false) => {
                list_depth += 1;
                if self_closing {
                    list_depth -= 1;
                }
            }
            ("dl", true) => {
                list_depth = list_depth.saturating_sub(1);
                // Leaving a list ends every folder whose heading was inside it: a
                // heading at depth D owns the list at depth D+1, so when that
                // list closes the heading is finished. Comparing with `>=` rather
                // than `>` is what stops a sibling folder inheriting the folder
                // before it, which is how "Dashboard" ended up filed under both
                // "Reading list" and "Work".
                while depth_of_folder
                    .last()
                    .is_some_and(|depth| *depth >= list_depth)
                {
                    folders.pop();
                    depth_of_folder.pop();
                }
            }
            ("h1", false) | ("h3", false) | ("h3", true) => {
                if name == "h3" && !closing {
                    let title = text_until(rest, "h3");
                    if folders.len() < MAX_FOLDER_DEPTH {
                        folders.push(title);
                        depth_of_folder.push(list_depth);
                    }
                }
            }
            ("a", false) => {
                let Some(url) = attribute(tag, "HREF") else {
                    continue;
                };
                let url = decode_entities(&url);
                let title = text_until(rest, "a");
                if found.len() >= MAX_BOOKMARKS {
                    break;
                }
                if folders.len() >= MAX_FOLDER_DEPTH {
                    // Recorded as too deep rather than silently flattened: the
                    // count in the summary has to add up.
                    found.push(Imported {
                        url,
                        title,
                        folders: vec![String::new(); MAX_FOLDER_DEPTH + 1],
                    });
                } else {
                    // An empty stack is a bookmark outside any folder, which is
                    // exactly what the top level of the file is.
                    found.push(Imported {
                        url,
                        title,
                        folders: folders.clone(),
                    });
                }
            }
            _ => {}
        }
    }

    if found.is_empty() {
        return Err(Error::NoBookmarks);
    }
    Ok(found)
}

/// The lowercased tag name of a tag body, without `/` or attributes.
fn tag_name(tag: &str) -> String {
    tag.trim_start_matches('/')
        .split(|c: char| c.is_whitespace() || c == '/')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// The value of an attribute, exactly as written between its quotes.
///
/// The search is for the lowercased name because exports disagree about case
/// (`HREF=` in Netscape files, `href=` elsewhere) and the tag was lowercased for
/// matching. `href=` must not match `xhref=`, so the name has to start a word.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let lowered = tag.to_ascii_lowercase();
    let needle = format!("{}=", name.to_ascii_lowercase());
    let mut from = 0usize;
    while let Some(found) = lowered.get(from..).and_then(|rest| rest.find(&needle)) {
        let start = from + found;
        let after = start + needle.len();
        let starts_word = start == 0
            || lowered[..start]
                .chars()
                .next_back()
                .is_some_and(|character| character.is_whitespace());
        let quote = lowered.get(after..).and_then(|rest| rest.chars().next());
        if let (true, Some(quote @ ('"' | '\''))) = (starts_word, quote) {
            let value_start = after + 1;
            let end = tag.get(value_start..).and_then(|rest| rest.find(quote))?;
            return Some(tag[value_start..value_start + end].to_string());
        }
        from = after;
    }
    None
}

/// The text of the element whose closing tag is `closing`.
///
/// Stops at that element's own closing tag, so an `<H3>` heading does not run on
/// to the next bookmark's `</A>`, and drops any nested markup, because an export
/// may put a tag inside a title. Whitespace inside the text is collapsed, which
/// is what the HTML it came from meant anyway.
fn text_until(rest: &str, closing: &str) -> String {
    let lowered = rest.to_ascii_lowercase();
    let needle = format!("</{closing}");
    let end = lowered.find(&needle).unwrap_or(0);
    let raw = &rest[..end];
    let mut text = String::with_capacity(raw.len());
    let mut inside_tag = false;
    for character in raw.chars() {
        match character {
            '<' => inside_tag = true,
            '>' => inside_tag = false,
            _ if !inside_tag => text.push(character),
            _ => {}
        }
    }
    decode_entities(text.split_whitespace().collect::<Vec<_>>().join(" ").trim())
}

/// Decode the HTML entities an export can contain in a URL or a title.
fn decode_entities(input: &str) -> String {
    if !input.contains('&') {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start + 1..];
        let Some(end) = rest[..rest.len().min(12)].find(';') else {
            // A bare `&` is legal text; keep it.
            out.push('&');
            continue;
        };
        let entity = &rest[..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            other => other
                .strip_prefix('#')
                .and_then(|digits| match digits.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => digits.parse::<u32>().ok(),
                })
                .and_then(char::from_u32),
        };
        match decoded {
            Some(character) => {
                out.push(character);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[start + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}
