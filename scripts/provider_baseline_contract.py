"""Strict provenance contract for provider-backed GPQA evidence."""

from __future__ import annotations

import hashlib
import re
from collections import Counter, defaultdict
from typing import Any

CONTRACT_SCHEMA = "cindx.provider-baseline-contract.v1"
BENCHMARK = "gpqa_diamond"
DOMAINS = {"Biology", "Chemistry", "Physics"}
TREATMENTS = {"direct_default", "cindx_auto", "cindx_pro"}
SOURCE_URL = "https://github.com/idavidrein/gpqa"
SOURCE_REVISION = "56686c06f5e19865c153de0fdb11be3890014df7"
SOURCE_SHA256 = "41d1213cd7a4998605a26c2798500652572007161b3a92817ba46b35befcd305"
LIMITS = {
    "model_call_timeout_seconds": 180,
    "treatment_deadline_seconds": 300,
    "max_output_tokens_per_call": 4096,
}
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
GIT_SHA_RE = re.compile(r"^[0-9a-f]{40}$")


class BaselineContractError(ValueError):
    """The raw evidence does not match the frozen provider baseline."""


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise BaselineContractError(message)


def _parse_gpqa_answer(output: str) -> str | None:
    def letter_from_fragment(fragment: str) -> str | None:
        trimmed = fragment.strip().strip(".,:;\"'")
        if len(trimmed) == 1 and trimmed in {"A", "B", "C", "D"}:
            return trimmed
        if (
            len(trimmed) == 3
            and trimmed.startswith("(")
            and trimmed.endswith(")")
            and trimmed[1] in {"A", "B", "C", "D"}
        ):
            return trimmed[1]
        return None

    markers = (
        "THE CORRECT ANSWER IS",
        "CORRECT ANSWER IS",
        "FINAL ANSWER:",
        "ANSWER IS",
        "ANSWER:",
    )
    for line in reversed(output.splitlines()):
        upper = line.strip().upper()
        direct = letter_from_fragment(upper)
        if direct is not None:
            return direct
        for marker in markers:
            if marker not in upper:
                continue
            suffix = upper.rsplit(marker, 1)[1]
            for fragment in suffix.split():
                parsed = letter_from_fragment(fragment)
                if parsed is not None:
                    return parsed
    return None


