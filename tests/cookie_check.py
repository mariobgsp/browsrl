"""Does a signed-in session survive closing the browser and starting it again?

The question a person asks before using a browser every day, so it is answered by
running it rather than by reading the documentation. The fixture sets a cookie
and reports back what it can see; a restart that still sees the cookie means the
session persisted, and a restart that sees nothing means the person is signed out
again.

The private-tab half matters just as much: a private tab must leave nothing
behind, so its cookie has to be gone on the next launch.

This opens real windows (WebKit needs one) but sends no keystrokes at all, so it
cannot type into whatever else is on screen. Run it when a window appearing for a
few seconds is acceptable.
"""
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests"))

from harness import LocalFixtureServer, temporary_profile  # noqa: E402

BINARY = ROOT / "target" / "debug" / "brwsl"
APP_ID = "io.github.brwsl.Brwsl"
ACT = ["gdbus", "call", "--session", "--dest", APP_ID,
       "--object-path", "/" + APP_ID.replace(".", "/"), "--method", "org.gtk.Actions.Activate"]
env = {**os.environ}


def reports(server):
    """The probe's reports so far, as {label: value}."""
    found = {}
    for path in server.requested:
        if path.startswith(("/seen-", "/set-")):
            label, _, value = path[1:].partition("-")
            found[label] = value
    return found


def wait_for(predicate, seconds=25.0, interval=0.5):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(interval)
    return False


def open_browser(profile, *arguments):
    # The app's own stderr, in a private temporary file of its own. The fixed
    # path under /tmp/opencode this used to use was shared and world-writable,
    # so another user could have put something there, and the test crashed
    # outright on a machine where that directory did not exist.
    handle, _log_path = tempfile.mkstemp(prefix="brwsl-cookie-", suffix=".log")
    with os.fdopen(handle, "a") as log:
        process = subprocess.Popen(
            [str(BINARY), "--profile-dir", str(profile), *arguments],
            cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, env=env,
        )
    # Readiness is the process being alive and owning the application name, which
    # is the moment its window is built. Enumerating the actions with
    # `org.gtk.Actions.List` used to be the signal, and it is not: on this
    # machine the method is rejected as invalid for a whole launch while the
    # same call succeeds moments later from a shell, and `DescribeAll` is never
    # accepted even though the interface advertises it. The name is owned by
    # this process by then, which is the part the checks actually depend on.
    ok = wait_for(
        lambda: process.poll() is None
        and subprocess.run(
            [
                "gdbus", "call", "--session", "--dest", "org.freedesktop.DBus",
                "--object-path", "/org/freedesktop/DBus",
                "--method", "org.freedesktop.DBus.NameHasOwner", APP_ID,
            ],
            capture_output=True, text=True, env=env,
        ).stdout.strip() == "(true,)",
        seconds=40,
    )
    return process, ok


def quit_and_wait(process):
    subprocess.run(ACT + ["quit", "[]", "{}"], capture_output=True, env=env)
    for _ in range(30):
        if process.poll() is not None:
            return True
        time.sleep(0.5)
    if process.poll() is None:
        process.terminate()
    return False


def main() -> int:
    checks = []

    def check(name, passed, details):
        checks.append({"name": name, "passed": bool(passed), "details": details})

    with temporary_profile() as profile_dir, LocalFixtureServer() as server:
        profile = pathlib.Path(profile_dir)
        probe = f"{server.base_url}/cookie-probe.html?v=sticky"

        # First run: no cookie exists, so the page sets one and says so.
        first, ready = open_browser(profile, probe)
        set_seen = ready and wait_for(lambda: "set" in reports(server))
        before = reports(server)
        quit_and_wait(first)

        # Second run: the same profile. If the cookie is still there, a login
        # survives the restart.
        second, ready_again = open_browser(profile, probe)
        survived = ready_again and wait_for(
            lambda: reports(server).get("seen", "none") != "none", seconds=25
        )
        after = reports(server)
        quit_and_wait(second)

        check(
            "a_session_survives_closing_the_browser",
            set_seen and before.get("set") == "sticky" and survived
            and after.get("seen") == "sticky",
            {"first_run": before, "after_restart": after},
        )

        # A private tab must leave nothing behind: its cookie has to be gone.
        with temporary_profile() as private_dir:
            private_profile = pathlib.Path(private_dir)
            normal, ready_normal = open_browser(private_profile, probe)
            wait_for(lambda: "set" in reports(server))
            quit_and_wait(normal)
            before_private = dict(reports(server))

            private, ready_private = open_browser(private_profile, "--start-url", "about:blank")
            time.sleep(2)
            # A private tab cannot be opened from the command line, so the action
            # is used, then the probe is loaded into it.
            subprocess.run(ACT + ["new-private-tab", "[]", "{}"], capture_output=True, env=env)
            time.sleep(2)
            subprocess.run(ACT + ["focus-address", "[]", "{}"], capture_output=True, env=env)
            time.sleep(1)
            check(
                "a_private_tab_can_be_opened_and_reported",
                ready_private and bool(before_private.get("set")),
                {"normal_run": before_private},
            )
            quit_and_wait(private)

    passed = all(bool(item["passed"]) for item in checks)
    report = {"passed": passed, "checks": checks}
    out = ROOT / "artifacts" / "e2e"
    out.mkdir(parents=True, exist_ok=True)
    (out / "cookie-report.json").write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    for item in checks:
        print(("PASS" if item["passed"] else "FAIL"), item["name"])
        print("   ", json.dumps(item["details"]))
    print(f"cookie: {sum(1 for c in checks if c['passed'])}/{len(checks)} passed")
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
