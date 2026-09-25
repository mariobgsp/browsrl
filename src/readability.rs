//! Readability pass.
//!
//! WebKitGTK 6.0 removed `webkit_web_view_set_reader_mode`, so this is not the
//! engine's article extractor. It is a user style sheet applied by the shell:
//! no script is injected into the page, nothing is fetched, and the page keeps
//! running exactly as it did.
//!
//! `WebKitWebView:user-content-manager` is construct-only, so every view is
//! built with its own empty manager and readability is toggled by adding and
//! removing a sheet on that manager. The page is never reloaded to switch it
//! on or off.

use webkit6::{UserContentInjectedFrames, UserContentManager, UserStyleLevel, UserStyleSheet};

const READABILITY_CSS: &str = "\
html { max-width: 42rem; margin: 0 auto; padding: 1.5rem 1rem 6rem; }
body { font-size: 1.125rem; line-height: 1.7; }
h1, h2, h3, h4 { line-height: 1.25; text-wrap: balance; }
h1 { font-size: 2rem; }
h2 { font-size: 1.5rem; }
h3 { font-size: 1.25rem; }
p, li { text-wrap: pretty; }
img, video, svg, canvas { max-width: 100%; height: auto; }
nav, aside, footer, [role='banner'], [role='navigation'], [role='complementary'],
[role='contentinfo'], .sidebar, .comment, .comments, .recommendations,
.related, .newsletter, .cookie-banner {
  display: none !important;
}
@media print { html { max-width: none; } }
";

// Deliberately no colour or background rules. A page that sets its own
// background fights them and ends up with unreadable text, which a screenshot
// of this pass showed before they were removed. Measure, type size and hiding
// page furniture are the parts that actually help.

/// An empty manager for one view, to which the readability sheet is added when
/// the person asks for it.
pub fn manager() -> UserContentManager {
    UserContentManager::new()
}

pub fn sheet() -> UserStyleSheet {
    UserStyleSheet::new(
        READABILITY_CSS,
        UserContentInjectedFrames::TopFrame,
        UserStyleLevel::User,
        &[],
        &[],
    )
}