def _validate_contract(
    contract: dict[str, Any],
) -> tuple[dict[str, dict[str, str]], dict[str, str]]:
    _require(
        contract.get("schema") == CONTRACT_SCHEMA,
        "unexpected baseline contract schema",
    )
    _require(
        contract.get("id") == "gpqa-diamond-provider-baseline-v1",
        "unexpected baseline contract id",
    )
    _require(contract.get("app_version") == "0.1.78", "baseline app version has drifted")
    _require(contract.get("benchmark") == BENCHMARK, "unexpected baseline benchmark")

    source = contract.get("source")
    _require(isinstance(source, dict), "baseline contract source is missing")
    for field in ("url", "revision", "file_sha256", "protocol"):
        _require(
            isinstance(source.get(field), str) and bool(source[field]),
            f"baseline source {field} is invalid",
        )
    _require(source["url"] == SOURCE_URL, "baseline source URL has drifted")
    _require(source["revision"] == SOURCE_REVISION, "baseline source revision has drifted")
    _require(source["file_sha256"] == SOURCE_SHA256, "baseline source file hash has drifted")

    sampling = contract.get("sampling")
    _require(isinstance(sampling, dict), "baseline sampling contract is missing")
    _require(sampling.get("selection_seed") == "gpqa-sample-v1", "baseline selection seed has drifted")
    _require(
        sampling.get("option_shuffle_seed") == "cindx-fugu-effect-pilot-v1",
        "baseline option shuffle seed has drifted",
    )
    _require(sampling.get("cases_per_domain") == 4, "baseline must contain four cases per domain")
    _require(sampling.get("total_cases") == 12, "baseline must contain twelve cases")
    cases = sampling.get("cases")
    _require(isinstance(cases, list) and len(cases) == 12, "baseline case manifest must contain twelve cases")
    cases_by_id: dict[str, dict[str, str]] = {}
    for case in cases:
        _require(isinstance(case, dict), "baseline case entry is invalid")
        case_id, domain = case.get("id"), case.get("domain")
        _require(isinstance(case_id, str) and bool(case_id), "baseline case id is invalid")
        _require(isinstance(domain, str) and domain in DOMAINS, "baseline case domain is invalid")
        _require(case_id not in cases_by_id, "baseline case ids must be unique")
        for field in ("input_sha256", "expected_sha256"):
            value = case.get(field)
            _require(
                isinstance(value, str) and bool(SHA256_RE.fullmatch(value)),
                f"baseline case {field} is invalid",
            )
        cases_by_id[case_id] = case
    _require(
        Counter(case["domain"] for case in cases_by_id.values())
        == Counter({domain: 4 for domain in DOMAINS}),
        "baseline domains must contain four cases each",
    )
    ordered_manifest = "\n".join(
        "\x1f".join(
            (
                case["id"],
                case["domain"],
                case["input_sha256"],
                case["expected_sha256"],
            )
        )
        for case in cases
    )
    _require(
        sampling.get("ordered_manifest_sha256")
        == hashlib.sha256(ordered_manifest.encode()).hexdigest(),
        "baseline ordered case manifest hash has drifted",
    )

    treatments = contract.get("treatments")
    _require(isinstance(treatments, list) and len(treatments) == 3, "baseline must declare three treatments")
    treatment_order = ["direct_default", "cindx_auto", "cindx_pro"]
    _require(
        [item.get("id") for item in treatments if isinstance(item, dict)]
        == treatment_order,
        "baseline treatment order has drifted",
    )
    treatment_boundaries: dict[str, str] = {}
    for treatment in treatments:
        _require(isinstance(treatment, dict), "baseline treatment entry is invalid")
        treatment_id = treatment.get("id")
        _require(
            isinstance(treatment_id, str) and treatment_id in TREATMENTS,
            "baseline treatment id is invalid",
        )
        _require(treatment_id not in treatment_boundaries, "baseline treatment ids must be unique")
        _require(
            treatment.get("product_treatment") == (treatment_id != "direct_default"),
            "baseline product treatment flag is invalid",
        )
        boundary = treatment.get("boundary")
        _require(isinstance(boundary, str) and bool(boundary.strip()), "baseline treatment boundary is missing")
        for field in ("prompt_profile", "prompt_profile_origin", "prompt_profile_sha256"):
            _require(
                isinstance(treatment.get(field), str) and bool(treatment[field]),
                f"baseline treatment {field} is invalid",
            )
        _require(
            bool(SHA256_RE.fullmatch(treatment["prompt_profile_sha256"])),
            "baseline treatment prompt profile hash is invalid",
        )
        _require(
            treatment.get("gepa_frozen") is False,
            "provider baseline must use committed built-in prompt profiles",
        )
        treatment_boundaries[treatment_id] = boundary
    _require(set(treatment_boundaries) == TREATMENTS, "baseline treatment set is incomplete")

    matrix = contract.get("matrix")
    _require(
        matrix == {
            "case_count": 12,
            "treatment_count": 3,
            "expected_runs": 36,
            "unique_key": ["case_id", "treatment"],
            "treatment_positions": [0, 1, 2],
            "runs_per_treatment_position": 4,
        },
        "baseline matrix contract is invalid",
    )
    _require(
        contract.get("evaluation_limits") == LIMITS,
        "baseline evaluation limits have drifted",
    )
    return cases_by_id, treatment_boundaries


