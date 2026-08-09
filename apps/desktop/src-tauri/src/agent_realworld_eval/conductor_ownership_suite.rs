use super::treatments::SUITE_SCHEMA;
use super::workflow_gepa_campaign_suite::leaks_route_answer;
use super::{RealworldCase, RealworldSuite};
use std::collections::BTreeSet;

const TRAIN_SUITE_ID: &str = "cindx-conductor-ownership-train-v1";
const HOLDOUT_SUITE_ID: &str = "cindx-conductor-ownership-holdout-v1";
const SUITE_VERSION: u32 = 1;
const CASE_CATEGORY: &str = "composite";

pub(super) fn is_conductor_ownership_suite(suite: &RealworldSuite) -> bool {
    matches!(suite.id.as_str(), TRAIN_SUITE_ID | HOLDOUT_SUITE_ID)
}

pub(super) fn validate_conductor_ownership_suite(suite: &RealworldSuite) -> Result<(), String> {
    let (expected_split, expected_cases): (&str, &[&str]) = match suite.id.as_str() {
        TRAIN_SUITE_ID => (
            "train",
            &["train-reconcile-release", "train-features-maintenance"],
        ),
        HOLDOUT_SUITE_ID => (
            "test",
            &["holdout-capacity-retention", "holdout-layers-vendor"],
        ),
        _ => return Err("Conductor ownership suite identity is invalid".to_string()),
    };
    if suite.schema != SUITE_SCHEMA
        || suite.version != SUITE_VERSION
        || suite.default_replicates != 1
        || suite.per_run_timeout_seconds != 600
        || suite.cases.len() != expected_cases.len()
        || leaks_route_answer(&suite.description)
    {
        return Err("Conductor ownership suite contract is invalid".to_string());
    }

    let observed_cases = suite
        .cases
        .iter()
        .map(|case| case.id.as_str())
        .collect::<BTreeSet<_>>();
    if observed_cases != expected_cases.iter().copied().collect() {
        return Err("Conductor ownership suite cases drifted".to_string());
    }

    let mut suite_paths = BTreeSet::new();
    for case in &suite.cases {
        validate_case(case, expected_split, &mut suite_paths)?;
    }
    Ok(())
}

fn validate_case<'a>(
    case: &'a RealworldCase,
    expected_split: &str,
    suite_paths: &mut BTreeSet<&'a str>,
) -> Result<(), String> {
    if case.category != CASE_CATEGORY
        || case.campaign_split.as_deref() != Some(expected_split)
        || case.expected_execution_mode.is_some()
        || case.seed_memory_prompt.is_some()
        || case.index_workspace
        || case.objective.contains("{{BROWSER_URL}}")
        || leaks_route_answer(&case.objective)
        || !case
            .objective
            .starts_with("Complete both independently verifiable work items.")
    {
        return Err(format!(
            "Conductor ownership case {} is not route blind",
            case.id
        ));
    }
    if case.files.len() < 7
        || case.verification.commands.len() != 8
        || case.verification.json_files.len() != 1
        || case.verification.immutable_files.len() < 6
        || case.verification.direct_output_contains.len() != 2
        || !case.verification.output_contains.is_empty()
        || !case.verification.output_not_contains.is_empty()
        || !case.verification.exact_files.is_empty()
        || !case.verification.file_contains.is_empty()
        || case.verification.required_tools_all.as_slice() != ["shell.run"]
        || case.verification.required_tools_any.as_slice()
            != ["file.read", "file.read_many", "file.search"]
        || !case.verification.allowed_tools.is_empty()
        || case.verification.browser_target_receipt.is_some()
        || case.verification.minimum_denied_permissions != 0
    {
        return Err(format!(
            "Conductor ownership case {} lost external verification",
            case.id
        ));
    }

    let case_paths = case
        .files
        .iter()
        .map(|fixture| fixture.path.as_str())
        .collect::<BTreeSet<_>>();
    if case_paths.len() != case.files.len()
        || !case_paths.iter().all(|path| suite_paths.insert(*path))
        || case
            .verification
            .immutable_files
            .iter()
            .any(|path| !case_paths.contains(path.as_str()))
    {
        return Err(format!(
            "Conductor ownership case {} has invalid fixture ownership",
            case.id
        ));
    }
    let check_paths = case
        .files
        .iter()
        .filter(|fixture| fixture.path.ends_with("/check.mjs"))
        .map(|fixture| fixture.path.as_str())
        .collect::<BTreeSet<_>>();
    if check_paths.len() != 2
        || !check_paths.iter().all(|path| {
            case.verification
                .immutable_files
                .iter()
                .any(|immutable| immutable == path)
                && case
                    .verification
                    .commands
                    .iter()
                    .any(|command| command.program == "node" && command.args.as_slice() == [*path])
        })
        || case
            .verification
            .commands
            .iter()
            .any(|command| command.program != "node" || command.args.is_empty())
    {
        return Err(format!(
            "Conductor ownership case {} has invalid verifier commands",
            case.id
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suite(bytes: &[u8]) -> RealworldSuite {
        serde_json::from_slice(bytes).unwrap()
    }

    #[test]
    fn frozen_train_and_holdout_suites_pass_the_full_validator() {
        let train = suite(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../benchmarks/agent/conductor-ownership-train-v1.json"
        )));
        let holdout = suite(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../benchmarks/agent/conductor-ownership-holdout-v1.json"
        )));

        super::super::validate_suite(&train).unwrap();
        super::super::validate_suite(&holdout).unwrap();
    }

    #[test]
    fn route_answers_and_split_drift_fail_closed() {
        let mut train = suite(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../benchmarks/agent/conductor-ownership-train-v1.json"
        )));
        train.cases[0].objective.push_str(" Use a workflow.");
        assert!(validate_conductor_ownership_suite(&train).is_err());

        let mut holdout = suite(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../benchmarks/agent/conductor-ownership-holdout-v1.json"
        )));
        holdout.cases[0].campaign_split = Some("train".to_string());
        assert!(validate_conductor_ownership_suite(&holdout).is_err());
    }
}
