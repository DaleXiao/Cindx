from __future__ import annotations

import hashlib
import json
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
ANALYZER = ROOT / "scripts" / "analyze-pilot-v2.py"


class PilotV2AnalyzerTests(unittest.TestCase):
    def test_sanitized_report_records_raw_digest_and_capture_time(self) -> None:
        raw = {
            "schema": "cindx.pilot_v2.raw.v2",
            "pilot_id": "test",
            "generated_at_ms": 1_700_000_000_000,
            "git_commit": "a" * 40,
            "app_version": "0.0.0",
            "gepa_frozen": False,
            "evaluation_limits": {"first_pass_run_cap": 1},
            "runs": [],
        }
        encoded = (json.dumps(raw, separators=(",", ":")) + "\n").encode()

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            raw_path = root / "raw.json"
            json_path = root / "sanitized.json"
            markdown_path = root / "report.md"
            raw_path.write_bytes(encoded)

            subprocess.run(
                [
                    "python3",
                    str(ANALYZER),
                    str(raw_path),
                    str(json_path),
                    str(markdown_path),
                ],
                cwd=ROOT,
                check=True,
            )

            digest = hashlib.sha256(encoded).hexdigest()
            sanitized = json.loads(json_path.read_text())
            markdown = markdown_path.read_text()
            self.assertEqual(sanitized["raw_evidence_sha256"], digest)
            self.assertIn(f"Raw evidence SHA-256: `{digest}`", markdown)
            self.assertIn("Captured: `2023-11-14T22:13:20Z`", markdown)
            self.assertIn("No targeted second pass was run", markdown)


if __name__ == "__main__":
    unittest.main()
