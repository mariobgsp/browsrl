"""Small stdlib-only harness for local browser checks."""

from __future__ import annotations

import http.server
import os
import pathlib
import socketserver
import subprocess
import tempfile
import threading
from typing import Any


ROOT = pathlib.Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "tests" / "fixtures"
BINARY = ROOT / "target" / "debug" / "rbrowse"
TIMEOUT_SECONDS = 180


class QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, format: str, *args: Any) -> None:
        return


class LocalFixtureServer:
    """Serve repository fixtures on loopback without contacting the Internet."""

    def __init__(self) -> None:
        self._server = socketserver.ThreadingTCPServer(
            ("127.0.0.1", 0), lambda *args, **kwargs: QuietHandler(*args, directory=str(FIXTURES), **kwargs)
        )
        self._server.daemon_threads = True
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)

    @property
    def base_url(self) -> str:
        host, port = self._server.server_address
        return f"http://{host}:{port}"

    def __enter__(self) -> "LocalFixtureServer":
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


def run_browser(*arguments: str, expect_code: int | None = 0) -> subprocess.CompletedProcess[str]:
    """Run the built binary and optionally assert its exit code."""
    if not BINARY.exists():
        raise SystemExit(f"missing {BINARY}; run `make build` first")
    result = subprocess.run(
        [str(BINARY), *arguments],
        cwd=ROOT,
        env=cargo_environment(),
        capture_output=True,
        text=True,
        timeout=TIMEOUT_SECONDS,
    )
    if expect_code is not None and result.returncode != expect_code:
        raise SystemExit(
            f"expected exit {expect_code} from {' '.join(arguments)}, "
            f"got {result.returncode}\nstdout: {result.stdout}\nstderr: {result.stderr}"
        )
    return result


def run_browser_smoke(profile: pathlib.Path, url: str) -> subprocess.CompletedProcess[str]:
    return run_browser(
        "--smoke", "--profile-dir", str(profile), "--start-url", url, expect_code=0
    )


def temporary_profile() -> tempfile.TemporaryDirectory[str]:
    return tempfile.TemporaryDirectory(prefix="rbrowse-e2e-")
