# The private-tab control is an icon, like every other control in the row

Written against: d24e392

## Evidence chain

- Surface: `src/gui.rs` window header bar, as rendered. OCR of a live window
  reads `Private` as rendered text in the header band, beside a symbolic
  new-tab button that has no text at all.
- Problem: one row holds two controls for the same task — open a tab — and they
  speak two different languages. The new-tab control is an Adwaita symbolic icon
  whose meaning lives in its tooltip; the private-tab control is a text button.
  The same control then contradicts itself: its visible label is `Private`
  (a mode name) while its tooltip is `New private tab` (an action).
- Design evidence: the rule this repository already wrote down for itself, in
  `src/gui.rs` `create_page`, immediately above the row this control joins:
  "Icons, not words. A text 'Reload' button sat in a row of symbolic buttons and
  looked like a different kind of control; every button here is an icon with a
  tooltip, so the meaning lives in the tooltip and the row reads as one thing.
  The names are the Adwaita symbolic ones, so they follow the system icon theme
  instead of shipping artwork." The README repeats the rule from the outside
  ("Crashed pages … the toolbar's reload control is an icon, not a word").
- Owner: `build_window` (`src/gui.rs`), the only place `private_tab` is built.
- Scope and affected surfaces: every window the shell opens; the private tab's
  `Ctrl+Shift+N` and `Ctrl+Shift+P` accelerators and its action are unaffected.
- Uncertainty: none. `view-conceal-symbolic` is present in both icon themes
  installed on this machine — `/usr/share/icons/Adwaita/symbolic/actions/view-conceal-symbolic.svg`
  and `/usr/share/icons/Yaru/scalable/actions/view-conceal-symbolic.svg` — so
  the icon resolves under either, and the file's existing icon names are all
  Adwaita names, so this adds no new theme dependency.

## Design decision

Build the private-tab control the way every other control in the chrome is
built: `gtk::Button::from_icon_name("view-conceal-symbolic")`, tooltip
`New private tab`. Nothing else changes — same pack point, same order, same
action, same accelerator. The label disappears rather than being restyled,
because the rule the repository wrote down is about the row reading as one
thing, and a styled word button would still be a word.

## Reuse

- `gtk::Button::from_icon_name` + `set_tooltip_text`, the exact construction
  every neighbouring control uses. Exemplar: `src/gui.rs` `build_window`, the
  `new_tab` button two lines above the one being changed.
- The Adwaita symbolic icon vocabulary the file already depends on
  (`tab-new-symbolic`, `view-refresh-symbolic`, `starred-symbolic`,
  `non-starred-symbolic`, `zoom-in-symbolic`, `view-reading-mode-symbolic`).
  Exemplar: `src/gui.rs` `create_page` and `set_bookmark_icon` /
  `set_readability_icon`.
- The existing tooltip text, which already names the action correctly.

No new primitive, no stylesheet, no shipped artwork.

## Changes

1. `src/gui.rs` → `build_window`
   - Change: `let private_tab = gtk::Button::with_label("Private");` becomes
     `let private_tab = gtk::Button::from_icon_name("view-conceal-symbolic");`.
     Leave `set_tooltip_text(Some("New private tab"))` and both `pack_start`
     calls exactly as they are.
   - Preserve: the button's `clicked` handler (it calls
     `shell_for_private.add_tab(TabMode::Private, "about:blank")`), its position
     in the row, its tooltip, and the `new-private-tab` action behind
     `Ctrl+Shift+N` / `Ctrl+Shift+P`.
   - Verify: the header row shows two symbolic icons of the same optical size;
     hovering the second reads `New private tab`; OCR of the rendered window no
     longer finds the word `Private` in the header band.

2. `README.md`
   - Change: none required. The README already states the icons-not-words rule
     this change brings the header into line with; no sentence describes the
     private control's appearance.
   - Preserve: the features table's Private row, which is about the ephemeral
     network session, not the control.
   - Verify: `make check-desktop` is unaffected (it validates `assets/`, not the
     README).

## Scope

- Inherit: the header bar in every window; the new-tab button beside it.
- Verify: `make e2e-gui` (`every_expected_action_is_registered` asserts the
  `new-private-tab` action is still registered; `a_private_tab_is_never_written_to_the_profile`
  drives the private tab), `make gate`.
- Exclude: the zoom percentage's permanent label in the field row, the `Clear history`
  text button in the library panel (a destructive action is allowed a word), the
  window chrome's band count (`design-plans/one-chrome-band.md`), and any new
  icon artwork.

## Validation

- Product: open the browser; the header shows the new-tab icon and the
  private-tab icon; clicking the second opens a private tab whose status line
  explains the ephemeral session.
- Interface: dark and light appearance; high-contrast; the two installed icon
  themes (`Adwaita`, `Yaru`); hover and keyboard focus on both header controls.
- System: no new icon dependency beyond the Adwaita symbolic names the file
  already uses, and no stylesheet introduced.
- Repository: `make gate` → 5 targets pass; `make e2e-gui` → 21 checks pass.

## Stop conditions

- Stop if `view-conceal-symbolic` fails to resolve on a machine whose only theme
  is one that lacks it, and report the theme rather than shipping artwork.
- Stop if the row cannot hold two same-size symbols at the window's narrowest
  supported width, and report the width rather than shrinking a different
  control.

## Design documentation

- After acceptance and validation: record in `DESIGN.md` (created by
  `design-plans/design-language.md`) that every control in the chrome is an
  Adwaita symbolic icon whose meaning lives in its tooltip, and that the only
  permitted words in the chrome are state readouts.
