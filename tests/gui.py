#!/usr/bin/env python3
"""GUI contract for Brwsl, driven through the window it really opens.

WebKitGTK 6.0 cannot be driven by WebKitWebDriver, because a session needs the
app to answer ``WebKitAutomationSession::create-web-view`` and the ``webkit6``
bindings do not expose that signal. Two things are used instead:

* the actions registered on the ``GApplication``, which are reachable over the
  session bus, and
* real key presses through ``wtype``, so the accelerators themselves are tested
  rather than assumed.

Assertions are made on what is observable from outside the process: the profile
database, and the process staying alive. It needs a display, a running binary
and ``wtype``; it is deliberately not part of ``make gate``.
"""

from __future__ import annotations

import json
import os
import pathlib
import shutil
import signal
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time

from harness import (
    BINARY,
    PROFILE_PREFIX,
    LocalFixtureServer,
    ROOT,
    read_database,
    temporary_profile,
)


REPORT = ROOT / "artifacts" / "e2e-gui" / "report.json"
APPLICATION_ID = "io.github.brwsl.Brwsl"
OBJECT_PATH = "/io/github/brwsl/Brwsl"
READY_SECONDS = 30

# Names this harness's own log files, so they can be recognised and removed
# without touching anything else in the temporary directory.
LOG_PREFIX = "brwsl-gui-"


def have_wtype() -> bool:
    return shutil.which("wtype") is not None


def looks_like_harness_browser(args: list[str]) -> bool:
    """Classify a command line: is it a browser this harness started?

    Kept separate from reading ``/proc`` so the rule can be checked against
    realistic command lines, including the ones that must be rejected.
    """
    if str(BINARY) not in args or "--profile-dir" not in args:
        return False
    index = args.index("--profile-dir") + 1
    if index >= len(args):
        return False
    profile = pathlib.Path(args[index])
    return (
        profile.name.startswith(PROFILE_PREFIX)
        and profile.parent == pathlib.Path(tempfile.gettempdir())
    )


def is_own_leftover(pid: str) -> bool:
    """True only for a browser this harness started on an earlier run.

    A browser the person started is never matched, because all of these have to
    hold at once:

    * the process is this test binary,
    * it was given a ``--profile-dir`` that is a direct child of the system
      temporary directory, and
    * that directory's name carries :data:`PROFILE_PREFIX`, the same constant
      :func:`temporary_profile` creates names from.

    A person's own profile lives in their data directory, so the last condition
    alone rules it out. The prefix is not a search pattern: it is compared
    against one parsed argument, so a path merely *containing* the text
    somewhere cannot match either.
    """
    if not pid.isdigit() or int(pid) == os.getpid():
        return False
    try:
        raw = pathlib.Path(f"/proc/{pid}/cmdline").read_bytes().decode(
            "utf-8", errors="replace"
        )
    except OSError:
        return False
    return looks_like_harness_browser([part for part in raw.split("\0") if part])


def clear_leftovers() -> None:
    """Retire browser processes left behind by an earlier run of this harness.

    Only processes accepted by :func:`is_own_leftover` are signalled, and only
    the harness's own log files are removed.
    """
    listing = subprocess.run(["pgrep", "-f", str(BINARY)], capture_output=True, text=True)
    mine = [pid for pid in listing.stdout.split() if is_own_leftover(pid)]
    for pid in mine:
        try:
            os.kill(int(pid), signal.SIGTERM)
        except ProcessLookupError:
            pass
    if mine:
        time.sleep(1.5)


class WtypeFailed(RuntimeError):
    """wtype could not deliver input, so no key-press result can be trusted."""


# Every key press is refused unless the focused window is this harness's own
# browser. wtype injects into whatever is focused, so pressing while a terminal,
# an editor or a person is on the keyboard would type a URL and press Return
# into their window. A refused press fails the check that wanted it, which is the
# right outcome: a missing keystroke is recoverable, a mangled terminal is not.
KEYBOARD: dict[str, object] = {
    "focus_known": False,
    "foreign_focus_refusals": 0,
    "delivery_failures": 0,
    "presses": 0,
    "refused_classes": [],
}


def focused_window_class() -> str | None:
    """The compositor's focused window class, or None if it cannot be read."""
    for command in (["hyprctl", "activewindow", "-j"], ["swaymsg", "-t", "get_tree", "-j"]):
        if not shutil.which(command[0]):
            continue
        result = subprocess.run(command, capture_output=True, text=True, timeout=10)
        if result.returncode != 0:
            continue
        try:
            data = json.loads(result.stdout or "{}")
        except json.JSONDecodeError:
            continue
        node = data if isinstance(data, dict) else _focused_sway_node(data)
        if isinstance(node, dict) and node.get("class"):
            KEYBOARD["focus_known"] = True
            return str(node["class"])
    return None


