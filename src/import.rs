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
    NoBookmarks,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::TooLarge { bytes, limit } => write!(
                f,
                "the file is {bytes} bytes, over the {limit} byte limit for an import"
            ),
            Error::NoBookmarks => write!(
                f,
                "no bookmarks were found; is this a browser's exported bookmarks file?"
            ),
        }
    }
}

/// Read a bookmark file from disk.
pub fn read_file(path: &std::path::Path) -> Result<Vec<Imported>, Error> {
    let bytes = std::fs::metadata(path).map(|meta| meta.len() as usize).unwrap_or(0);
    if bytes > MAX_BYTES {
        return Err(Error::TooLarge {
            bytes,
            limit: MAX_BYTES,
        });
    }
    let raw = std::fs::read(path).map_err(|error| Error::NoBookmarks).map_err(|_| {
        // A read failure is reported as "no bookmarks" rather than leaking a path
        // or an OS message into the summary the person reads.
        Error::NoBookmarks
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
    let mut lowest_list_depth = 0usize;
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
            ("DL", false) => {
                list_depth += 1;
                // Entering a list directly inside another list, with no folder
                // heading between them, is how the top level is written.
                if list_depth > 1 && folders.len() < depth_of_folder.len() {
                    lowest_list_depth = list_depth;
                }
                if self_closing {
                    list_depth -= 1;
                }
            }
            ("DL", true) => {
                list_depth = list_depth.saturating_sub(1);
                // Leaving a list ends the folder that list belonged to.
                while folders
                    .last()
                    .is_some_and(|_| depth_of_folder.last().is_some_and(|d| *d > list_depth))
                {
                    folders.pop();
                    depth_of_folder.pop();
                }
                lowest_list_depth = list_depth;
            }
            ("H1", false) | ("H3", false) | ("H3", true) => {
                if name == "H3" && !closing {
                    let title = text_of(tag, rest);
                    if folders.len() < MAX_FOLDER_DEPTH {
                        folders.push(title);
                        depth_of_folder.push(list_depth);
                    }
                }
            }
            ("A", false) => {
                let Some(url) = attribute(tag, "HREF") else {
                    continue;
                };
                let url = decode_entities(&url);
                let title = text_of(tag, rest);
                if found.len() >= MAX_BOOKMARKS {
                    break;
                }
                if list_depth <= lowest_list_depth {
                    // Outside any folder, or in the outermost list.
                    found.push(Imported {
                        url,
                        title,
                        folders: Vec::new(),
                    });
                } else if folders.len() >= MAX_FOLDER_DEPTH {
                    // Recorded as too deep rather than silently flattened: the
                    // count in the summary has to add up.
                    found.push(Imported {
                        url,
                        title,
                        folders: vec![String::new(); MAX_FOLDER_DEPTH + 1],
                    });
                } else {
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
fn attribute<'a>(tag: &'a str, name: &str) -> Option<String> {
    let lowered = tag.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(found) = lowered[from..].find(&format!("{name}=")) {
        let start = from + found;
        // Must be preceded by whitespace, so href= does not match xhref=.
        let preceded_ok = start == 0
            || lowered[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_whitespace());
        let after = start + name.len() + 1;
        let quote = lowered[after..].chars().next();
        if preceded_ok && (quote == Some('"') || quote == Some('\'')) {
            let quote = quote.expect("checked above");
            let value_start = after + 1;
            // The tag slice is in the original case, so search it directly.
            let value_start = tag
                .get(value_start..)
                .map(|_| value_start)
                .unwrap_or(value_start);
            let Some(end) = tag[value_start..].find(quote) else {
                return None;
            };
            return Some(tag[value_start..value_start + end].to_string());
        }
        from = start + name.len();
    }
    None
}

/// The text of an `<A>` element: what follows the tag up to `</A>`.
fn text_of(tag: &str, rest: &str) -> String {
    let end = rest.to_ascii_lowercase().find("</a").unwrap_or(0);
    let raw = &rest[..end];
    // An <A> may hold nested markup in some exports; drop the tags and keep the
    // text, which is what a bookmark title is.
    let mut text = String::with_capacity(raw.len());
    let mut depth = 0usize;
    let mut pieces = raw.split('<');
    for (index, piece) in pieces.by_ref().enumerate() {
        if index == 0 {
            text.push_str(piece);
            continue;
        }
        let Some(close) = piece.find('>') else {
            break;
        };
        let inner = &piece[..close];
        if inner.starts_with('/') {
            depth = depth.saturating_sub(1);
        } else if !inner.ends_with('/') {
            depth += 1;
        }
        // A nested tag's text is the piece after its `>`.
        if let Some(tail) = piece.get(close + 1..) {
            text.push_str(tail);
        }
    }
    let _ = depth;
    let _ = tag;
    decode_entities(text.trim())
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
