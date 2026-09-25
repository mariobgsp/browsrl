#!/usr/bin/env python3
"""GUI contract for R Browse, driven through the window it really opens.

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
import sqlite3
import subprocess
import sys
import tempfile
import time

from harness import BINARY, LocalFixtureServer, ROOT, read_database, temporary_profile


REPORT = ROOT / "artifacts" / "e2e-gui" / "report.json"
APPLICATION_ID = "io.github.rbrowse.RBrowse"
OBJECT_PATH = "/io/github/rbrowse/RBrowse"
READY_SECONDS = 30

# Only leftovers from this harness are cleared: they are recognisable by their
# temporary profile directory. A browser the person started is left alone.
LEFTOVER_MARKER = "rbrowse-gui-"


def have_wtype() -> bool:
    return shutil.which("wtype") is not None


def clear_leftovers() -> None:
    listing = subprocess.run(["pgrep", "-f", str(BINARY)], capture_output=True, text=True)
    for pid in listing.stdout.split():
        if not pid.isdigit() or int(pid) == os.getpid():
            continue
        command = subprocess.run(
            ["/proc", pid, "cmdline"], capture_output=True, text=True
        ).stdout
        if LEFTOVER_MARKER not in command:
            continue
        try:
            os.kill(int(pid), signal.SIGTERM)
        except ProcessLookupError:
            pass
    if listing.stdout.split():
        time.sleep(1.5)


def press(key: str, ctrl: bool = False, shift: bool = False) -> None:
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
    result = subprocess.run(command, capture_output=True, text=True, timeout=30)
    if result.returncode != 0:
        raise SystemExit(f"wtype failed: {result.stderr.strip()}")


def type_text(text: str) -> None:
    result = subprocess.run(
        ["wtype", text], capture_output=True, text=True, timeout=30
    )
    if result.returncode != 0:
        raise SystemExit(f"wtype failed: {result.stderr.strip()}")


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


def query(profile: pathlib.Path, sql: str) -> list[str]:
    connection = sqlite3.connect(f"file:{profile}/session.sqlite?mode=ro", uri=True)
    try:
        return [row[0] for row in connection.execute(sql)]
    finally:
        connection.close()


class Browser:
    def __init__(self, profile: pathlib.Path, url: str) -> None:
        self.profile = profile
        self.log = tempfile.NamedTemporaryFile(  # a file, not a pipe: an undrained
            prefix=LEFTOVER_MARKER,  # pipe would block the browser mid-test
            suffix=".log",
            delete=False,
        )
        self.process = subprocess.Popen(
            [str(BINARY), "--profile-dir", str(profile), "--start-url", url],
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
        browser = Browser(profile, first)
        try:
            time.sleep(3)
            tables = read_database(profile / "session.sqlite")["tables"]
            check(
                "window_starts_and_creates_schema",
                browser.alive()
                and all(name in tables for name in ("bookmarks", "history", "session_tabs")),
                {"tables": tables},
            )

            # The bookmark accelerator itself, not just the action behind it.
            press("d", ctrl=True)
            time.sleep(1.5)
            after_first = query(profile, "SELECT url FROM bookmarks")
            press("d", ctrl=True)
            time.sleep(1.5)
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
            # focused tab, and the result has to reach history.
            press("l", ctrl=True)
            time.sleep(0.5)
            type_text(second)
            press("Return")
            time.sleep(3)
            history = query(profile, "SELECT url FROM history ORDER BY id")
            check(
                "ctrl_l_typing_and_enter_navigates",
                second in history,
                {"history": history, "expected_to_contain": second},
            )

            activate("new-tab")
            time.sleep(1)
            activate("readability")
            activate("next-tab")
            activate("previous-tab")
            press("h", ctrl=True)
            time.sleep(1)
            check(
                "actions_and_keys_leave_the_window_alive",
                browser.alive(),
                {"alive": browser.alive(), "log": browser.output()[-400:]},
            )
        finally:
            browser.stop()

    passed = all(bool(check["passed"]) for check in checks)
    report = {
        "passed": passed,
        "binary": str(BINARY.relative_to(ROOT)),
        "driver": "GApplication actions over the session bus, plus wtype key presses",
        "checks": checks,
        "not_covered": [
            "visual results are checked by screenshot, not by an assertion",
            "WebKitWebDriver cannot drive this app in WebKitGTK 6.0",
        ],
    }
    REPORT.parent.mkdir(parents=True, exist_ok=True)
    REPORT.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    for item in checks:
        print(("PASS" if item["passed"] else "FAIL"), item["name"])
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
