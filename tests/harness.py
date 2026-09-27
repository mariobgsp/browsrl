"""Small stdlib-only harness for local browser checks."""

from __future__ import annotations

import http.server
import json
import os
import pathlib
import socketserver
import sqlite3
import subprocess
import tempfile
import threading
from typing import Any


ROOT = pathlib.Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "tests" / "fixtures"
BINARY = ROOT / "target" / "debug" / "brwsl"
TIMEOUT_SECONDS = 180


class QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, format: str, *args: Any) -> None:
        return


class RequestLog:
    """The paths a fixture server has served, safe to read while requests run.

    A page reports what happened to it by requesting a path, so the outcome the
    shell produced becomes visible to a test without reading a screenshot or
    inferring it from a title.
    """

    def __init__(self) -> None:
        self._paths: list[str] = []
        self._lock = threading.Lock()

    def add(self, path: str) -> None:
        with self._lock:
            self._paths.append(path)

    def paths(self) -> list[str]:
        with self._lock:
            return list(self._paths)


class RecordingHandler(QuietHandler):
    """A fixture handler that records every path it is asked for."""

    def __init__(self, *args: Any, log: RequestLog, **kwargs: Any) -> None:
        self.log = log
        super().__init__(*args, **kwargs)

    def do_GET(self) -> None:  # noqa: N802 - the name is fixed by the base class
        path = self.path.split("?", 1)[0]
        self.log.add(path)
        if path == "/download":
            # An attachment whose server-supplied name tries to climb out of the
            # download directory. The traversal has to be reduced to a leaf by the
            # browser, and the file has to land inside the profile.
            body = b"payload\n"
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header(
                "Content-Disposition", 'attachment; filename="../../evil-payload.txt"'
            )
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        super().do_GET()


class LocalFixtureServer:
    """Serve repository fixtures on loopback without contacting the Internet."""

    def __init__(self) -> None:
        self.request_log = RequestLog()
        self._server = socketserver.ThreadingTCPServer(
            ("127.0.0.1", 0),
            lambda *args, **kwargs: RecordingHandler(
                *args,
                log=self.request_log,
                directory=str(FIXTURES),
                **kwargs,
            ),
        )
        self._server.daemon_threads = True
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)

    @property
    def base_url(self) -> str:
        host, port = self._server.server_address
        return f"http://{host}:{port}"

    @property
    def requested(self) -> list[str]:
        """Every path served so far, in order, with query strings dropped."""
        return self.request_log.paths()

    def __enter__(self) -> LocalFixtureServer:
        self._thread.start()
        return self

    def __exit__(self, *_: object) -> None:
        self._server.shutdown()
        self._server.server_close()
        self._thread.join(timeout=5)


def cargo_environment() -> dict[str, str]:
    environment = os.environ.copy()
    prefix = environment.get("WEBKIT_PREFIX", "/usr")
    package_path = f"{prefix}/lib/pkgconfig"
    environment["PKG_CONFIG_PATH"] = os.pathsep.join(
        part for part in (package_path, environment.get("PKG_CONFIG_PATH", "")) if part
    )
    if prefix != "/usr":
        sysroot = prefix[:-4] if prefix.endswith("/usr") else prefix
        environment.setdefault("PKG_CONFIG_SYSROOT_DIR", sysroot)
        environment["LD_LIBRARY_PATH"] = os.pathsep.join(
            part for part in (f"{prefix}/lib", environment.get("LD_LIBRARY_PATH", "")) if part
        )
    return environment


def record(checks: list[dict[str, object]], name: str, passed: bool, details: dict[str, object]) -> None:
    """Add one check to the report a contract is building.

    Every contract speaks this shape, so the report on disk has one owner and a
    new check cannot invent a field of its own.
    """
    checks.append({"name": name, "passed": bool(passed), "details": details})


def print_checks(checks: list[dict[str, object]]) -> None:
    for item in checks:
        print(("PASS" if item["passed"] else "FAIL"), item["name"])


