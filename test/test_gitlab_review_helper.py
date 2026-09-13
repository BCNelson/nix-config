"""Exercise the review helper without GitLab credentials or network access."""

import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "home-manager/bcnelson/_mixins/claude/skill/pr-review-response/src/fetch-unresolved-comments.sh"


class GitLabReviewTests(unittest.TestCase):
    def run_helper(self, pages=(), args=(), fail="", mr=None, provider="gitlab"):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "mr.json").write_text(json.dumps(mr if mr is not None else {
                "project_id": 12, "source_project_id": 99, "iid": 7,
                "title": "Review me", "web_url": "https://git.example.com/team/sub/repo/-/merge_requests/7",
            }))
            (root / "pages.json").write_text("\n".join(json.dumps(page) for page in pages))
            stub = root / "glab"
            stub.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys
root = pathlib.Path(os.environ["REVIEW_FIXTURES"])
with (root / "calls").open("a") as log:
    log.write(json.dumps(sys.argv[1:]) + "\\n")
if sys.argv[1] == os.environ["REVIEW_FAIL"]:
    sys.exit(1)
if sys.argv[1:3] == ["mr", "view"]:
    print((root / "mr.json").read_text())
elif sys.argv[1] == "api":
    print((root / "pages.json").read_text())
else:
    sys.exit(2)
''')
            stub.chmod(0o755)
            (root / "gh").symlink_to(stub)
            env = dict(os.environ, PATH=f"{root}:{os.environ['PATH']}",
                       REVIEW_FIXTURES=str(root), REVIEW_FAIL=fail)
            result = subprocess.run(["bash", str(SCRIPT), *(["--gitlab"] if provider == "gitlab" else []), *args],
                                    env=env, text=True, capture_output=True)
            log = root / "calls"
            calls = [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
            return result, calls

    def note(self, body, **overrides):
        return dict({"id": 10, "body": body, "author": {"username": "reviewer"},
                     "created_at": "2026-09-07", "resolvable": True,
                     "resolved": False, "system": False}, **overrides)

    def test_pages_filtering_replies_and_deleted_lines(self):
        pages = [[
            {"id": "open", "notes": [self.note("Keep this", position={"old_path": "old.py", "old_line": 8}),
                                        self.note("Reply context", resolvable=False),
                                        self.note("System event", system=True, resolvable=False)]},
            {"id": "closed", "notes": [self.note("Already resolved", resolved=True)]},
            {"id": "standalone", "notes": [self.note("Standalone comment", resolvable=False)]},
        ], [{"id": "page-two", "notes": [self.note("Later page")]}]]
        result, calls = self.run_helper(pages)
        self.assertEqual(result.returncode, 0, result.stderr)
        for text in ["Keep this", "Reply context", "old.py:8", "Later page", "#note_10"]:
            self.assertIn(text, result.stdout)
        for text in ["Already resolved", "Standalone comment", "System event"]:
            self.assertNotIn(text, result.stdout)
        self.assertEqual(calls[0], ["mr", "view", "--output", "json"])
        self.assertEqual(calls[1], ["api", "--hostname", "git.example.com", "--paginate",
                                    "projects/12/merge_requests/7/discussions?per_page=100"])

    def test_explicit_review_and_repository(self):
        args = ["42", "--repo", "git.example.com/team/sub/repo"]
        result, calls = self.run_helper([[]], args)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls[0], ["mr", "view", *args, "--output", "json"])

    def test_errors_propagate(self):
        for command in ["mr", "api"]:
            with self.subTest(command=command):
                result, _ = self.run_helper(fail=command)
                self.assertNotEqual(result.returncode, 0)

    def test_missing_metadata_stops_before_api(self):
        result, calls = self.run_helper(mr={"message": "Not found"})
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(len(calls), 1)

    def test_github_explicit_review_still_works(self):
        response = {"data": {"repository": {"pullRequest": {
            "title": "GitHub review", "url": "https://github.com/team/repo/pull/3",
            "reviewThreads": {"nodes": []},
        }}}}
        result, calls = self.run_helper([response], ["team/repo", "3"], provider="github")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("GitHub review", result.stdout)
        self.assertEqual(calls[0][:2], ["api", "graphql"])


if __name__ == "__main__":
    unittest.main()
