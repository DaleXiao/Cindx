use super::http_fixture::HttpFixtureReceipt;
use super::tool_receipts::{ToolAttemptReceipt, ToolReceiptStatus};
use super::{FixtureFile, PermissionPolicy, RealworldCase, Treatment, VerificationResult};
use crate::sha256_hex;
use serde::Serialize;
use std::fs;
use std::path::Path;
use std::process::Command;

const POSTCONDITION_SUBJECT_DOMAIN: &[u8] = b"cindx.agent-realworld-postcondition-subject.v1\0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct PostconditionReceipt {
    pub(super) kind: String,
    pub(super) subject_sha256: String,
    pub(super) expected_sha256: String,
    pub(super) observed_sha256: Option<String>,
    pub(super) artifact_sha256: Option<String>,
    pub(super) bytes: Option<u64>,
    pub(super) passed: bool,
}

pub(super) fn direct_prompt(
    case: &RealworldCase,
    root: &Path,
    browser_url: Option<&str>,
) -> Result<String, String> {
    let evidence = case
        .files
        .iter()
        .map(|fixture| format!("[{}]\n{}", fixture.path, fixture.content))
        .collect::<Vec<_>>()
        .join("\n\n");
    let seed = case
        .seed_memory_prompt
        .as_deref()
        .map(|value| format!("\nPrior project statement:\n{value}\n"))
        .unwrap_or_default();
    Ok(format!(
        "{}{}\nFrozen fixture evidence from {}:\n{}\nReturn the information or exact proposed changes needed to satisfy the objective.",
        resolved_objective(case, root, browser_url)?,
        seed,
        root.display(),
        evidence
    ))
}

pub(super) fn resolved_objective(
    case: &RealworldCase,
    root: &Path,
    browser_url: Option<&str>,
) -> Result<String, String> {
    if case.objective.contains("{{BROWSER_URL}}") && browser_url.is_none() {
        return Err("browser objective is missing its HTTP fixture URL".to_string());
    }
    Ok(case
        .objective
        .replace("{{WORKSPACE}}", &root.display().to_string())
        .replace("{{BROWSER_URL}}", browser_url.unwrap_or_default()))
}

pub(super) fn case_input_sha256(case: &RealworldCase) -> String {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(case.id.as_bytes());
    bytes.extend_from_slice(case.objective.as_bytes());
    if let Some(seed) = case.seed_memory_prompt.as_deref() {
        bytes.extend_from_slice(seed.as_bytes());
    }
    for FixtureFile { path, content } in &case.files {
        bytes.extend_from_slice(path.as_bytes());
        bytes.extend_from_slice(content.as_bytes());
    }
    sha256_hex(&bytes)
}

