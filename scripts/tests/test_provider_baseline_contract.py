"""Synthetic validator fixtures only; these tests are not model-quality evidence."""

from __future__ import annotations

import copy
import hashlib
import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parents[1]
REPOSITORY_ROOT = SCRIPT_DIR.parent
sys.path.insert(0, str(SCRIPT_DIR))

SPEC = importlib.util.spec_from_file_location(
    "analyze_fugu_effect_eval", SCRIPT_DIR / "analyze-fugu-effect-eval.py"
)
assert SPEC and SPEC.loader
ANALYZER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ANALYZER)

from provider_baseline_contract import BaselineContractError  # noqa: E402


class ProviderBaselineContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.contract_path = (
            REPOSITORY_ROOT / "benchmarks/agent/provider-baseline-v1.json"
        )
        cls.contract_bytes = cls.contract_path.read_bytes()
        cls.contract = json.loads(cls.contract_bytes)

    def fixture(self) -> dict:
        runs = []
        for case_index, case in enumerate(self.contract["sampling"]["cases"]):
            for treatment_index, treatment in enumerate(self.contract["treatments"]):
                treatment_id = treatment["id"]
                expected_answer = next(
                    answer
                    for answer in "ABCD"
                    if hashlib.sha256(answer.encode()).hexdigest()
                    == case["expected_sha256"]
                )
                output = f"RAW_OUTPUT_SECRET\nFinal answer: {expected_answer}"
                output_hash = hashlib.sha256(output.encode()).hexdigest()
                runs.append(
                    {
                        "benchmark": "gpqa_diamond",
                        "case_id": case["id"],
                        "category": case["domain"],
                        "treatment": treatment_id,
                        "treatment_position": (treatment_index - case_index) % 3,
                        "requested_policy": treatment_id,
                        "effective_policy": "synthetic-test-policy",
                        "models": ["synthetic-test-model"],
                        "prompt_profile": treatment["prompt_profile"],
                        "prompt_profile_origin": treatment[
                            "prompt_profile_origin"
                        ],
                        "prompt_profile_sha256": treatment[
                            "prompt_profile_sha256"
                        ],
                        "prompt_profile_artifact_sha256": None,
                        "gepa_frozen": treatment["gepa_frozen"],
                        "succeeded": True,
                        "latency_ms": 1,
                        "prompt_tokens": 1,
                        "completion_tokens": 1,
                        "total_tokens": 2,
                        "first_token_latency_ms": 1,
                        "input_sha256": case["input_sha256"],
                        "expected_sha256": case["expected_sha256"],
                        "output_sha256": output_hash,
                        "parsed_answer": expected_answer,
                        "exact_score": 1.0,
                        "expected": expected_answer,
                        "output": output,
                        "error": "Authorization RAW_ERROR_SECRET",
                        "authorization": "Bearer RAW_AUTH_SECRET",
                        "api_key": "RAW_RUN_API_KEY_SECRET",
                    }
                )
        source = self.contract["source"]
        return {
            "schema": "cindx.external_effect_eval.raw.v3",
            "generated_at_ms": 1,
            "git_commit": "a" * 40,
            "app_version": self.contract["app_version"],
            "provider_endpoint": "https://secret-provider.example/v1",
            "authorization": "Bearer RAW_TOP_AUTH_SECRET",
            "api_key": "RAW_TOP_API_KEY_SECRET",
            "configured_models": {
                "default": "synthetic-test-model",
                "api_key": "RAW_MODEL_API_KEY_SECRET",
            },
            "evaluation_limits": copy.deepcopy(
                self.contract["evaluation_limits"]
            ),
            "sources": [
                {
                    "benchmark": "gpqa_diamond",
                    "source_url": source["url"],
                    "revision": source["revision"],
                    "file_sha256": source["file_sha256"],
                    "sample_count": 12,
                    "protocol": source["protocol"],
                }
            ],
            "runs": runs,
        }

    def build_report(self, raw: dict) -> dict:
        raw_bytes = json.dumps(raw, sort_keys=True).encode()
        return ANALYZER.build_report(
            raw_bytes, raw, self.contract_bytes, self.contract
        )

    def test_complete_fixture_passes_and_sanitizes_secrets(self) -> None:
        report = self.build_report(self.fixture())
        encoded = json.dumps(report, sort_keys=True)
        self.assertEqual(len(report["runs"]), 36)
        self.assertIn("provider_endpoint_sha256", report)
        for secret in (
            "secret-provider.example",
            "RAW_OUTPUT_SECRET",
            "RAW_ERROR_SECRET",
            "RAW_AUTH_SECRET",
            "RAW_RUN_API_KEY_SECRET",
            "RAW_TOP_AUTH_SECRET",
            "RAW_TOP_API_KEY_SECRET",
            "RAW_MODEL_API_KEY_SECRET",
        ):
            self.assertNotIn(secret, encoded)
        self.assertEqual({run["error_class"] for run in report["runs"]}, {"other"})

    def test_missing_and_duplicate_matrix_entries_are_rejected(self) -> None:
        missing = self.fixture()
        missing["runs"].pop()
        with self.assertRaises(BaselineContractError):
            self.build_report(missing)

        duplicate = self.fixture()
        duplicate["runs"][-1] = copy.deepcopy(duplicate["runs"][0])
        with self.assertRaises(BaselineContractError):
            self.build_report(duplicate)

        missing_metadata = self.fixture()
        del missing_metadata["app_version"]
        with self.assertRaises(BaselineContractError):
            self.build_report(missing_metadata)

        wrong_version = self.fixture()
        wrong_version["app_version"] = "0.1.77"
        with self.assertRaises(BaselineContractError):
            self.build_report(wrong_version)

    def test_provenance_and_budget_drift_are_rejected(self) -> None:
        for mutate in (
            lambda raw: raw["sources"][0].__setitem__("revision", "b" * 40),
            lambda raw: raw["sources"][0].__setitem__("file_sha256", "b" * 64),
            lambda raw: raw["evaluation_limits"].__setitem__(
                "model_call_timeout_seconds", 181
            ),
        ):
            with self.subTest(mutate=mutate):
                raw = self.fixture()
                mutate(raw)
                with self.assertRaises(BaselineContractError):
                    self.build_report(raw)

        drifted_contract = copy.deepcopy(self.contract)
        drifted_contract["sampling"]["cases"].reverse()
        with self.assertRaises(BaselineContractError):
            ANALYZER.build_report(
                json.dumps(self.fixture(), sort_keys=True).encode(),
                self.fixture(),
                json.dumps(drifted_contract, sort_keys=True).encode(),
                drifted_contract,
            )

    def test_full_commit_and_run_hash_formats_are_required(self) -> None:
        raw = self.fixture()
        raw["git_commit"] = "a" * 39
        with self.assertRaises(BaselineContractError):
            self.build_report(raw)

        for field in ("input_sha256", "expected_sha256", "output_sha256"):
            with self.subTest(field=field):
                raw = self.fixture()
                raw["runs"][0][field] = "not-a-sha"
                with self.assertRaises(BaselineContractError):
                    self.build_report(raw)

        consistently_wrong = self.fixture()
        case_id = consistently_wrong["runs"][0]["case_id"]
        for run in consistently_wrong["runs"]:
            if run["case_id"] == case_id:
                run["input_sha256"] = "b" * 64
        with self.assertRaises(BaselineContractError):
            self.build_report(consistently_wrong)

    def test_treatment_positions_are_balanced(self) -> None:
        raw = self.fixture()
        self.build_report(raw)
        raw["runs"][1]["treatment_position"] = raw["runs"][0][
            "treatment_position"
        ]
        with self.assertRaises(BaselineContractError):
            self.build_report(raw)

        reverse_rotation = self.fixture()
        for run in reverse_rotation["runs"]:
            run["treatment_position"] = (-run["treatment_position"]) % 3
        with self.assertRaises(BaselineContractError):
            self.build_report(reverse_rotation)

    def test_scores_hashes_and_profiles_are_recomputed(self) -> None:
        mutations = (
            lambda run: run.__setitem__("exact_score", 0.0),
            lambda run: run.__setitem__("parsed_answer", "D"),
            lambda run: run.__setitem__("output", f"{run['output']} changed"),
            lambda run: run.__setitem__("output_sha256", "b" * 64),
            lambda run: run.__setitem__("prompt_profile_sha256", "c" * 64),
            lambda run: run.__setitem__("gepa_frozen", True),
            lambda run: run.pop("exact_score"),
            lambda run: run.pop("prompt_profile_artifact_sha256"),
        )
        for mutation in mutations:
            with self.subTest(mutation=mutation):
                raw = self.fixture()
                mutation(raw["runs"][0])
                with self.assertRaises(BaselineContractError):
                    self.build_report(raw)

    def test_generic_mode_still_accepts_gpqa_and_mrcr(self) -> None:
        raw = self.fixture()
        source_hash = "c" * 64
        raw["sources"].append(
            {
                "benchmark": "mrcr_v2_8_needle",
                "source_url": "https://huggingface.co/datasets/openai/mrcr",
                "revision": "2025-12-05-bugfix",
                "file_sha256": source_hash,
                "sample_count": 1,
                "protocol": "synthetic validator fixture",
            }
        )
        for treatment in ("direct_default", "cindx_auto_model_route"):
            raw["runs"].append(
                {
                    "benchmark": "mrcr_v2_8_needle",
                    "case_id": "row-synthetic",
                    "category": "synthetic",
                    "treatment": treatment,
                    "requested_policy": treatment,
                    "effective_policy": "synthetic-test-policy",
                    "models": ["synthetic-test-model"],
                    "succeeded": True,
                    "latency_ms": 1,
                    "prompt_tokens": 1,
                    "completion_tokens": 1,
                    "total_tokens": 2,
                    "first_token_latency_ms": 1,
                    "input_sha256": "d" * 64,
                    "expected_sha256": "e" * 64,
                    "output_sha256": "f" * 64,
                    "prefix_valid": True,
                    "expected": "synthetic answer",
                    "output": "synthetic answer",
                    "error": None,
                }
            )
        raw_bytes = json.dumps(raw, sort_keys=True).encode()
        report = ANALYZER.build_report(raw_bytes, raw)
        self.assertIn(
            "mrcr_v2_8_needle",
            {row["benchmark"] for row in report["aggregates"]},
        )
        self.assertNotIn("baseline_contract", report)

    def test_domain_finding_is_data_driven_and_tie_aware(self) -> None:
        rows = [
            {"treatment": "cindx_auto", "category": "Biology", "budgeted_score": 0.75},
            {"treatment": "cindx_auto", "category": "Chemistry", "budgeted_score": 0.5},
            {"treatment": "cindx_auto", "category": "Physics", "budgeted_score": 0.5},
        ]
        finding = ANALYZER.diagnostic_domain_finding(rows, "cindx_auto")
        self.assertIn("Chemistry and Physics were tied", finding)
        self.assertNotIn("Biology was", finding)

    def test_markdown_next_gate_is_neutral_and_protocol_first(self) -> None:
        report = self.build_report(self.fixture())
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "report.md"
            ANALYZER.render_markdown(report, output, "Synthetic contract test")
            markdown = output.read_text(encoding="utf-8")
        self.assertIn("Goal 10 does not tune routing or orchestration", markdown)
        self.assertIn(
            "Repeat the identical frozen protocol with complete usage telemetry",
            markdown,
        )
        self.assertNotIn("Chemistry was the weakest", markdown)
        self.assertNotIn("highest-priority harness issue", markdown)


if __name__ == "__main__":
    unittest.main()
