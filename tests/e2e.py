#!/usr/bin/env python3
"""Deterministic, offline end-to-end contract for the first R Browse slice.

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

from harness import (
    BINARY,
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
        help_result.stdout.startswith("Usage: rbrowse")
        and version_result.stdout.strip() == "rbrowse 0.1.0"
        and unknown_result.stderr.startswith("unknown argument")
        and missing_value.stderr.strip() == "--profile-dir needs a path",
        {
            "help_code": help_result.returncode,
            "version_code": version_result.returncode,
            "unknown_code": unknown_result.returncode,
            "missing_value_code": missing_value.returncode,
        },
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
