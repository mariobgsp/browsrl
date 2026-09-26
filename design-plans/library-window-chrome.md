# The library panels wear the same chrome as the browsing window

Written against: d24e392

## Evidence chain

- Surface: `src/library.rs`, the bookmarks and history panels, as rendered.
  OCR of a live panel reads `Brwsl — Bookmarks` in a plain OS title bar, and
  nothing else above the list: no in-app title bar, no close control, no
  place the panel's own controls live except a strip under the list.
- Problem: the two panels are the only windows in the app without a designed
  chrome. `library::open` builds an `adw::Window` whose content is a bare
  `gtk::Box` — a scroller with 12 px margins, plus, for History, a second box
  holding a `Clear history` text button. There is no `adw::HeaderBar`, so the
  window falls back to a plain title bar, which is a different visual language
  from the browsing window the person just came from. The same code also composes
  the title in the opposite order from the browsing window: the panel says
  `Brwsl — Bookmarks`, the browsing window says `<page> — Brwsl`.
- Design evidence: the app's own header bar is the owner of this decision and is
  already built two files away, in `build_window` (`src/gui.rs`), where
  `adw::HeaderBar::new()` plus `pack_start` is how a control enters a window's
  chrome. The title convention is the one the browsing window documents in
  `gui.rs`: "The window title follows the page in front, the way every browser
  does", implemented as `format!("{title} — Brwsl")`.
- Owner: `library::open` (`src/library.rs`), the only place both panels are built.
- Scope and affected surfaces: `Ctrl+Shift+B` (bookmarks) and `Ctrl+H`
  (history), and the single-instance behaviour the module relies on (the window
  is registered with the application so a second shortcut raises it instead of
  stacking copies).
- Uncertainty: none about the correction. The only judgement is whether the
  `Clear history` control belongs in the header or under the list; the header
  is where a window puts its controls, and the strip under the list exists only
  because the window has no header.

## Design decision

Build each panel the way the browsing window is built: an `adw::HeaderBar` on
top with the panel's name as its title widget, content below. For History, the
`Clear history` button moves from the strip under the list into the header bar's
end, which is the only place a window-level control belongs — that is what
removes the second band. The window title becomes `<panel> — Brwsl`, matching
the browsing window's order, so the two windows of the same app title themselves
the same way.

Nothing about the list changes: same `adw::ActionRow` entries, same titles and
subtitles, same URL tooltips, same activation, same empty-state copy, same
scroller margins.

## Reuse

- `adw::HeaderBar::new()` + `pack_start`/`pack_end` for controls.
  Exemplar: `src/gui.rs`, `build_window` — the browsing window's own chrome.
- `adw::WindowTitle` for the panel name (the standard libadwaita title widget);
  otherwise a plain `gtk::Label` in `set_title_widget` is acceptable, but
  `adw::WindowTitle` is the type the platform styles as a header title.
- `format!("{title} — Brwsl")`, the browsing window's exact title format.
  Exemplar: `src/gui.rs`, the `LoadEvent::Finished` branch.
- `LibraryKind::title()` (`"Bookmarks"` / `"History"`) — the panel's own name
  already exists and is the string both the header title and the window title
  should use, so the name lives in one place.

No new primitive, no stylesheet, no new string.

## Changes

1. `src/library.rs` → `library::open`, window construction
   - Change: build an `adw::HeaderBar` for the window, set the panel name as its
     title widget, and make the window's content a `gtk::Box` holding the header
     bar then the scroller. Keep `adw::Window::builder()`'s application
     registration, the `default_width(560)` / `default_height(520)`, the
     `install_shifted_letter_shortcuts` call, and the scroller's 12 px margins.
   - Preserve: the shifted-letter key controller on this window (its comment
     records that without it `Ctrl+Shift+B` silently did nothing with a panel in
     front), `on_open` on activation, and the window closing itself after a
     choice.
   - Verify: a panel shows a header bar with `Bookmarks` / `History` in it, the
     same bar style as the browsing window, and the window title reads
     `Bookmarks — Brwsl`.

2. `src/library.rs` → the History panel's control
   - Change: move the existing `clear` button (keeping its label, its handler,
     its `populate` repopulate call, and its `eprintln!` reporting) out of the
     box under the list and into the header bar's `pack_end`. Delete that box and
     its 6/6/12/12 px margins with it. A destructive action may keep its word
     label; the icons-not-words rule applies to the navigation row, and this
     control is not part of it.
   - Preserve: clearing removes every history row and the list repopulates in
     place, the empty-state copy after clearing, and the absence of a clear
     control on the Bookmarks panel.
   - Verify: `make e2e-gui` passes `clear_browsing_data_erases_history` and
     `ctrl_shift_b_opens_the_bookmarks_window`; the History panel's only control
     sits in the header, and the list now reaches the bottom edge.

3. `src/library.rs` → window title
   - Change: `format!("Brwsl — {}", kind.title())` becomes
     `format!("{} — Brwsl", kind.title())`, the browsing window's order.
   - Preserve: the panel name itself, from `LibraryKind::title()`.
   - Verify: OCR of the rendered panel title reads `Bookmarks — Brwsl`.

4. `README.md`
   - Change: the features table's Library row says "Bookmark and history
     windows from the toolbar actions". Neither bar carries a control for these
     actions, and the app has no menu (`grep -rn "menubar\|MenuModel" src/` is
     empty), so today they are keyboard-only (`Ctrl+Shift+B`, `Ctrl+H`). Correct
     the row to name the shortcuts, unless the chrome work in
     `design-plans/one-chrome-band.md` is also adding a control, in which case
     name where. This is a documentation correction, not a behaviour change, and
     it must not be done by deleting the claim without checking that the
     shortcut is really the only path.
   - Preserve: the rest of the Library row, including history clearing.
   - Verify: `make gate` is unaffected; the README's keyboard row already lists
     both shortcuts.

## Scope

- Inherit: `Ctrl+Shift+B` and `Ctrl+H`, the single-instance window registration,
  `populate` and every `Entry` it produces, `tests/gui.py`.
- Verify: `make e2e-gui` (21 checks, including the two that open a panel and the
  one that clears history), `make gate`.
- Exclude: the browsing window's own chrome
  (`design-plans/one-chrome-band.md`), the panel's list styling and row
  contents, a search field in the panels (that is a new feature, not a design
  correction), and the browser window's title order, which is already correct.

## Validation

- Product: open bookmarks, then history, then bookmarks again; each raises the
  existing panel with a header bar; pick a row, and the page opens in the
  browsing window; clear the history, and the list empties in place.
- Interface: both panels; empty and populated lists; a long URL in a subtitle
  and in a tooltip; a panel opened while the other is in front; dark and light
  appearance.
- System: the panel's chrome is the same widget type the browsing window builds,
  in the same idiom, and the panel name comes from `LibraryKind::title()` so it
  is not spelled twice.
- Repository: `make gate` → 5 targets pass; `make e2e-gui` → 21 checks pass.

## Stop conditions

- Stop if adding a header bar to an `adw::Window` in this app changes the
  single-instance behaviour the module comment relies on, and report it.
- Stop if the `Clear history` button in the header is not reachable by keyboard,
  and report it rather than keeping a second copy under the list.
- Stop if the em-dash title order is deliberate for panels (a repository comment
  or a screenshot says so), and report the counterevidence instead of changing
  it.

## Design documentation

- After acceptance and validation: record in `DESIGN.md` (created by
  `design-plans/design-language.md`) that every window in the app is an
  `adw::HeaderBar` over its content, and that window titles read
  `<what> — Brwsl`.
