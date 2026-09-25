#!/usr/bin/env python3
"""GUI contract for Browsrl, driven through the window it really opens.

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

from harness import (
    BINARY,
    PROFILE_PREFIX,
    LocalFixtureServer,
    ROOT,
    read_database,
    temporary_profile,
)


REPORT = ROOT / "artifacts" / "e2e-gui" / "report.json"
APPLICATION_ID = "io.github.browsrl.Browsrl"
OBJECT_PATH = "/io/github/browsrl/Browsrl"
READY_SECONDS = 30

# Names this harness's own log files, so they can be recognised and removed
# without touching anything else in the temporary directory.
LOG_PREFIX = "browsrl-gui-"


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
    run_wtype(command)


def type_text(text: str) -> None:
    run_wtype(["wtype", text])


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
    def __init__(self, profile: pathlib.Path, url: str) -> None:
        self.profile = profile
        self.log = tempfile.NamedTemporaryFile(  # a file, not a pipe: an undrained
            prefix=LOG_PREFIX,  # pipe would block the browser mid-test
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
            # The whole action surface at once: a missing registration is a
            # silent way to ship a dead button or a dead shortcut.
            expected_actions = {
                "new-tab", "new-private-tab", "close-tab", "next-tab",
                "previous-tab", "reload", "bookmark", "bookmarks", "history",
                "focus-address", "copy-page-address", "readability",
                "zoom-in", "zoom-out", "zoom-reset", "print", "restore-closed",
                "clear-browsing-data",
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
            navigated = False
            for _ in range(4):
                press("l", ctrl=True)
                time.sleep(0.5)
                type_text(second)
                press("Return")
                navigated = wait_for(
                    lambda: second in query(profile, "SELECT url FROM history"),
                    seconds=8,
                )
                if navigated:
                    break
            history = query(profile, "SELECT url FROM history ORDER BY id")
            check(
                "ctrl_l_typing_and_enter_navigates",
                navigated,
                {"history": history, "expected_to_contain": second},
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
            check(
                "actions_and_keys_leave_the_window_alive",
                browser.alive(),
                {"alive": browser.alive(), "log": browser.output()[-400:]},
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
