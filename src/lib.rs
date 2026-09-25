//! Dependency-light core of R Browse: configuration, the navigation boundary,
//! and the local session store. The GTK/libadwaita/WebKit shell lives in the
//! binary so the core can be exercised head-less by the smoke path.

pub mod bookmarks;
pub mod config;
pub mod history;
pub mod navigation;
pub mod storage;