def validate_provider_baseline(raw: dict[str, Any], contract: dict[str, Any]) -> None:
    """Validate all provenance and paired-matrix invariants before publishing."""

    cases_by_id, _ = _validate_contract(contract)
    treatment_profiles = {
        treatment["id"]: treatment for treatment in contract["treatments"]
    }
    git_commit = raw.get("git_commit")
    _require(
        isinstance(git_commit, str) and bool(GIT_SHA_RE.fullmatch(git_commit)),
        "raw git_commit must be a full lowercase Git SHA",
    )
    _require(
        raw.get("app_version") == contract["app_version"],
        "raw app_version does not match the baseline contract",
    )
    _require(
        isinstance(raw.get("provider_endpoint"), str)
        and bool(raw["provider_endpoint"].strip()),
        "raw provider endpoint must be non-empty",
    )
    _require(
        raw.get("evaluation_limits") == LIMITS,
        "raw evaluation limits do not match the baseline",
    )

    sources = raw.get("sources")
    _require(
        isinstance(sources, list) and len(sources) == 1,
        "raw evidence must contain exactly one GPQA source",
    )
    source = sources[0]
    expected_source = contract["source"]
    _require(isinstance(source, dict), "raw GPQA source is invalid")
    _require(source.get("benchmark") == BENCHMARK, "raw source benchmark has drifted")
    _require(source.get("source_url") == expected_source["url"], "raw source URL has drifted")
    _require(source.get("revision") == expected_source["revision"], "raw source revision has drifted")
    _require(source.get("file_sha256") == expected_source["file_sha256"], "raw source file hash has drifted")
    _require(source.get("sample_count") == 12, "raw source sample count must be twelve")
    _require(source.get("protocol") == expected_source["protocol"], "raw source protocol has drifted")

    runs = raw.get("runs")
    _require(
        isinstance(runs, list) and len(runs) == 36,
        "raw evidence must contain the exact 12 x 3 matrix",
    )
    observed: set[tuple[str, str]] = set()
    case_positions = {
        case_id: case_index for case_index, case_id in enumerate(cases_by_id)
    }
    treatment_positions = {
        treatment: treatment_index
        for treatment_index, treatment in enumerate(
            ("direct_default", "cindx_auto", "cindx_pro")
        )
    }
    positions_by_case: dict[str, set[int]] = defaultdict(set)
    treatment_position_counts: Counter[tuple[str, int]] = Counter()
    for run in runs:
        _require(isinstance(run, dict), "raw run entry is invalid")
        _require(run.get("benchmark") == BENCHMARK, "raw run benchmark has drifted")
        case_id, treatment = run.get("case_id"), run.get("treatment")
        _require(
            isinstance(case_id, str) and case_id in cases_by_id,
            "raw run contains an unpinned case",
        )
        _require(
            isinstance(treatment, str) and treatment in TREATMENTS,
            "raw run contains an unexpected treatment",
        )
        case = cases_by_id[case_id]
        _require(run.get("category") == case["domain"], "raw run domain has drifted")
        _require(run.get("requested_policy") == treatment, "raw run crosses its product treatment boundary")
        profile = treatment_profiles[treatment]
        for field in (
            "prompt_profile",
            "prompt_profile_origin",
            "prompt_profile_sha256",
            "gepa_frozen",
        ):
            _require(
                run.get(field) == profile[field],
                f"raw {field} does not match the frozen treatment profile",
            )
        _require(
            "prompt_profile_artifact_sha256" in run
            and run.get("prompt_profile_artifact_sha256") is None,
            "provider baseline excludes external prompt profile artifacts",
        )
        _require(
            "parsed_answer" in run and "exact_score" in run,
            "raw GPQA verifier fields are incomplete",
        )
        treatment_position = run.get("treatment_position")
        _require(
            isinstance(treatment_position, int)
            and not isinstance(treatment_position, bool)
            and treatment_position in {0, 1, 2},
            "raw treatment_position must be 0, 1, or 2",
        )
        expected_position = (
            treatment_positions[treatment] - case_positions[case_id]
        ) % 3
        _require(
            treatment_position == expected_position,
            "raw treatment_position does not match the frozen Latin rotation",
        )
        key = (case_id, treatment)
        _require(key not in observed, "raw case-treatment pairs must be unique")
        observed.add(key)
        positions_by_case[case_id].add(treatment_position)
        treatment_position_counts[(treatment, treatment_position)] += 1
        for field in ("input_sha256", "expected_sha256", "output_sha256"):
            value = run.get(field)
            _require(
                isinstance(value, str) and bool(SHA256_RE.fullmatch(value)),
                f"raw {field} is invalid",
            )
            if field != "output_sha256":
                _require(value == case[field], f"raw {field} does not match the pinned case")
        expected_answer = run.get("expected")
        output = run.get("output")
        _require(
            isinstance(expected_answer, str) and expected_answer in {"A", "B", "C", "D"},
            "raw expected answer is invalid",
        )
        _require(isinstance(output, str), "raw output must be text")
        _require(
            hashlib.sha256(expected_answer.encode()).hexdigest()
            == run["expected_sha256"],
            "raw expected answer hash mismatch",
        )
        _require(
            hashlib.sha256(output.encode()).hexdigest() == run["output_sha256"],
            "raw output hash mismatch",
        )
        parsed_answer = _parse_gpqa_answer(output)
        _require(
            run.get("parsed_answer") == parsed_answer,
            "raw parsed answer does not match the output",
        )
        expected_score = None if parsed_answer is None else float(parsed_answer == expected_answer)
        _require(
            run.get("exact_score") == expected_score,
            "raw exact score does not match the independently parsed answer",
        )

    expected = {
        (case_id, treatment)
        for case_id in cases_by_id
        for treatment in TREATMENTS
    }
    _require(observed == expected, "raw evidence is missing required case-treatment pairs")
    _require(
        all(positions == {0, 1, 2} for positions in positions_by_case.values()),
        "each case must use every treatment position exactly once",
    )
    _require(
        all(
            treatment_position_counts[(treatment, position)] == 4
            for treatment in TREATMENTS
            for position in (0, 1, 2)
        ),
        "each treatment must occupy each position exactly four times",
    )
