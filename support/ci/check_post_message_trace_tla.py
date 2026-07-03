#!/usr/bin/env python3
"""Check the postMessage trace TLA+ witness."""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
GOOD_FIXTURE = ROOT / "components/script-runtime-api/tests/fixtures/post_message_trace.ndjson"
BAD_FIXTURE = ROOT / "components/script-runtime-api/tests/fixtures/post_message_trace_bad_sync.ndjson"
GENERATOR = ROOT / "components/script-runtime-api/tools/scheduler_trace_to_tla.py"
SPEC_DIR = ROOT / "docs/tla/post_message_trace"
BASE_SPEC = SPEC_DIR / "PostMessage.tla"
TRACE_SPEC = SPEC_DIR / "PostMessageTrace.tla"
CFG = SPEC_DIR / "PostMessageTrace.cfg"
CHECKED_IN_DATA = SPEC_DIR / "PostMessageTraceData.tla"
MODULE_NAME = "PostMessageTraceData"


def run(command: list[str], cwd: Path | None = None, check: bool = True) -> subprocess.CompletedProcess[str]:
    print("+", " ".join(command))
    return subprocess.run(command, cwd=cwd, check=check, text=True)


def generate_trace_data(fixture: Path, output: Path) -> str:
    run(
        [
            sys.executable,
            str(GENERATOR),
            str(fixture),
            str(output),
            "--module-name",
            MODULE_NAME,
        ]
    )
    return output.read_text(encoding="utf-8")


def run_tlc(java_bin: str, jar: Path, cwd: Path) -> subprocess.CompletedProcess[str]:
    return run(
        [
            java_bin,
            "-cp",
            str(jar),
            "tlc2.TLC",
            "-cleanup",
            "-workers",
            "1",
            "-config",
            CFG.name,
            TRACE_SPEC.name,
        ],
        cwd=cwd,
        check=False,
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tla-tools-jar", type=Path, help="path to tla2tools.jar")
    parser.add_argument("--java-bin", default="java", help="java executable to run TLC")
    args = parser.parse_args()

    for path in [GOOD_FIXTURE, BAD_FIXTURE, GENERATOR, BASE_SPEC, TRACE_SPEC, CFG, CHECKED_IN_DATA]:
        if not path.exists():
            raise SystemExit(f"missing required file: {path}")

    with tempfile.TemporaryDirectory(prefix="post-message-trace-tla-") as tmp:
        tmp_path = Path(tmp)
        generated = tmp_path / f"{MODULE_NAME}.tla"
        text = generate_trace_data(GOOD_FIXTURE, generated)
        for needle in [
            f"---- MODULE {MODULE_NAME} ----",
            'boundary |-> "post_message"',
            'phase |-> "deliver"',
            'detail |-> "1"',
        ]:
            if needle not in text:
                raise SystemExit(f"generated TLA missing {needle!r}")
        checked_in = CHECKED_IN_DATA.read_text(encoding="utf-8")
        if text != checked_in:
            raise SystemExit(
                "checked-in PostMessageTraceData.tla is stale; regenerate it with "
                "components/script-runtime-api/tools/scheduler_trace_to_tla.py"
            )

        jar = args.tla_tools_jar
        if jar is None:
            print("No --tla-tools-jar supplied; generated-data check complete.")
            return
        if not jar.exists():
            raise SystemExit(f"missing tla2tools.jar: {jar}")

        shutil.copy2(BASE_SPEC, tmp_path / BASE_SPEC.name)
        shutil.copy2(TRACE_SPEC, tmp_path / TRACE_SPEC.name)
        shutil.copy2(CFG, tmp_path / CFG.name)

        good = run_tlc(args.java_bin, jar, tmp_path)
        if good.returncode != 0:
            raise SystemExit("good postMessage trace failed TLC")

        generate_trace_data(BAD_FIXTURE, generated)
        bad = run_tlc(args.java_bin, jar, tmp_path)
        if bad.returncode == 0:
            raise SystemExit("bad postMessage trace unexpectedly satisfied the TLA witness")


if __name__ == "__main__":
    main()
