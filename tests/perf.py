#!/usr/bin/env python3
"""Small repeatable startup/storage benchmark for the first slice."""

from __future__ import annotations

import json
import pathlib
import statistics
import sys
import time

from harness import ROOT, run_browser_smoke, temporary_profile


REPORT = ROOT / "artifacts" / "perf.json"


def main() -> int:
    with temporary_profile() as profile_dir:
        durations = []
        for _ in range(5):
            started = time.perf_counter()
            run_browser_smoke(pathlib.Path(profile_dir), "http://127.0.0.1:9/perf.html")
            durations.append(time.perf_counter() - started)
    report = {
        "iterations": len(durations),
        "mean_seconds": round(statistics.mean(durations), 4),
        "max_seconds": round(max(durations), 4),
        "note": "Measures the offline smoke path, not WebKit rendering throughput.",
    }
    REPORT.parent.mkdir(parents=True, exist_ok=True)
    REPORT.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
