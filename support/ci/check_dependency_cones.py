#!/usr/bin/env python3
"""Dependency-cone witnesses for Genet profile boundaries."""

from __future__ import annotations

import json
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


def is_beneath(path: pathlib.Path, parent: pathlib.Path) -> bool:
    try:
        path.resolve().relative_to(parent.resolve())
    except ValueError:
        return False
    return True


def assert_genet_extract_cone() -> None:
    manifest = load_toml(ROOT / "components" / "genet-extract" / "Cargo.toml")
    deps = dependency_names(manifest.get("dependencies", {}))
    expected = {"layout_dom_api"}
    if deps != expected:
        fail(f"genet-extract dependencies are {sorted(deps)}, expected {sorted(expected)}")
    build_deps = dependency_names(manifest.get("build-dependencies", {}))
    if build_deps:
        fail(f"genet-extract build-dependencies must stay empty, found {sorted(build_deps)}")
    dev_deps = dependency_names(manifest.get("dev-dependencies", {}))
    allowed_dev_deps = {"genet-static-dom"}
    if dev_deps - allowed_dev_deps:
        fail(
            "genet-extract dev-dependencies contain non-fixture deps: "
            f"{sorted(dev_deps - allowed_dev_deps)}"
        )

    forbidden = {
        "genet-layout",
        "genet-render",
        "paint",
        "paint_list_render",
        "netrender",
        "wgpu",
    }
    if deps & forbidden:
        fail(f"genet-extract pulled render deps directly: {sorted(deps & forbidden)}")


def cargo_metadata() -> dict:
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
    return json.loads(result.stdout)


def assert_cargo_metadata_sees_extract(metadata: dict) -> None:
    if not any(package["name"] == "genet-extract" for package in metadata["packages"]):
        fail("cargo metadata did not report the genet-extract workspace package")


def assert_ports_depend_inward(metadata: dict) -> None:
    components = (ROOT / "components").resolve()
    ports = (ROOT / "ports").resolve()
    host_api = []
    pelt_packages = {}

    for package in metadata["packages"]:
        manifest = pathlib.Path(package["manifest_path"]).resolve()
        if package["name"] == "genet-host-api":
            host_api.append(manifest)
        if package["name"] in {"pelt", "pelt-desktop"}:
            pelt_packages[package["name"]] = manifest
        if not is_beneath(manifest, components):
            continue
        for dependency in package["dependencies"]:
            path = dependency.get("path")
            if path is not None and is_beneath(pathlib.Path(path), ports):
                fail(
                    f"{manifest.relative_to(ROOT)} depends on port package "
                    f"{dependency['name']} at {pathlib.Path(path).relative_to(ROOT)}"
                )

    expected_api = (components / "genet-host-api" / "Cargo.toml").resolve()
    if host_api != [expected_api]:
        rendered = [str(path.relative_to(ROOT)) for path in host_api]
        fail(
            f"genet-host-api manifests are {rendered}, "
            "expected components/genet-host-api/Cargo.toml"
        )

    expected_pelt = {
        "pelt": (ports / "pelt" / "Cargo.toml").resolve(),
        "pelt-desktop": (ports / "pelt" / "desktop" / "Cargo.toml").resolve(),
    }
    if pelt_packages != expected_pelt:
        rendered = {
            name: str(path.relative_to(ROOT)) for name, path in pelt_packages.items()
        }
        fail(f"Pelt package manifests are {rendered}, expected {expected_pelt}")


def main() -> None:
    assert_genet_extract_cone()
    metadata = cargo_metadata()
    assert_cargo_metadata_sees_extract(metadata)
    assert_ports_depend_inward(metadata)
    print("dependency-cone witnesses passed")


if __name__ == "__main__":
    main()
