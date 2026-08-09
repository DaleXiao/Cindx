use super::workflow_gepa_campaign_contract::{
    CampaignSplit, CAMPAIGN_SCHEMA, CAMPAIGN_SUITE_ID, CAMPAIGN_VERSION,
};
use super::{RealworldCase, RealworldSuite};
use orchestrator::sha256_hex;
use serde_json::Value;
use std::collections::BTreeSet;

pub(super) fn validate_campaign_suite(suite: &RealworldSuite) -> Result<(), String> {
    if suite.schema != CAMPAIGN_SCHEMA
        || suite.id != CAMPAIGN_SUITE_ID
        || suite.version != CAMPAIGN_VERSION
        || suite.cases.len() != 8
    {
        return Err("Workflow GEPA suite identity or case count is invalid".to_string());
    }
    if leaks_route_answer(&suite.description) {
        return Err("Workflow GEPA suite description leaks a routing answer".to_string());
    }
    let mut ids = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let mut splits = Vec::new();
    for case in &suite.cases {
        let split = CampaignSplit::parse(case)?;
        splits.push((split, case.category.as_str()));
        validate_route_blind_case(case)?;
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
            || case.verification.commands.len() != 4
            || case
                .verification
                .commands
                .iter()
                .any(|command| command.program != "node")
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
        let expected_count = expected
            .iter()
            .filter(|entry| **entry == expected_entry)
            .count();
        let observed_count = splits
            .iter()
            .filter(|entry| **entry == expected_entry)
            .count();
        if observed_count != expected_count {
            return Err("Workflow GEPA train/validation/test strata are unbalanced".to_string());
        }
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
    let check = case
        .files
        .iter()
        .find(|fixture| fixture.path.ends_with("/check.mjs"))
        .ok_or_else(|| format!("Workflow GEPA coding case {} has no public check", case.id))?;
    let markers: &[&str] = match case.id.as_str() {
        "coding-reconcile-records" => &[
            "greatest revision",
            "later input record",
            "winning `void`",
            "without mutating",
        ],
        "coding-resolve-features" => &[
            "Equal revisions prefer an override",
            "later record within the same source",
            "without mutating",
        ],
        "coding-allocate-capacity" => &[
            "descending priority",
            "original input order",
            "omit zero-unit allocations",
            "Do not mutate",
        ],
        "coding-deployment-layers" => &[
            "topological layers",
            "sorted lexicographically",
            "Include isolated services",
            "contains `cycle`",
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
        || case.verification.commands[0].args.as_slice() != [check.path.as_str()]
        || !case.verification.commands[0]
            .stdout_contains
            .ends_with("-check-passed")
        || case.verification.commands[1..].iter().any(|command| {
            !command.args.iter().any(|argument| argument == "--eval")
                || !command.stdout_contains.ends_with("-verified")
        })
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
        "research-release-decision" => &[
            "exactly keys release, limit_ms, rollback_owner, authority, and rejected",
            "signed-record precedence",
            "preserve metrics-file order in rejected",
        ],
        "research-maintenance-window" => &[
            "exactly keys window, region, start_utc, approver, authority, and rejected",
            "signed-record precedence",
            "preserve windows-file order in rejected",
        ],
        "research-current-retention" => &[
            "exactly keys dataset, days, owner, authority, and basis",
            "publication and legal-hold precedence",
        ],
        "research-vendor-award" => &[
            "exactly keys choice, rejected, authority, owner, and reason",
            "compliance, signed-addendum precedence, reliability, and cost rules",
            "preserve scorecard order in rejected",
            "highest reliability among compliant vendors",
        ],
        _ => {
            return Err(format!("unknown Workflow GEPA research case {}", case.id));
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
        || case.verification.commands[1..].iter().any(|command| {
            !command.args.iter().any(|argument| argument == "--eval")
                || !command.stdout_contains.ends_with("-verified")
        })
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

fn validate_route_blind_case(case: &RealworldCase) -> Result<(), String> {
    if case.expected_execution_mode.is_some() || leaks_route_answer(&case.objective) {
        return Err(format!(
            "Workflow GEPA case {} leaks a routing answer",
            case.id
        ));
    }
    Ok(())
}

fn leaks_route_answer(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    [
        "multi-model",
        "multi model",
        "workflow",
        "solve this directly",
        "single model",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
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
            "/../../../benchmarks/agent/workflow-gepa-v7.json"
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
        assert_eq!(
            cases_for_split(&suite, CampaignSplit::Train).unwrap().len(),
            2
        );
        assert_eq!(
            cases_for_split(&suite, CampaignSplit::Validation)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            cases_for_split(&suite, CampaignSplit::Test).unwrap().len(),
            4
        );

        let mut missing_contract: Value = serde_json::from_slice(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../benchmarks/agent/workflow-gepa-v7.json"
        )))
        .unwrap();
        missing_contract["cases"][1]["files"][0]["content"] =
            Value::String("Return host and port.".to_string());
        let missing_contract: RealworldSuite = serde_json::from_value(missing_contract).unwrap();
        assert!(validate_campaign_suite(&missing_contract)
            .unwrap_err()
            .contains("complete behavior contract"));
    }

    #[test]
    fn frozen_suite_rejects_hidden_answers_and_route_leakage() {
        let mut hidden_answer = frozen_suite();
        let award = hidden_answer
            .cases
            .iter_mut()
            .find(|case| case.id == "research-vendor-award")
            .unwrap();
        award.objective = award.objective.replace(
            "highest reliability among compliant vendors",
            "best compliant vendor",
        );
        assert!(validate_campaign_suite(&hidden_answer)
            .unwrap_err()
            .contains("lacks answer verification"));

        let mut leaked_route = frozen_suite();
        leaked_route.cases[0].objective =
            format!("Solve this directly. {}", leaked_route.cases[0].objective);
        assert!(validate_campaign_suite(&leaked_route)
            .unwrap_err()
            .contains("leaks a routing answer"));

        let mut leaked_field = frozen_suite();
        leaked_field.cases[0].expected_execution_mode = Some("direct".to_string());
        assert!(validate_campaign_suite(&leaked_field)
            .unwrap_err()
            .contains("leaks a routing answer"));

        let mut leaked_description = frozen_suite();
        leaked_description
            .description
            .push_str(" Workflow route contract.");
        assert!(validate_campaign_suite(&leaked_description)
            .unwrap_err()
            .contains("description leaks a routing answer"));
    }

    #[test]
    fn hidden_postconditions_accept_behaviorally_equivalent_coding_solutions() {
        let suite = frozen_suite();
        let alternatives = [
            (
                "coding-reconcile-records",
                "wgv7_1/reconcile.mjs",
                r#"export function reconcile(records) {
  if (!Array.isArray(records)) throw new TypeError('records');
  const selected = new Map(); let rejected = 0;
  records.forEach((record, index) => {
    const valid = record && typeof record.id === 'string' && record.id.trim() && typeof record.account === 'string' && record.account.trim() && Number.isInteger(record.amount) && Number.isInteger(record.revision) && record.revision >= 0 && (record.status === 'posted' || record.status === 'void');
    if (!valid) { rejected += 1; return; }
    const prior = selected.get(record.id);
    if (!prior || record.revision > prior.record.revision || record.revision === prior.record.revision && index > prior.index) selected.set(record.id, { record, index });
  });
  const totals = new Map();
  for (const { record } of selected.values()) if (record.status === 'posted') { const account = record.account.trim(); totals.set(account, (totals.get(account) ?? 0) + record.amount); }
  return { accounts: [...totals].sort(([a],[b]) => a.localeCompare(b)).map(([account,total]) => ({account,total})), rejected };
}
"#,
            ),
            (
                "coding-resolve-features",
                "wgv7_2/features.mjs",
                r#"export function resolveFeatures(base, overrides) {
  if (!Array.isArray(base) || !Array.isArray(overrides)) throw new TypeError('inputs');
  const chosen = new Map();
  for (const [source, values] of [[0,base],[1,overrides]]) values.forEach((entry,index) => {
    if (!entry || typeof entry.key !== 'string' || !entry.key.trim() || typeof entry.enabled !== 'boolean' || !Number.isInteger(entry.revision) || entry.revision < 0) return;
    const key=entry.key.trim(), prior=chosen.get(key), rank=[entry.revision,source,index];
    if (!prior || rank[0] > prior.rank[0] || rank[0] === prior.rank[0] && (rank[1] > prior.rank[1] || rank[1] === prior.rank[1] && rank[2] > prior.rank[2])) chosen.set(key,{rank,value:{key,enabled:entry.enabled,revision:entry.revision}});
  });
  return [...chosen.values()].map(item=>item.value).sort((a,b)=>a.key.localeCompare(b.key));
}
"#,
            ),
            (
                "coding-allocate-capacity",
                "wgv7_3/capacity.mjs",
                r#"export function allocateCapacity(requests, capacity) {
  if (!Array.isArray(requests) || !Number.isInteger(capacity) || capacity < 0) throw new TypeError('inputs');
  let rejected=0;
  const valid=requests.map((request,index)=>({request,index})).filter(({request})=>{ const ok=request && typeof request.id==='string' && request.id.trim() && Number.isInteger(request.units) && request.units>0 && Number.isInteger(request.priority); if(!ok) rejected+=1; return ok; }).sort((a,b)=>b.request.priority-a.request.priority || a.index-b.index);
  let remaining=capacity; const allocations=[];
  for (const {request} of valid) { const units=Math.min(request.units,remaining); if(units>0) allocations.push({id:request.id,units}); remaining-=units; }
  return {allocations,remaining,rejected};
}
"#,
            ),
            (
                "coding-deployment-layers",
                "wgv7_4/layers.mjs",
                r#"export function deploymentLayers(services, dependencies) {
  if (!Array.isArray(services) || !Array.isArray(dependencies) || services.some(v=>typeof v!=='string'||!v) || new Set(services).size!==services.length) throw new TypeError('inputs');
  const known=new Set(services), incoming=new Map(services.map(v=>[v,new Set()]));
  for(const edge of dependencies){ if(!edge || !known.has(edge.before) || !known.has(edge.after) || edge.before===edge.after) throw new TypeError('edge'); incoming.get(edge.after).add(edge.before); }
  const remaining=new Set(services), layers=[];
  while(remaining.size){ const layer=[...remaining].filter(v=>[...incoming.get(v)].every(dep=>!remaining.has(dep))).sort(); if(!layer.length) throw new Error('cycle'); layers.push(layer); for(const value of layer) remaining.delete(value); }
  return layers;
}
"#,
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
