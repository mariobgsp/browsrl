# Brwsl

An independent, local-first Linux browser shell in Rust on GTK4, libadwaita and
WebKitGTK 6.0, in the spirit of the macOS browser
[driceroland/Search](https://github.com/driceroland/Search). No upstream code is
vendored, and no feature parity is claimed.

Working today: tabs, a strict navigation boundary, lazy WebView creation,
private tabs, bookmarks, history, one address field that offers this profile's
own addresses, and a local profile where a signed-in session survives a restart.
Everything else is listed under [Not implemented](#not-implemented).

## Features

| Area | What works |
| --- | --- |
| Tabs | Add, close, cycle with wrap-around, per-tab back/forward/reload, dense re-numbering, last tab resets instead of closing. The window title follows the page in front |
| Navigation | `http`/`https`/`about:blank` allowlist, bare hosts normalised to HTTPS, opt-in search |
| Address field | One field per window, showing the tab in front. Offers the addresses this profile has been to, most recent first, only when asked (Down arrow or the field's trailing control); Escape dismisses it. Asking is the only thing that reads anything — typing narrows what is already in memory |
| Privacy | Private tabs on an ephemeral network session: never restored, never bookmarked, never in history |
| Bookmarks | Star toggle or `Ctrl+D`, deduplicated by URL, `file:` and other schemes refused. `Ctrl+Shift+I` imports another browser's export through a file chooser |
| History | Written on a finished load, repeat visits collapsed, pruned to 5000 rows |
| Downloads | `profile/downloads`, `0600`, server names reduced to a safe leaf, collisions numbered |
| Sessions | Cookies in `data/cookies.txt` (`0600`, Netscape text) so a signed-in session survives a restart; `Ctrl+Shift+Delete` deletes it |
| Readability | Per-tab style sheet that constrains measure, enlarges type and hides page chrome. No script injected |
| Zoom | Per-tab, 50–300%, level shown in the bar |
| Print | `Ctrl+P` opens the WebKit print dialog |
| Closed tabs | `Ctrl+Shift+T` reopens the last closed tab, sixteen deep |
| Library | Bookmark and history windows (`Ctrl+Shift+B`, `Ctrl+H`) with history clearing |
| Site permissions | Every capability request is refused unless clicked through: the prompt's "Allow" control is unreachable by keyboard, so Return, Escape and dismissal all refuse |
| Clear browsing data | `Ctrl+Shift+Delete` empties the history table and clears cookies, storage and caches |
| Failures | A failed load names the URL and the error in the status line; a dead web process is reported with its reason |
| Session restore | Tabs and selection written on startup, tab switch, tab close and window close |
| Profile | One local directory, `0700`/`0600`, SQLite schema with in-place migration |

## Build and run

Needs GTK4, libadwaita 1.x, WebKitGTK 6.0, SQLite, `pkg-config` and a C
toolchain; Rust is pinned by `rust-toolchain.toml`. Video and audio also need the
GStreamer elements WebKitGTK looks up by name — `autoaudiosink`, `avdec_h264`,
`opusdec` (`gst-plugins-good`, `gst-libav` on Arch). Without them WebKitGTK 6.0
crashes the renderer, so `make doctor` names them when they are missing.

```sh
make doctor    # toolchain and library versions, including the GStreamer elements
make build
make run
```

`WEBKIT_PREFIX` (default `/usr`) is used for both `pkg-config` and
`LD_LIBRARY_PATH`, so a relocated prefix stays local to one command:

```sh
WEBKIT_PREFIX=/path/to/prefix/usr make build
```

A relocated prefix needs `lib/pkgconfig/webkitgtk-6.0.pc` plus the matching
headers, libraries and WebKit helper processes. The WebKit sandbox is never
disabled and there is no flag that turns it off.

## Install

```sh
sudo make install                     # or: make install PREFIX=$HOME/.local
make install DESTDIR=/tmp/stage       # staging only, touches nothing
make uninstall PREFIX=$HOME/.local
```

Installing is what makes the app findable: the desktop entry lands in
`share/applications` and the icon in the hicolor theme, and the desktop-entry
database and icon cache are rebuilt afterwards. With `PREFIX=$HOME/.local` no
root is needed and the entry's `Exec=brwsl %u` resolves on the session `PATH`.

`make dist` builds `dist/brwsl-<version>.tar.gz` after `make verify`, with no
`.git`, and it builds on its own.

Installing does **not** make Brwsl the default browser. That is
`xdg-settings set default-web-browser io.github.brwsl.Brwsl.desktop`, left
deliberate. The entry registers `http` and `https`, so Brwsl opens links handed
to it by other applications, one at a time.

`assets/brwsl-128.png` is the project logo: 128×128, opaque, on the white it was
drawn on, with no alpha channel. `make packaging` checks its shape and nothing in
the build regenerates it. `scripts/make-icon.py` is the earlier pixel-`b`
generator, kept for reference — running it over the logo replaces the artwork.

## Command line

```text
Usage: brwsl [URL] [--profile-dir PATH] [--database-path PATH] [--data-dir PATH]
               [--cache-dir PATH] [--download-dir PATH] [--start-url URL]
               [--search-endpoint URL] [--no-restore] [--import-bookmarks FILE]
               [--smoke] [--storage-check]
```

| Flag | Meaning |
| --- | --- |
| `[URL]` | One URL to open, as the desktop entry's `%u` hands over. A second URL is refused with a message. A launch URL is not swallowed by a saved session: the session restores and the URL opens selected on top |
| `--profile-dir` | Profile root; defaults to `$XDG_DATA_HOME/brwsl` |
| `--database-path`, `--data-dir`, `--cache-dir` | Override individual paths inside the profile |
| `--download-dir` | Where downloads go; defaults to `downloads` in the profile |
| `--start-url` | URL for the first tab; defaults to `about:blank` |
| `--search-endpoint` | Enables `search:` queries. Without it, search text is rejected rather than sent anywhere. Must be `https`, except a loopback host such as a local SearxNG |
| `--no-restore` | Ignore stored tabs and start from `--start-url` |
| `--import-bookmarks FILE` | Merge another browser's Netscape export and print a JSON summary. Head-less on purpose: the one time this has to work is when the browser is closed. `Ctrl+Shift+I` does the same through a file chooser while it is open |
| `--smoke` | Head-less contract: validate the URL, write one session row, read it back, print JSON |
| `--storage-check` | Head-less exercise of bookmarks, history, download-name policy and schema version, as JSON |

Exit codes: `0` success, `--help` and `--version`; `1` failed startup or smoke
check; `2` malformed invocation.

Brwsl is single-instance: a second launch becomes a tab in the first window, so
two processes never write the same profile. If a session database is held anyway,
startup fails with an explicit message rather than a bare SQLite error.

`--import-bookmarks` is bounded because the file is untrusted input: 32 MiB size
cap, 50,000 bookmark cap, folder depth cap, and a stop at the first tag that
never closes. A truncated file keeps what came before the cut. Only `http` and
`https` survive — the same rule as a hand-typed bookmark, so an import cannot
smuggle in a `file:` or `javascript:` URL. One unusable entry never stops the
rest, and the JSON keeps `failed` apart from `refused` so a full disk is never
reported as a policy decision:

```json
{"added":9,"duplicates":1,"refused":2,"too_deep":0,"failed":0,"folders":3,"parsed":12}
```

## Keyboard

`Ctrl+T` `Ctrl+Shift+N` `Ctrl+Shift+P` `Ctrl+W` `Ctrl+Shift+T` `Ctrl+Tab`
`Ctrl+Shift+Tab` `Ctrl+L` `Ctrl+R` `F5` `Ctrl+D` `Ctrl+Shift+B` `Ctrl+H`
`Ctrl+Shift+R` `Ctrl++` `Ctrl+-` `Ctrl+0` `Ctrl+P` `Ctrl+C` `Ctrl+Shift+Delete`
`Ctrl+Shift+I` `Ctrl+Shift+Q`

`Ctrl+Shift+T`, `Ctrl+Shift+N`, `Ctrl+Shift+P`, `Ctrl+Shift+B` and
`Ctrl+Shift+R` cannot be GTK accelerators: the parser folds
`<Primary><Shift>X` to the lowercase keyval with SHIFT in the mask, which no key
event can match. They are handled by a window-level `EventControllerKey`, which
sees the event before the focus widget does, and each window installs its own —
otherwise a library window in front would swallow them. A `debug_assert` in
`install_actions` rejects the spelling that cannot work. Shifted *non*-letter
shortcuts (`Ctrl+Shift+Delete`, `Ctrl+Shift+Tab`) are ordinary accelerators and
do work.

## Profile and data

The profile is `$XDG_DATA_HOME/brwsl` unless `--profile-dir` says otherwise: a
`session.sqlite` (`0600`) plus `data/` and `cache/` (`0700`), all re-hardened at
startup. SQLite's `-wal` and `-shm` sidecars carry the same URLs and titles as
the database and are re-hardened to `0600` too. The project has been renamed
twice, so a profile left under an earlier name is adopted on the first launch
rather than abandoned.

* `navigation.rs` is the only place a URL is accepted: `http`, `https`,
  `about:blank`. A bare host becomes `https://host/`; `file:`, `javascript:`,
  `data:` and other `about:` pages are rejected with a message, not rewritten.
  Search is refused unless `--search-endpoint` is given, so the shell cannot
  become an accidental search relay.
* The session database stores tab URLs and titles only. It is not a browsing
  history and it records nothing from a private tab.
* A private tab runs on `NetworkSession::new_ephemeral()`. In WebKitGTK 6.0 the
  website-data manager belongs to the network session, so it reports
  `is_ephemeral() == true` and has no data directory: private data stays in
  memory and is not shared with the normal session. This is not a claim of
  isolation from the OS or the network.
* There is no telemetry, account, sync, remote history or update client, and no
  code path that would add one. The shell opens no sockets of its own; every
  request goes through the WebKit network process.
* Wayland is preferred by the native GTK stack, with the X11 fallback behind it.

## Source layout

The visual rules — what a control is, how many bands stand between the window and
the page, what user-facing copy may name — are in [DESIGN.md](DESIGN.md).

| Path | Responsibility |
| --- | --- |
| `src/lib.rs` | Library root; no GUI dependencies. |
| `src/config.rs` | CLI parsing, exit codes, profile paths, and the profile's permission policy: the `0600`/`0700` modes, `private_file`, `private_directory`. |
| `src/navigation.rs` | Navigation policy: `http`, `https`, `about:blank`, opt-in search. |
| `src/storage.rs` | Versioned SQLite schema, migration, transactional save, and hardening of the database and its sidecars through `config`. |
| `src/bookmarks.rs` | Bookmark store: add, remove, list, deduplicated by URL. |
| `src/history.rs` | History store: finished-load records, collapsed repeats, 5000-row cap. |
| `src/downloads.rs` | Download destination policy: server names reduced to a safe leaf inside the profile. |
| `src/readability.rs` | The per-tab readability style sheet. Not the engine's reader, which WebKitGTK 6.0 no longer exposes. |
| `src/main.rs` | Process entry: arguments, the `--smoke` path, exit codes. |
| `src/library.rs` | The bookmark and history windows. |
| `src/prompt.rs` | The capability prompt, handed an origin and a handle to parent to. |
| `src/gui.rs` | `AdwApplication`, the window chrome (one bar carrying the tab strip and the window's controls, then the field row), the address field and the addresses it offers, the capability prompt, `PageState`, the window's `WebContext`, and lazy `WebView` realization. |
The library half is free of GTK and WebKit so it can be checked head-less
(`make check-core`) and exercised by the end-to-end contract without a display.
`gui.rs` holds a `Browser` bundle (context, normal and private network sessions,
search endpoint) that every tab shares, so per-tab state stays in `PageState`.

A tab starts as an `AdwTabPage` holding the page area, a placeholder label and
the status line — no WebKit object exists yet, and the bar that addresses the page
belongs to the window. Selecting the page calls `realize_page`, which creates a
`WebView` on the tab's `WebKitNetworkSession` and starts the pending load.
`LoadEvent::Finished` copies the final URI and title into the address field, the
tab label, the bookmark star and (for normal tabs) the history store, but only
while that tab is the one in front. Closing the window writes the normal tabs to
SQLite in one transaction; private tabs are filtered out and never restored. The
session is written on tab close and window close, so a `SIGKILL` loses whatever
changed since the last of those.

## Verification

```sh
make gate     # the machine-checkable subset, no renderer or display needed
make verify   # gate plus packaging and diagram freshness
```

| Target | What it proves |
| --- | --- |
| `make fmt-check` | `cargo fmt` is clean. |
| `make clippy` | No clippy warnings with `-D warnings`. |
| `make check-core` | The library half compiles without GTK or WebKit. |
| `make build` | The real binary links. |
| `make e2e` | The offline contract, writing `artifacts/e2e/report.json`. |
| `make e2e-gui` | Launches the real window and drives its actions over the session bus, writing `artifacts/e2e-gui/report.json`. Needs a display. |
| `make perf` | Five smoke runs, writing `artifacts/perf.json`. |
| `make diagrams-check` | Every committed SVG/PNG matches a fresh render. |
| `make packaging` | The desktop entry, AppStream metadata and icon validate. |
| `make gate` / `make verify` | All of the above except the diagram check, plus a printed check count / `gate` and `diagrams-check`. |

`.pi/verify.json` wires `make gate` into the project verification gate, so a
session in this repository has to produce a passing report before its work counts
as complete. The `api`, `ui` and `visual` dimensions are waived there with written
reasons: a GTK desktop application with no HTTP surface and no route-based visual
expectations.

`make e2e` runs the built binary against loopback fixtures in `tests/fixtures`
and asserts observable behaviour: SQLite round-trip, session restore, rejection of
non-web and look-alike input (`file:`, `javascript:`, `data:`, `HTTP://`,
`//host`, `about:config`) against their exact messages, bare-host normalisation,
search requiring an explicit endpoint, the `https`-only endpoint policy, the CLI
exit-code contract, that the default profile directory is the profile and not a
directory inside itself, and permission hardening that starts from a deliberately
world-readable profile. No Internet access needed. The permission check is
mutation-tested: with hardening disabled it fails on `session.sqlite` staying
`0666` while everything else passes, so it does not pass by construction.

`make e2e-gui` needs the keyboard, so it takes focus — run it when you are not
typing. It asserts what is observable from outside: the schema created, the first
page reaching history, `Ctrl+D` writing and removing a bookmark, `Ctrl+L` plus
typing and Enter navigating into history, clearing browsing data emptying the
history table, and a sequence of actions and keys leaving the process alive. It
waits for the app's own readiness signal rather than sleeping, retries a press
until the state it should change has changed, and retries a failed delivery — an
unbound accelerator leaves the profile untouched, so every attempt fails and the
check fails. It only ever retires its own leftovers: a process is signalled only
if it is this test binary, was given a `--profile-dir` that is a direct child of
the system temporary directory, and that directory carries the harness's profile
prefix. A browser you started yourself lives in your data directory and cannot
match.

Because `wtype` injects into whichever window is focused, every press is gated on
the focused window being this harness's own browser, read from `hyprctl` or
`swaymsg`. A refused press fails the check that wanted it and is counted under
`keyboard` in the report, rather than being typed into a terminal anyway. On a
compositor that cannot be queried, focus is unknowable rather than known-foreign,
so the gate passes and the report records `focus_queryable: false`.

`WebKitWebDriver` works against WebKit's MiniBrowser but cannot drive this shell:
WebKitGTK 6.0 requires the app to answer `WebKitAutomationSession::create-web-view`
with a view created `is-controlled-by-automation`, and the `webkit6` 0.6.1
bindings do not expose that signal. The shell does not use WebDriver rather than
ship a fragile hand-written closure around it. Visual results — page rendering,
the readability pass, the library windows — are checked by screenshot, not by an
assertion, as are the print dialog (modal, would block the run) and the library
rows (asserted only through the database behind them).

One check cannot pass everywhere: this machine has no camera, so WebKitGTK 6.0
fails a video request during device enumeration and answers a notification
request with `denied` on its own, without asking the application; geolocation never
settles. The capability check therefore reports `verified: false` with that reason
instead of claiming a pass, and starts failing the moment a request does arrive and
the answer is not a refusal.

To check the window by hand:

```sh
make build
(cd tests/fixtures && python3 -m http.server 8123 --bind 127.0.0.1 &)
./target/debug/brwsl --profile-dir "$(mktemp -d)" --start-url http://127.0.0.1:8123/one.html
```

Expect one `Tab 1`, the address entry showing the requested URL, the fixture body
rendered by WebKit, and `data/` populated with WebKit's own storage.

## Architecture

![Brwsl module architecture](diagrams/architecture.svg)
![Brwsl tab lifecycle](diagrams/tab-lifecycle.svg)
![Brwsl profile and privacy boundary](diagrams/profile-and-privacy.svg)

PNG fallbacks sit next to the SVGs. The sources are `diagrams/*.puml`; they use
`!pragma layout smetana`, so rendering needs PlantUML and no Graphviz:

```sh
PLANTUML_JAR=/path/to/plantuml.jar make diagrams
PLANTUML_JAR=/path/to/plantuml.jar make diagrams-check
```

`check-diagrams.sh` renders every `diagrams/*.puml` into a temporary directory and
fails if a committed SVG or PNG is stale or missing. The committed assets were
rendered with PlantUML 1.2024.7, so a different release can legitimately produce
different bytes; re-render in that case.

## Not implemented

| Missing | Why |
| --- | --- |
| Password storage | Needs Secret Service; the profile would otherwise hold credentials it should not |
| Extensions | WebKitGTK 2.52 exposes only part of the WebExtensions surface, so this waits for the API that actually exists |
| DRM playback | No Widevine or equivalent in WebKitGTK |
| Update management | A browser with an updater is a browser with a remote channel |
| Reader-mode extraction | The readability pass restyles a page; it does not extract an article the way a reader engine would |
| Find in page, save page | `webkit_web_view_find_*` and the serialisation surface do not exist in WebKitGTK 6.0 |
| Tab reordering by drag, tear-off windows | Not implemented |
| Content blocking | Not implemented |
| Per-site permission memory | A capability request is asked about every time and refused by default; the answer is not remembered, because there is no list to store and `PermissionRequest` exposes no URI to key one on |
| Engine-side navigation policy | The address bar holds to the allowlist, but a page's own navigation cannot: `WebKitWebView::decide-policy` in WebKitGTK 6.0 carries only a `PolicyDecision` and a decision type, with no `NavigationAction` and no URI to judge. WebKit's own restrictions still apply |
| Archive packaging | `make dist` builds a source tarball, but nothing is submitted to a distribution |

This is a working browser shell, not a finished product, and not a functional
superset of the browser it is inspired by.