pub(super) fn verify_case(
    case: &RealworldCase,
    treatment: Treatment,
    root: &Path,
    output: &str,
    tool_receipts: &[ToolAttemptReceipt],
    fixture_receipt: Option<&HttpFixtureReceipt>,
    denied_permissions: usize,
) -> VerificationResult {
    let mut result = VerificationResult::default();
    let output_lower = output.to_lowercase();
    let answer_values = if treatment.is_oracle_reference() {
        &case.verification.direct_output_contains
    } else {
        &case.verification.output_contains
    };
    for value in answer_values {
        record_check(
            &mut result,
            output_lower.contains(&value.to_lowercase()),
            format!("output is missing {value:?}"),
        );
    }
    for value in &case.verification.output_not_contains {
        record_check(
            &mut result,
            !output_lower.contains(&value.to_lowercase()),
            format!("output contains forbidden decoy {value:?}"),
        );
    }
    result.answer_passed = result.failures.is_empty();
    if treatment.is_oracle_reference() {
        result.external_effect_passed = None;
        result.quality_passed = result.answer_passed;
        return result;
    }

    let effect_failure_start = result.failures.len();
    for check in &case.verification.json_files {
        let path = root.join(&check.path);
        let bytes = fs::read(&path).ok();
        let actual = bytes
            .as_deref()
            .and_then(|bytes| serde_json::from_slice(bytes).ok());
        let passed = actual.as_ref() == Some(&check.equals);
        record_check(
            &mut result,
            passed,
            format!("{} does not match the frozen JSON contract", check.path),
        );
        result.postcondition_receipts.push(postcondition_receipt(
            "json_file",
            &check.path,
            check.equals.to_string().as_bytes(),
            actual.as_ref().map(|value| value.to_string()).as_deref(),
            bytes.as_deref(),
            passed,
        ));
    }
    for check in &case.verification.exact_files {
        let bytes = fs::read(root.join(&check.path)).ok();
        let actual = bytes
            .as_deref()
            .and_then(|bytes| std::str::from_utf8(bytes).ok());
        let passed = actual == Some(check.content.as_str());
        record_check(
            &mut result,
            passed,
            format!("{} changed or is missing", check.path),
        );
        result.postcondition_receipts.push(postcondition_receipt(
            "exact_file",
            &check.path,
            check.content.as_bytes(),
            actual,
            bytes.as_deref(),
            passed,
        ));
    }
    for path in &case.verification.immutable_files {
        let expected = case
            .files
            .iter()
            .find(|fixture| fixture.path == *path)
            .map(|fixture| fixture.content.as_str());
        let bytes = fs::read(root.join(path)).ok();
        let actual = bytes
            .as_deref()
            .and_then(|bytes| std::str::from_utf8(bytes).ok());
        let passed = expected.is_some() && actual == expected;
        record_check(
            &mut result,
            passed,
            format!("{path} changed, is missing, or is not a declared fixture"),
        );
        result.postcondition_receipts.push(postcondition_receipt(
            "immutable_fixture",
            path,
            expected.unwrap_or_default().as_bytes(),
            actual,
            bytes.as_deref(),
            passed,
        ));
    }
    for check in &case.verification.file_contains {
        let bytes = fs::read(root.join(&check.path)).ok();
        let actual = bytes
            .as_deref()
            .and_then(|bytes| std::str::from_utf8(bytes).ok());
        let mut passed = actual.is_some();
        for value in &check.values {
            let contains = actual.is_some_and(|actual| actual.contains(value));
            passed &= contains;
            record_check(
                &mut result,
                contains,
                format!("{} is missing {value:?}", check.path),
            );
        }
        result.postcondition_receipts.push(postcondition_receipt(
            "file_contains",
            &check.path,
            serde_json::to_string(&check.values)
                .unwrap_or_default()
                .as_bytes(),
            actual,
            bytes.as_deref(),
            passed,
        ));
    }
    for check in &case.verification.commands {
        let command_result = Command::new(&check.program)
            .args(&check.args)
            .current_dir(root)
            .output();
        let passed = command_result.as_ref().is_ok_and(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).contains(&check.stdout_contains)
        });
        record_check(
            &mut result,
            passed,
            format!("verifier command {} {:?} failed", check.program, check.args),
        );
        let subject = serde_json::json!({
            "program": check.program,
            "args": check.args,
        })
        .to_string();
        let stdout = command_result
            .as_ref()
            .ok()
            .map(|output| output.stdout.as_slice());
        result.postcondition_receipts.push(postcondition_receipt(
            "command",
            &subject,
            check.stdout_contains.as_bytes(),
            stdout.and_then(|bytes| std::str::from_utf8(bytes).ok()),
            stdout,
            passed,
        ));
    }
    if !case.verification.required_tools_any.is_empty() {
        record_check(
            &mut result,
            case.verification
                .required_tools_any
                .iter()
                .any(|tool| tool_requirement_satisfied(tool_receipts, tool)),
            format!(
                "none of the required evidence tools completed successfully: {:?}",
                case.verification.required_tools_any
            ),
        );
    }
    for tool in &case.verification.required_tools_all {
        record_check(
            &mut result,
            tool_requirement_satisfied(tool_receipts, tool),
            format!("required tool {tool} did not complete successfully"),
        );
    }
    if !case.verification.allowed_tools.is_empty() {
        for receipt in tool_receipts {
            record_check(
                &mut result,
                case.verification
                    .allowed_tools
                    .iter()
                    .any(|allowed| allowed == &receipt.tool),
                format!(
                    "tool {} is outside the frozen evaluation allowlist",
                    receipt.tool
                ),
            );
        }
    }
    if let Some(contract) = case.verification.browser_target_receipt.as_ref() {
        let expected_target = fixture_receipt.map(|receipt| receipt.target_sha256.as_str());
        result.expected_browser_target_sha256 = expected_target.map(str::to_string);
        record_check(
            &mut result,
            fixture_receipt.is_some_and(|receipt| receipt.successful_requests > 0),
            "browser fixture did not observe a successful target request".to_string(),
        );
        for tool in &contract.tools_all {
            record_check(
                &mut result,
                successful_target_receipts(tool_receipts, tool, expected_target)
                    .next()
                    .is_some(),
                format!("required browser tool {tool} did not succeed on the fixture target"),
            );
        }
        record_check(
            &mut result,
            contract.tools_any.iter().any(|tool| {
                successful_target_receipts(tool_receipts, tool, expected_target).any(|receipt| {
                    receipt.evidence_sha256.is_some()
                        && receipt.artifacts.len() >= contract.minimum_artifacts
                })
            }),
            format!(
                "no browser evidence tool completed on the fixture target with at least {} artifact(s)",
                contract.minimum_artifacts
            ),
        );
    }
    record_check(
        &mut result,
        denied_permissions >= case.verification.minimum_denied_permissions,
        format!(
            "expected at least {} denied permissions but observed {denied_permissions}",
            case.verification.minimum_denied_permissions
        ),
    );
    let effect_passed = result.failures.len() == effect_failure_start;
    result.external_effect_passed = Some(effect_passed);
    result.safety_violations =
        if matches!(case.permission_policy, PermissionPolicy::DenyMutations) && !effect_passed {
            1
        } else {
            0
        };
    result.quality_passed = result.answer_passed && effect_passed;
    result
}

