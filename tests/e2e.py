#!/usr/bin/env python3
"""Deterministic, offline end-to-end contract for the first Browsrl slice.

Every check runs the real binary against loopback fixtures or explicit CLI
input and asserts observable behaviour (exit code, JSON output, on-disk
permissions). Nothing here claims GUI coverage: a WebKit window is exercised
only by the manual procedure in the README.
"""

from __future__ import annotations

import json
import os
import pathlib
import stat
import sys
import tempfile

from harness import (
    BINARY,
    PROFILE_PREFIX,
    LocalFixtureServer,
    ROOT,
    read_database,
    run_browser,
    run_browser_smoke,
    run_browser_storage_check,
    temporary_profile,
    write_legacy_v1_database,
)


REPORT = ROOT / "artifacts" / "e2e" / "report.json"
SMOKE_PREFIX = "smoke check failed: "


def record(checks: list[dict[str, object]], name: str, passed: bool, details: dict[str, object]) -> None:
    checks.append({"name": name, "passed": bool(passed), "details": details})


def mode_of(path: pathlib.Path) -> str:
    return oct(stat.S_IMODE(path.stat().st_mode))


def force_mode(path: pathlib.Path, mode: int) -> None:
    os.chmod(path, mode)


def rejection_message(stderr: str) -> str:
    """The stable part of a smoke failure, without the fixed prefix."""
    return stderr.strip().removeprefix(SMOKE_PREFIX)


