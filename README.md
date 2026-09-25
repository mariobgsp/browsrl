# R Browse

R Browse is an independent, local-first Linux browser shell written in Rust on
top of GTK4, libadwaita, and WebKitGTK 6.0. It is an independent
implementation in the spirit of the macOS browser
[driceroland/Search](https://github.com/driceroland/Search); no upstream code is
vendored here, and this repository does not claim feature parity with it.

This repository is an early but working slice: a real window with tabs, a strict
navigation boundary, lazy WebView creation, private tabs, bookmarks, browsing
history, keyboard shortcuts, and a local SQLite profile. Everything still
missing is listed as deferred below rather than stubbed out.

## Features

| Area | What works today |
| --- | --- |
| Tabs | Add, close, cycle, per-tab back/forward/reload, dense re-numbering, last tab resets instead of closing |
| Navigation | Strict `http`/`https`/`about:blank` allowlist, bare hosts normalised to HTTPS, opt-in search |
| Privacy | Private tabs on an ephemeral network session: never restored, never bookmarked, never in history |
| Bookmarks | Star toggle in the toolbar or `Ctrl+D`, deduplicated by URL, `file:` and other schemes refused |
| History | Written when WebKit reports a finished load, repeat visits collapsed, pruned to 5000 rows |
| Downloads | Saved into `profile/downloads`, server-supplied names reduced to a safe leaf, collisions numbered |
| Readability | Per-tab pass that constrains measure, enlarges type and hides page chrome, applied as a user style sheet with no script injected |
| Library | Bookmark and history windows from the toolbar actions, with history clearing |
| Session | Tabs restored on launch, saved on tab close and window close |
| Keyboard | `Ctrl+T` `Ctrl+Shift+N` `Ctrl+Shift+P` `Ctrl+W` `Ctrl+Tab` `Ctrl+Shift+Tab` `Ctrl+L` `Ctrl+R` `F5` `Ctrl+D` `Ctrl+Shift+B` `Ctrl+H` `Ctrl+Shift+R` `Ctrl+C` |
| Profile | One local directory, `0700`/`0600`, SQLite schema with in-place migration |

## Architecture

![R Browse module architecture](diagrams/architecture.svg)

![R Browse tab lifecycle](diagrams/tab-lifecycle.svg)

![R Browse profile and privacy boundary](diagrams/profile-and-privacy.svg)

PNG fallbacks are committed next to the SVGs: [architecture](diagrams/architecture.png),
[tab lifecycle](diagrams/tab-lifecycle.png), and
[profile and privacy](diagrams/profile-and-privacy.png). The sources are
`diagrams/architecture.puml`, `diagrams/tab-lifecycle.puml`, and
`diagrams/profile-and-privacy.puml`.

### Source layout

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
| `src/gui.rs` | `AdwApplication`, window, tab strip, `PageState`, the window's `WebContext`, and lazy `WebView` realization. |

The library half is deliberately free of GTK and WebKit so it can be checked
head-less (`make check-core`) and exercised by the end-to-end contract without a
display. `gui.rs` holds a `Browser` bundle (context, normal and private network
sessions, search endpoint) that every tab shares, so per-tab state stays in
`PageState`.

### How a tab works

1. `gui.rs` creates an `AdwTabPage` holding a toolbar and a placeholder label.
   No WebKit object exists yet.
2. When the page is selected, `realize_page` creates a `WebView` bound to the
   tab's `WebKitNetworkSession` and starts the pending load.
3. `LoadEvent::Finished` copies the final URI and title into the address entry
   and the tab label.
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

```
Usage: rbrowse [--profile-dir PATH] [--database-path PATH] [--data-dir PATH]
               [--cache-dir PATH] [--start-url URL] [--search-endpoint URL]
               [--no-restore] [--smoke]
```

| Flag | Meaning |
| --- | --- |
| `--profile-dir` | Profile root; defaults to `$XDG_DATA_HOME/rbrowse`. |
| `--database-path`, `--data-dir`, `--cache-dir` | Override individual paths inside the profile. |
| `--start-url` | URL for the first tab; defaults to `about:blank`. |
| `--search-endpoint` | Enables `search:` queries. Without it, search text is rejected instead of being sent anywhere. Must be `https`, except for a loopback host such as a local SearxNG. |
| `--no-restore` | Ignore stored tabs and start from `--start-url`. |
| `--smoke` | Head-less contract path: validate the URL, write one session row, read it back, print JSON. |
| `--storage-check` | Head-less exercise of bookmarks, history, download-name policy and the schema version, printed as JSON. |
| `--download-dir` | Where downloads are saved; defaults to `downloads` inside the profile. |

Exit codes: `0` for success and for `--help`/`--version`, `1` for a failed
startup or smoke check, `2` for a malformed invocation.

R Browse is single-instance: a second launch becomes a tab in the first window,
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

The profile holds `session.sqlite` (`0600`) plus `data/` and `cache/`
directories (`0700`), and every one of them is created or re-hardened at
startup. SQLite's `-wal` and `-shm` sidecars carry the same URLs and titles as
the database and are created with the process umask, so they are re-hardened to
`0600` as well. Cookies and site data stay inside WebKit's own storage under
`data/`. Nothing is written outside the profile directory, and no global GTK,
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
exit-code contract, and permission hardening that starts from a deliberately
world-readable profile. It needs no Internet access.

The permission check is mutation-tested: with the hardening disabled it fails on
`session.sqlite` remaining `0666` while every other check still passes, so it is
not a check that passes by construction.

### Manual GUI verification

The window and live WebKit rendering are verified by hand on a real session,
not by the automated contract. Serve a fixture and start the browser:

```sh
make build
(cd tests/fixtures && python3 -m http.server 8123 --bind 127.0.0.1 &)
./target/debug/rbrowse --profile-dir "$(mktemp -d)" \
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
creates, that `Ctrl+D` writes a bookmark and a second press removes it, that
`Ctrl+L` followed by typing and `Enter` navigates and reaches history, and that
a sequence of actions and keys leaves the process alive. It writes
`artifacts/e2e-gui/report.json`.

Those key checks are mutation-tested: putting the accelerators back on a `win.`
action group that is never populated, which is a bug this project actually had,
makes both key checks fail.

What remains unasserted is the visual result: page rendering, the readability
pass, and the library windows are checked by screenshot rather than by an
assertion.

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
* Wayland is preferred by the native GTK stack, with the GTK X11 fallback.

Deliberately not implemented:

| Missing | Why |
| --- | --- |
| Password storage | Needs Secret Service; the profile would otherwise hold credentials it should not |
| Extensions | WebKitGTK 2.52 exposes only part of the WebExtensions surface, so this is gated on the API that actually exists rather than emulated |
| DRM playback | No Widevine or equivalent in WebKitGTK; nothing to wire up |
| Update management | A browser with an updater is a browser with a remote channel; this one has none by design |
| Reader-mode extraction | The readability pass restyles a page; it does not extract an article the way a reader engine would |
| Find in page, print, save page, zoom controls | Not implemented |
| Tab reordering by drag, tear-off windows, closed-tab restore | Not implemented |
| Content blocking | Not implemented |
| Site permissions and prompts | WebKit handles the defaults; there is no per-site UI |
| Archive packaging | `make install` covers local installation, not submission to a distribution |

This is a working browser shell, not a finished product, and it is not a
functional superset of the macOS browser it is inspired by.

## Installing

`make install` places the release binary, the desktop entry, the AppStream
metadata and the icon under `$(PREFIX)`, defaulting to `/usr`:

```sh
make build-release
sudo make install                     # or: make install PREFIX=$HOME/.local
make install DESTDIR=/tmp/stage       # staging only, touches nothing
make uninstall PREFIX=$HOME/.local
```

The desktop entry registers the `http` and `https` handlers, so R Browse can
open links handed to it by other applications. `make packaging` validates the
entry with `desktop-file-validate` and the metadata with `appstreamcli`; the one
accepted AppStream warning is the missing project homepage, because this
repository has no public URL yet.

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
