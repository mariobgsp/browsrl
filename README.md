# Brwsl

Brwsl is an independent, local-first Linux browser shell written in Rust on
top of GTK4, libadwaita, and WebKitGTK 6.0. It is an independent
implementation in the spirit of the macOS browser
[driceroland/Search](https://github.com/driceroland/Search); no upstream code is
vendored here, and this repository does not claim feature parity with it.

This repository is an early but working slice: a real window with tabs, a strict
navigation boundary, lazy WebView creation, private tabs, bookmarks, browsing
history, one address field that offers what this profile has already been to, and
a local profile where a signed-in session survives closing the browser.
Everything still missing is listed as deferred below rather than stubbed out.

## Features

| Area | What works today |
| --- | --- |
| Tabs | Add, close, cycle with wrap-around, per-tab back/forward/reload, dense re-numbering, last tab resets instead of closing. The window title follows the page in front |
| Navigation | Strict `http`/`https`/`about:blank` allowlist, bare hosts normalised to HTTPS, opt-in search |
| Address field | One field for every tab, showing the tab in front and offering the addresses this profile has been to, most recent first, from its own history: never looked up, read at most once every thirty seconds, and never offered for the app's own writes to the field |
| Privacy | Private tabs on an ephemeral network session: never restored, never bookmarked, never in history |
| Bookmarks | Star toggle in the window's bar or `Ctrl+D`, deduplicated by URL, `file:` and other schemes refused. `Ctrl+Shift+I` imports another browser's exported bookmarks file through a file chooser |
| History | Written when WebKit reports a finished load, repeat visits collapsed, pruned to 5000 rows |
| Downloads | Saved into `profile/downloads` as `0600`, server-supplied names reduced to a safe leaf, collisions numbered |
| Sessions | A signed-in session survives closing the browser: cookies are kept in `data/cookies.txt` (`0600`, Netscape text format), because WebKit keeps them in memory unless a browser names the file. `Ctrl+Shift+Delete` deletes it, since clearing site data cannot reach a file of its own |
| Readability | Per-tab pass that constrains measure, enlarges type and hides page chrome, applied as a user style sheet with no script injected |
| Zoom | Per-tab, 50% to 300%, with the level shown in the window's bar |
| Print | `Ctrl+P` opens the WebKit print dialog for the selected tab |
| Closed tabs | `Ctrl+Shift+T` reopens the last closed tab, sixteen deep |
| Library | Bookmark and history windows on `Ctrl+Shift+B` and `Ctrl+H`, with history clearing |
| Site permissions | Every capability request is refused unless it is clicked through: the prompt is a window whose "Allow" control the keyboard cannot reach, so Return and Escape both refuse, and a deliberate click on "Allow" is the only way in |
| Clear browsing data | `Ctrl+Shift+Delete` empties the history table and clears WebKit's cookies, storage and caches for the profile's session |
| Load failures | A failed load names the URL and the error in the status line instead of leaving a blank page |
| Crashed pages | A web process that dies is reported with the reason; the bar's reload control is an icon, not a word |
| Session | Tabs and the selected tab restored on launch, saved on startup, on every tab switch, on tab close and on window close. A `quit` action closes cleanly, which is the only way the session is written on the way out |
| Keyboard | `Ctrl+T` `Ctrl+Shift+N` `Ctrl+Shift+P` `Ctrl+W` `Ctrl+Shift+T` `Ctrl+Tab` `Ctrl+Shift+Tab` `Ctrl+L` `Ctrl+R` `F5` `Ctrl+D` `Ctrl+Shift+B` `Ctrl+H` `Ctrl+Shift+R` `Ctrl++` `Ctrl+-` `Ctrl+0` `Ctrl+P` `Ctrl+C` `Ctrl+Shift+Delete` `Ctrl+Shift+I` `Ctrl+Shift+Q` |
| Profile | One local directory, `0700`/`0600`, SQLite schema with in-place migration |

## Architecture

![Brwsl module architecture](diagrams/architecture.svg)

![Brwsl tab lifecycle](diagrams/tab-lifecycle.svg)

![Brwsl profile and privacy boundary](diagrams/profile-and-privacy.svg)

PNG fallbacks are committed next to the SVGs: [architecture](diagrams/architecture.png),
[tab lifecycle](diagrams/tab-lifecycle.png), and
[profile and privacy](diagrams/profile-and-privacy.png). The sources are
`diagrams/architecture.puml`, `diagrams/tab-lifecycle.puml`, and
`diagrams/profile-and-privacy.puml`.

### Source layout

The visual rules - what a control is, how many bands stand between the window
and the page, how a title reads, what user-facing copy may name - are in
[DESIGN.md](DESIGN.md).

| Path | Responsibility |
| --- | --- |
| `src/lib.rs` | Library root; no GUI dependencies. |
| `src/config.rs` | CLI parsing, exit codes, profile paths, `0700` directory hardening. |
| `src/navigation.rs` | The navigation policy: `http`, `https`, `about:blank`, and opt-in search. |
| `src/storage.rs` | Versioned SQLite schema, migration, transactional save, `0600` hardening of the database and its `-wal`/`-shm` sidecars. |
| `src/bookmarks.rs` | Bookmark store: add, remove, list, deduplicated by URL. |
| `src/history.rs` | History store: finished-load records, collapsed repeats, capped at 5000 rows. |
| `src/downloads.rs` | Download destination policy: server-supplied names reduced to a safe leaf inside the profile. |
| `src/readability.rs` | The per-tab readability style sheet. Not the engine's reader, which WebKitGTK 6.0 no longer exposes. |
| `src/main.rs` | Process entry: arguments, the `--smoke` path, exit codes. |
| `src/gui.rs` | `AdwApplication`, the window chrome (one bar carrying the tab strip and the window's controls, then the field row), the address field and the addresses it offers, the capability prompt, `PageState`, the window's `WebContext`, and lazy `WebView` realization. |

The library half is deliberately free of GTK and WebKit so it can be checked
head-less (`make check-core`) and exercised by the end-to-end contract without a
display. `gui.rs` holds a `Browser` bundle (context, normal and private network
sessions, search endpoint) that every tab shares, so per-tab state stays in
`PageState`.

### How a tab works

1. `gui.rs` creates an `AdwTabPage` holding the page area, a placeholder label
   and the status line. No WebKit object exists yet, and the bar that addresses
   the page belongs to the window rather than to the tab.
2. When the page is selected, `realize_page` creates a `WebView` bound to the
   tab's `WebKitNetworkSession` and starts the pending load.
3. `LoadEvent::Finished` copies the final URI and title into the address field
   and the tab label, but only while the tab is the one in front.
4. A finished load updates the address entry, the tab title, the bookmark star,
   and (for normal tabs) the history store.
5. Closing the window writes the normal tabs to SQLite in one transaction.
   Private tabs are filtered out and are never restored. The session is written
   when a tab closes and when the window closes, so a `SIGKILL` or a crash loses
   whatever changed since the last of those.

## Requirements and build

The default feature needs GTK4, libadwaita 1.x, WebKitGTK 6.0 (the `webkit6`
feature targets WebKit 2.52), SQLite, `pkg-config`, and a C toolchain. Rust is
pinned by `rust-toolchain.toml`.

Video and audio additionally need GStreamer elements that WebKitGTK looks up by
name: `autoaudiosink` for output and a decoder for whatever codec the site
serves (`avdec_h264`, `opusdec`). Without them WebKitGTK 6.0 does not degrade
gracefully — it crashes the renderer — so `make doctor` checks for them and
names the packages when they are missing. On Arch: `gst-plugins-good` and
`gst-libav`.

```sh
make doctor    # toolchain and library versions
make build
make run
```

`WEBKIT_PREFIX` defaults to the portable system prefix `/usr` and is used for
both `pkg-config` and `LD_LIBRARY_PATH`, so a relocated prefix stays local to
one command:

```sh
WEBKIT_PREFIX=/path/to/prefix/usr make doctor
WEBKIT_PREFIX=/path/to/prefix/usr make build
WEBKIT_PREFIX=/path/to/prefix/usr make run
```

The relocated prefix needs `lib/pkgconfig/webkitgtk-6.0.pc` plus the matching
headers, libraries, and WebKit helper processes. The WebKit sandbox is never
disabled and there is no flag that turns it off.

## Command line

```text
Usage: brwsl [URL] [--profile-dir PATH] [--database-path PATH] [--data-dir PATH]
               [--cache-dir PATH] [--download-dir PATH] [--start-url URL]
               [--search-endpoint URL] [--no-restore] [--import-bookmarks FILE]
               [--smoke] [--storage-check]
```

| Flag | Meaning |
| --- | --- |
| `--profile-dir` | Profile root; defaults to `$XDG_DATA_HOME/brwsl`. |
| `--database-path`, `--data-dir`, `--cache-dir` | Override individual paths inside the profile. |
| `--start-url` | URL for the first tab; defaults to `about:blank`. |
| `[URL]` | A single URL to open, exactly like every other browser. This is what the desktop entry's `%u` hands over when a link is opened from another application. One link at a time: a second URL is refused with a message rather than silently ignored, and the entry asks for `%u` rather than `%U` so it never promises more than the app delivers. A URL given at launch is **not** swallowed by a saved session: the session is restored and the URL opens on top of it, selected, the way the other browsers behave. |
| `--search-endpoint` | Enables `search:` queries. Without it, search text is rejected instead of being sent anywhere. Must be `https`, except for a loopback host such as a local SearxNG. |
| `--no-restore` | Ignore stored tabs and start from `--start-url`. |
| `--import-bookmarks FILE` | Merge another browser's exported bookmarks into this profile and print a JSON summary of what happened. Head-less on purpose: the one time this has to work is when the browser is closed. The `import-bookmarks` action does the same thing through a file chooser while it is open. |
| `--smoke` | Head-less contract path: validate the URL, write one session row, read it back, print JSON. |
| `--storage-check` | Head-less exercise of bookmarks, history, download-name policy and the schema version, printed as JSON. |
| `--download-dir` | Where downloads are saved; defaults to `downloads` inside the profile. |

Exit codes: `0` for success and for `--help`/`--version`, `1` for a failed
startup or smoke check, `2` for a malformed invocation.

Brwsl is single-instance: a second launch becomes a tab in the first window,
so two processes never write the same profile. If a session database is held by
another process anyway, startup fails with an explicit message rather than a
bare SQLite error.

### Navigation policy

`navigation.rs` is the only place a URL is accepted, and it is deliberately
small: `http`, `https`, and `about:blank`. A bare host such as `example.com`
becomes `https://example.com/`; `file:`, `javascript:`, `data:`, and other
`about:` pages are rejected with a message instead of being silently rewritten.
Search is opt-in: without `--search-endpoint`, `search:` input is refused, so
the shell can never become an accidental search relay to a third party.

## Profile and session data

The profile is `$XDG_DATA_HOME/brwsl` unless `--profile-dir` says otherwise, and
it holds `session.sqlite` (`0600`) plus `data/` and `cache/` directories (`0700`),
and every one of them is created or re-hardened at startup. The project has been
renamed twice, so a profile left under either earlier name is adopted on the first
launch rather than abandoned — a rename that quietly started on an empty profile
would look exactly like losing every bookmark. SQLite's `-wal` and `-shm` sidecars carry the same URLs and titles as
the database and are created with the process umask, so they are re-hardened to
`0600` as well. Cookies and site data stay inside WebKit's own storage under
`data/`, and the cookie store is `data/cookies.txt` — a file, `0600`, that
`Ctrl+Shift+Delete` removes. Nothing is written outside the profile directory, and no global GTK,
desktop, or OpenCode configuration is touched.

## Verification

```sh
make gate     # the machine-checkable subset, no renderer or display needed
make verify   # gate plus the diagram freshness check
```

| Target | What it proves |
| --- | --- |
| `make fmt-check` | `cargo fmt` is clean. |
| `make clippy` | No clippy warnings with `-D warnings`. |
| `make check-core` | The library half compiles without GTK or WebKit. |
| `make build` | The real binary links. |
| `make e2e` | The offline contract below, writing `artifacts/e2e/report.json`. |
| `make e2e-gui` | Launches the real window and drives its actions over the session bus, writing `artifacts/e2e-gui/report.json`. Needs a display. |
| `make perf` | Five smoke runs, writing `artifacts/perf.json`. |
| `make diagrams-check` | Every committed SVG/PNG matches a fresh render. |
| `make packaging` | The desktop entry, AppStream metadata and icon validate. |
| `make gate` | All of the above except the diagram check, plus a printed check count. |
| `make verify` | `gate` and `diagrams-check`. |

`.pi/verify.json` wires `make gate` into the project verification gate, so a
session in this repository has to produce a passing report before its work
counts as complete. The `api`, `ui`, and `visual` dimensions are waived there
with written reasons, because this is a GTK desktop application with no HTTP
surface and no route-based visual expectations.

`make e2e` runs the built binary against loopback fixtures from `tests/fixtures`
and asserts observable behaviour: SQLite round-trip, session restore, rejection
of non-web and look-alike input (`file:`, `javascript:`, `data:`, `HTTP://`,
`//host`, `about:config`) against their exact messages, bare-host normalization,
search requiring an explicit endpoint, the `https`-only endpoint policy, the CLI
exit-code contract, that the default profile directory is the profile and not a
directory inside itself, and permission hardening that starts from a
deliberately world-readable profile. It needs no Internet access.

The permission check is mutation-tested: with the hardening disabled it fails on
`session.sqlite` remaining `0666` while every other check still passes, so it is
not a check that passes by construction.

### Manual GUI verification

The window and live WebKit rendering are verified by hand on a real session,
not by the automated contract. Serve a fixture and start the browser:

```sh
make build
(cd tests/fixtures && python3 -m http.server 8123 --bind 127.0.0.1 &)
./target/debug/brwsl --profile-dir "$(mktemp -d)" \
    --start-url http://127.0.0.1:8123/one.html
```

Expected: one `Tab 1`, the address entry showing the requested URL, the fixture
body rendered by WebKit, and `data/` populated with WebKit's own storage. This
is how the two-tab and blank-address regressions seen during development were
found, so it is worth repeating after tab-strip changes.

### GUI verification

`WebKitWebDriver` is installed and works against WebKit's own MiniBrowser, but
it cannot drive this shell: WebKitGTK 6.0 requires the app to answer
`WebKitAutomationSession::create-web-view` and return a view created with
`is-controlled-by-automation`, and the `webkit6` 0.6.1 bindings (the current
release) do not expose that detailed signal. Rather than ship a fragile
hand-written closure around it, the shell does not use WebDriver.

The window is still verified automatically, in two ways. Its actions are
registered on the `GApplication`, which makes them reachable over the session
bus, and its accelerators are exercised with `wtype`, so real key presses are
tested rather than assumed. `make e2e-gui` launches the real browser against a
loopback fixture and asserts what is observable from outside: the schema it
creates, that the first page reaches history, that `Ctrl+D` writes a bookmark and
a second press removes it, that `Ctrl+L` followed by typing and `Enter` navigates
and reaches history, that clearing browsing data empties the history table, and
that a sequence of actions and keys leaves the process alive. It writes
`artifacts/e2e-gui/report.json`.

That run needs the keyboard, so it takes focus: run it when you are not typing.
It only ever retires its *own* leftovers. A process is signalled only if it is
this test binary, was given a `--profile-dir` that is a direct child of the
system temporary directory, and that directory carries the same prefix the
harness creates profiles from. A browser you started yourself lives in your data
directory, so it cannot match. The rule is a pure function, and the offline
contract checks it against nine command lines, including a personal profile, a
prefix buried in a nested path, and a path that tries to climb out of the
temporary directory with `..`.

It also refuses to type when the keyboard belongs to someone else. `wtype`
injects into whichever window is focused, so a press landing while a terminal or
an editor is focused would type a URL and press Return into it. Every press is
therefore gated on the focused window being this harness's own browser, read from
`hyprctl` or `swaymsg`; a refused press fails the check that wanted it and is
counted in `artifacts/e2e-gui/report.json` under `keyboard`, rather than being
typed anyway. On a compositor that cannot be queried, focus is unknowable rather
than known-foreign, so the gate passes and the report records
`focus_queryable: false`. The gate is covered offline by asserting that a press
with a foreign window focused never reaches `wtype` at all, and the sway output
parser is checked against a synthetic tree, because it has to skip a focused
*container* to find the focused window inside it.

Two things that key checks are up against, both real on a compositor: a mapped
window does not hold the keyboard the instant it appears, and `wtype` can fail
while the session is busy. So the contract waits for the app's own readiness
signal (the first page landing in history) rather than a fixed sleep, retries a
press until the state it should change has changed, and retries a failed
delivery. None of that can hide a dead accelerator: an unbound key leaves the
profile untouched, so every attempt fails and the check fails. A delivery that
cannot happen at all is recorded as a failure in its own right.

Those key checks are mutation-tested: putting the accelerators back on a `win.`
action group that is never populated, which is a bug this project actually had,
makes both key checks fail.

### Shortcuts whose key is a shifted letter

`Ctrl+Shift+T`, `Ctrl+Shift+N`, `Ctrl+Shift+P`, `Ctrl+Shift+B` and `Ctrl+Shift+R`
are **not** GTK accelerators, and cannot be. This was measured, not guessed:
GTK's parser folds every `<Primary><Shift>X` to the *lowercase* keyval with
SHIFT in the modifier mask, and matching then compares keyval and modifiers
exactly. A real `Ctrl+Shift+T` arrives as `T` with Ctrl+Shift, so the accel asks
for something no key event can be. Probed directly against GTK:

| Written | Parses to |
| --- | --- |
| `<Primary><Shift>t` | keyval `0x74` (`t`), Ctrl+Shift |
| `<Primary><Shift>T` | keyval `0x74` (`t`), Ctrl+Shift — identical |
| `<Primary>T` | keyval `0x54` (`T`), Ctrl only |

None of the three can match, so the shortcut is dead while the action behind it
works perfectly. Five bindings were dead for exactly this reason, and no test
noticed, because every shortcut exercised until now was unshifted. Shifted
*non*-letter shortcuts are unaffected: `Ctrl+Shift+Delete` and `Ctrl+Shift+Tab`
are ordinary accelerators and do work.

Those five are therefore handled by a window-level `EventControllerKey`, which
sees the event before the focus widget does, and each window installs its own —
a controller belongs to one widget, so without that a library window in front
silently swallowed them. A `debug_assert` in `install_actions` rejects the
spelling that cannot work, so the mistake cannot be reintroduced through the
accelerator table; it was verified by putting `<Primary><Shift>t` back and
watching the binary abort at startup with that explanation.

What remains unasserted is the visual result: page rendering, the readability
pass, and the library windows are checked by screenshot rather than by an
assertion.

Three of the newer behaviours are covered differently, and it is worth being
precise about which is which. Clearing browsing data is asserted: the contract
activates the action and reads the history table before and after. The
capability dialog and the two status messages (a failed load, a dead web
process) are asserted too — a failed load is provoked with a port that was bound
and released first, a dead web process by ending this run's own, and a capability
request where the machine can raise one at all.

What is *not* asserted is the capability answer on a machine that cannot raise a
request. This one has no camera, so WebKitGTK 6.0 fails a video request during
device enumeration — "no device was found amongst 0 devices",
`OverconstrainedError` — and answers a notification request with `denied` on its
own, without asking the application; geolocation never settles. The check
therefore reports `verified: false` with that reason instead of claiming a pass,
and starts failing the moment a request does arrive and the answer is not a
refusal.

Two features are not observable from outside the process at all and are checked
by hand: the print dialog, which is modal and would block the run, and the
library windows' contents, whose rows are asserted only by the profile database
behind them.

### Bookmark import

`--import-bookmarks FILE`, or `Ctrl+Shift+I` for the same thing through a file
chooser, reads the Netscape bookmark format that Firefox, Chrome, Chromium and
Brave all export. The file is untrusted input, so it is bounded: a 32 MiB size
cap, a 50,000 bookmark cap, a folder depth cap, and a stop at the first tag that
never closes rather than a guess at what the rest of the file meant. A truncated
file keeps the bookmarks that came before the cut, which is asserted.

Only `http` and `https` survive — the same rule the store applies to a bookmark
typed by hand, so an import is not a way to smuggle a `file:` or `javascript:`
URL into the browser. HTML entities in titles and URLs are decoded, nested
folders are read and reported (they are not stored, because folders are listed
as deferred), duplicates collapse by URL, and one unusable entry never stops the
rest: a bookmark file is often the only copy of that list a person has.

Everything is counted rather than reported as a single yes:

```json
{"added":9,"duplicates":1,"refused":2,"too_deep":0,"failed":0,"folders":3,"parsed":12}
```

`failed` is kept apart from `refused` on purpose, so a full disk or a locked
database is never presented as a policy decision. Ten assertions cover what a
real export contains — a duplicate inside one file, an untitled bookmark, a
`javascript:` URL, a truncated file — and the folder-structure check is
mutation-tested: never popping the folder stack makes it fail.

## Privacy and current boundaries

* WebKit owns page rendering, website data, cookies, and caching.
* A private tab runs on `NetworkSession::new_ephemeral()`. In WebKitGTK 6.0 the
  website-data manager belongs to the network session, so that session's
  manager reports `is_ephemeral() == true` and has no data directory at all:
  private cookies and site data stay in memory, are never written under
  `data/`, and are not shared with the normal session. It is not a claim of
  isolation from the operating system or the network itself.
* The session database stores tab URLs and titles only. It is not a browsing
  history, and it records nothing from a private tab.
* There is no telemetry, account, sync, remote history, or update client in this
  slice, and no code path that would add one. The shell opens no sockets of its
  own: every request goes through the WebKit network process.
* A capability request — camera, microphone, screen, location — is refused unless
  it is clicked through. The prompt is a window whose "Allow" control the keyboard
  cannot reach, so Return and Escape both refuse, and dismissing it refuses: a
  grant takes a deliberate click and nothing else. The answer is not remembered,
  so a site has to ask again.
* Wayland is preferred by the native GTK stack, with the GTK X11 fallback.
* Video and audio depend entirely on the GStreamer plugins WebKitGTK finds on the
  machine, and the failure is harsh: with no audio sink, WebKitGTK 6.0 reports
  `GStreamer element autoaudiosink not found`, logs a NULL-pointer warning from
  inside the web process and then **crashes the renderer**. The shell reports
  that crash rather than pretending the page is fine. `make doctor` checks for
  the elements by name — `autoaudiosink`, `avdec_h264`, `opusdec` — and says what
  to install when one is missing. On Arch that is `gst-plugins-good` and
  `gst-libav`; with them present a YouTube video plays with sound, measured by
  PipeWire reporting live audio outputs.

Deliberately not implemented:

| Missing | Why |
| --- | --- |
| Password storage | Needs Secret Service; the profile would otherwise hold credentials it should not |
| Extensions | WebKitGTK 2.52 exposes only part of the WebExtensions surface, so this is gated on the API that actually exists rather than emulated |
| DRM playback | No Widevine or equivalent in WebKitGTK; nothing to wire up |
| Update management | A browser with an updater is a browser with a remote channel; this one has none by design |
| Reader-mode extraction | The readability pass restyles a page; it does not extract an article the way a reader engine would |
| Find in page | `webkit_web_view_find_*` does not exist in WebKitGTK 6.0; there is no typed binding to call |
| Save page | Needs the same removed find/serialisation surface, or an injected script that the 6.0 bindings also lack |
| Tab reordering by drag, tear-off windows | Not implemented |
| Content blocking | Not implemented |
| Per-site permission memory | A capability request is asked about every time and refused by default, but the answer is not remembered per site: there is no allow or deny list to store, and `PermissionRequest` exposes no URI to key one on |
| Engine-side navigation policy | The address bar refuses anything outside `http`, `https` and `about:blank`, but a page's *own* navigation cannot be held to the same rule: `WebKitWebView::decide-policy` in WebKitGTK 6.0 carries only a `PolicyDecision` and a decision type, with no `NavigationAction` and therefore no URI to judge, and the only signal carrying a `NavigationAction` is `create` for new windows. WebKit's own restrictions still apply to what a page may load |
| Archive packaging | `make dist` builds a source tarball, but nothing is submitted to a distribution |

This is a working browser shell, not a finished product, and it is not a
functional superset of the macOS browser it is inspired by.

## Distributing

`make dist` builds `dist/brwsl-<version>.tar.gz` after the gate, the packaging
checks and the diagram freshness check have all passed, so an archive can never
carry a stale diagram or an unvalidated desktop entry. It is a source tarball
with no `.git`, and it builds on its own:

```sh
make dist
tar xzf dist/brwsl-0.1.0.tar.gz -C /tmp && cd /tmp/brwsl-0.1.0 && cargo build
```

## Installing

`make install` places the release binary, the desktop entry, the AppStream
metadata and the icon under `$(PREFIX)`, defaulting to `/usr`:

```sh
make build-release
sudo make install                     # or: make install PREFIX=$HOME/.local
make install DESTDIR=/tmp/stage       # staging only, touches nothing
make uninstall PREFIX=$HOME/.local
```

Installing is what makes the app *findable*: the entry lands in
`share/applications` and the icon in the hicolor theme, and `make install`
rebuilds the desktop-entry database and the icon cache afterwards, because a
desktop environment shows neither until those caches are refreshed. With
`PREFIX=$HOME/.local` no root is needed, and `~/.local/bin` is on the session
`PATH`, so the `Exec=brwsl %u` line resolves for a launcher-started app. After
installing, the app appears in the application launcher under **Brwsl** and
opens with its icon.

The first run creates its profile at `$XDG_DATA_HOME/brwsl` — a fresh profile,
not the old `rbrowse` directory.

The desktop entry registers the `http` and `https` handlers, so Brwsl can
open links handed to it by other applications, one link at a time. Installing
does **not** make it the default browser: that is
`xdg-settings set default-web-browser io.github.brwsl.Brwsl.desktop`, and it
is left to you on purpose, since taking over link handling is not something an
installer should do behind your back. `make packaging` validates the entry with
`desktop-file-validate` and the metadata with `appstreamcli`, and fails only on
an `E:` line. The metadata carries the project's homepage and author, so the
missing-homepage warning it used to raise is gone.

## The icon

`assets/brwsl-128.png` is the project logo, 128×128, with a transparent
background. It is made from the supplied JPEG: 90% of that image measured as
near-white, which is the plate the mark was drawn on rather than part of the
mark, so the white is cut out and the logo sits on whatever theme the desktop
uses. A white square on a dark dock reads as a missing icon, which is the one
thing an icon must not do.

The file is the artwork, not a build product: `make packaging` checks that it is
a 128×128 PNG and fails on a truncated or placeholder file, and nothing in the
build regenerates it.

```sh
make packaging                                  # validates the desktop entry, metadata and icon
```

`scripts/make-icon.py` is the *earlier* generator: it draws a lowercase Latin `b`
as a pixel grid, and running it over `assets/brwsl-128.png` replaces the logo with
that glyph. It is kept for reference and the grid is still editable, so if the
pixel letterform is ever wanted back, run it deliberately — and re-check the
icon afterwards.

## PlantUML assets

```sh
PLANTUML_JAR=/path/to/plantuml.jar make diagrams
PLANTUML_JAR=/path/to/plantuml.jar make diagrams-check
```

The sources use `!pragma layout smetana`, so rendering needs only PlantUML
itself and no Graphviz installation. `check-diagrams.sh` renders every
`diagrams/*.puml` into a temporary directory, compares each committed SVG and
PNG byte for byte, and fails if an asset is stale or missing. Regenerate with
`make diagrams` after editing a source. The committed assets were rendered with
PlantUML 1.2024.7, so a different PlantUML release can legitimately produce
different bytes; re-render in that case.
