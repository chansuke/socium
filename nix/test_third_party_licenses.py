"""Tests for third-party-licenses.py; the package build runs them first."""
import importlib.util
import io
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "third_party_licenses", Path(__file__).with_name("third-party-licenses.py")
)
licenses = importlib.util.module_from_spec(spec)
spec.loader.exec_module(licenses)


class Notices(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp())
        self.spdx = self.root / "spdx"
        self.spdx.mkdir()
        for name in ("MIT", "Apache-2.0", "GPL-2.0-or-later", "LLVM-exception"):
            (self.spdx / f"{name}.txt").write_text(f"text of {name}\n")

    def crate(self, name, manifest, files=()):
        crate = self.root / "vendor" / "src" / name
        crate.mkdir(parents=True)
        (crate / "Cargo.toml").write_text("[package]\n" + manifest)
        for path in files:
            (crate / path).parent.mkdir(parents=True, exist_ok=True)
            (crate / path).write_text(f"contents of {path}\n")

    def run_notices(self):
        out = io.StringIO()
        missing = licenses.notices(self.root / "vendor", self.spdx, out)
        return out.getvalue(), missing

    def test_license_files_are_copied(self):
        self.crate("a-1.0.0", 'license = "MIT"\n', ["LICENSE-MIT", "src/lib.rs"])
        text, missing = self.run_notices()
        self.assertEqual(missing, [])
        self.assertIn("contents of LICENSE-MIT", text)
        self.assertNotIn("text of MIT", text)

    def test_reuse_directory_and_license_file_are_read(self):
        self.crate("b-1.0.0", 'license-file = "docs/TERMS"\n', ["LICENSES/MIT.txt", "docs/TERMS"])
        text, missing = self.run_notices()
        self.assertEqual(missing, [])
        self.assertIn("-- LICENSES/MIT.txt", text)
        self.assertIn("-- docs/TERMS", text)

    def test_a_notice_alone_still_gets_the_license_text(self):
        self.crate("c-1.0.0", 'license = "Apache-2.0"\n', ["NOTICE"])
        text, missing = self.run_notices()
        self.assertEqual(missing, [])
        self.assertIn("contents of NOTICE", text)
        self.assertIn("text of Apache-2.0", text)

    def test_expressions_name_every_license_and_exception(self):
        self.crate("d-1.0.0", 'license = "MIT/Apache-2.0 OR (GPL-2.0+ WITH LLVM-exception)"\nauthors = ["A", "B"]\n')
        text, missing = self.run_notices()
        self.assertEqual(missing, [])
        self.assertIn("Authors: A, B", text)
        for name in ("MIT", "Apache-2.0", "GPL-2.0-or-later", "LLVM-exception"):
            self.assertIn(f"text of {name}", text)

    def test_an_unknown_license_without_a_file_is_reported(self):
        self.crate("e-1.0.0", 'license = "Weird-1.0"\n')
        self.crate("f-1.0.0", "")
        _, missing = self.run_notices()
        self.assertEqual(len(missing), 2)
        self.assertIn("Weird-1.0", missing[0])


if __name__ == "__main__":
    unittest.main()