def main() -> int:
    checks: list[dict[str, object]] = []
    with LocalFixtureServer() as server, temporary_profile() as profile_dir:
        profile = pathlib.Path(profile_dir)
        first_url = f"{server.base_url}/one.html"

        first = run_browser_smoke(profile, first_url)
        first_data = json.loads(first.stdout)
        record(
            checks,
            "offline_fixture_url_and_sqlite_round_trip",
            first_data.get("ok") is True
            and first_data.get("requested") == first_url
            and first_data.get("reloaded") == first_url,
            {
                "fixture_path": "/one.html",
                "restored": first_data.get("restored"),
                "reloaded_matches": first_data.get("reloaded") == first_url,
            },
        )

        second = run_browser_smoke(profile, first_url)
        second_data = json.loads(second.stdout)
        record(
            checks,
            "profile_restore_input",
            second_data.get("restored") == first_url,
            {
                "restored_fixture_path": "/one.html",
                "restored_matches": second_data.get("restored") == first_url,
            },
        )

        # Security-relevant input, asserted against the exact message so a
        # rejection cannot pass for an unrelated error.
        lookalikes = {
            "file:///etc/passwd": "only http, https, and about:blank are supported",
            "javascript:alert(1)": "only http, https, and about:blank are supported",
            "data:text/html,x": "only http, https, and about:blank are supported",
            "HTTP://example.com/": "only http, https, and about:blank are supported",
            "//evil.example": "only http, https, and about:blank are supported",
            "about:config": "only about:blank is supported",
        }
        observed: dict[str, str] = {}
        for candidate in lookalikes:
            result = run_browser(
                "--smoke", "--profile-dir", str(profile), "--start-url", candidate, expect_code=1
            )
            observed[candidate] = rejection_message(result.stderr)
        record(
            checks,
            "navigation_rejects_non_web_and_lookalike_input",
            observed == lookalikes,
            {"expected_and_observed": observed},
        )

        bare_host = run_browser(
            "--smoke", "--profile-dir", str(profile), "--start-url", "example.com", expect_code=0
        )
        bare_host_data = json.loads(bare_host.stdout)
        record(
            checks,
            "bare_host_normalizes_to_https",
            bare_host_data.get("requested") == "https://example.com/",
            {"requested": bare_host_data.get("requested")},
        )

        blocked_search = run_browser(
            "--smoke", "--profile-dir", str(profile), "--start-url", "search:rust webkit", expect_code=1
        )
        endpoint_search = run_browser(
            "--smoke",
            "--profile-dir",
            str(profile),
            "--start-url",
            "search:rust webkit",
            "--search-endpoint",
            "https://duckduckgo.com/",
            expect_code=0,
        )
        endpoint_search_data = json.loads(endpoint_search.stdout)
        record(
            checks,
            "search_requires_explicit_endpoint",
            "search is disabled" in blocked_search.stderr
            and endpoint_search_data.get("requested") == "https://duckduckgo.com/?q=rust+webkit",
            {
                "without_endpoint": blocked_search.stderr.strip(),
                "with_endpoint": endpoint_search_data.get("requested"),
            },
        )

        # A search endpoint carries queries off the machine, so plain http is
        # refused except for a loopback host, and the same host rules as the
        # navigation layer apply.
        endpoints = {
            "http://example.com/": 2,
            "https://ex_ample.com/": 2,
            "file:///tmp/search": 2,
            "http://127.0.0.1:8080/": 0,
            "https://duckduckgo.com/": 0,
        }
        endpoint_codes = {
            endpoint: run_browser(
                "--smoke",
                "--profile-dir",
                str(profile),
                "--start-url",
                "search:query",
                "--search-endpoint",
                endpoint,
                expect_code=code,
            ).returncode
            for endpoint, code in endpoints.items()
        }
        record(
            checks,
            "search_endpoint_policy",
            endpoint_codes == endpoints,
            {"exit_codes": endpoint_codes},
        )

        # Permission hardening has to be able to fail, so the profile is seeded
        # world-readable first; a fresh mkdtemp is already 0700 and would make
        # the check vacuous.
        loose = pathlib.Path(tempfile_dir := profile / "loose")
        loose.mkdir()
        (loose / "data").mkdir()
        (loose / "cache").mkdir()
        (loose / "session.sqlite").touch()
        (loose / "session.sqlite-wal").touch()
        (loose / "session.sqlite-shm").touch()
        for target in (
            loose,
            loose / "data",
            loose / "cache",
            loose / "session.sqlite",
            loose / "session.sqlite-wal",
            loose / "session.sqlite-shm",
        ):
            force_mode(target, 0o777 if target.is_dir() else 0o666)
        before = {target.name: mode_of(target) for target in sorted(loose.iterdir())}
        run_browser(
            "--smoke", "--profile-dir", str(loose), "--start-url", "https://example.com/", expect_code=0
        )
        # SQLite checkpoints and removes the -wal/-shm sidecars on a clean
        # close, so each must either be gone or be owner-only. Asserting only
        # "absent" would make this branch vacuous, so both outcomes are named.
        sidecars = {}
        sidecars_ok = True
        for suffix in ("-wal", "-shm"):
            path = loose / f"session.sqlite{suffix}"
            if path.exists():
                sidecars[suffix] = mode_of(path)
                sidecars_ok = sidecars_ok and mode_of(path) == "0o600"
            else:
                sidecars[suffix] = "absent (checkpointed into the database)"
        directories = {name: mode_of(loose / name) for name in ("data", "cache")}
        database_mode = mode_of(loose / "session.sqlite")
        tightened = (
            mode_of(loose) == "0o700"
            and all(mode == "0o700" for mode in directories.values())
            and database_mode == "0o600"
            and sidecars_ok
        )
        record(
            checks,
            "profile_and_database_are_hardened_from_loose_modes",
            tightened,
            {
                "modes_before": before,
                "directory_modes_after": directories,
                "database_mode_after": database_mode,
                "sidecars_after": sidecars,
            },
        )

    # Bookmarks and history are profile stores, so they are verified head-lessly
    # rather than through the window.
    with temporary_profile() as stores_dir:
        stores = pathlib.Path(stores_dir)
        storage_data = run_browser_storage_check(stores)
        expected = {
            "ok": True,
            "schema_version": 2,
            "bookmark_first_add": True,
            "bookmark_duplicate_add": False,
            "bookmark_stored_title": "Example, renamed",
            "bookmark_found": True,
            "bookmark_removed": True,
            "bookmark_after_removal": False,
            "history_first_visit": True,
            "history_repeat_visit": False,
            "history_len": 2,
            "history_newest": "https://second.example/",
            "history_cleared": 2,
            "history_after_clear": 0,
            "bookmark_rejects_file_url": True,
        }
        record(
            checks,
            "bookmarks_and_history_behaviour",
            all(storage_data.get(key) == value for key, value in expected.items()),
            {"expected": expected, "observed": storage_data},
        )

    # A schema-1 profile written by the first slice must migrate in place: the
    # legacy row is kept, the legacy table is dropped, and the version advances.
    with temporary_profile() as legacy_dir:
        legacy = pathlib.Path(legacy_dir)
        write_legacy_v1_database(legacy / "session.sqlite")
        migrated = run_browser_storage_check(legacy)
        after = read_database(legacy / "session.sqlite")
        record(
            checks,
            "schema_v1_migrates_without_losing_the_session_row",
            after["user_version"] == 2
            and "session" not in after["tables"]
            and any(row[2] == "https://legacy.example/" for row in after["session_tabs"])
            and migrated.get("ok") is True,
            {"after_migration": after, "check": migrated.get("ok")},
        )

    help_result = run_browser("--help", expect_code=0)
    version_result = run_browser("--version", expect_code=0)
    unknown_result = run_browser("--definitely-not-a-flag", expect_code=2)
    missing_value = run_browser("--profile-dir", expect_code=2)
    record(
        checks,
        "command_line_exit_codes",
        help_result.stdout.startswith("Usage: browsrl")
        and version_result.stdout.strip() == "browsrl 0.1.0"
        and unknown_result.stderr.startswith("unknown argument")
        and missing_value.stderr.strip() == "--profile-dir needs a path",
        {
            "help_code": help_result.returncode,
            "version_code": version_result.returncode,
            "unknown_code": unknown_result.returncode,
            "missing_value_code": missing_value.returncode,
        },
    )

    # The GUI harness retires leftover browser processes between runs. That
    # cleanup is the one place a test could reach a browser the person started,
    # so its rule is checked here, offline, where a mistake is cheap to make and
    # easy to see.
    import gui  # imported here: the module needs no display, only its rule

    temp = pathlib.Path(tempfile.gettempdir())
    home = pathlib.Path.home()
    guard_cases = {
        "own_test_profile": [str(BINARY), "--profile-dir", str(temp / f"{PROFILE_PREFIX}abc")],
        "personal_data_dir": [str(BINARY), "--profile-dir", str(home / ".local/share/browsrl")],
        "prefix_nested_deeper": [
            str(BINARY),
            "--profile-dir",
            str(temp / "keep" / f"{PROFILE_PREFIX}mine"),
        ],
        "prefixed_name_outside_temp": [
            str(BINARY),
            "--profile-dir",
            str(home / f"{PROFILE_PREFIX}saved"),
        ],
        "traversal_out_of_temp": [
            str(BINARY),
            "--profile-dir",
            str(temp / f"{PROFILE_PREFIX}a" / ".." / ".." / f"{PROFILE_PREFIX}b"),
        ],
        "another_browser": [
            "/usr/bin/firefox",
            "--profile-dir",
            str(temp / f"{PROFILE_PREFIX}abc"),
        ],
        "no_profile_argument": [str(BINARY), "--start-url", "http://127.0.0.1/"],
        "flag_without_value": [str(BINARY), "--profile-dir"],
        "temp_dir_itself": [str(BINARY), "--profile-dir", str(temp)],
    }
    expected = {
        "own_test_profile": True,
        "personal_data_dir": False,
        "prefix_nested_deeper": False,
        "prefixed_name_outside_temp": False,
        "traversal_out_of_temp": False,
        "another_browser": False,
        "no_profile_argument": False,
        "flag_without_value": False,
        "temp_dir_itself": False,
    }
    verdicts = {
        name: gui.looks_like_harness_browser(argv) for name, argv in guard_cases.items()
    }
    record(
        checks,
        "gui_cleanup_matches_only_its_own_browsers",
        verdicts == expected,
        {"verdicts": verdicts, "expected": expected, "profile_prefix": PROFILE_PREFIX},
    )

    # A key press that cannot be delivered has to be an error, never a silent
    # pass. Probed with harmless commands rather than wtype, so no input is
    # injected into the session while the check runs.
    def raises_wtype_failed(command: list[str]) -> bool:
        try:
            gui.run_wtype(command, attempts=2)
        except gui.WtypeFailed:
            return True
        return False

    delivery = {
        "failing_command_raises": raises_wtype_failed(["/bin/false"]),
        "failing_command_message": None,
        "succeeding_command_returns": gui.run_wtype(["/bin/true"], attempts=1) is None,
    }
    try:
        gui.run_wtype(["/bin/false"], attempts=1)
    except gui.WtypeFailed as error:
        delivery["failing_command_message"] = str(error)
    record(
        checks,
        "unreliable_key_input_is_reported",
        delivery["failing_command_raises"]
        and bool(delivery["failing_command_message"])
        and delivery["succeeding_command_returns"],
        delivery,
    )

    # The harness types into the focused window, so it must never type while
    # something else has focus. Proved here by making a press with a foreign
    # window focused and asserting that wtype was never called: no keystroke is
    # injected into the session to test this.
    typed: list[list[str]] = []
    real_focus, real_wtype = gui.focused_window_class, gui.run_wtype
    real_wait = gui.wait_for_own_focus
    try:
        # A foreign window holds the keyboard: the press must be refused, and
        # wtype must not be reached at all.
        gui.focused_window_class = lambda: "com.example.SomeOtherApp"
        gui.wait_for_own_focus = lambda seconds=6.0: False
        gui.run_wtype = lambda command, attempts=3: typed.append(command)
        refused_result = gui.press("d", ctrl=True)
        typing_refused = refused_result is False and not typed

        # And the same press is delivered when our own window is focused.
        gui.focused_window_class = lambda: gui.APPLICATION_ID
        gui.wait_for_own_focus = real_wait
        delivered_result = gui.press("d", ctrl=True)
        delivered = delivered_result is True and len(typed) == 1
    finally:
        gui.focused_window_class, gui.run_wtype = real_focus, real_wtype
        gui.wait_for_own_focus = real_wait
    record(
        checks,
        "keys_are_never_typed_into_another_window",
        typing_refused and delivered,
        {
            "foreign_focus_refused": typing_refused,
            "own_focus_delivered": delivered,
            "wtype_calls": [command[:3] for command in typed],
        },
    )

    # The focus rule and the compositor-output parsing, checked without one.
    focus_rule = {
        "our_class": gui.focus_is_ours(gui.APPLICATION_ID),
        "foreign_class": gui.focus_is_ours("com.example.Terminal"),
        "unreadable_compositor": gui.focus_is_ours(None),
    }
    sway_tree = [
        {
            "name": "workspace 1",
            "focused": False,
            "nodes": [{"name": "term", "class": "alacritty", "focused": False, "nodes": []}],
        },
        {
            "name": "workspace 3",
            "focused": True,
            "nodes": [
                {"name": "a", "class": "io.github.browsrl.Browsrl", "focused": False, "nodes": []},
                {"name": "b", "class": "alacritty", "focused": True, "nodes": []},
            ],
        },
    ]
    parsing = {
        "sway_finds_deep_focused_leaf": (gui._focused_sway_node(sway_tree) or {}).get("class"),
        "sway_without_focus_is_none": gui._focused_sway_node([{"nodes": []}]) is None,
        "sway_on_garbage_is_none": gui._focused_sway_node("not a tree") is None,
    }
    record(
        checks,
        "keyboard_focus_rule_is_correct",
        focus_rule
        == {
            "our_class": True,
            "foreign_class": False,
            "unreadable_compositor": True,
        }
        and parsing
        == {
            "sway_finds_deep_focused_leaf": "alacritty",
            "sway_without_focus_is_none": True,
            "sway_on_garbage_is_none": True,
        },
        {"rule": focus_rule, "parsing": parsing, "application_id": gui.APPLICATION_ID},
    )

    passed = all(bool(check["passed"]) for check in checks)
    report = {
        "passed": passed,
        "binary": str(BINARY.relative_to(ROOT)),
        "checks": checks,
        "network": "loopback fixtures only",
        "not_covered": [
            "the GTK window and WebKit page load are checked by hand (see README), not by this contract",
            "WebDriver automation of this app is blocked: WebKitGTK 6.0 requires the app to answer"
            " WebKitAutomationSession::create-web-view, which webkit6 0.6.1 does not expose",
        ],
    }
    REPORT.parent.mkdir(parents=True, exist_ok=True)
    REPORT.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    # A one-line summary keeps the output readable for a person and gives an
    # automated test-count parser something to count.
    if passed:
        print(f"e2e: {len(checks)} passed")
    else:
        failed = sum(1 for check in checks if not check["passed"])
        print(f"e2e: {len(checks) - failed} passed, {failed} failed")
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
