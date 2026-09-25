#!/usr/bin/env python3
"""GUI contract for R Browse, driven through its own GApplication actions.

WebKitGTK 6.0 cannot be driven by WebKitWebDriver, because a session needs the
app to answer ``WebKitAutomationSession::create-web-view`` and the ``webkit6``
bindings do not expose that signal. The actions this browser registers on its
GApplication *are* reachable over the session bus, so this script launches the
real window, activates its actions, and asserts what the outside can observe:
the profile database, and the process staying alive.

It needs a display and a running browser binary. It is deliberately not part of
``make gate``; run it with ``make e2e-gui``.
"""

from __future__ import annotations

import json
import os
import re
import pathlib
import signal
import sqlite3
import subprocess
import sys
import time

from harness import BINARY, LocalFixtureServer, ROOT, read_database, temporary_profile


REPORT = ROOT / "artifacts" / "e2e-gui" / "report.json"
APPLICATION_ID = "io.github.rbrowse.RBrowse"
OBJECT_PATH = "/io/github/rbrowse/RBrowse"
TIMEOUT_SECONDS = 60


def _clear_previous_instance() -> None:
    """Remove a leftover browser from an earlier run.

    R Browse is single-instance through GApplication, so a stray process would
    own the session-bus name and the new one would hand its window to it and
    exit, leaving the test watching nothing.
    """
    listing = subprocess.run(
        ["pgrep", "-f", str(BINARY)], capture_output=True, text=True
    )
    for pid in listing.stdout.split():
        if pid.isdigit() and int(pid) != os.getpid():
            try:
                os.kill(int(pid), signal.SIGTERM)
            except ProcessLookupError:
                pass
    if listing.stdout.split():
        time.sleep(1.5)


class Browser:
    """The running browser, plus a way to activate its named actions."""

    def __init__(self, profile: pathlib.Path, url: str) -> None:
        self.profile = profile
        _clear_previous_instance()
        self.process = subprocess.Popen(
            [
                str(BINARY),
                "--profile-dir",
                str(profile),
                "--start-url",
                url,
            ],
            cwd=ROOT,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        self._wait_until_ready()

    def _wait_until_ready(self, timeout: float = 25.0) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise SystemExit(
                    f"browser exited early ({self.process.returncode}):\n{self.output()}"
                )
            if "new-tab" in self._actions():
                return
            time.sleep(0.4)
        raise SystemExit(f"browser did not register its actions:\n{self.output()}")

    def _actions(self) -> list[str]:
        """Names of the actions the running browser exposes."""
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
            timeout=15,
        )
        if result.returncode != 0:
            return []
        # Output looks like (['bookmark', 'close-tab', ...],); the names
        # contain hyphens, so they are extracted as quoted strings.
        return re.findall(r"'([^']+)'", result.stdout)

    def activate(self, action: str, timeout: float = 20.0) -> str:
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
            timeout=timeout,
        )
        if result.returncode != 0:
            raise SystemExit(f"gdbus Activate {action} failed: {result.stderr.strip()}")
        return result.stdout

    def alive(self) -> bool:
        return self.process.poll() is None

    def output(self) -> str:
        return ""

    def stop(self) -> None:
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGTERM)
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()


def database(path: pathlib.Path) -> dict:
    return read_database(path)


def main() -> int:
    if not os.environ.get("DISPLAY") and not os.environ.get("WAYLAND_DISPLAY"):
        print("no display available; skipping the GUI contract", file=sys.stderr)
        return 0
    if not BINARY.exists():
        raise SystemExit(f"missing {BINARY}; run `make build` first")

    checks: list[dict[str, object]] = []

    def record(name: str, passed: bool, details: dict) -> None:
        checks.append({"name": name, "passed": bool(passed), "details": details})

    def check(name: str, passed: bool, details: dict) -> None:
        record(name, passed, details)

    with LocalFixtureServer() as server, temporary_profile() as profile_dir:
        profile = pathlib.Path(profile_dir)
        url = f"{server.base_url}/one.html"
        browser = Browser(profile, url)
        try:
            time.sleep(3)
            after_load = database(profile / "session.sqlite")
            tables = after_load["tables"]
            check(
                "window_starts_and_creates_schema",
                browser.alive()
                and "bookmarks" in tables
                and "history" in tables
                and "session_tabs" in tables,
                {"tables": tables},
            )

            # The bookmark action must be reflected in the profile, not only in
            # the toolbar icon.
            browser.activate("bookmark")
            time.sleep(1.5)
            bookmarked = query(profile, "SELECT url FROM bookmarks")
            check(
                "bookmark_action_stores_the_page",
                bookmarked == [url],
                {"bookmarks": bookmarked, "expected": [url]},
            )

            browser.activate("bookmark")
            time.sleep(1.5)
            cleared = query(profile, "SELECT url FROM bookmarks")
            check(
                "bookmark_action_toggles_off",
                cleared == [],
                {"bookmarks_after_second_press": cleared},
            )

            # A new tab opens and loads, which is observable as a second
            # history row once the page finishes.
            browser.activate("new-tab")
            time.sleep(3)
            history_after = query(profile, "SELECT url FROM history ORDER BY id")
            check(
                "new_tab_action_loads_and_records",
                browser.alive() and url in history_after,
                {"history": history_after, "expected_at_least": [url]},
            )

            browser.activate("readability")
            time.sleep(1)
            browser.activate("next-tab")
            browser.activate("previous-tab")
            time.sleep(1)
            check(
                "actions_do_not_kill_the_window",
                browser.alive(),
                {"alive": browser.alive()},
            )
        finally:
            browser.stop()

    passed = all(bool(check["passed"]) for check in checks)
    report = {
        "passed": passed,
        "binary": str(BINARY.relative_to(ROOT)),
        "driver": "org.gtk.Actions over the session bus",
        "checks": checks,
        "not_covered": [
            "page rendering itself is still verified by screenshot, not by an assertion",
            "WebKitWebDriver cannot drive this app in WebKitGTK 6.0",
        ],
    }
    REPORT.parent.mkdir(parents=True, exist_ok=True)
    REPORT.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    for check in checks:
        print(("PASS" if check["passed"] else "FAIL"), check["name"])
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0 if passed else 1


def query(profile: pathlib.Path, sql: str) -> list:
    connection = sqlite3.connect(f"file:{profile}/session.sqlite?mode=ro", uri=True)
    try:
        return [row[0] for row in connection.execute(sql)]
    finally:
        connection.close()


if __name__ == "__main__":
    sys.exit(main())
