//! R Browse entry point: argument handling, the offline smoke path, and the
//! native shell hand-off.

use rbrowse::{config::Config, navigation, storage};
use serde_json::json;

#[cfg(feature = "webkit")]
mod gui;

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

    if config.storage_check {
        if let Err(error) = run_storage_check(&config) {
            eprintln!("storage check failed: {error}");
            std::process::exit(1);
        }
        return;
    }

    #[cfg(feature = "webkit")]
    if let Err(error) = gui::run(config) {
        eprintln!("R Browse: {error}");
        std::process::exit(1);
    }

    #[cfg(not(feature = "webkit"))]
    {
        let _ = &config;
        eprintln!("R Browse was built without WebKit; use --smoke or --storage-check");
    }
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
        && rejected;

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