def write_report(path: pathlib.Path, report: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")


class Browser:
    """The built binary, launched one way.

    `run` waits and checks the exit code; `spawn` returns the live process for a
    check that has to keep the window open. Both build the argv, the environment
    and the log file in one place, so a relocated `WEBKIT_PREFIX` and the private
    per-process log are not re-decided by each contract.
    """

    def __init__(self, *arguments: str) -> None:
        if not BINARY.exists():
            raise SystemExit(f"missing {BINARY}; run `make build` first")
        self.arguments = arguments

    def run(self, expect_code: int | None = 0) -> subprocess.CompletedProcess[str]:
        result = subprocess.run(
            [str(BINARY), *self.arguments],
            cwd=ROOT,
            env=cargo_environment(),
            capture_output=True,
            text=True,
            timeout=TIMEOUT_SECONDS,
        )
        if expect_code is not None and result.returncode != expect_code:
            raise SystemExit(
                f"expected exit {expect_code}, got {result.returncode}: {result.stderr[-400:]}"
            )
        return result

    def spawn(self) -> subprocess.Popen[str]:
        # The app's own stderr goes to a private temporary file of its own. A
        # fixed path under /tmp would be shared and world-writable, so another
        # user could have put something there.
        handle, log_path = tempfile.mkstemp(prefix="brwsl-", suffix=".log")
        with os.fdopen(handle, "a") as log:
            process = subprocess.Popen(
                [str(BINARY), *self.arguments],
                cwd=ROOT,
                stdout=log,
                stderr=subprocess.STDOUT,
                env=cargo_environment(),
                text=True,
            )
        self.log_path = pathlib.Path(log_path)
        return process


def run_browser(*arguments: str, expect_code: int | None = 0) -> subprocess.CompletedProcess[str]:
    """Run the built binary and optionally assert its exit code."""
    return Browser(*arguments).run(expect_code=expect_code)


def run_browser_smoke(profile: pathlib.Path, url: str) -> subprocess.CompletedProcess[str]:
    return run_browser(
        "--smoke", "--profile-dir", str(profile), "--start-url", url, expect_code=0
    )


def run_browser_storage_check(profile: pathlib.Path) -> dict[str, Any]:
    """Run the head-less profile check and return its JSON output."""
    result = run_browser(
        "--storage-check", "--profile-dir", str(profile), expect_code=0
    )
    return json.loads(result.stdout)


def write_legacy_v1_database(path: pathlib.Path) -> None:
    """Create a schema-1 database the way the first slice wrote it.

    The first slice stored a single row in a `session` table and stamped
    user_version 1, so a migration test has to reproduce that shape exactly.
    """
    connection = sqlite3.connect(path)
    try:
        connection.execute(
            "CREATE TABLE session (id INTEGER PRIMARY KEY, url TEXT NOT NULL)"
        )
        connection.execute(
            "INSERT INTO session(id, url) VALUES (1, 'https://legacy.example/')"
        )
        connection.execute(
            """CREATE TABLE session_tabs (
                 id INTEGER PRIMARY KEY,
                 position INTEGER NOT NULL CHECK (position >= 0),
                 url TEXT NOT NULL,
                 title TEXT NOT NULL,
                 selected INTEGER NOT NULL CHECK (selected IN (0, 1))
             )"""
        )
        connection.execute("PRAGMA user_version = 1")
        connection.commit()
    finally:
        connection.close()


def read_database(path: pathlib.Path) -> dict[str, Any]:
    """Read back the facts a migration has to get right."""
    connection = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    try:
        tables = {
            row[0]
            for row in connection.execute(
                "SELECT name FROM sqlite_master WHERE type = 'table'"
            )
        }
        tabs = connection.execute(
            "SELECT id, position, url, title, selected FROM session_tabs"
            " ORDER BY position, id"
        ).fetchall()
        return {
            "user_version": connection.execute("PRAGMA user_version").fetchone()[0],
            "tables": sorted(tables),
            "session_tabs": tabs,
        }
    finally:
        connection.close()


# Every profile this harness makes is a fresh directory directly under the
# system temporary directory with this prefix. Cleanup keys off the same
# constant, so a guard can never drift away from whatever created the profile.
PROFILE_PREFIX = "brwsl-e2e-"


def temporary_profile() -> tempfile.TemporaryDirectory[str]:
    return tempfile.TemporaryDirectory(prefix=PROFILE_PREFIX)
