# A design language for Brwsl, written down once

Written against: d24e392

## Evidence chain

- Surface: the whole drawn surface — the browsing window's chrome, the per-tab
  page area, and the two library panels.
- Problem: the repository has no design documentation at all. `ls DESIGN.md`
  finds nothing, there is no stylesheet (`grep -rl "CssProvider" src/` is empty,
  `assets/` holds only the icon, the desktop entry and the appstream metadata),
  and there are no tokens. Every visible decision is a one-off line inside
  `build_window`, `create_page` and `library::open`, and the rules the project has
  already discovered are written as comments above the code that happens to need
  them — the icons-not-words rule, the title convention, the "the enum's own
  variant names are developer-facing" rule, the readability sheet's refusal to set
  colours. A second agent cannot find those without reading the whole file, and
  the next change to the chrome will be made without them.
- Design evidence: those four in-repo decisions, quoted, plus the target this
  project names as its model (`driceroland/Search`, whose `Design.swift` puts
  every colour and every metric in one place and says why: "the page *is* the
  ground and everything the browser draws has to get out of its way", with
  `Metrics.strip = 52` for the whole strip above the page).
- Owner: the repository itself — `DESIGN.md` at the root, read by agents before
  they touch `src/gui.rs` or `src/library.rs`.
- Scope and affected surfaces: no rendered surface changes. This plan changes
  documentation only, and only after the three sibling plans have landed, so the
  document describes what the app actually does.
- Uncertainty: none. Every rule below is already true of the code, or becomes
  true with `design-plans/one-chrome-band.md` and
  `design-plans/library-window-chrome.md`.

## Design decision

Write `DESIGN.md` as a short list of rules with the reason beside each one, in
the same voice the code comments already use: what the palette is (the system
theme, by name, with no project colours), what a control is (an Adwaita symbolic
icon, meaning in the tooltip), how many bands stand between the window and the
page (one, plus the field row), where state is written (the status line, and
nothing permanent in a row of controls), how window titles are composed
(`<what> — Brwsl`), and what user-facing copy may and may not name (never a
class, never an enum variant, never a mechanism). No token table, no colour
values, no spacing scale: the app has none, and inventing a scale nobody uses
would be a new system rather than a document of the existing one.

## Reuse

- The four existing decisions, quoted verbatim, as the body of the rules.
  Exemplars: the comment above the toolbar in `src/gui.rs` `create_page`; the
  `LoadEvent::Finished` branch that composes `format!("{title} — Brwsl")`;
  `describe_termination` in `src/gui.rs`; the comment above
  `READABILITY_CSS` in `src/readability.rs`.
- `AGENTS.md` / `CLAUDE.md` for the file-naming convention already in force
  (`AGENTS.md` is canonical; there is no `CLAUDE.md` to merge). `DESIGN.md` sits
  beside them, not instead of one.
- The icon names already in use, as the vocabulary's examples: `tab-new-symbolic`,
  `view-conceal-symbolic`, `go-previous-symbolic`, `go-next-symbolic`,
  `view-refresh-symbolic`, `starred-symbolic` / `non-starred-symbolic`,
  `view-reading-mode-symbolic` / `view-reading-mode-checked-symbolic`,
  `zoom-in-symbolic`, `zoom-out-symbolic`.

No new primitive; this is a document.

## Changes

1. `DESIGN.md` (new, at the repository root)
   - Change: create the file with these rules, each with its reason, and nothing
     else:
     - **Palette** — the system theme decides every colour; the app names
       Adwaita / libadwaita and `*-symbolic` icon names and defines no colours of
       its own. Reason: the app is meant to follow the desktop it runs on, and
       the readability sheet already refuses to set colours for the same reason
       ("a page that sets its own background fights them and ends up with
       unreadable text").
     - **Controls** — every control in the chrome is an Adwaita symbolic icon
       whose meaning lives in its tooltip. Reason: the rule already written in
       `create_page`, from a text "Reload" button that "looked like a different
       kind of control". The one exception is a destructive action, which keeps
       its word (`Clear history`).
     - **Chrome** — the window is the tab strip in the title bar, then the field
       row, then the page. The page is the ground. Reason: 202 px of a 1157 px
       window measured on the live app, none of it the page.
     - **State** — the status line is where the app reports what it is doing;
       a row of controls carries no permanent words. Reason: the zoom percentage
       is the one counterexample found, and the row rule is what it breaks.
     - **Titles** — a window title reads `<what> — Brwsl`; the browsing
       window's `<what>` is the page in front. Reason: the convention the
       browsing window already implements.
     - **Copy** — user-facing copy never names a class, an enum variant, a
       session or a directory. Reason: `describe_termination` exists only to
       stop the enum's variant names reaching the screen.
     - **Panels** — every window is an `adw::HeaderBar` over its content.
       Reason: the browsing window's pattern, which the library panels did not
       follow.
   - Preserve: nothing in existing files; this is a new file. Leave the
     comments in `src/gui.rs` and `src/readability.rs` exactly as they are —
     they are the quotes this document is built from, and deleting them would
     lose the reason at the point of use.
   - Verify: `DESIGN.md` names no colour value, no hex code, no font size and no
     spacing number; every rule in it can be checked against a line of `src/`.

2. `README.md`
   - Change: add one line under Architecture pointing at `DESIGN.md` for the
     visual rules, so the entry point is discoverable from where a reader starts.
   - Preserve: the rest of the Architecture section, including the diagrams and
     the source-layout table.
   - Verify: `make gate` is unaffected; the link is a plain relative path.

3. `AGENTS.md` (if one is added later at this level, not this change)
   - Change: none now. When the repository's own guidance file gains a
     "before you change the UI, read DESIGN.md" line, that is a separate change
     with the owner who maintains that file.

## Scope

- Inherit: nothing rendered; the two sibling plans must land first so the
  document is true.
- Verify: `make gate` and `make e2e-gui` are unaffected by a new Markdown file.
- Exclude: any token table, colour palette, type scale, spacing scale, icon
  artwork, component library, or CSS file. If the app ever needs one, that is a
  design change with its own plan, not a side effect of writing this down.

## Validation

- Product: none — no rendered change. The test is that a reader can state every
  visual rule the app follows, and find each one in the code.
- Interface: check the file against the live window: the chrome rule matches the
  bands measured on screen, the controls rule matches every control in the
  header, the field row and both panels, the titles rule matches both windows.
- System: `DESIGN.md` is the only new file; no other guidance file is created
  or edited, and the repository keeps one canonical instruction file.
- Repository: `make gate` → 5 targets pass, unchanged by the addition.

## Stop conditions

- Stop if any rule in the list cannot be checked against a line of `src/`, and
  drop that rule rather than writing it from memory.
- Stop if the sibling plans were not accepted, because the chrome, titles and
  panels rules would then describe work that has not happened; report which
  plans are missing instead of writing a document the code does not support.
- Stop if `DESIGN.md` starts accumulating values, and report it: the point of the
  document is that this app has no values of its own.

## Design documentation

- This plan *is* the documentation change: it creates `DESIGN.md`. After it is
  accepted, the destination for every later visual decision is that file, and
  the sibling plans' "Design documentation" sections are the entries that fill
  it in.
