"""Tests for release version validation and Cargo file updates."""

import tempfile
import unittest
from pathlib import Path

from prepare_release import parse_version, prepare_release, replace_version


MANIFEST = '''[package]
name = "png2svg"
version = "0.1.0" # keep this comment
rust-version = "1.75"
publish = false

[package.metadata.dist]
dist = true
'''
LOCK = '''# Keep the lockfile format for Rust 1.75.
version = 3

[[package]]
name = "png2svg"
version = "0.1.0"

[[package]]
name = "other"
version = "9.8.7"
'''


class VersionTests(unittest.TestCase):
    """Reject invalid or unsafe input before it becomes a Git tag."""

    def test_valid_versions(self):
        for version, parts in [("0.1.0", (0, 1, 0)), ("1.20.300", (1, 20, 300))]:
            with self.subTest(version=version):
                self.assertEqual(parse_version(version), parts)

    def test_invalid_versions(self):
        for version in ["", "v0.1.0", "1.0", "01.0.0", "1.0.0-rc.1", "1.0.0+build",
                        "1.0.0\n", " 1.0.0", "1.0.0$(id)", "-1.0.0", "1.0.0/other",
                        f"{2**64}.0.0", "9" * 1000 + ".0.0"]:
            with self.subTest(version=version):
                with self.assertRaises(ValueError):
                    parse_version(version)

    def test_maximum_cargo_version_parts(self):
        value = 2**64 - 1
        self.assertEqual(parse_version(f"{value}.{value}.{value}"), (value, value, value))


class FileTests(unittest.TestCase):
    """Keep unrelated metadata and reject inconsistent version files."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "Cargo.toml").write_text(MANIFEST, encoding="utf-8")
        (self.root / "Cargo.lock").write_text(LOCK, encoding="utf-8")

    def test_update_preserves_comments_and_lock_format(self):
        self.assertEqual(prepare_release(self.root, "0.2.0"), "0.1.0")
        self.assertEqual((self.root / "Cargo.toml").read_text(), MANIFEST.replace("0.1.0", "0.2.0"))
        self.assertEqual((self.root / "Cargo.lock").read_text(), LOCK.replace("0.1.0", "0.2.0"))

    def test_first_release_or_retry_can_keep_current_version(self):
        prepare_release(self.root, "0.1.0")
        self.assertEqual((self.root / "Cargo.toml").read_text(), MANIFEST)
        self.assertEqual((self.root / "Cargo.lock").read_text(), LOCK)

    def test_check_does_not_write(self):
        prepare_release(self.root, "1.0.0", check=True)
        self.assertEqual((self.root / "Cargo.toml").read_text(), MANIFEST)
        self.assertEqual((self.root / "Cargo.lock").read_text(), LOCK)

    def test_downgrade_does_not_write(self):
        with self.assertRaises(ValueError):
            prepare_release(self.root, "0.0.9")
        self.assertEqual((self.root / "Cargo.toml").read_text(), MANIFEST)
        self.assertEqual((self.root / "Cargo.lock").read_text(), LOCK)

    def test_lock_mismatch_does_not_write(self):
        (self.root / "Cargo.lock").write_text(LOCK.replace("0.1.0", "0.0.9"))
        with self.assertRaises(ValueError):
            prepare_release(self.root, "0.2.0")
        self.assertEqual((self.root / "Cargo.toml").read_text(), MANIFEST)

    def test_duplicate_package_does_not_write(self):
        (self.root / "Cargo.lock").write_text(LOCK + '\n[[package]]\nname = "png2svg"\nversion = "0.1.0"\n')
        with self.assertRaises(ValueError):
            prepare_release(self.root, "0.2.0")
        self.assertEqual((self.root / "Cargo.toml").read_text(), MANIFEST)

    def test_wrong_package_is_rejected(self):
        (self.root / "Cargo.toml").write_text(MANIFEST.replace("png2svg", "another"))
        with self.assertRaises(ValueError):
            prepare_release(self.root, "0.2.0")

    def test_missing_version_line_is_rejected(self):
        with self.assertRaises(ValueError):
            replace_version('[package]\nname = "png2svg"\n', "[package]", "png2svg", "1.0.0")

    def test_cli_reports_invalid_input(self):
        import subprocess
        import sys
        script = Path(__file__).with_name("prepare_release.py")
        result = subprocess.run([sys.executable, str(script), "v1.0.0"], cwd=self.root,
                                capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 1)
        self.assertIn("Release version error", result.stderr)
        self.assertEqual((self.root / "Cargo.toml").read_text(), MANIFEST)


if __name__ == "__main__":
    unittest.main()
