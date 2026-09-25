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

    #[cfg(feature = "webkit")]
    if let Err(error) = gui::run(config) {
        eprintln!("R Browse: {error}");
        std::process::exit(1);
    }

    #[cfg(not(feature = "webkit"))]
    {
        let _ = &config;
        eprintln!("R Browse was built without WebKit; use --smoke for the core path");
    }
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