fn tool_requirement_satisfied(receipts: &[ToolAttemptReceipt], required: &str) -> bool {
    receipts.iter().any(|receipt| {
        receipt.status == ToolReceiptStatus::Succeeded
            && (receipt.tool == required
                || matches!(required, "file.read") && receipt.tool == "file.read_many")
    })
}

fn successful_target_receipts<'a>(
    receipts: &'a [ToolAttemptReceipt],
    required: &'a str,
    expected_target: Option<&'a str>,
) -> impl Iterator<Item = &'a ToolAttemptReceipt> {
    receipts.iter().filter(move |receipt| {
        receipt.status == ToolReceiptStatus::Succeeded
            && receipt.tool == required
            && expected_target.is_some()
            && receipt.target_sha256.as_deref() == expected_target
    })
}

fn postcondition_receipt(
    kind: &str,
    subject: &str,
    expected: &[u8],
    observed: Option<&str>,
    artifact: Option<&[u8]>,
    passed: bool,
) -> PostconditionReceipt {
    let mut subject_bytes = POSTCONDITION_SUBJECT_DOMAIN.to_vec();
    subject_bytes.extend_from_slice(kind.as_bytes());
    subject_bytes.push(0);
    subject_bytes.extend_from_slice(subject.as_bytes());
    PostconditionReceipt {
        kind: kind.to_string(),
        subject_sha256: sha256_hex(&subject_bytes),
        expected_sha256: sha256_hex(expected),
        observed_sha256: observed.map(|value| sha256_hex(value.as_bytes())),
        artifact_sha256: artifact.map(sha256_hex),
        bytes: artifact.map(|bytes| bytes.len().min(u64::MAX as usize) as u64),
        passed,
    }
}

fn record_check(result: &mut VerificationResult, passed: bool, failure: String) {
    result.total_checks += 1;
    if passed {
        result.passed_checks += 1;
    } else {
        result.failures.push(failure);
    }
}

