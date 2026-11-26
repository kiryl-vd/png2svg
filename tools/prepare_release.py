#!/usr/bin/env python3
"""Validate a stable release version and update the Cargo version files.

Python 3.11 or newer is required. Only the package version is changed.
The Cargo lockfile format stays compatible with the project's MSRV.
"""

import argparse
import re
import sys
import tomllib
from pathlib import Path


VERSION = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")


def parse_version(value):
    """Return the numeric parts of a stable Cargo version or reject it."""
    if len(value) > 62 or VERSION.fullmatch(value) is None:
        raise ValueError("Use a stable version such as 0.1.0, without a v prefix.")
    parts = tuple(int(part) for part in value.split("."))
    if any(part > 2**64 - 1 for part in parts):
        raise ValueError("Each version part must fit in an unsigned 64-bit integer.")
    return parts


def replace_version(text, header, name, version):
    """Change one named package version without rewriting unrelated TOML."""
    pattern = re.compile(
        r"^" + re.escape(header) + r"[^\n]*\n.*?(?=^\[|\Z)",
        re.MULTILINE | re.DOTALL,
    )
    matches = []
    for match in pattern.finditer(text):
        section = tomllib.loads(match.group())
        package = section["package"]
        if isinstance(package, list):
            package = package[0]
        if package.get("name") == name:
            matches.append(match)
    if len(matches) != 1:
        raise ValueError(f"Expected one {name} package in {header} sections.")
    match = matches[0]
    section, count = re.subn(
        r'^(version\s*=\s*)"[^"\n]*"',
        lambda item: item.group(1) + '"' + version + '"',
        match.group(),
        flags=re.MULTILINE,
    )
    if count != 1:
        raise ValueError("Expected one package version line.")
    return text[: match.start()] + section + text[match.end() :]


def prepare_release(root, version, check=False):
    """Validate both files before writing the requested package version."""
    requested = parse_version(version)
    manifest_path = root / "Cargo.toml"
    lock_path = root / "Cargo.lock"
    manifest = manifest_path.read_text(encoding="utf-8")
    lock = lock_path.read_text(encoding="utf-8")
    package = tomllib.loads(manifest)["package"]
    if package["name"] != "png2svg":
        raise ValueError("This tool only releases png2svg.")
    current = package["version"]
    if requested < parse_version(current):
        raise ValueError(f"Version {version} is older than {current}.")
    locked = [p for p in tomllib.loads(lock)["package"] if p["name"] == "png2svg"]
    if len(locked) != 1 or locked[0]["version"] != current:
        raise ValueError("Cargo.toml and Cargo.lock must have the same package version.")
    updated_manifest = replace_version(manifest, "[package]", "png2svg", version)
    updated_lock = replace_version(lock, "[[package]]", "png2svg", version)
    if not check:
        manifest_path.write_text(updated_manifest, encoding="utf-8")
        lock_path.write_text(updated_lock, encoding="utf-8")
    return current


def main():
    """Handle command line input and return a readable validation error."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version")
    parser.add_argument("--check", action="store_true", help="validate without writing")
    args = parser.parse_args()
    try:
        current = prepare_release(Path.cwd(), args.version, check=args.check)
    except (ValueError, KeyError, OSError) as error:
        print(f"Release version error: {error}", file=sys.stderr)
        return 1
    print(f"Release version: {current} -> {args.version}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
