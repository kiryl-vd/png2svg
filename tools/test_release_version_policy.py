"""Tests for published version reuse, skipped versions and release history."""

import copy
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from release_request import prepare_request, read_release_history
from release_version_policy import check_release_sequence, next_versions
from test_prepare_release import MANIFEST, LOCK
from test_release_request import MERGED_PR, REPOSITORY


def release(version, draft=False, prerelease=False):
    """Create a GitHub release response fixture without Git tag state."""
    return {"tag_name": version, "draft": draft, "prerelease": prerelease}


class SequenceTests(unittest.TestCase):
    """Allow only one canonical patch, minor or major version increase."""

    def test_canonical_next_versions(self):
        self.assertEqual(next_versions("1.9.9"), ["1.9.10", "1.10.0", "2.0.0"])
        for version in next_versions("1.9.9"):
            with self.subTest(version=version):
                check_release_sequence(version, "1.9.9", [release("v1.9.9")])

    def test_skipped_versions_are_rejected(self):
        for version in ("3.0.0", "2.1.0", "2.0.1", "1.11.0", "1.10.1", "1.9.11"):
            with self.subTest(version=version):
                with self.assertRaisesRegex(ValueError, "Allowed: 1.9.10, 1.10.0, 2.0.0"):
                    check_release_sequence(version, "1.9.9", [release("v1.9.9")])

    def test_patch_and_minor_reset_rules(self):
        self.assertEqual(next_versions("2.3.4"), ["2.3.5", "2.4.0", "3.0.0"])

    def test_published_version_cannot_be_reused_even_without_a_tag(self):
        with self.assertRaisesRegex(ValueError, "already been published"):
            check_release_sequence("1.9.9", "1.9.9", [release("v1.9.9")])

    def test_older_published_version_cannot_be_reused(self):
        with self.assertRaisesRegex(ValueError, "already been published"):
            check_release_sequence("1.8.0", "1.9.9", [release("v1.8.0"), release("v1.9.9")])

    def test_latest_means_highest_semver_not_response_order(self):
        history = [release("v1.9.9"), release("v0.9.0"), release("v1.9.10")]
        check_release_sequence("1.9.11", "1.9.10", history)
        with self.assertRaises(ValueError):
            check_release_sequence("1.9.10", "1.9.10", history)

    def test_drafts_prereleases_and_non_version_tags_do_not_define_baseline(self):
        history = [release("v1.9.9"), release("v9.0.0", draft=True),
                   release("v7.0.0", prerelease=True), release("v8.0.0-rc.1", prerelease=True),
                   release("nightly")]
        check_release_sequence("2.0.0", "1.9.9", history)

    def test_published_stable_tag_marked_prerelease_is_still_reserved(self):
        with self.assertRaisesRegex(ValueError, "already been published"):
            check_release_sequence("2.0.0", "2.0.0", [release("v1.9.9"), release("v2.0.0", prerelease=True)])

    def test_unprefixed_published_version_is_reserved(self):
        with self.assertRaisesRegex(ValueError, "already been published"):
            check_release_sequence("1.9.9", "1.9.9", [release("1.9.9")])

    def test_first_release_uses_current_package_version(self):
        check_release_sequence("0.1.0", "0.1.0", [])
        with self.assertRaisesRegex(ValueError, "first stable release"):
            check_release_sequence("3.0.0", "0.1.0", [])

    def test_unpublished_retry_keeps_current_version(self):
        check_release_sequence("1.9.10", "1.9.10", [release("v1.9.9")])

    def test_numeric_limits_do_not_overflow_or_roll_over(self):
        limit = 2**64 - 1
        self.assertEqual(next_versions(f"1.2.{limit}"), ["1.3.0", "2.0.0"])
        self.assertEqual(next_versions(f"{limit}.{limit}.{limit}"), [])


class HistoryTests(unittest.TestCase):
    """Read every API page and reject malformed history before release changes."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "Cargo.toml").write_text(MANIFEST)
        (self.root / "Cargo.lock").write_text(LOCK)
        self.history_path = self.root / "history.json"

    def test_all_pages_are_used(self):
        self.history_path.write_text(json.dumps([[release("v1.9.9")], [release("v2.0.0")]]))
        history = read_release_history(self.history_path)
        self.assertEqual(len(history), 2)
        check_release_sequence("2.0.1", "2.0.0", history)
        with self.assertRaises(ValueError):
            check_release_sequence("1.9.10", "1.9.9", history)

    def test_invalid_or_incomplete_page_format_is_rejected(self):
        for value in ({"message": "API failure"}, [release("v1.9.9")], [["not an object"]]):
            with self.subTest(value=value):
                self.history_path.write_text(json.dumps(value))
                with self.assertRaises(ValueError):
                    read_release_history(self.history_path)

    def test_empty_history(self):
        self.history_path.write_text("[[]]")
        self.assertEqual(read_release_history(self.history_path), [])

    def test_manual_jump_does_not_change_version_files(self):
        with self.assertRaises(ValueError):
            prepare_request(self.root, "workflow_dispatch", {"inputs": {"version": "3.0.0"}},
                            REPOSITORY, [release("v0.1.0")])
        self.assertEqual((self.root / "Cargo.toml").read_text(), MANIFEST)
        self.assertEqual((self.root / "Cargo.lock").read_text(), LOCK)

    def test_merged_pr_is_checked_again_against_published_history(self):
        (self.root / "Cargo.toml").write_text(MANIFEST.replace("0.1.0", "3.0.0"))
        (self.root / "Cargo.lock").write_text(LOCK.replace("0.1.0", "3.0.0"))
        event = copy.deepcopy(MERGED_PR)
        event["pull_request"]["head"]["ref"] = "release/v3.0.0"
        with self.assertRaises(ValueError):
            prepare_request(self.root, "pull_request", event, REPOSITORY, [release("v1.9.9")])

    def test_cli_rejects_jump_before_emitting_release_outputs(self):
        event_path = self.root / "event.json"
        event_path.write_text(json.dumps({"inputs": {"version": "3.0.0"}}))
        self.history_path.write_text(json.dumps([[release("v0.1.0")]]))
        env = dict(os.environ, GITHUB_EVENT_PATH=str(event_path), GITHUB_EVENT_NAME="workflow_dispatch",
                   GITHUB_REPOSITORY=REPOSITORY)
        script = Path(__file__).with_name("release_request.py")
        result = subprocess.run([sys.executable, str(script), "--release-history", str(self.history_path)],
                                cwd=self.root, env=env, capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 1)
        self.assertIn("skips a release step", result.stderr)
        self.assertNotIn("is_release=true", result.stdout)


class WorkflowPolicyTests(unittest.TestCase):
    """Fetch and check release history before any version PR is created."""

    def test_history_is_paginated_and_checked_before_writes(self):
        text = (Path(__file__).resolve().parents[1] / ".github/workflows/publish-release.yml").read_text()
        self.assertIn('gh api --paginate --slurp "repos/$GITHUB_REPOSITORY/releases?per_page=100"', text)
        check = text.index('python tools/release_request.py --release-history release-history.json')
        write = text.index('python tools/prepare_release.py "$RELEASE_VERSION"')
        self.assertLess(check, write)
        self.assertIn('git show-ref --verify --quiet "refs/tags/v$RELEASE_VERSION"', text)


if __name__ == "__main__":
    unittest.main()