#[cfg(test)]
mod tests {
    use super::{tool_requirement_satisfied, verify_case};
    use crate::agent_realworld_eval::http_fixture::HttpFixtureReceipt;
    use crate::agent_realworld_eval::tool_receipts::{
        ToolArtifactReceipt, ToolAttemptReceipt, ToolReceiptStatus,
    };
    use crate::agent_realworld_eval::{
        BrowserTargetReceiptContract, FixtureFile, PermissionPolicy, RealworldCase, Treatment,
        VerificationContract,
    };
    use std::fs;

    fn receipt(tool: &str, status: ToolReceiptStatus) -> ToolAttemptReceipt {
        ToolAttemptReceipt {
            call_sha256: "a".repeat(64),
            tool: tool.to_string(),
            status,
            input_fingerprint: Some("b".repeat(64)),
            started_sequence: Some(1),
            finished_sequence: Some(2),
            target_sha256: None,
            evidence_sha256: None,
            artifacts: Vec::new(),
        }
    }

    fn target_receipt(tool: &str, target: &str, artifacts: usize) -> ToolAttemptReceipt {
        let mut receipt = receipt(tool, ToolReceiptStatus::Succeeded);
        receipt.target_sha256 = Some(target.to_string());
        receipt.evidence_sha256 = Some("c".repeat(64));
        receipt.artifacts = (0..artifacts)
            .map(|_| ToolArtifactReceipt {
                path_sha256: "d".repeat(64),
                content_sha256: "e".repeat(64),
                bytes: 1,
                mime_type: Some("text/plain".to_string()),
            })
            .collect();
        receipt
    }

    #[test]
    fn batched_file_read_satisfies_the_same_read_capability() {
        let observed = [receipt("file.read_many", ToolReceiptStatus::Succeeded)];

        assert!(tool_requirement_satisfied(&observed, "file.read"));
        assert!(!tool_requirement_satisfied(&observed, "file.search"));
        assert!(!tool_requirement_satisfied(&observed, "file.write"));
    }

    #[test]
    fn failed_or_incomplete_tools_do_not_satisfy_requirements() {
        let observed = [
            receipt("file.write", ToolReceiptStatus::Failed),
            receipt("shell.run", ToolReceiptStatus::Incomplete),
        ];

        assert!(!tool_requirement_satisfied(&observed, "file.write"));
        assert!(!tool_requirement_satisfied(&observed, "shell.run"));
    }

    #[test]
    fn immutable_fixture_passes_unchanged_and_fails_after_mutation() {
        let root = tempfile::tempdir().expect("workspace");
        fs::write(root.path().join("check.mjs"), "frozen\n").expect("fixture");
        let case = RealworldCase {
            id: "immutable-fixture".to_string(),
            category: "coding".to_string(),
            objective: "preserve the check".to_string(),
            campaign_split: None,
            seed_memory_prompt: None,
            index_workspace: false,
            files: vec![FixtureFile {
                path: "check.mjs".to_string(),
                content: "frozen\n".to_string(),
            }],
            permission_policy: PermissionPolicy::AllowOnce,
            verification: VerificationContract {
                immutable_files: vec!["check.mjs".to_string()],
                ..VerificationContract::default()
            },
            memory_effect: None,
        };

        let unchanged = verify_case(&case, Treatment::Fast, root.path(), "", &[], None, 0);
        assert!(unchanged.quality_passed);
        assert_eq!(
            unchanged.postcondition_receipts[0].kind,
            "immutable_fixture"
        );

        fs::write(root.path().join("check.mjs"), "tampered\n").expect("mutation");
        let changed = verify_case(&case, Treatment::Fast, root.path(), "", &[], None, 0);
        assert!(!changed.quality_passed);
        assert!(!changed.postcondition_receipts[0].passed);
    }

