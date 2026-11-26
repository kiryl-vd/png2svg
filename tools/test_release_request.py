"""Tests for version PR requests and publication after a protected merge."""

import copy
import tempfile
import unittest
from pathlib import Path

from release_request import prepare_request, requested_version
from test_prepare_release import MANIFEST, LOCK


REPOSITORY = "kiryl-vd/png2svg"
MERGED_PR = {"action": "closed", "pull_request": {
    "merged": True,
    "base": {"ref": "main"},
    "head": {"ref": "release/v0.2.0", "repo": {"full_name": REPOSITORY}},
}}


class RequestTests(unittest.TestCase):
    """Only an explicit request or merged version branch can release."""

    def test_manual_request(self):
        self.assertEqual(requested_version("workflow_dispatch", {"inputs": {"version": "0.2.0"}}, REPOSITORY), "0.2.0")

    def test_merged_version_pr(self):
        self.assertEqual(requested_version("pull_request", MERGED_PR, REPOSITORY), "0.2.0")

    def test_regular_fix_pr_does_not_release(self):
        event = copy.deepcopy(MERGED_PR)
        event["pull_request"]["head"]["ref"] = "fix/release-checksums"
        self.assertIsNone(requested_version("pull_request", event, REPOSITORY))

    def test_unmerged_pr_does_not_release(self):
        event = copy.deepcopy(MERGED_PR)
        event["pull_request"]["merged"] = False
        self.assertIsNone(requested_version("pull_request", event, REPOSITORY))

    def test_fork_or_other_base_does_not_release(self):
        for field in ("fork", "base", "deleted_repository"):
            event = copy.deepcopy(MERGED_PR)
            if field == "fork":
                event["pull_request"]["head"]["repo"]["full_name"] = "other/png2svg"
            elif field == "base":
                event["pull_request"]["base"]["ref"] = "other"
            else:
                event["pull_request"]["head"]["repo"] = None
            with self.subTest(field=field):
                self.assertIsNone(requested_version("pull_request", event, REPOSITORY))

    def test_invalid_version_branch_is_rejected(self):
        for version in ("01.2.0", "0.2.0$(id)", "0.2.0\n", "0.2.0/other"):
            event = copy.deepcopy(MERGED_PR)
            event["pull_request"]["head"]["ref"] = "release/v" + version
            with self.subTest(version=version):
                with self.assertRaises(ValueError):
                    requested_version("pull_request", event, REPOSITORY)

    def test_other_event_does_not_release(self):
        self.assertIsNone(requested_version("push", {}, REPOSITORY))

    def test_manual_invalid_version_is_rejected(self):
        with self.assertRaises(ValueError):
            requested_version("workflow_dispatch", {"inputs": {"version": "v1.0.0"}}, REPOSITORY)


class PreparedRequestTests(unittest.TestCase):
    """Merged release versions must match the protected source exactly."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "Cargo.toml").write_text(MANIFEST)
        (self.root / "Cargo.lock").write_text(LOCK)

    def test_higher_manual_version_is_valid_without_changing_files(self):
        result = prepare_request(self.root, "workflow_dispatch", {"inputs": {"version": "0.2.0"}}, REPOSITORY)
        self.assertEqual(result, ("0.2.0", "0.1.0"))
        self.assertEqual((self.root / "Cargo.toml").read_text(), MANIFEST)

    def test_unchanged_first_version_is_valid(self):
        result = prepare_request(self.root, "workflow_dispatch", {"inputs": {"version": "0.1.0"}}, REPOSITORY)
        self.assertEqual(result, ("0.1.0", "0.1.0"))

    def test_merge_with_wrong_version_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "merged release PR"):
            prepare_request(self.root, "pull_request", MERGED_PR, REPOSITORY)
        self.assertEqual((self.root / "Cargo.toml").read_text(), MANIFEST)

    def test_merge_with_matching_version_is_valid(self):
        (self.root / "Cargo.toml").write_text(MANIFEST.replace("0.1.0", "0.2.0"))
        (self.root / "Cargo.lock").write_text(LOCK.replace("0.1.0", "0.2.0"))
        self.assertEqual(prepare_request(self.root, "pull_request", MERGED_PR, REPOSITORY), ("0.2.0", "0.2.0"))


class ProtectedWorkflowTests(unittest.TestCase):
    """Keep version changes in PRs and run CI on their branch commits."""

    def test_no_direct_main_push_or_ref_update(self):
        workflow = (Path(__file__).resolve().parents[1] / ".github/workflows/publish-release.yml").read_text()
        self.assertNotIn("git push origin main", workflow)
        self.assertNotIn("git update-ref refs/heads/main", workflow)
        self.assertIn('git push origin "refs/heads/$branch"', workflow)
        self.assertIn('gh pr create --repo "$GITHUB_REPOSITORY" --base main --head "$branch"', workflow)

    def test_ci_dispatch_and_merged_commit_checkout(self):
        root = Path(__file__).resolve().parents[1]
        workflow = (root / ".github/workflows/publish-release.yml").read_text()
        ci = (root / ".github/workflows/ci.yml").read_text()
        self.assertIn('gh workflow run ci.yml --repo "$GITHUB_REPOSITORY" --ref "$branch"', workflow)
        self.assertIn("  workflow_dispatch:\n", ci)
        self.assertIn("github.event.pull_request.merge_commit_sha || 'main'", workflow)
        self.assertIn("if: needs.prepare.outputs.publish == 'true'", workflow)
        self.assertIn("types: [closed]", workflow)


if __name__ == "__main__":
    unittest.main()