def _focused_sway_node(tree: object) -> dict | None:
    """Find the focused *window* in a sway tree.

    Sway marks both outputs and workspaces as focused, so the first focused node
    found is not necessarily a window. The search therefore only accepts a
    focused node that carries a class, and keeps descending past a focused
    container to reach the window inside it.
    """
    if not isinstance(tree, list):
        return None
    for node in tree:
        if not isinstance(node, dict):
            continue
        if node.get("focused") and node.get("class"):
            return node
        found = _focused_sway_node(node.get("nodes", []))
        if found is not None:
            return found
    return None


def focus_is_ours(current: str | None) -> bool:
    """Whether a focused-window class means the keyboard is ours.

    Split out from the query so the rule can be checked without a compositor.
    ``None`` means the compositor could not be asked, which is answered yes: an
    unqueryable compositor leaves focus unknown rather than known-foreign, and
    refusing every press there would make the contract unrunnable rather than
    safe.
    """
    if current is None:
        return True
    return current == APPLICATION_ID


def own_window_has_focus() -> bool:
    """True when the browser this harness opened is the focused window."""
    return focus_is_ours(focused_window_class())


def wait_for_own_focus(seconds: float = 6.0) -> bool:
    deadline = time.monotonic() + seconds
    while True:
        if own_window_has_focus():
            return True
        if time.monotonic() >= deadline:
            return False
        time.sleep(0.25)


def require_own_focus() -> bool:
    """Refuse input unless our own window holds the keyboard.

    A refusal is counted rather than raised: it fails the one check that wanted
    the input, and the run carries on so the rest of the report still says what
    happened. The counter is asserted at the end, so a run that was refused is
    never reported as clean.
    """
    if wait_for_own_focus():
        return True
    KEYBOARD["foreign_focus_refusals"] = int(KEYBOARD["foreign_focus_refusals"]) + 1
    refused = KEYBOARD["refused_classes"]
    if isinstance(refused, list):
        refused.append(focused_window_class())
    return False


def run_wtype(command: list[str], attempts: int = 3) -> None:
    """Deliver input, retrying a transient wtype failure.

    wtype talks to the compositor and occasionally fails while the session is
    busy. Retrying is safe because a failed delivery sends nothing, whereas a
    delivery that merely had no effect is retried by the caller instead.
    """
    problem = ""
    for _ in range(attempts):
        result = subprocess.run(command, capture_output=True, text=True, timeout=30)
        if result.returncode == 0:
            return
        problem = result.stderr.strip() or f"exit {result.returncode}"
        time.sleep(1.0)
    raise WtypeFailed(f"{' '.join(command)}: {problem}")


def deliver(command: list[str]) -> bool:
    """Deliver one input command, if the keyboard is ours to type on."""
    if not require_own_focus():
        return False
    KEYBOARD["presses"] = int(KEYBOARD["presses"]) + 1
    try:
        run_wtype(command)
    except WtypeFailed:
        KEYBOARD["delivery_failures"] = int(KEYBOARD["delivery_failures"]) + 1
        return False
    return True


def press(key: str, ctrl: bool = False, shift: bool = False) -> bool:
    """Send a real key press to the focused window.

    ``-k`` is required for special keys: without it wtype *types* the key name,
    which silently produced the text "Return" in the address entry once.
    """
    modifiers: list[str] = []
    if ctrl:
        modifiers.append("ctrl")
    if shift:
        modifiers.append("shift")
    command = ["wtype"]
    for modifier in modifiers:
        command += ["-M", modifier]
    command += ["-k", key]
    for modifier in modifiers:
        command += ["-m", modifier]
    return deliver(command)


def type_text(text: str) -> bool:
    return deliver(["wtype", text])


def activate(action: str) -> str:
    result = subprocess.run(
        [
            "gdbus",
            "call",
            "--session",
            "--dest",
            APPLICATION_ID,
            "--object-path",
            OBJECT_PATH,
            "--method",
            "org.gtk.Actions.Activate",
            action,
            "[]",
            "{}",
        ],
        capture_output=True,
        text=True,
        timeout=30,
    )
    if result.returncode != 0:
        raise SystemExit(f"activating {action} failed: {result.stderr.strip()}")
    return result.stdout


def action_names() -> list[str]:
    result = subprocess.run(
        [
            "gdbus",
            "call",
            "--session",
            "--dest",
            APPLICATION_ID,
            "--object-path",
            OBJECT_PATH,
            "--method",
            "org.gtk.Actions.List",
        ],
        capture_output=True,
        text=True,
        timeout=30,
    )
    if result.returncode != 0:
        return []
    import re

    # The output is ([...],) and the names contain hyphens.
    return re.findall(r"'([^']+)'", result.stdout)


