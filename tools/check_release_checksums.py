#!/usr/bin/env python3
"""Check that sha256.sum covers every archive in a cargo-dist release plan."""

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path


CHECKSUM = re.compile(r"([0-9a-fA-F]{64}) [ *](.+)")


def read_checksums(path):
    """Read a SHA-256 list and reject duplicates or unsafe file names."""
    entries = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line or line.startswith("#"):
            continue
        match = CHECKSUM.fullmatch(line)
        if match is None:
            raise ValueError("Invalid SHA-256 line.")
        digest, name = match.groups()
        if "/" in name or "\\" in name or name in (".", ".."):
            raise ValueError("Checksum names must refer to files in the release directory.")
        if name in entries:
            raise ValueError(f"Duplicate checksum: {name}")
        entries[name] = digest.lower()
    return entries


def verify_checksums(plan, directory):
    """Require a correct checksum for every binary and source archive."""
    expected = {
        name for name, artifact in plan["artifacts"].items()
        if artifact["kind"] in ("executable-zip", "source-tarball")
    }
    if not expected:
        raise ValueError("The release plan contains no archives.")
    entries = read_checksums(directory / "sha256.sum")
    if set(entries) != expected:
        missing = sorted(expected - set(entries))
        extra = sorted(set(entries) - expected)
        raise ValueError(f"Incomplete checksum list. Missing: {missing}. Unexpected: {extra}.")
    for name in sorted(expected):
        path = directory / name
        if path.is_symlink():
            raise ValueError(f"Release archives must not be symbolic links: {name}")
        digest = hashlib.sha256()
        with path.open("rb") as archive:
            while chunk := archive.read(1024 * 1024):
                digest.update(chunk)
        if digest.hexdigest() != entries[name]:
            raise ValueError(f"SHA-256 mismatch: {name}")
    return len(expected)


def main():
    """Report incomplete or incorrect checksums before assets are published."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("plan", type=Path)
    parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    try:
        plan = json.loads(args.plan.read_text(encoding="utf-8"))
        count = verify_checksums(plan, args.directory)
    except (ValueError, KeyError, OSError) as error:
        print(f"Release checksum error: {error}", file=sys.stderr)
        return 1
    print(f"SHA-256 checked for all {count} release archives.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
