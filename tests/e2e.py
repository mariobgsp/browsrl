#!/usr/bin/env python3
"""Deterministic, offline end-to-end contract for the first R Browse slice.

Every check runs the real binary against loopback fixtures or explicit CLI
input and asserts observable behaviour (exit code, JSON output, on-disk
permissions). Nothing here claims GUI coverage: a WebKit window is exercised
only when a display and WebDriver are available, which is reported separately.
"""

from __future__ import annotations

import json
import pathlib
import stat
import sys

from harness import (
    BINARY,
    LocalFixtureServer,
    ROOT,
    run_browser,
    run_browser_smoke,
    temporary_profile,
)


REPORT = ROOT / "artifacts" / "e2e" / "report.json"


def record(checks: list[dict[str, object]], name: str, passed: bool, details: dict[str, object]) -> None:
    checks.append({"name": name, "passed": bool(passed), "details": details})


def mode_of(path: pathlib.Path) -> str:
    return oct(stat.S_IMODE(path.stat().st_mode))


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

        rejected = {}
        for candidate in ("file:///etc/passwd", "javascript:alert(1)", "about:settings", "data:text/html,x"):
            result = run_browser(
                "--smoke", "--profile-dir", str(profile), "--start-url", candidate, expect_code=1
            )
            rejected[candidate] = result.stderr.strip()
        record(
            checks,
            "navigation_rejects_non_web_schemes",
            all("only " in message for message in rejected.values()),
            {"errors": rejected},
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

        database_files = {
            path.name: mode_of(path)
            for path in sorted(profile.iterdir())
            if path.is_file() and path.name.startswith("session.sqlite")
        }
        record(
            checks,
            "profile_and_database_are_user_private",
            mode_of(profile) == "0o700"
            and bool(database_files)
            and all(mode == "0o600" for mode in database_files.values()),
            {
                "profile_mode": mode_of(profile),
                "database_modes": database_files,
                "profile": "temporary profile directory",
            },
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
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
