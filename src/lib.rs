//! Dependency-light core of Browsrl: configuration, the navigation boundary,
//! and the local session store. The GTK/libadwaita/WebKit shell lives in the
//! binary so the core can be exercised head-less by the smoke path.

pub mod bookmarks;
pub mod config;
pub mod downloads;
pub mod history;
pub mod import;
pub mod navigation;
#[cfg(feature = "webkit")]
pub mod readability;
pub mod storage;
