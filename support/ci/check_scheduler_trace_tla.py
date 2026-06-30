#!/usr/bin/env python3
"""Check the scheduler trace TLA+ witness."""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "components/script-runtime-api/tests/fixtures/scheduler_trace.ndjson"
GENERATOR = ROOT / "components/script-runtime-api/tools/scheduler_trace_to_tla.py"
SPEC_DIR = ROOT / "docs/tla/scheduler_trace"
SPEC = SPEC_DIR / "SchedulerTrace.tla"
CFG = SPEC_DIR / "SchedulerTrace.cfg"
CHECKED_IN_DATA = SPEC_DIR / "SchedulerTraceData.tla"


def run(command: list[str], cwd: Path | None = None) -> None:
    print("+", " ".join(command))
    subprocess.run(command, cwd=cwd, check=True)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tla-tools-jar", type=Path, help="path to tla2tools.jar")
    args = parser.parse_args()

    for path in [FIXTURE, GENERATOR, SPEC, CFG, CHECKED_IN_DATA]:
        if not path.exists():
            raise SystemExit(f"missing required file: {path}")

    with tempfile.TemporaryDirectory(prefix="scheduler-trace-tla-") as tmp:
        tmp_path = Path(tmp)
        generated = tmp_path / "SchedulerTraceData.tla"
        run([sys.executable, str(GENERATOR), str(FIXTURE), str(generated)])
        text = generated.read_text(encoding="utf-8")
        for needle in [
            "---- MODULE SchedulerTraceData ----",
            'boundary |-> "run_timers"',
            'phase |-> "performed"',
            'detail |-> "fired=1"',
        ]:
            if needle not in text:
                raise SystemExit(f"generated TLA missing {needle!r}")
        checked_in = CHECKED_IN_DATA.read_text(encoding="utf-8")
        if text != checked_in:
            raise SystemExit(
                "checked-in SchedulerTraceData.tla is stale; regenerate it with "
                "components/script-runtime-api/tools/scheduler_trace_to_tla.py"
            )

        jar = args.tla_tools_jar
        if jar is None:
            print("No --tla-tools-jar supplied; generated-data check complete.")
            return
        if not jar.exists():
            raise SystemExit(f"missing tla2tools.jar: {jar}")

        shutil.copy2(SPEC, tmp_path / SPEC.name)
        shutil.copy2(CFG, tmp_path / CFG.name)
        run(
            [
                "java",
                "-cp",
                str(jar),
                "tlc2.TLC",
                "-cleanup",
                "-workers",
                "1",
                "-config",
                CFG.name,
                SPEC.name,
            ],
            cwd=tmp_path,
        )


if __name__ == "__main__":
    main()
