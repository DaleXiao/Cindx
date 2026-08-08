use super::workflow_gepa_campaign_contract::{
    CampaignSplit, CAMPAIGN_SCHEMA, CAMPAIGN_SUITE_ID,
};
use super::{RealworldCase, RealworldSuite};
use orchestrator::sha256_hex;
use serde_json::Value;
use std::collections::BTreeSet;

pub(super) fn validate_campaign_suite(suite: &RealworldSuite) -> Result<(), String> {
    if suite.schema != CAMPAIGN_SCHEMA
        || suite.id != CAMPAIGN_SUITE_ID
        || suite.version != 5
        || suite.cases.len() != 8
    {
        return Err("Workflow GEPA suite identity or case count is invalid".to_string());
    }
    let mut ids = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let mut splits = Vec::new();
    for case in &suite.cases {
        let split = CampaignSplit::parse(case)?;
        splits.push((split, case.category.as_str()));
        validate_route_contract(case)?;
        if case.id.trim().is_empty()
            || !ids.insert(case.id.as_str())
            || case.objective.trim().is_empty()
            || case.files.is_empty()
            || case.objective.contains("{{BROWSER_URL}}")
            || case.seed_memory_prompt.is_some()
            || case.index_workspace
        {
            return Err(format!("Workflow GEPA case {} is invalid", case.id));
        }
        if case.verification.immutable_files.is_empty()
            || !case.verification.exact_files.is_empty()
            || !case.verification.file_contains.is_empty()
            || case.verification.commands.len() != 1
            || case.verification.commands[0].program != "node"
            || case.verification.required_tools_all.as_slice() != ["shell.run"]
            || case.verification.required_tools_any.is_empty()
        {
            return Err(format!(
                "Workflow GEPA case {} lacks neutral postconditions or tool evidence",
                case.id
            ));
        }
        let mut case_paths = BTreeSet::new();
        for fixture in &case.files {
            super::validate_relative_path(&fixture.path)?;
            if !case_paths.insert(fixture.path.as_str()) || !paths.insert(fixture.path.as_str()) {
                return Err(format!("duplicate campaign fixture path {}", fixture.path));
            }
        }
        let check = case
            .files
            .iter()
            .find(|fixture| fixture.path.ends_with("/check.mjs"))
            .ok_or_else(|| format!("Workflow GEPA case {} has no public check", case.id))?;
        if !case
            .verification
            .immutable_files
            .iter()
            .all(|path| case_paths.contains(path.as_str()))
            || !case
                .verification
                .immutable_files
                .iter()
                .any(|path| path == &check.path)
        {
            return Err(format!(
                "Workflow GEPA case {} has an invalid immutable fixture contract",
                case.id
            ));
        }
        match case.category.as_str() {
            "coding" => validate_coding_contract(case)?,
            "research" => validate_research_contract(case, check.content.as_str())?,
            other => {
                return Err(format!(
                    "Workflow GEPA case {} has unsupported category {other}",
                    case.id
                ));
            }
        }
    }
    let expected = [
        (CampaignSplit::Train, "coding"),
        (CampaignSplit::Train, "research"),
        (CampaignSplit::Validation, "coding"),
        (CampaignSplit::Validation, "research"),
        (CampaignSplit::Test, "coding"),
        (CampaignSplit::Test, "coding"),
        (CampaignSplit::Test, "research"),
        (CampaignSplit::Test, "research"),
    ];
    for expected_entry in expected {
        let expected_count = expected.iter().filter(|entry| **entry == expected_entry).count();
        let observed_count = splits.iter().filter(|entry| **entry == expected_entry).count();
        if observed_count != expected_count {
            return Err("Workflow GEPA train/validation/test strata are unbalanced".to_string());
        }
    }
    if !ids.contains("route-collaboration-proof") {
        return Err("Workflow GEPA suite lacks its route exercise case".to_string());
    }
    Ok(())
}