def wait_for_no_instance(seconds: float = 20.0) -> bool:
    """Wait until no browser owns the application name.

    The shell is single-instance, so launching while a previous run's process is
    still shutting down hands the new arguments to that one and exits: the run
    then drives an instance it does not own, and fails somewhere unrelated a few
    steps later. Waiting for the name to be unowned first removes that race.
    """
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if not action_names():
            return True
        time.sleep(0.5)
    return False


def query(profile: pathlib.Path, sql: str) -> list[str]:
    connection = sqlite3.connect(f"file:{profile}/session.sqlite?mode=ro", uri=True)
    try:
        return [row[0] for row in connection.execute(sql)]
    finally:
        connection.close()


def session_tabs(profile: pathlib.Path) -> list[tuple[int, str, int]]:
    """The saved session: (position, url, selected) in tab order."""
    connection = sqlite3.connect(f"file:{profile}/session.sqlite?mode=ro", uri=True)
    try:
        return [
            (int(row[0]), row[1], int(row[2]))
            for row in connection.execute(
                "SELECT position, url, selected FROM session_tabs ORDER BY position"
            )
        ]
    finally:
        connection.close()


def selected_url(profile: pathlib.Path) -> str | None:
    for _, url, selected in session_tabs(profile):
        if selected:
            return url
    return None


def path_hits(paths: list[str], wanted: str) -> int:
    return sum(1 for path in paths if path == wanted)


def clipboard_text() -> str | None:
    """What the clipboard currently holds, or None if it cannot be read."""
    if not shutil.which("wl-paste"):
        return None
    try:
        result = subprocess.run(
            ["wl-paste", "--no-newline"], capture_output=True, text=True, timeout=5
        )
    except subprocess.TimeoutExpired:
        return None
    return result.stdout if result.returncode == 0 else None


def our_toplevels() -> list[str]:
    """Titled windows the compositor says belong to this application."""
    if not shutil.which("hyprctl"):
        return []
    result = subprocess.run(
        ["hyprctl", "clients", "-j"], capture_output=True, text=True, timeout=10
    )
    try:
        clients = json.loads(result.stdout or "[]")
    except json.JSONDecodeError:
        return []
    return [
        str(client.get("title") or "")
        for client in clients
        if client.get("class") == APPLICATION_ID
    ]


def navigate_to(url: str, profile: pathlib.Path, attempts: int = 3) -> bool:
    """Type a URL into the address entry and wait for it to reach history."""
    for _ in range(attempts):
        if not press("l", ctrl=True):
            return False
        time.sleep(0.5)
        if not type_text(url):
            return False
        if not press("Return"):
            return False
        if wait_for(
            lambda: url in query(profile, "SELECT url FROM history"), seconds=8
        ):
            return True
    return False


def closed_port() -> int:
    """A loopback port with nothing listening on it.

    Bound and released rather than guessed, so the port is one that was free a
    moment ago instead of one that might be served by something else.
    """
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


def web_process_children(pid: int) -> list[int]:
    """The browser's direct children, read from procfs rather than by name."""
    children: list[int] = []
    for task in pathlib.Path(f"/proc/{pid}/task").glob("*"):
        listed = (task / "children")
        try:
            children.extend(int(part) for part in listed.read_text().split())
        except OSError:
            continue
    return children


def web_process_pids(pid: int) -> list[int]:
    """Every process in this browser's tree that is a WebKit web process.

    Found through the browser's own process tree, never by a pattern over every
    process on the machine, and only where the command line names
    WebKitWebProcess. Under WebKitGTK 6.0 the sandbox means these run as
    ``bwrap``, so ``comm`` says bubblewrap for every WebKit helper and cannot
    tell them apart; the command line can.
    """
    found: list[int] = []
    for child in web_process_children(pid):
        for candidate in (child, *web_process_children(child)):
            try:
                command_line = pathlib.Path(f"/proc/{candidate}/cmdline").read_bytes()
            except OSError:
                continue
            if b"WebKitWebProcess" not in command_line:
                continue
            found.append(candidate)
    return found


def kill_web_process(pid: int) -> list[int]:
    """End this run's web processes, deepest first, and return what was killed.

    Deepest first because the outer process is the sandbox wrapper: killing it
    alone leaves the process inside running and WebKit simply starts another, so
    nothing is ever reported as having died.
    """
    killed: list[int] = []
    for candidate in reversed(web_process_pids(pid)):
        try:
            os.kill(candidate, signal.SIGKILL)
        except ProcessLookupError:
            continue
        killed.append(candidate)
    return killed