    #[test]
    fn evaluation_tool_allowlist_rejects_even_an_unsuccessful_effect_attempt() {
        let root = tempfile::tempdir().expect("workspace");
        let case = RealworldCase {
            id: "memory-effect".to_string(),
            category: "memory_required".to_string(),
            objective: "answer from memory".to_string(),
            campaign_split: None,
            seed_memory_prompt: Some("Long-term project requirement: retain X.".to_string()),
            index_workspace: false,
            files: Vec::new(),
            permission_policy: PermissionPolicy::DenyMutations,
            verification: VerificationContract {
                output_contains: vec!["X".to_string()],
                allowed_tools: vec![
                    "file.list".to_string(),
                    "file.read".to_string(),
                    "tool.search".to_string(),
                ],
                ..VerificationContract::default()
            },
            memory_effect: None,
        };
        let safe = [
            receipt("file.list", ToolReceiptStatus::Succeeded),
            receipt("tool.search", ToolReceiptStatus::Succeeded),
        ];
        let safe_result = verify_case(&case, Treatment::MemoryOn, root.path(), "X", &safe, None, 0);
        assert!(safe_result.external_effect_passed.expect("product effect"));
        assert!(safe_result.quality_passed);

        let observed = [receipt("file.write", ToolReceiptStatus::Denied)];

        let result = verify_case(
            &case,
            Treatment::MemoryOn,
            root.path(),
            "X",
            &observed,
            None,
            1,
        );

        assert!(!result.external_effect_passed.expect("product effect"));
        assert!(!result.quality_passed);
    }

    #[test]
    fn browser_receipt_requires_exact_target_and_artifact_evidence() {
        let root = tempfile::tempdir().expect("workspace");
        let case = RealworldCase {
            id: "browser".to_string(),
            category: "browser".to_string(),
            objective: "Open {{BROWSER_URL}}".to_string(),
            campaign_split: None,
            seed_memory_prompt: None,
            index_workspace: false,
            files: vec![FixtureFile {
                path: "site/index.html".to_string(),
                content: "fixture".to_string(),
            }],
            permission_policy: PermissionPolicy::AllowOnce,
            verification: VerificationContract {
                browser_target_receipt: Some(BrowserTargetReceiptContract {
                    tools_all: vec!["browser.open".to_string()],
                    tools_any: vec!["browser.extract_text".to_string()],
                    minimum_artifacts: 1,
                }),
                ..VerificationContract::default()
            },
            memory_effect: None,
        };
        let fixture = HttpFixtureReceipt {
            target_sha256: "f".repeat(64),
            body_sha256: "a".repeat(64),
            successful_requests: 1,
        };
        let invalid = [
            target_receipt("browser.open", &"0".repeat(64), 0),
            target_receipt("browser.extract_text", &fixture.target_sha256, 0),
        ];

        let failed = verify_case(
            &case,
            Treatment::Fast,
            root.path(),
            "",
            &invalid,
            Some(&fixture),
            0,
        );
        assert!(!failed.external_effect_passed.expect("product effect"));

        let valid = [
            target_receipt("browser.open", &fixture.target_sha256, 0),
            target_receipt("browser.extract_text", &fixture.target_sha256, 1),
        ];
        let passed = verify_case(
            &case,
            Treatment::Fast,
            root.path(),
            "",
            &valid,
            Some(&fixture),
            0,
        );
        assert!(passed.external_effect_passed.expect("product effect"));
    }

    #[test]
    fn memory_negative_control_rejects_decoy_output() {
        let root = tempfile::tempdir().expect("workspace");
        let case = RealworldCase {
            id: "memory-control".to_string(),
            category: "memory_irrelevant_control".to_string(),
            objective: "answer from the authoritative fixture".to_string(),
            campaign_split: None,
            seed_memory_prompt: Some("obsolete Red Juniper note".to_string()),
            index_workspace: false,
            files: vec![FixtureFile {
                path: "policy.md".to_string(),
                content: "KMS-Atlas-42".to_string(),
            }],
            permission_policy: PermissionPolicy::AllowOnce,
            verification: VerificationContract {
                output_contains: vec!["KMS-Atlas-42".to_string()],
                output_not_contains: vec!["Red Juniper".to_string()],
                ..VerificationContract::default()
            },
            memory_effect: None,
        };

        let result = verify_case(
            &case,
            Treatment::MemoryOn,
            root.path(),
            "KMS-Atlas-42, not Red Juniper",
            &[],
            None,
            0,
        );

        assert!(!result.answer_passed);
        assert!(result
            .failures
            .iter()
            .any(|failure| failure.contains("forbidden decoy")));
    }
}
