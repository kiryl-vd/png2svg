#!/usr/bin/env python3
"""Identify a manual version request or a merged release pull request."""

import argparse
import json
import os
import sys
from pathlib import Path

from prepare_release import parse_version, prepare_release
from release_version_policy import check_release_sequence


BRANCH_PREFIX = "release/v"


def requested_version(event_name, event, repository):
    """Return a release version, or None for unrelated pull request events."""
    if event_name == "workflow_dispatch":
        version = event["inputs"]["version"]
    elif event_name == "pull_request":
        request = event["pull_request"]
        head = request["head"]
        if (not request.get("merged") or request["base"]["ref"] != "main"
                or (head.get("repo") or {}).get("full_name") != repository
                or not head["ref"].startswith(BRANCH_PREFIX)):
            return None
        version = head["ref"][len(BRANCH_PREFIX):]
    else:
        return None
    parse_version(version)
    return version


def prepare_request(root, event_name, event, repository, releases):
    """Check version files and published history before any release changes."""
    version = requested_version(event_name, event, repository)
    if version is None:
        return None
    current = prepare_release(root, version, check=True)
    if event_name == "pull_request" and current != version:
        raise ValueError("A merged release PR must contain the requested version.")
    check_release_sequence(version, current, releases)
    return version, current


def read_release_history(path):
    """Read all pages from gh api --paginate --slurp and reject bad input."""
    pages = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(pages, list) or any(not isinstance(page, list) for page in pages):
        raise ValueError("Release history must contain all API pages.")
    releases = [release for page in pages for release in page]
    if any(not isinstance(release, dict) for release in releases):
        raise ValueError("Release history entries must be objects.")
    return releases


def main():
    """Write safe step outputs for GitHub Actions without changing files."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--release-history", type=Path, required=True)
    args = parser.parse_args()
    try:
        event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text(encoding="utf-8"))
        releases = read_release_history(args.release_history)
        result = prepare_request(Path.cwd(), os.environ["GITHUB_EVENT_NAME"], event,
                                 os.environ["GITHUB_REPOSITORY"], releases)
    except (ValueError, KeyError, OSError) as error:
        print(f"Release request error: {error}", file=sys.stderr)
        return 1
    if result is None:
        print("is_release=false")
    else:
        version, current = result
        print("is_release=true")
        print(f"version={version}")
        print(f"current={current}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
