//! Brwsl entry point: argument handling, the offline smoke path, and the
//! native shell hand-off.

use brwsl::{config::Config, downloads, import, navigation, storage};
use serde_json::json;

#[cfg(feature = "webkit")]
mod gui;
#[cfg(feature = "webkit")]
mod library;

fn main() {
    let config = match Config::from_args() {
        Ok(config) => config,
        Err(failure) => {
            if failure.code == 0 {
                println!("{failure}");
            } else {
                eprintln!("{failure}");
            }
            std::process::exit(failure.code);
        }
    };

    if config.smoke {
        if let Err(error) = run_smoke(&config) {
            eprintln!("smoke check failed: {error}");
            std::process::exit(1);
        }
        return;
    }

    if let Some(path) = config.import_bookmarks.clone() {
        // A maintenance command: import the file, report exactly what happened,
        // and exit. It deliberately does not open a window, because the one time
        // this has to work is when the browser is closed and the profile is
        // therefore not being written by anything else.
        match run_import(&config, &path) {
            Ok(report) => println!("{report}"),
            Err(error) => {
                eprintln!("import failed: {error}");
                std::process::exit(2);
            }
        }
        return;
    }

    if config.storage_check {
        if let Err(error) = run_storage_check(&config) {
            eprintln!("storage check failed: {error}");
            std::process::exit(1);
        }
        return;
    }

    #[cfg(feature = "webkit")]
    if let Err(error) = gui::run(config) {
        eprintln!("Brwsl: {error}");
        std::process::exit(1);
    }

    #[cfg(not(feature = "webkit"))]
    {
        let _ = &config;
        eprintln!("Brwsl was built without WebKit; use --smoke or --storage-check");
    }
}

/// Import another browser's bookmark file into this profile.
///
/// Printed as JSON because this is a command a person or a script runs, and "how
/// many actually went in" is the only question that matters. The folder paths of
/// a few entries are included so it can be seen that the structure was read
/// rather than flattened.
fn run_import(config: &Config, path: &std::path::Path) -> Result<String, String> {
    config.ensure_profile_dirs()?;
    let store = storage::SessionStore::open(config.database_path())?;
    let entries = import::read_file(path).map_err(|error| error.to_string())?;
    let report = import::apply(&store, &entries);
    let mut sample = serde_json::Map::new();
    for (url, folders) in &report.sample {
        sample.insert(url.clone(), serde_json::json!(folders));
    }
    Ok(serde_json::json!({
        "added": report.summary.added,
        "duplicates": report.summary.duplicates,
        "refused": report.summary.refused,
        "failed": report.summary.failed,
        "too_deep": report.summary.too_deep,
        "folders": report.summary.folders,
        "parsed": entries.len(),
        "sample_folders": sample,
        "summary": report.summary.to_string(),
    })
    .to_string())
}

/// Exercise the profile-level stores head-lessly: bookmarks, history, and the
/// schema version. This is the contract surface the end-to-end suite asserts,
/// so it prints every intermediate result rather than a single verdict.
fn run_storage_check(config: &Config) -> Result<(), String> {
    config.ensure_profile_dirs()?;
    let store = storage::SessionStore::open(config.database_path())?;

    let page = "https://example.com/";
    let first_add = store.add_bookmark(page, "Example")?;
    let second_add = store.add_bookmark(page, "Example, renamed")?;
    let stored_title = store
        .bookmarks()?
        .first()
        .map(|bookmark| bookmark.title.clone())
        .unwrap_or_default();
    let is_bookmarked = store.is_bookmarked(page)?;
    let removed = store.remove_bookmark(page)?;
    let after_removal = store.is_bookmarked(page)?;

    let created = store.record_visit(page, "Example")?;
    let repeated = store.record_visit(page, "Example again")?;
    store.record_visit("https://second.example/", "Second")?;
    let newest = store
        .history(1)?
        .first()
        .map(|entry| entry.url.clone())
        .unwrap_or_default();
    let history_len = store.history_len()?;
    let cleared = store.clear_history()?;
    let history_after_clear = store.history_len()?;
    let rejected = store.add_bookmark("file:///etc/passwd", "nope").is_err();

    // Download names are server-controlled, so the destination policy is part
    // of the same head-less contract rather than only a GUI concern.
    let download_dir = config.download_dir().to_path_buf();
    let taken: fn(&str) -> bool = |_| false;
    let cases = [
        ("report.pdf", Some("report.pdf")),
        ("../../etc/passwd", Some("passwd")),
        ("/etc/shadow", Some("shadow")),
        (".bashrc", None),
        ("..", None),
        ("", None),
    ];
    let mut names_ok = true;
    let mut observed: Vec<(String, Option<String>)> = Vec::new();
    for (suggested, expected) in cases {
        let chosen = downloads::reserve(&download_dir, suggested, taken)
            .ok()
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            });
        names_ok &= chosen.as_deref() == expected;
        observed.push((suggested.to_string(), chosen));
    }
    let escapes = downloads::reserve(&download_dir, "../../escape.txt", taken)
        .ok()
        .and_then(|path| path.parent().map(|parent| parent == download_dir))
        .unwrap_or(false);
    names_ok &= escapes;
    // A reservation no longer creates anything: WebKit opens the destination
    // itself, so a placeholder would make the download fail. Nothing is left in
    // the download directory to clean up.

    let ok = first_add
        && !second_add
        && stored_title == "Example, renamed"
        && is_bookmarked
        && removed
        && !after_removal
        && created
        && !repeated
        && history_len == 2
        && newest == "https://second.example/"
        && cleared == 2
        && history_after_clear == 0
        && rejected
        && names_ok;

    println!(
        "{}",
        json!({
            "ok": ok,
            "schema_version": store.schema_version()?,
            "bookmark_first_add": first_add,
            "bookmark_duplicate_add": second_add,
            "bookmark_stored_title": stored_title,
            "bookmark_found": is_bookmarked,
            "bookmark_removed": removed,
            "bookmark_after_removal": after_removal,
            "history_first_visit": created,
            "history_repeat_visit": repeated,
            "history_len": history_len,
            "history_newest": newest,
            "history_cleared": cleared,
            "history_after_clear": history_after_clear,
            "bookmark_rejects_file_url": rejected,
            "download_name_policy": observed,
            "download_stays_in_profile": escapes,
        })
    );
    Ok(())
}

fn run_smoke(config: &Config) -> Result<(), String> {
    config.ensure_profile_dirs()?;
    let requested = navigation::normalize_input_with_search(
        &config.start_url,
        config.search_endpoint.as_deref(),
    )?;
    let mut store = storage::SessionStore::open(config.database_path())?;
    let restored = store.load()?.into_iter().next().map(|tab| tab.url);
    store.save_normal(&[storage::SessionTab {
        id: 1,
        position: 0,
        url: requested.clone(),
        title: "Smoke tab".to_string(),
        selected: true,
    }])?;
    let reloaded = store
        .load()?
        .into_iter()
        .next()
        .map(|tab| tab.url)
        .unwrap_or_default();
    println!(
        "{}",
        json!({
            "ok": reloaded == requested,
            "requested": requested,
            "restored": restored,
            "reloaded": reloaded,
            "profile": config.profile_dir,
        })
    );
    Ok(())
}
