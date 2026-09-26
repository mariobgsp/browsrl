"""Bookmark import, exercised through the real binary.

The fixture is deliberately messy in the ways a real export is: nested folders,
an escaped ampersand in a URL and in a title, a duplicate URL, a `file:` URL, a
`javascript:` URL, and a bookmark with no title. An importer that only copes with
tidy input passes a tidy fixture and loses a person's bookmarks.

Assertions read the profile database directly rather than trusting the summary
the command prints, so a wrong count cannot make itself look right.
"""
import json
import pathlib
import sqlite3
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests"))

from harness import PROFILE_PREFIX, run_browser, temporary_profile  # noqa: E402

FIXTURE = ROOT / "tests" / "fixtures" / "bookmarks-import.html"

# Two titles below are the store's pre-existing behaviour rather than the
# importer's: a later title for the same URL wins, and an empty title is stored
# as "Untitled bookmark". The storage check asserts both, so the importer
# inherits them instead of inventing different rules.

EXPECTED = {
    "https://example.com/docs?a=1&b=2": "Docs & guides",
    "https://news.example.org/": "News",
    "https://intranet.example.com/dashboard": "Dashboard",
    "https://intranet.example.com/deep/one": "One",
    "https://intranet.example.com/deep/two": "Two",
    "https://issues.example.com/": "Issues",
    "https://unfiled.example.net/": "Unfiled bookmark",
    "https://duplicate.example.com/": "Second copy, same URL",
    "https://no-title.example.com/": "Untitled bookmark",
}


def stored_bookmarks(profile):
    connection = sqlite3.connect(f"file:{profile}/session.sqlite?mode=ro", uri=True)
    try:
        return {row[0]: row[1] for row in connection.execute("SELECT url, title FROM bookmarks")}
    finally:
        connection.close()


def main() -> int:
    checks = []

    def check(name, passed, details):
        checks.append({"name": name, "passed": bool(passed), "details": details})

    with temporary_profile() as profile_dir:
        profile = pathlib.Path(profile_dir)
        summary = json.loads(
            run_browser("--import-bookmarks", str(FIXTURE), "--profile-dir", str(profile)).stdout
        )
        stored = stored_bookmarks(profile)

        check(
            "import_reads_a_real_bookmark_file",
            summary.get("added") == len(EXPECTED) and stored == EXPECTED,
            {"summary_added": summary.get("added"), "stored": stored},
        )
        check(
            "import_refuses_non_web_urls",
            summary.get("refused") == 2
            and not any(url.startswith(("file:", "javascript:")) for url in stored),
            {"refused": summary.get("refused"), "urls": sorted(stored)},
        )
        check(
            "import_decodes_html_entities",
            stored.get("https://example.com/docs?a=1&b=2") == "Docs & guides",
            {"title": stored.get("https://example.com/docs?a=1&b=2")},
        )
        check(
            "import_collapses_a_duplicate_url",
            summary.get("duplicates") == 1
            and len([url for url in stored if url == "https://duplicate.example.com/"]) == 1,
            {"duplicates": summary.get("duplicates")},
        )
        check(
            "import_keeps_folder_structure",
            summary.get("folders") == 3
            and summary.get("sample_folders", {}).get("https://intranet.example.com/dashboard")
            == ["Work"]
            and summary.get("sample_folders", {}).get("https://intranet.example.com/deep/one")
            == ["Work", "Deep"]
            and summary.get("sample_folders", {}).get("https://issues.example.com/") == ["Work"]
            and summary.get("sample_folders", {}).get("https://unfiled.example.net/") == [],
            {"sample_folders": summary.get("sample_folders")},
        )
        check(
            "import_accepts_a_bookmark_with_no_title",
            "https://no-title.example.com/" in stored,
            {"urls": sorted(stored)},
        )

        # Importing the same file twice is the most likely way a person loses
        # trust in the feature, so it has to add nothing the second time.
        again = json.loads(
            run_browser("--import-bookmarks", str(FIXTURE), "--profile-dir", str(profile)).stdout
        )
        check(
            "importing_twice_adds_nothing_new",
            again.get("added") == 0
            and again.get("duplicates")
            == again.get("parsed", 0) - again.get("refused", 0)
            and stored_bookmarks(profile) == EXPECTED,
            {"second_run": again},
        )

        # Files that are not bookmark files, and files that do not exist, must be
        # refused in words a person can act on.
        not_bookmarks = profile / "not-bookmarks.html"
        not_bookmarks.write_text("<html><body><p>hello</p></body></html>")
        wrong = run_browser(
            "--import-bookmarks", str(not_bookmarks), "--profile-dir", str(profile), expect_code=2
        )
        missing = run_browser(
            "--import-bookmarks", str(profile / "nope.html"), "--profile-dir", str(profile),
            expect_code=2,
        )
        check(
            "import_refuses_a_file_with_no_bookmarks",
            "no bookmarks were found" in wrong.stderr,
            {"stderr": wrong.stderr.strip()[:160]},
        )
        check(
            "import_reports_a_file_it_cannot_read",
            "could not read" in missing.stderr,
            {"stderr": missing.stderr.strip()[:160]},
        )

    # A truncated file must not lose the bookmarks that came before the cut.
    truncated = FIXTURE.read_text(encoding="utf-8")[: FIXTURE.read_text(encoding="utf-8").index("intranet")]
    with temporary_profile() as profile_dir:
        profile = pathlib.Path(profile_dir)
        cut = profile / "cut.html"
        cut.write_text(truncated, encoding="utf-8")
        partial = json.loads(
            run_browser("--import-bookmarks", str(cut), "--profile-dir", str(profile)).stdout
        )
        check(
            "import_survives_a_truncated_file",
            partial.get("added", 0) >= 2 and len(stored_bookmarks(profile)) == partial.get("added"),
            {"summary": partial, "stored": sorted(stored_bookmarks(profile))},
        )

    passed = all(bool(item["passed"]) for item in checks)
    report = {"passed": passed, "checks": checks}
    out = ROOT / "artifacts" / "e2e"
    out.mkdir(parents=True, exist_ok=True)
    (out / "import-report.json").write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    for item in checks:
        print(("PASS" if item["passed"] else "FAIL"), item["name"])
    print(f"import: {sum(1 for c in checks if c['passed'])}/{len(checks)} passed")
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
