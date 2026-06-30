#!/usr/bin/env python3
"""Generate SchedulerTraceData.tla from scheduler trace NDJSON."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any


ALLOWED_BOUNDARIES = {
    "eval",
    "dispatch_event",
    "run_event_loop",
    "run_timers",
    "pump_microtasks",
    "timer_task",
}

ALLOWED_PHASES = {"start", "end", "error", "performed"}


def tla_string(value: str) -> str:
    out = ['"']
    for char in value:
        code = ord(char)
        if char == '"':
            out.append(r"\"")
        elif char == "\\":
            out.append(r"\\")
        elif char == "\n":
            out.append(r"\n")
        elif char == "\r":
            out.append(r"\r")
        elif char == "\t":
            out.append(r"\t")
        elif code < 0x20:
            out.append(f"\\u{code:04x}")
        else:
            out.append(char)
    out.append('"')
    return "".join(out)


def load_trace(path: Path) -> list[dict[str, Any]]:
    events: list[dict[str, Any]] = []
    with path.open("r", encoding="utf-8") as handle:
        for line_no, line in enumerate(handle, start=1):
            stripped = line.strip()
            if not stripped:
                continue
            try:
                event = json.loads(stripped)
            except json.JSONDecodeError as error:
                raise SystemExit(f"{path}:{line_no}: invalid JSON: {error}") from error
            events.append(event)
    validate_trace(path, events)
    return events


def validate_trace(path: Path, events: list[dict[str, Any]]) -> None:
    if not events:
        raise SystemExit(f"{path}: trace is empty")
    for expected_seq, event in enumerate(events, start=1):
        seq = event.get("seq")
        if seq != expected_seq:
            raise SystemExit(f"{path}: expected seq {expected_seq}, got {seq!r}")
        boundary = event.get("boundary")
        phase = event.get("phase")
        if boundary not in ALLOWED_BOUNDARIES:
            raise SystemExit(f"{path}: seq {seq}: invalid boundary {boundary!r}")
        if phase not in ALLOWED_PHASES:
            raise SystemExit(f"{path}: seq {seq}: invalid phase {phase!r}")
        detail = event.get("detail", "")
        if detail is not None and not isinstance(detail, str):
            raise SystemExit(f"{path}: seq {seq}: detail must be a string or null")


def render_module(events: list[dict[str, Any]], module_name: str) -> str:
    lines = [
        f"---- MODULE {module_name} ----",
        "EXTENDS Sequences",
        "",
        "Trace ==",
        "    <<",
    ]
    for index, event in enumerate(events):
        comma = "," if index + 1 < len(events) else ""
        detail = event.get("detail") or ""
        lines.append(
            "      [seq |-> {seq}, boundary |-> {boundary}, phase |-> {phase}, "
            "detail |-> {detail}]{comma}".format(
                seq=event["seq"],
                boundary=tla_string(event["boundary"]),
                phase=tla_string(event["phase"]),
                detail=tla_string(detail),
                comma=comma,
            )
        )
    lines.extend(["    >>", "", "====", ""])
    return "\n".join(lines)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("input", type=Path, help="scheduler trace NDJSON")
    parser.add_argument("output", type=Path, help="SchedulerTraceData.tla output path")
    parser.add_argument("--module-name", default="SchedulerTraceData")
    args = parser.parse_args()

    events = load_trace(args.input)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(render_module(events, args.module_name), encoding="utf-8")


if __name__ == "__main__":
    main()
