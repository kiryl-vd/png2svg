"""Prevent reuse of published versions and skipped stable version numbers."""

from prepare_release import parse_version


MAX_PART = 2**64 - 1


def next_versions(version):
    """Return valid one-step patch, minor and major versions without overflow."""
    major, minor, patch = parse_version(version)
    candidates = []
    if patch < MAX_PART:
        candidates.append(f"{major}.{minor}.{patch + 1}")
    if minor < MAX_PART:
        candidates.append(f"{major}.{minor + 1}.0")
    if major < MAX_PART:
        candidates.append(f"{major + 1}.0.0")
    return candidates


def check_release_sequence(version, current, releases):
    """Validate against all published releases, not just GitHub's latest label.

    Published stable tag numbers stay reserved even if someone removes a tag.
    Drafts and prereleases do not define the previous stable release. Equal
    package versions remain valid for the first release or an unpublished retry.
    """
    requested = parse_version(version)
    package_version = parse_version(current)
    used = set()
    stable = set()
    for release in releases:
        if release["draft"]:
            continue
        tag = release["tag_name"]
        try:
            parts = parse_version(tag[1:] if tag.startswith("v") else tag)
        except ValueError:
            # Non-version tags and prerelease suffixes are not stable versions.
            continue
        used.add(parts)
        if not release["prerelease"]:
            stable.add(parts)
    if requested in used:
        raise ValueError(f"Version {version} has already been published.")
    if not stable:
        if requested != package_version:
            raise ValueError(f"The first stable release must use the current version {current}.")
        return
    # Publication dates and GitHub's latest flag can be changed independently.
    previous = ".".join(str(part) for part in max(stable))
    allowed = next_versions(previous)
    if version not in allowed:
        choices = ", ".join(allowed) if allowed else "none (Cargo version limit reached)"
        raise ValueError(f"Version {version} skips a release step after {previous}. Allowed: {choices}.")
