#!/usr/bin/env python3
"""Dependency-cone witnesses for Serval profile boundaries."""

from __future__ import annotations

import pathlib
import subprocess
import sys
import tomllib


ROOT = pathlib.Path(__file__).resolve().parents[2]


def fail(message: str) -> None:
    print(f"dependency-cone witness failed: {message}", file=sys.stderr)
    raise SystemExit(1)


def load_toml(path: pathlib.Path) -> dict:
    with path.open("rb") as fh:
        return tomllib.load(fh)


def dependency_names(table: dict) -> set[str]:
    return set(table.keys())


def assert_serval_extract_cone() -> None:
    manifest = load_toml(ROOT / "components" / "serval-extract" / "Cargo.toml")
    deps = dependency_names(manifest.get("dependencies", {}))
    expected = {"layout_dom_api"}
    if deps != expected:
        fail(f"serval-extract dependencies are {sorted(deps)}, expected {sorted(expected)}")
    build_deps = dependency_names(manifest.get("build-dependencies", {}))
    if build_deps:
        fail(f"serval-extract build-dependencies must stay empty, found {sorted(build_deps)}")
    dev_deps = dependency_names(manifest.get("dev-dependencies", {}))
    allowed_dev_deps = {"serval-static-dom"}
    if dev_deps - allowed_dev_deps:
        fail(
            "serval-extract dev-dependencies contain non-fixture deps: "
            f"{sorted(dev_deps - allowed_dev_deps)}"
        )

    forbidden = {
        "serval-layout",
        "serval-render",
        "paint",
        "paint_list_render",
        "netrender",
        "wgpu",
    }
    if deps & forbidden:
        fail(f"serval-extract pulled render deps directly: {sorted(deps & forbidden)}")


def assert_cargo_metadata_sees_extract() -> None:
    result = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        cwd=ROOT,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        fail(f"cargo metadata failed:\n{result.stderr}")
    if '"name":"serval-extract"' not in result.stdout:
        fail("cargo metadata did not report the serval-extract workspace package")


def main() -> None:
    assert_serval_extract_cone()
    assert_cargo_metadata_sees_extract()
    print("dependency-cone witnesses passed")


if __name__ == "__main__":
    main()
