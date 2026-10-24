#!/usr/bin/env python3
"""Exercise release gates with real Git repositories and offline API fixtures."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


SOURCE = Path(__file__).resolve().parent.parent


class ReleaseCandidateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="ewf-release-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        tools = self.root / "bin"
        tools.mkdir()
        self.output = self.root / "output"
        self.metadata = self.root / "metadata.json"
        self.metadata.write_text(json.dumps({"packages": [
            {"name": "ewf-image", "version": "0.6.1"},
            {"name": "aff4-image", "version": "0.3.0"},
        ]}))
        self.checks_path = self.root / "checks.json"
        config = SOURCE / ".github/main-branch-protection.json"
        required = json.loads(config.read_text())["required_status_checks"]["checks"]
        self.checks = [{
            "name": item["context"], "app": {"id": item["app_id"]},
            "status": "completed", "conclusion": "success",
            "started_at": "2026-01-01T00:00:00Z",
        } for item in required]
        self.save_checks()
        for name, body in {
            "cargo": 'cat "$RELEASE_TEST_METADATA"\n',
            "gh": ('if [[ "${RELEASE_TEST_API_FAIL:-}" == 1 ]]; then exit 1; fi\n'
                   'cat "$RELEASE_TEST_CHECKS"\n'),
        }.items():
            path = tools / name
            path.write_text("#!/usr/bin/env bash\nset -eu\n" + body)
            path.chmod(0o755)
        self.env = dict(os.environ, PATH=str(tools) + os.pathsep + os.environ["PATH"],
                        GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_SYSTEM=os.devnull,
                        GITHUB_REPOSITORY="ebrig/ewf-image", GITHUB_OUTPUT=str(self.output),
                        RELEASE_TEST_METADATA=str(self.metadata),
                        RELEASE_TEST_CHECKS=str(self.checks_path))
        self.git("init", "--quiet", "--initial-branch=main")
        self.git("config", "user.name", "Release tests")
        self.git("config", "user.email", "release-tests@example.invalid")
        (self.repo / ".github").mkdir()
        shutil.copyfile(config, self.repo / ".github/main-branch-protection.json")
        shutil.copyfile(SOURCE / "scripts/verify-release-candidate.sh", self.repo / "verify.sh")
        self.git("add", ".")
        self.git("commit", "--quiet", "-m", "Release fixture")
        remote = self.root / "remote.git"
        self.git("init", "--quiet", "--bare", str(remote))
        self.git("remote", "add", "origin", str(remote))
        self.git("push", "--quiet", "origin", "main")
        for tag in ("v0.6.1", "aff4-image-v0.3.0"):
            self.git("tag", "-a", tag, "-m", "Release fixture")

    def git(self, *args):
        return subprocess.run(["git", *args], cwd=self.repo, env=self.env,
                              text=True, capture_output=True, check=True)

    def save_checks(self):
        self.checks_path.write_text(json.dumps({"check_runs": self.checks}))

    def gate(self, tag="v0.6.1", package=None):
        self.output.unlink(missing_ok=True)
        result = subprocess.run(["bash", "verify.sh", tag], cwd=self.repo, env=self.env,
                                text=True, capture_output=True)
        if package:
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(self.output.read_text(), f"package={package}\n")
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertFalse(self.output.exists(), "Rejected release selected a publish package")
        return result

    def test_selects_ewf_from_workspace(self):
        self.gate(package="ewf-image")

    def test_selects_aff4_from_workspace(self):
        self.gate("aff4-image-v0.3.0", package="aff4-image")

    def test_rejects_unknown_tags(self):
        for tag in ("", "ewf-cli-v0.2.0", "aff4-image-vbad", "v1.2", "aff4-image-0.3.0"):
            with self.subTest(tag=tag):
                self.gate(tag)

    def test_requires_local_annotated_tag(self):
        self.gate("v0.6.2")
        self.git("tag", "-d", "v0.6.1")
        self.git("tag", "v0.6.1")
        self.gate()

    def test_rejects_version_mismatch(self):
        self.git("tag", "-a", "aff4-image-v0.4.0", "-m", "Wrong version")
        self.gate("aff4-image-v0.4.0")

    def test_rejects_missing_or_ambiguous_package(self):
        for packages in ([], [{"name": "aff4-image", "version": "0.3.0"}] * 2):
            with self.subTest(packages=packages):
                self.metadata.write_text(json.dumps({"packages": packages}))
                self.gate("aff4-image-v0.3.0")

    def test_rejects_wrong_checkout(self):
        self.git("commit", "--quiet", "--allow-empty", "-m", "Another commit")
        self.gate()

    def test_rejects_tag_outside_main(self):
        self.git("commit", "--quiet", "--allow-empty", "-m", "Unmerged commit")
        self.git("tag", "-f", "-a", "v0.6.1", "-m", "Unmerged release")
        self.gate()

    def test_rejects_dirty_checkout(self):
        with (self.repo / "verify.sh").open("a") as script:
            script.write("\n# Local change\n")
        self.gate()

    def test_rejects_unsuccessful_required_checks(self):
        for status, conclusion in (("completed", "failure"), ("completed", "skipped"),
                                   ("completed", "cancelled"), ("in_progress", None)):
            with self.subTest(status=status, conclusion=conclusion):
                self.checks[0].update(status=status, conclusion=conclusion)
                self.save_checks()
                self.gate("aff4-image-v0.3.0")

    def test_rejects_missing_required_check(self):
        self.checks.pop()
        self.save_checks()
        self.gate()

    def test_rejects_wrong_provider(self):
        self.checks[0]["app"]["id"] = 1
        self.save_checks()
        self.gate()

    def test_rejects_newer_failed_check(self):
        self.checks.append(dict(self.checks[0], conclusion="failure",
                                started_at="2026-02-01T00:00:00Z"))
        self.save_checks()
        self.gate()

    def test_rejects_unavailable_check_api(self):
        self.env["RELEASE_TEST_API_FAIL"] = "1"
        self.gate()


if __name__ == "__main__":
    unittest.main()
