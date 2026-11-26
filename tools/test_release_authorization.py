"""Check the authorization conditions in both release jobs with write access.

The workflow uses only equality checks joined with AND. These tests evaluate
that small condition format directly, without a YAML dependency.
"""

import re
import unittest
from pathlib import Path


WORKFLOW = Path(__file__).resolve().parents[1] / ".github/workflows/publish-release.yml"
OWNER = {
    "repository": "kiryl-vd/png2svg",
    "ref": "refs/heads/main",
    "actor_id": "61699922",
    "actor": "kiryl-vd",
    "triggering_actor": "kiryl-vd",
}


def job_conditions(job):
    """Read a job's literal authorization checks and reject other operators."""
    text = WORKFLOW.read_text(encoding="utf-8")
    block = re.search(r"^  " + re.escape(job) + r":\n(.*?)(?=^  [\w-]+:|\Z)",
                      text, re.MULTILINE | re.DOTALL)
    if block is None:
        raise AssertionError(f"Missing job: {job}")
    condition = re.search(r"^    if: >-\n((?:      .+\n)+)", block.group(1), re.MULTILINE)
    if condition is None:
        raise AssertionError(f"Missing authorization check: {job}")
    predicates = []
    for item in condition.group(1).strip().split("&&"):
        match = re.fullmatch(r"github\.(\w+) == '([^']+)'", item.strip())
        if match is None:
            raise AssertionError(f"Unexpected authorization expression: {item}")
        predicates.append(match.groups())
    return predicates


def authorized(job, context):
    """Evaluate all required literal checks for a workflow invocation."""
    return all(context.get(key) == value for key, value in job_conditions(job))


class AuthorizationTests(unittest.TestCase):
    """Reject other accounts, branches, forks and unauthorized reruns."""

    def test_both_write_jobs_require_every_owner_condition(self):
        for job in ("prepare", "publish"):
            with self.subTest(job=job):
                self.assertEqual(dict(job_conditions(job)), OWNER)
                self.assertTrue(authorized(job, OWNER))

    def test_another_account_cannot_start_a_release(self):
        context = dict(OWNER, actor="another-user", actor_id="999", triggering_actor="another-user")
        for job in ("prepare", "publish"):
            with self.subTest(job=job):
                self.assertFalse(authorized(job, context))

    def test_another_account_cannot_rerun_an_owner_release(self):
        context = dict(OWNER, triggering_actor="another-user")
        self.assertFalse(authorized("prepare", context))
        self.assertFalse(authorized("publish", context))

    def test_owner_cannot_rerun_a_release_started_by_someone_else(self):
        context = dict(OWNER, actor="another-user", actor_id="999")
        self.assertFalse(authorized("prepare", context))
        self.assertFalse(authorized("publish", context))

    def test_wrong_branch_repository_or_account_id_is_rejected(self):
        changes = {"repository": "another-user/png2svg", "ref": "refs/heads/test",
                   "actor_id": "999", "actor": "another-user", "triggering_actor": "another-user"}
        for key, value in changes.items():
            for job in ("prepare", "publish"):
                with self.subTest(key=key, job=job):
                    self.assertFalse(authorized(job, dict(OWNER, **{key: value})))

    def test_missing_identity_is_rejected(self):
        for key in OWNER:
            context = dict(OWNER)
            del context[key]
            for job in ("prepare", "publish"):
                with self.subTest(key=key, job=job):
                    self.assertFalse(authorized(job, context))


if __name__ == "__main__":
    unittest.main()
