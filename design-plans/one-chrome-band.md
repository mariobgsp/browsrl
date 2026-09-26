# One window band: the page is what the window is mostly

Written against: d24e392

## Evidence chain

- Surface: `src/gui.rs` window chrome, as rendered. Measured on a live window
  (`--profile-dir` throwaway, 930×1157 capture, dark theme) by scanning one
  column of pixels down the left third: header bar y 1–57, tab bar y 59–116,
  per-tab toolbar y 126–167, status strip y 168–201, page content from y 203.
- Problem: four stacked bands stand between the window's title bar and the page.
  202 px of a 1157 px window — 17% of the height — and none of it is the page.
  Three of the four are hand-built (`adw::HeaderBar` + `adw::TabBar` in a
  `gtk::Box`, then a per-tab `gtk::Box` toolbar, then a per-tab `gtk::Label`);
  the fourth is always reserved, even when it holds nothing, which is the case
  on every normal tab.
- Design evidence:
  - `driceroland/Search` `Sources/Search/Design.swift`: "the page *is* the
    ground and everything the browser draws has to get out of its way", and
    `Metrics.strip = 52` for the entire strip above the page. The project's
    README already names this repository as the thing Brwsl is "in the spirit
    of".
  - The governing platform primitive already exists and this window bypasses
    it: `adw::TabView::header_bar()` is the bar libadwaita builds to put the
    tab strip *in* the title bar, and `adw::TabView::set_header_bar()` exists so
    an application can supply its own. The current code builds a second header
    bar and a separate `adw::TabBar` on top of it instead, which is why the
    same tab strip is drawn twice over (`tab_bar.set_view(Some(&tab_view))` plus
    the view's own header bar) and why the window has a title bar that holds
    nothing but two buttons.
- Owner: `build_window` (`src/gui.rs`) for the window chrome, `create_page`
  (`src/gui.rs`) for the row inside each tab.
- Scope and affected surfaces: every tab of every window the shell opens, the
  two library panels' parent window, and all 21 checks in `tests/gui.py`, which
  drive this chrome through the real window.
- Uncertainty: none about the band count. The one behaviour-visible part is
  that the address field, the navigation row and the tab-scoped controls stop
  being per-tab widgets and become window widgets that act on the selected tab;
  Change 2 spells out every behaviour that must survive that.

## Design decision

Build the window on `adw::TabView`'s own header bar, and put the address field
and the navigation row in the band below it. The per-tab toolbar is deleted, not
restyled: the controls that lived in it (back, forward, reload, zoom, bookmark,
readability) all act on the *selected* tab already, so they belong to the
window, and the row they sit in is the row that also holds the field. The status
line stops reserving a band when it is empty.

Result: the chrome becomes the two bands libadwaita and GNOME Web both use —
title bar with the tab strip inside it, then the field row — about 84 px instead
of 202 px, and the page starts immediately below the field. Every control keeps
its current icon, tooltip, accelerator and effect; only the widget that owns it
changes.

## Reuse

- `adw::TabView::header_bar()` / `set_header_bar()` — the tab strip in the title
  bar, and the pack points for window-level controls.
  Exemplar: `libadwaita-0.9.2` `src/tab_view.rs`; used today only implicitly,
  since the code never calls either.
- `adw::HeaderBar` as the constructor for the field row's band.
  Exemplar: `src/gui.rs`, `build_window` — the `header` that is being replaced
  is the same widget type, so the construction pattern is already in the file.
- `Shell::selected()` — how a window-level control finds the tab it acts on.
  Exemplar: `src/gui.rs`, `Shell::focus_address`, which already reaches the
  selected tab's field through it.
- `gtk::Entry` with `set_activate`, and the `connect_address` navigation
  contract (Enter → `navigate_state` → the same error reporting in the status
  line). Exemplar: `src/gui.rs`, `connect_address`.

No new primitive is required. The decision is expressible entirely with
`adw::TabView`, `adw::HeaderBar`, `gtk::Box` and `gtk::Entry`, all of which the
file already uses.

## Changes

1. `src/gui.rs` → `build_window`
   - Change: build the window on the tab view's own header bar. Call
     `tab_view.header_bar()` (or `set_header_bar()` with an `adw::HeaderBar` you
     build), `pack_start` the existing `new_tab` and `private_tab` buttons into
     it, and `pack_end` the navigation and tab-scoped controls named in Change 3.
     Delete the standalone `adw::HeaderBar` and the standalone `adw::TabBar`, and
     set the window's content to a `gtk::Box` holding the tab view, then the
     field row from Change 2. Keep `tab_view.set_vexpand(true)`.
   - Preserve: the window title (`"Brwsl"`, later `"<page> — Brwsl"`), the
     1100×760 default size, the single-window-per-application behaviour
     (`connect_activate` returns early when `application.windows()` is not
     empty), the private download directory cleanup on close, the session save
     on `connect_close_request`, and the `DIAG` session diagnostic.
   - Verify: the window opens with one band containing the tab strip, the two
     new-tab controls and the field row below it; a pixel scan of the window
     shows the page starting within ~90 px of the top edge instead of ~202 px.

2. `src/gui.rs` → the address field becomes a window widget
   - Change: build one `gtk::Entry` in `build_window`, keep the existing
     placeholder text and `set_width_chars(48)`, and give it a `connect_activate`
     that resolves the selected tab at activation time (the shape
     `Shell::focus_address` already uses) instead of capturing a `PageRef`. Set
     its text from the selected tab's URL in the existing
     `tab_view.connect_selected_page_notify` handler and in the existing
     `LoadEvent::Finished` branch. `Shell::focus_address` then grabs this entry
     and selects its text, unchanged.
   - Preserve: `Ctrl+L` focus-and-select, Enter-to-navigate through
     `navigate_state` with the same status-line error, the field following the
     page that finishes loading, the field showing the selected tab's URL after
     a tab switch (per-tab entries did this implicitly; the window entry has to
     do it in the selection handler), and `about:blank` on a fresh tab.
   - Verify: `make e2e-gui` passes `ctrl_l_typing_and_enter_navigates` and
     `start_url_reaches_history`; switching to an already-loaded tab shows that
     tab's URL in the field, not the previous tab's.

3. `src/gui.rs` → the navigation row and the tab-scoped controls move to the window
   - Change: move `back`, `forward`, `reload`, `zoom_out`, `zoom_label`,
     `zoom_in`, `bookmark` and `readable` out of `create_page` into the field row
     of `build_window`. Each one resolves the selected tab at click time through
     `Shell::selected()` and then calls the same function it calls today
     (`navigate_state` through the history/reload handlers, `set_zoom`,
     `toggle_bookmark`, `toggle_readability`). Keep every icon name, every
     tooltip string, `zoom_label` `set_width_chars(5)` / `set_xalign(0.5)` /
     `set_sensitive(false)`, and the `toolbar` margins and spacing
     (`gtk::Box::new(gtk::Orientation::Horizontal, 6)`, 6 px margins).
   - Preserve: the icon-only vocabulary and every tooltip, the `Back`/`Forward`
     history semantics, the zoom range and step, the bookmark refusals for
     private and blank tabs, the readability toggle's idempotent sheet
     attachment, and `set_bookmark_icon` / `set_readability_icon` reflecting
     state on tab switch (`refresh_bookmark` already runs there; the zoom label
     and readability icon need the same sync added).
   - Verify: `make e2e-gui` passes `ctrl_d_bookmarks_and_unbookmarks_the_page`,
     `reload_refetches_the_current_page`, `the_readability_pass_changes_the_page_layout`
     and every other check; the zoom percentage is still visible at all times.

4. `src/gui.rs` → `create_page` keeps only the page and the status line
   - Change: delete the per-tab `toolbar` box and its append to `content`. Keep
     `placeholder` and `status` in `content` in that order. Give `status` an
     empty string as its normal-tab initial value and set
     `status.set_visible(false)` whenever it is set to an empty string, so a
     normal tab reserves no band; every existing `state.status.set_text(...)` call
     site reports a non-empty message, so this only affects the empty case.
   - Preserve: the private-tab initial message, `set_xalign(0.0)`,
     `set_wrap(true)`, the 8/8/4 px margins, and the `log_event` pairing that
     writes page-level events to stderr as well.
   - Verify: a normal tab's page reaches the bottom edge of the window; a failed
     load and a private tab still show their message (`make e2e-gui`:
     `a_failed_load_names_the_url_and_the_error`).

5. `src/gui.rs` → `PageState` shrinks to page state
   - Change: remove the `address`, `bookmark`, `readable` and `zoom_label` widget
     fields from `PageState` (the `readable_on` flag, the `zoom` level and every
     function that reads them stay). Update `set_bookmark_icon`,
     `set_readability_icon` and `set_zoom` to take the widgets from the window,
     or move them next to the window-level controls.
   - Preserve: every call site of the three helpers, and the session/tab
     bookkeeping, which does not depend on these widgets.
   - Verify: `cargo clippy --all-targets -- -D warnings` and `cargo fmt --all --
     --check` are clean; no dangling `PageState` widget references remain.

## Scope

- Inherit: every tab, both library panels (they parent to the window), the
  session save/restore path, `tests/gui.py`, `tests/e2e.py`.
- Verify: `make e2e-gui` (21 checks, real window), `make gate`, and the
  `the_readability_pass_changes_the_page_layout` check in particular, since the
  pass is a page-level style sheet and must survive the page box shrinking.
- Exclude: the library panels' own chrome (that is `design-plans/library-window-chrome.md`),
  the private-tab button's icon (`design-plans/private-tab-icon.md`), the
  `Tab N` labels, the bookmark and history entry points, the readability CSS,
  and anything about behaviour or performance.

## Validation

- Product: open the browser, load a page, add a tab, switch tabs, zoom, bookmark,
  toggle readability, and confirm each control does what it did before and that
  the page is visibly larger than it was.
- Interface: one tab and six tabs; a private tab (its status message and the
  field's contents); a long URL in the field; a failed load (status appears, no
  permanent band); a page that finishes loading after a tab switch.
- System: the new composition uses `adw::TabView`, `adw::HeaderBar`, `gtk::Box`
  and `gtk::Entry` only — the same widgets the file already uses — and introduces
  no second place where the tab strip or the field is built.
- Repository: `make gate` → 5 targets pass; `make e2e-gui` → 21 checks pass.

## Stop conditions

- Stop if a window-level control cannot resolve the selected tab at click time
  without changing what it does, and report which control.
- Stop if `adw::TabView`'s header bar cannot host both the new-tab controls and
  the field row without a third band, and report the layout that is achievable
  rather than inventing one.
- Stop if the status line's disappearing band makes a transient message push the
  page down visibly while a page is loading; report it instead of adding an
  animation or a fixed reservation.

## Design documentation

- After acceptance and validation: record in `DESIGN.md` (created by
  `design-plans/design-language.md`) that the window is one band plus the field
  row, that the page is the ground, and that per-tab controls are window
  controls that act on the selected tab.