pub(super) fn cases_for_split(
    suite: &RealworldSuite,
    split: CampaignSplit,
) -> Result<Vec<&RealworldCase>, String> {
    suite
        .cases
        .iter()
        .filter_map(|case| match CampaignSplit::parse(case) {
            Ok(observed) if observed == split => Some(Ok(case)),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

pub(super) fn training_dataset_sha256(suite: &RealworldSuite) -> Result<String, String> {
    let cases = cases_for_split(suite, CampaignSplit::Train)?
        .into_iter()
        .map(|case| {
            serde_json::json!({
                "id": case.id,
                "category": case.category,
                "objective": case.objective,
                "expected_execution_mode": case.expected_execution_mode,
                "files": case.files.iter().map(|file| serde_json::json!({
                    "path": file.path,
                    "content": file.content,
                })).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_vec(&cases)
        .map(|bytes| sha256_hex(&bytes))
        .map_err(|error| format!("failed to encode training dataset: {error}"))
}

fn validate_coding_contract(case: &RealworldCase) -> Result<(), String> {
    let spec = case
        .files
        .iter()
        .find(|fixture| fixture.path.ends_with("/spec.md"))
        .ok_or_else(|| format!("Workflow GEPA coding case {} has no public spec", case.id))?;
    let markers: &[&str] = match case.id.as_str() {
        "coding-calculate-total" => &["price * quantity", "empty array", "Do not mutate"],
        "coding-parse-port" => &["base-10 number", "not a string", "exactly one colon"],
        "coding-normalize-tags" => &[
            "trim surrounding whitespace",
            "lowercase",
            "duplicate",
            "sorted",
            "Do not mutate",
        ],
        "coding-select-latest" => &[
            "greatest revision",
            "original record",
            "Do not mutate or reorder",
        ],
        _ => return Err(format!("unknown Workflow GEPA coding case {}", case.id)),
    };
    if !case.objective.contains(&spec.path)
        || !case
            .verification
            .immutable_files
            .iter()
            .any(|path| path == &spec.path)
        || markers.iter().any(|marker| !spec.content.contains(marker))
        || !case.verification.json_files.is_empty()
        || !case.verification.commands[0]
            .args
            .iter()
            .any(|argument| argument == "--eval")
        || !case.verification.commands[0]
            .stdout_contains
            .ends_with("-hidden-verified")
    {
        return Err(format!(
            "Workflow GEPA coding case {} does not disclose its complete behavior contract",
            case.id
        ));
    }
    Ok(())
}

fn validate_research_contract(case: &RealworldCase, check_content: &str) -> Result<(), String> {
    let public_contract_markers: &[&str] = match case.id.as_str() {
        "research-authoritative-threshold" => &[
            "exactly keys owner, threshold, and authority",
            "Copy the approved owner name",
            "approved percentage",
            "superseding decision identifier",
            "exactly as written",
        ],
        "research-measurement-choice" => &[
            "exactly keys winner, margin, and basis",
            "lower-case candidate label",
            "numeric difference",
            "exact string validated score",
        ],
        "research-approved-region" => &[
            "exactly keys region, expiry, and approver",
            "Copy all three values exactly as written",
        ],
        "route-collaboration-proof" => &[
            "exactly keys choice, rejected, and reason",
            "candidate label in lower case",
            "input-file order",
            "exact string lowest verified error rate among approved candidates",
        ],
        _ => {
            return Err(format!(
                "unknown Workflow GEPA research case {}",
                case.id
            ));
        }
    };
    if case.verification.json_files.len() != 1
        || case.verification.immutable_files.len() != case.files.len()
        || case.verification.commands[0].args.as_slice()
            != [case
                .files
                .iter()
                .find(|fixture| fixture.path.ends_with("/check.mjs"))
                .map(|fixture| fixture.path.as_str())
                .unwrap_or_default()]
        || !case.verification.commands[0]
            .stdout_contains
            .ends_with("-check-passed")
        || public_contract_markers
            .iter()
            .any(|marker| !case.objective.contains(marker))
    {
        return Err(format!(
            "Workflow GEPA research case {} lacks answer verification",
            case.id
        ));
    }
    let mut expected_values = Vec::new();
    collect_scalar_values(
        &case.verification.json_files[0].equals,
        &mut expected_values,
    );
    if expected_values
        .iter()
        .any(|value| check_content.contains(value))
    {
        return Err(format!(
            "Workflow GEPA case {} leaks an expected answer into its public check",
            case.id
        ));
    }
    Ok(())
}

fn validate_route_contract(case: &RealworldCase) -> Result<(), String> {
    let expected = match case.id.as_str() {
        "coding-calculate-total" => Some("direct"),
        "research-authoritative-threshold" | "route-collaboration-proof" => Some("workflow"),
        _ => None,
    };
    if case.expected_execution_mode.as_deref() != expected {
        return Err(format!(
            "Workflow GEPA case {} has an invalid route contract",
            case.id
        ));
    }
    let disclosed = match expected {
        Some("direct") => case
            .objective
            .contains("directly without creating a multi-model workflow"),
        Some("workflow") => case
            .objective
            .contains("Use independent multi-model collaboration"),
        None => true,
        Some(_) => false,
    };
    if !disclosed {
        return Err(format!(
            "Workflow GEPA case {} hides its route requirement",
            case.id
        ));
    }
    Ok(())
}

fn collect_scalar_values(value: &Value, values: &mut Vec<String>) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_scalar_values(item, values);
            }
        }
        Value::Object(object) => {
            for item in object.values() {
                collect_scalar_values(item, values);
            }
        }
        Value::String(value) => values.push(value.clone()),
        Value::Number(value) => values.push(value.to_string()),
        Value::Bool(value) => values.push(value.to_string()),
        Value::Null => values.push("null".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_realworld_eval::tool_receipts::{ToolAttemptReceipt, ToolReceiptStatus};
    use crate::agent_realworld_eval::verification::verify_case;
    use crate::agent_realworld_eval::{materialize_case, Treatment};
    use std::fs;

    fn frozen_suite() -> RealworldSuite {
        serde_json::from_slice(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../benchmarks/agent/workflow-gepa-v5.json"
        )))
        .unwrap()
    }

    fn successful_tool(tool: &str) -> ToolAttemptReceipt {
        ToolAttemptReceipt {
            call_sha256: "a".repeat(64),
            tool: tool.to_string(),
            status: ToolReceiptStatus::Succeeded,
            input_fingerprint: Some("b".repeat(64)),
            started_sequence: Some(1),
            finished_sequence: Some(2),
            target_sha256: None,
            evidence_sha256: None,
            artifacts: Vec::new(),
        }
    }

    #[test]
    fn frozen_suite_has_explicit_balanced_strata_and_public_contracts() {
        let suite = frozen_suite();
        validate_campaign_suite(&suite).unwrap();
        assert_eq!(cases_for_split(&suite, CampaignSplit::Train).unwrap().len(), 2);
        assert_eq!(
            cases_for_split(&suite, CampaignSplit::Validation)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(cases_for_split(&suite, CampaignSplit::Test).unwrap().len(), 4);

        let mut missing_contract: Value = serde_json::from_slice(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../benchmarks/agent/workflow-gepa-v5.json"
        )))
        .unwrap();
        missing_contract["cases"][1]["files"][0]["content"] =
            Value::String("Return host and port.".to_string());
        let missing_contract: RealworldSuite =
            serde_json::from_value(missing_contract).unwrap();
        assert!(validate_campaign_suite(&missing_contract)
            .unwrap_err()
            .contains("complete behavior contract"));
    }

    #[test]
    fn frozen_suite_rejects_hidden_output_normalization_or_route_requirements() {
        let mut hidden_normalization = frozen_suite();
        let measurement = hidden_normalization
            .cases
            .iter_mut()
            .find(|case| case.id == "research-measurement-choice")
            .unwrap();
        measurement.objective = measurement
            .objective
            .replace("lower-case candidate label", "candidate label");
        assert!(validate_campaign_suite(&hidden_normalization)
            .unwrap_err()
            .contains("lacks answer verification"));

        let mut hidden_route = frozen_suite();
        let collaboration = hidden_route
            .cases
            .iter_mut()
            .find(|case| case.id == "route-collaboration-proof")
            .unwrap();
        collaboration.objective = collaboration
            .objective
            .replace("Use independent multi-model collaboration", "Investigate carefully");
        assert!(validate_campaign_suite(&hidden_route)
            .unwrap_err()
            .contains("hides its route requirement"));
    }

    #[test]
    fn hidden_postconditions_accept_behaviorally_equivalent_coding_solutions() {
        let suite = frozen_suite();
        let alternatives = [
            (
                "coding-calculate-total",
                "wg1/calc.mjs",
                "export function total(items) { let value = 0; for (const item of items) value += item.price * item.quantity; return value; }\n",
            ),
            (
                "coding-parse-port",
                "wg2/parser.mjs",
                "export function parseEndpoint(value) { const [host, port] = value.split(':'); return { host, port: parseInt(port, 10) }; }\n",
            ),
            (
                "coding-normalize-tags",
                "wg3/normalize.mjs",
                "export function normalizeTags(values) { const clean = values.map((value) => value.trim().toLowerCase()).filter(Boolean); return [...new Set(clean)].sort(); }\n",
            ),
            (
                "coding-select-latest",
                "wg4/select.mjs",
                "export function latest(records) { return records.reduce((best, value) => value.revision > best.revision ? value : best); }\n",
            ),
        ];
        let receipts = [successful_tool("file.read"), successful_tool("shell.run")];
        for (id, path, implementation) in alternatives {
            let case = suite.cases.iter().find(|case| case.id == id).unwrap();
            let temp = tempfile::tempdir().unwrap();
            materialize_case(temp.path(), case).unwrap();
            fs::write(temp.path().join(path), implementation).unwrap();
            let output = case.verification.direct_output_contains.join(" ");
            let result = verify_case(
                case,
                Treatment::Pro,
                temp.path(),
                &output,
                &receipts,
                None,
                0,
            );
            assert!(result.quality_passed, "{id}: {:?}", result.failures);
        }
    }
}