def wait_for(effect, seconds: float = 30.0, interval: float = 0.5) -> bool:
    """Poll ``effect`` until it is true or the budget runs out."""
    deadline = time.monotonic() + seconds
    while True:
        if effect():
            return True
        if time.monotonic() >= deadline:
            return False
        time.sleep(interval)


def press_until(
    key: str,
    effect,
    *,
    ctrl: bool = False,
    shift: bool = False,
    attempts: int = 6,
    settle: float = 1.5,
) -> bool:
    """Press a key until ``effect`` reports the change, or give up.

    A mapped window does not hold keyboard focus the instant it appears, so a
    single press issued right after startup can land nowhere. Retrying cannot
    paper over a dead accelerator: a binding that does nothing leaves the state
    unchanged, so every attempt fails and the check fails.
    """
    for _ in range(attempts):
        press(key, ctrl=ctrl, shift=shift)
        if wait_for(effect, seconds=settle, interval=0.25):
            return True
    return False


class Browser:
    def __init__(self, profile: pathlib.Path, url: str | None) -> None:
        self.profile = profile
        self.log = tempfile.NamedTemporaryFile(  # a file, not a pipe: an undrained
            prefix=LOG_PREFIX,  # pipe would block the browser mid-test
            suffix=".log",
            delete=False,
        )
        arguments = [str(BINARY), "--profile-dir", str(profile)]
        # No URL means "reopen the browser", which is what a person does and what
        # a session-restore check has to test. Passing a URL is a *different*
        # thing: it opens that page on top of the restored session.
        if url is not None:
            arguments += ["--start-url", url]
        self.process = subprocess.Popen(
            arguments,
            cwd=ROOT,
            stdout=self.log,
            stderr=subprocess.STDOUT,
            text=True,
        )
        self._wait_ready()

    def _wait_ready(self) -> None:
        deadline = time.monotonic() + READY_SECONDS
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise SystemExit(f"browser exited early:\n{self.output()}")
            if "new-tab" in action_names():
                return
            time.sleep(0.4)
        raise SystemExit(f"browser never registered its actions:\n{self.output()}")

    def output(self) -> str:
        self.log.flush()
        return pathlib.Path(self.log.name).read_text(encoding="utf-8", errors="replace")

    def alive(self) -> bool:
        return self.process.poll() is None

    def stop(self) -> None:
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGTERM)
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
        self.log.close()
        pathlib.Path(self.log.name).unlink(missing_ok=True)


def main() -> int:
    if not os.environ.get("DISPLAY") and not os.environ.get("WAYLAND_DISPLAY"):
        print("no display available; skipping the GUI contract", file=sys.stderr)
        return 0
    if not BINARY.exists():
        raise SystemExit(f"missing {BINARY}; run `make build` first")
    if not have_wtype():
        print("wtype is not installed; skipping the key-press checks", file=sys.stderr)
        return 0

    clear_leftovers()
    checks: list[dict[str, object]] = []

    def check(name: str, passed: bool, details: dict) -> None:
        checks.append({"name": name, "passed": bool(passed), "details": details})

    with LocalFixtureServer() as server, temporary_profile() as profile_dir:
        profile = pathlib.Path(profile_dir)
        first = f"{server.base_url}/one.html"
        second = f"{server.base_url}/two.html"
        if not wait_for_no_instance():
            raise SystemExit(
                "another browser still owns the application name; refusing to start a"
                " second one, because this run would drive an instance it does not own"
            )
        browser = Browser(profile, first)
        try:
            time.sleep(3)
            tables = read_database(profile / "session.sqlite")["tables"]
            # The whole action surface at once: a missing registration is a
            # silent way to ship a dead button or a dead shortcut.
            expected_actions = {
                "new-tab", "new-private-tab", "close-tab", "next-tab",
                "previous-tab", "reload", "bookmark", "bookmarks", "history",
                "focus-address", "copy-page-address", "readability",
                "zoom-in", "zoom-out", "zoom-reset", "print", "restore-closed",
                "clear-browsing-data", "import-bookmarks", "quit",
            }
            registered = set(action_names())
            check(
                "every_expected_action_is_registered",
                expected_actions <= registered,
                {
                    "missing": sorted(expected_actions - registered),
                    "registered": sorted(registered),
                },
            )

            check(
                "window_starts_and_creates_schema",
                browser.alive()
                and all(name in tables for name in ("bookmarks", "history", "session_tabs")),
                {"tables": tables},
            )

            # The first page landing in history is the app's own signal that the
            # view exists and the load finished, which is also the point at which
            # the window can hold the keyboard.
            first_load = wait_for(
                lambda: first in query(profile, "SELECT url FROM history"), seconds=45
            )
            check(
                "start_url_reaches_history",
                first_load,
                {"history": query(profile, "SELECT url FROM history")},
            )
            time.sleep(1.5)

            # The bookmark accelerator itself, not just the action behind it.
            press_until("d", lambda: query(profile, "SELECT url FROM bookmarks") == [first], ctrl=True)
            after_first = query(profile, "SELECT url FROM bookmarks")
            press_until("d", lambda: query(profile, "SELECT url FROM bookmarks") == [], ctrl=True)
            after_second = query(profile, "SELECT url FROM bookmarks")
            check(
                "ctrl_d_bookmarks_and_unbookmarks_the_page",
                after_first == [first] and after_second == [],
                {
                    "after_first_press": after_first,
                    "after_second_press": after_second,
                    "expected": [first],
                },
            )

            # Ctrl+L, typing, and Enter have to reach the address entry of the
            # focused tab, and the result has to reach history. Whether the entry
            # ended up focused is not observable from outside, so the whole
            # sequence is repeated a few times: a dead shortcut never navigates,
            # so every attempt fails and the check fails with it.
            navigated = navigate_to(second, profile, attempts=4)
            history = query(profile, "SELECT url FROM history ORDER BY id")
            check(
                "ctrl_l_typing_and_enter_navigates",
                navigated,
                {"history": history, "expected_to_contain": second},
            )

            # The field can offer the addresses this profile has been to, and it
            # does not offer them on its own: typing narrows them, and the rows
            # come up only when the field is asked - the arrow key, or the control
            # at the end of it. Every step is recorded, because the harness
            # refuses to type when the keyboard is not ours and says so by
            # returning False, and a check that reported "nothing was offered"
            # when it had typed nothing would be worse than no check.
            def offered() -> int:
                return browser.output().count("showing ") + browser.output().count(
                    "put the addresses"
                )

            before = offered()
            focused = wait_for_own_focus(seconds=8.0)
            pressed = press("l", ctrl=True) if focused else False
            time.sleep(0.5)
            # The port is the part every address from this server shares, and the
            # host is not: the server may be reached as 127.0.0.1 or as localhost
            # depending on how it was bound, and only the port is in both.
            typed = type_text(server.base_url.rsplit(":", 1)[-1]) if pressed else False
            time.sleep(1.5)
            offered_by_typing = offered() > before
            # Asked for, with the arrow key.
            asked = press("Down") if typed else False
            time.sleep(1.5)
            shown_after_asking = offered() > before + (1 if offered_by_typing else 0)
            # And put away again with Escape.
            put_away = press("Escape") if asked else False
            time.sleep(1.5)
            # A reload rewrites the field from the shell's side, which must not
            # cost the field its ability to be asked.
            activate("reload")
            time.sleep(3)
            after_reload = press("l", ctrl=True) and press("Down")
            time.sleep(1.5)
            still_askable = offered() > (before + 1 + (1 if put_away else 0))
            check(
                "the_field_offers_addresses_only_when_asked",
                typed
                and not offered_by_typing
                and shown_after_asking
                and put_away
                and after_reload
                and still_askable,
                {
                    "focused": focused,
                    "ctrl_l_pressed": pressed,
                    "typed": typed,
                    "history": query(profile, "SELECT url FROM history"),
                    "reads": browser.output().count("read "),
                    "offered_by_typing_alone": offered_by_typing,
                    "shown_after_asking": shown_after_asking,
                    "put_away_with_escape": put_away,
                    "still_askable_after_a_reload": still_askable,
                },
            )

            # Reload has to reach the engine, which is observable as the page being
            # fetched a second time.
            before_reload = path_hits(server.requested, "/two.html")
            activate("reload")
            reloaded = wait_for(
                lambda: path_hits(server.requested, "/two.html") > before_reload,
                seconds=20,
            )
            check(
                "reload_refetches_the_current_page",
                reloaded,
                {
                    "hits_before": before_reload,
                    "hits_after": path_hits(server.requested, "/two.html"),
                },
            )

            # Tab lifecycle, asserted through the saved session because that is
            # the only record of tab structure a test can read. The session is
            # written when the selection changes and when a tab closes, so a close
            # is what makes the remaining tabs visible.
            activate("new-tab")
            time.sleep(1)
            navigate_to(f"{server.base_url}/one.html", profile, attempts=3)
            two_tabs = wait_for(
                lambda: len(session_tabs(profile)) >= 1, seconds=20
            )
            activate("close-tab")
            after_close = wait_for(
                lambda: [url for _, url, _ in session_tabs(profile)] == [second],
                seconds=20,
            )
            check(
                "closing_a_tab_removes_it_from_the_saved_session",
                two_tabs and after_close,
                {
                    "expected_after_close": [second],
                    "session": session_tabs(profile),
                },
            )

            # Ctrl+Shift+T has to bring the tab back. This is the binding that
            # exposed the shifted-accelerator trap: the action worked while the
            # shortcut did nothing, because <Primary><Shift>t can never match.
            restored = press_until(
                "t",
                lambda: len(session_tabs(profile)) == 2
                and f"{server.base_url}/one.html"
                in [url for _, url, _ in session_tabs(profile)],
                ctrl=True,
                shift=True,
                attempts=3,
                settle=3.0,
            )
            check(
                "a_closed_tab_is_reopened_by_ctrl_shift_t",
                restored,
                {"expected_two_tabs": 2, "session": session_tabs(profile)},
            )

            # Cycling has to move the selected flag, which is persisted on every
            # switch, so this is a real record of which tab is in front. It only
            # means something with more than one tab open, which is asserted rather
            # than assumed.
            tabs_open = len(session_tabs(profile))
            session_before = session_tabs(profile)
            before_cycle = selected_url(profile)
            activate("next-tab")
            moved = wait_for(
                lambda: selected_url(profile) not in (None, before_cycle), seconds=15
            )
            session_after_next = session_tabs(profile)
            after_next = selected_url(profile)
            activate("previous-tab")
            returned = wait_for(
                lambda: selected_url(profile) == before_cycle, seconds=15
            )
            check(
                "tab_cycling_moves_the_selected_tab",
                tabs_open > 1 and moved and returned,
                {
                    "tabs_open": tabs_open,
                    "before": before_cycle,
                    "after_next": after_next,
                    "after_previous": selected_url(profile),
                    "session_before": session_before,
                    "session_after_next": session_after_next,
                },
            )

            # A private tab has to be invisible to the profile: not in history,
            # not in the saved session, and not bookmarkable. The page announces
            # itself first, so its absence from those tables is a fact about a page
            # that really loaded.
            activate("new-private-tab")
            time.sleep(1)
            private_url = f"{server.base_url}/private.html"
            navigate_to(private_url, profile, attempts=3)
            private_loaded = wait_for(
                lambda: "/private-tab-loaded" in server.requested, seconds=20
            )
            time.sleep(1.5)
            check(
                "a_private_tab_is_never_written_to_the_profile",
                private_loaded
                and private_url not in query(profile, "SELECT url FROM history")
                and private_url not in [url for _, url, _ in session_tabs(profile)],
                {
                    "page_loaded": private_loaded,
                    "history": query(profile, "SELECT url FROM history"),
                    "session": session_tabs(profile),
                },
            )
            activate("close-tab")
            time.sleep(1)

            # A download whose server-supplied name tries to escape the download
            # directory has to land inside the profile as a plain leaf. It is
            # driven through the address bar rather than navigate_to, because a
            # download never becomes a history row and waiting for one would only
            # retry the same download.
            press("l", ctrl=True)
            time.sleep(0.5)
            type_text(f"{server.base_url}/download")
            press("Return")
            download = profile / "downloads" / "evil-payload.txt"
            arrived = wait_for(download.exists, seconds=40)
            mode = oct(download.stat().st_mode & 0o777) if download.exists() else None
            escaped = sorted(
                str(path)
                for path in profile.parent.glob("evil-payload.txt")
            )
            check(
                "a_download_is_sanitised_into_the_profile",
                arrived
                and download.read_text(encoding="utf-8") == "payload\n"
                and not escaped
                and mode == "0o600",
                {
                    "arrived": arrived,
                    "path": str(download),
                    "mode": mode,
                    "escaped_to": escaped,
                    "downloads_dir": sorted(
                        str(path) for path in (profile / "downloads").glob("*")
                    )
                    if (profile / "downloads").is_dir()
                    else "no downloads directory",
                    "log": browser.output()[-400:],
                },
            )

            # The readability pass has to actually change the page. measure.html
            # reports its computed max-width on a timer, so applying and removing
            # the sheet is visible as a change in what the page reports.
            measure_url = f"{server.base_url}/measure.html"
            navigate_to(measure_url, profile, attempts=3)
            wait_for(lambda: any(p.startswith("/measure-") for p in server.requested), seconds=20)
            before_readable = {
                path for path in server.requested if path.startswith("/measure-")
            }
            activate("readability")
            wait_for(
                lambda: bool(
                    {
                        path
                        for path in server.requested
                        if path.startswith("/measure-")
                    }
                    - before_readable
                ),
                seconds=20,
            )
            after_enabling = {
                path for path in server.requested if path.startswith("/measure-")
            }
            changed = after_enabling - before_readable
            activate("readability")
            after_toggle = {
                path for path in server.requested if path.startswith("/measure-")
            }
            check(
                "the_readability_pass_changes_the_page_layout",
                bool(changed) and len(after_toggle) > len(before_readable),
                {
                    "before": sorted(before_readable),
                    "new_after_enabling": sorted(changed),
                    "after": sorted(after_toggle),
                },
            )

            # Copying the address has to reach the clipboard, which is the only
            # place that value is observable from outside the process.
            activate("copy-page-address")
            copied = wait_for(
                lambda: (clipboard_text() or "") == measure_url, seconds=10
            )
            check(
                "copy_page_address_reaches_the_clipboard",
                copied,
                {"expected": measure_url, "clipboard": clipboard_text()},
            )

            # A URL that cannot be served has to be reported rather than left as a
            # blank page. The port is bound and released first, so nothing is
            # listening on it and the failure is certain rather than likely.
            dead_port = closed_port()
            dead_url = f"http://127.0.0.1:{dead_port}/missing.html"
            navigate_to(dead_url, profile, attempts=3)
            reported = wait_for(
                lambda: "could not load" in browser.output()
                and dead_url in browser.output(),
                seconds=20,
            )
            check(
                "a_failed_load_names_the_url_and_the_error",
                reported,
                {
                    "url": dead_url,
                    "log": browser.output()[-400:],
                },
            )

            # A capability request must be refused. What can be asserted depends
            # on the machine, and the difference is recorded rather than hidden:
            #
            # * If WebKit raises the request, the shell has to answer it with a
            #   refusal. The dialog's default response is Deny and Return
            #   activates it, and the page reports what it was told, so granting
            #   by accident fails the check.
            # * If WebKit never raises one, nothing was tested. That happens on a
            #   machine with no camera, where WebKitGTK 6.0 fails a video request
            #   during device enumeration (OverconstrainedError, "no device was
            #   found amongst 0 devices") and answers a notification request with
            #   "denied" on its own, so the app is never asked. The check then
            #   passes with verified: false rather than pretending, and it starts
            #   failing the moment a request does arrive.
            activate("new-tab")
            time.sleep(1)
            permission_url = f"{server.base_url}/permission.html"
            navigate_to(permission_url, profile, attempts=3)
            page_loaded = wait_for(
                lambda: "/permission.html" in server.requested, seconds=20
            )
            asked = wait_for(
                lambda: "asking for a capability" in browser.output(), seconds=20
            )
            if asked:
                # The test answers its own dialog; clicking Allow would fail the
                # check, which is the point.
                press("Return")
            answered = wait_for(
                lambda: any(
                    path.startswith("/permission-") for path in server.requested
                ),
                seconds=25,
            )
            outcomes = [
                path
                for path in server.requested
                if path.startswith("/permission-") and "never-answered" not in path
            ]
            granted = [path for path in outcomes if "granted" in path]
            if asked:
                verified = answered and len(outcomes) == 1 and not granted
            else:
                verified = not granted
            check(
                "a_capability_request_is_refused_by_default",
                page_loaded and verified,
                {
                    "verified": asked,
                    "page_loaded": page_loaded,
                    "shell_raised_the_dialog": asked,
                    "outcomes": outcomes,
                    "granted": granted,
                    "reason_if_unverified": (
                        None
                        if asked
                        else "this machine cannot raise a capability request: WebKitGTK 6.0"
                        " fails video at device enumeration and denies notifications"
                        " itself, so the app is never asked"
                    ),
                    "log": browser.output()[-400:],
                },
            )

            # A web process that dies has to be reported. The processes are found
            # through the browser's own tree and named by their command line
            # before any signal, so nothing outside this run is ever touched.
            killed = kill_web_process(browser.process.pid)
            crash_reported = bool(killed) and wait_for(
                lambda: "stopped responding" in browser.output(), seconds=25
            )
            check(
                "a_dead_web_process_is_reported",
                crash_reported,
                {
                    "killed": killed,
                    "log": browser.output()[-400:],
                },
            )

            # Clearing browsing data has to reach the profile, not just the UI.
            before_clear = query(profile, "SELECT url FROM history")
            activate("clear-browsing-data")
            time.sleep(3)
            after_clear = query(profile, "SELECT url FROM history")
            check(
                "clear_browsing_data_erases_history",
                bool(before_clear) and after_clear == [],
                {
                    "before": before_clear,
                    "after": after_clear,
                    "log": browser.output()[-300:],
                },
            )

            activate("new-tab")
            time.sleep(1)
            activate("readability")
            activate("next-tab")
            activate("previous-tab")
            press("h", ctrl=True)
            time.sleep(1)

            # Ctrl+Shift+B has to open the bookmarks window, and it has to do so
            # while a *library* window holds the keyboard: that is the case that
            # was broken, because the shifted-letter shortcuts are served by a key
            # controller and a controller belongs to one window, so the library
            # windows each needed their own. The compositor is the only thing that
            # can see a window appear, so this check needs hyprctl and says it was
            # not run when there is none.
            before_windows = len(our_toplevels())
            press_until(
                "b",
                lambda: len(our_toplevels()) > before_windows,
                ctrl=True,
                shift=True,
                attempts=3,
                settle=2.0,
            )
            after_windows = our_toplevels()
            check(
                "ctrl_shift_b_opens_the_bookmarks_window",
                bool(our_toplevels()) and len(after_windows) > before_windows,
                {
                    "windows_before": before_windows,
                    "windows_after": len(after_windows),
                    "titles": after_windows,
                    "compositor_queryable": bool(our_toplevels()),
                },
            )

            check(
                "actions_and_keys_leave_the_window_alive",
                browser.alive(),
                {"alive": browser.alive(), "log": browser.output()[-400:]},
            )

            # Session restore, which can only be observed by starting a second
            # browser on the same profile, so it comes last. The first browser is
            # stopped first: this shell is single-instance, so a second launch
            # while one is running would become a tab in it instead of restoring
            # anything.
            saved = session_tabs(profile)
            browser.stop()
            browser = Browser(profile, None)
            restored = wait_for(
                lambda: [url for _, url, _ in session_tabs(profile)]
                == [url for _, url, _ in saved],
                seconds=30,
            )
            check(
                "a_relaunch_restores_the_saved_tabs",
                restored and bool(saved),
                {
                    "saved": saved,
                    "after_relaunch": session_tabs(profile),
                    "log": browser.output()[-300:],
                },
            )

            # A URL given at launch must not be swallowed by a session that
            # happens to be on disk. Getting this wrong makes a click look like
            # it did nothing: the old tabs come back and the page asked for never
            # appears.
            saved_now = session_tabs(profile)
            link = f"{server.base_url}/one.html"
            browser.stop()
            browser = Browser(profile, link)
            opened = wait_for(
                lambda: link in [url for _, url, _ in session_tabs(profile)],
                seconds=30,
            )
            after_link = session_tabs(profile)
            check(
                "a_url_given_at_launch_opens_even_with_a_saved_session",
                opened and len(after_link) == len(saved_now) + 1,
                {
                    "url": link,
                    "before": saved_now,
                    "after": after_link,
                    "log": browser.output()[-300:],
                },
            )
        except WtypeFailed as error:
            # Input could not be delivered at all, so the key-press checks above
            # never got a fair run. Record that as its own failure instead of
            # aborting, so the report still lists what did pass.
            check(
                "key_presses_reached_the_window",
                False,
                {"error": str(error), "log": browser.output()[-400:]},
            )
        finally:
            browser.stop()

    passed = all(bool(check["passed"]) for check in checks)
    degraded = int(KEYBOARD["foreign_focus_refusals"]) > 0 or int(
        KEYBOARD["delivery_failures"]
    ) > 0
    if degraded:
        # Said out loud rather than buried: a refusal is not a failed check, but
        # it does mean a keystroke was skipped, and the run was not a clean one.
        print(
            f"warning: keyboard degraded: {KEYBOARD['foreign_focus_refusals']} press(es)"
            f" refused because another window had focus"
            f" ({KEYBOARD['refused_classes']}),"
            f" {KEYBOARD['delivery_failures']} delivery failure(s)",
            file=sys.stderr,
        )
    report = {
        "passed": passed,
        "binary": str(BINARY.relative_to(ROOT)),
        "driver": "GApplication actions over the session bus, plus wtype key presses",
        "checks": checks,
        "keyboard": {
            **KEYBOARD,
            "degraded": degraded,
            "rule": (
                "a key press is only delivered while the focused window is "
                f"{APPLICATION_ID}; refusals are counted, not typed anyway"
            ),
            "focus_queryable": KEYBOARD["focus_known"],
        },
        "not_covered": [
            "visual results are checked by screenshot, not by an assertion",
            "WebKitWebDriver cannot drive this app in WebKitGTK 6.0",
            "the deny-by-default answer is only asserted where WebKit raises a"
            " capability request; a machine with no camera cannot raise one, and the"
            " check then reports verified: false instead of claiming a pass",
        ],
    }
    REPORT.parent.mkdir(parents=True, exist_ok=True)
    REPORT.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    for item in checks:
        print(("PASS" if item["passed"] else "FAIL"), item["name"])
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
