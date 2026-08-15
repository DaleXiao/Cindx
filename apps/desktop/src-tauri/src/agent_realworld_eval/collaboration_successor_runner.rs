use super::execution::*;
use super::preflight::{
    now_millis, validate_current_preflight_authority, validate_new_external_path,
    SuccessorPreflightReceipt,
};
use super::{
    parse_and_validate_protocol, ValidatedProtocol, PROTOCOL_ID, PROTOCOL_RELATIVE_PATH,
    SUITE_RELATIVE_PATH,
};
use crate::agent_realworld_eval::collaboration_learning_capture::
    CollaborationLearningFrozenCaptureAuthority;
use crate::agent_realworld_eval::setup::{activate_evaluation_data_root, build_evaluation_app};
use crate::agent_realworld_eval::workflow_gepa_campaign_contract::CampaignSplit;
use crate::agent_realworld_eval::workflow_gepa_campaign_execution::{
    execute_collaboration_learning_successor_pair, preflight_matched_workspaces,
    EvaluationDataEnvironment, MatchedRoutePairRun, ProductRunLedger,
};
use crate::agent_realworld_eval::workflow_gepa_campaign_contract::ProductRunReceipt;
use crate::agent_realworld_eval::{RawRun, RealworldSuite};
use crate::agent_execution_constraint::AgentExecutionConstraint;
use crate::configuration_models::{ProviderConfig, SidecarConfig};
use agent_application::{
    AgentOutcomeTerminalStatusV1, AgentOutcomeUsageCompletenessV1,
    CollaborationLearningAggregateStatusV1, CollaborationLearningCandidateV1,
    CollaborationLearningConfigV1, CollaborationLearningEvidenceSetV1,
    CollaborationLearningHoldoutReservationV1, CollaborationLearningPairV1,
    CollaborationLearningPolicyV1, CollaborationRepairV1,
    CollaborationLearningSplitV1, CollaborationSpecialistInvocationV1,
    CollaborationVerificationV1, COLLABORATION_CONTEXT_BUDGET_COMPACT_BPS,
    COLLABORATION_CONTEXT_BUDGET_EXPANDED_BPS,
};
use orchestrator::sha256_hex;
use agent_core::EventKind;
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};
use tauri::Manager;

const PREFLIGHT_RECEIPT_ENV: &str = "CINDX_COLLABORATION_SUCCESSOR_PREFLIGHT_RECEIPT";
const AUTHORIZATION_ENV: &str = "CINDX_COLLABORATION_SUCCESSOR_AUTHORIZATION";
const OUTPUT_ROOT_ENV: &str = "CINDX_COLLABORATION_SUCCESSOR_OUTPUT_ROOT";
const AUTHORIZE_FLAG: &str = "--authorize-once";
const AUTHORIZATION_TTL_MS: u64 = 15 * 60 * 1_000;
// Defensive load bound for a runner image, not an evidence parameter; real
// runner binaries are far smaller and absurd images still fail closed.
const MAX_RUNNER_BYTES: u64 = 1024 * 1024 * 1024;

pub(in super::super) fn run_authorize() -> Result<(), String> {
    require_authorize_arguments(std::env::args_os())?;
    let runner_bytes = authorize_runner_bytes()?;
    let inputs = StaticInputs::load(true)?;
    inputs.validate_current_authority()?;
    let provider = crate::configuration_persistence::load_provider_config();
    inputs.validate_provider(&provider)?;
    let now = now_millis()?;
    let expires_at_ms = now
        .checked_add(AUTHORIZATION_TTL_MS)
        .ok_or_else(|| "successor authorization expiry overflowed".to_string())?;
    let nonce = authorization_nonce(now, &inputs.authorization_path, &inputs.output_root);
    let authorization = issue_authorization(AuthorizationIssue {
        protocol: &inputs.protocol(),
        preflight: &inputs.preflight,
        authorization_path: &inputs.authorization_path,
        output_root: &inputs.output_root,
        runner_bytes: &runner_bytes,
        issued_at_ms: now,
        expires_at_ms,
        nonce: &nonce,
        credential: &provider.api_key,
    })?;
    inputs.validate_provider(&provider)?;
    write_authorization_new(&inputs.authorization_path, &authorization)?;
    eprintln!(
        "[collaboration-successor-authorize] authorization={} digest={} runner={} expires_at_ms={} provider_calls=0",
        inputs.authorization_path.display(),
        authorization.authorization_sha256(),
        authorization.runner_sha256(),
        expires_at_ms,
    );
    Ok(())
}

pub(in super::super) fn run_execute() -> Result<(), String> {
    if std::env::args_os().len() != 1 {
        return Err("collaboration successor execute accepts no command-line arguments".into());
    }
    let runner_bytes = execute_runner_bytes()?;
    let repo_root = repository_root()?;
    let raw_output_root = required_path(OUTPUT_ROOT_ENV)?;
    if raw_output_root.exists() {
        let output_root = existing_external_path(&raw_output_root, &repo_root, "output root")?;
        let (_, recovery) = SuccessorExecutionJournal::recover(&output_root)?;
        return Err(format!(
            "successor output root is already consumed and cannot execute again: {recovery:?}"
        ));
    }

    let inputs = StaticInputs::load(false)?;
    inputs.validate_current_authority()?;
    let provider = crate::configuration_persistence::load_provider_config();
    inputs.validate_provider(&provider)?;
    let validated = load_and_validate_authorization(AuthorizationValidation {
        protocol: &inputs.protocol(),
        preflight: &inputs.preflight,
        authorization_path: &inputs.authorization_path,
        output_root: &inputs.output_root,
        runner_bytes: &runner_bytes,
        credential: &provider.api_key,
        now_ms: now_millis()?,
    })?;
    inputs.validate_provider(&provider)?;
    run_after_exact_runner_binding(
        validated.authorization().runner_sha256(),
        &runner_bytes,
        || {
            let tombstone = consume_authorization_once(&validated, now_millis()?)?;
            let mut journal = SuccessorExecutionJournal::create_new(
                &inputs.output_root,
                &validated,
                &tombstone,
            )?;
            journal.reserve_campaign()?;
            let result = execute_fixed_campaign(&inputs, provider, &mut journal);
            if let Err(error) = &result {
                if journal.terminal_disposition().is_none() {
                    let reason = format!("execution_error:{}", sha256_hex(error.as_bytes()));
                    let _ = journal.freeze(&reason, TerminalDispositionV1::Censored);
                }
            }
            result
        }
    )
}

fn run_after_exact_runner_binding<T>(
    expected_sha256: &str,
    runner_bytes: &[u8],
    start: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    if sha256_hex(runner_bytes) != expected_sha256 {
        return Err("successor execute binary differs from its authorization".into());
    }
    start()
}

struct StaticInputs {
    repo_root: PathBuf,
    manifest_bytes: Vec<u8>,
    suite_bytes: Vec<u8>,
    preflight_bytes: Vec<u8>,
    preflight: SuccessorPreflightReceipt,
    authorization_path: PathBuf,
    output_root: PathBuf,
}

impl StaticInputs {
    fn load(authorization_must_be_new: bool) -> Result<Self, String> {
        let repo_root = repository_root()?;
        let manifest_bytes = fs::read(repo_root.join(PROTOCOL_RELATIVE_PATH))
            .map_err(|error| format!("failed to read successor protocol: {error}"))?;
        let suite_bytes = fs::read(repo_root.join(SUITE_RELATIVE_PATH))
            .map_err(|error| format!("failed to read successor suite: {error}"))?;
        parse_and_validate_protocol(&manifest_bytes, &suite_bytes)?;
        let preflight_path = existing_external_path(
            &required_path(PREFLIGHT_RECEIPT_ENV)?,
            &repo_root,
            "preflight receipt",
        )?;
        let preflight_bytes = read_private_file(&preflight_path, "successor preflight receipt")?;
        let preflight: SuccessorPreflightReceipt = serde_json::from_slice(&preflight_bytes)
            .map_err(|error| format!("invalid successor preflight receipt JSON: {error}"))?;
        let mut canonical_preflight = serde_json::to_vec_pretty(&preflight)
            .map_err(|error| format!("failed to canonicalize successor preflight: {error}"))?;
        canonical_preflight.push(b'\n');
        if canonical_preflight != preflight_bytes {
            return Err("successor preflight receipt is not in its canonical encoding".into());
        }
        let output_root = validate_new_external_path(
            &required_path(OUTPUT_ROOT_ENV)?,
            &repo_root,
            "output root",
        )?;
        let authorization_path = if authorization_must_be_new {
            validate_new_external_path(
                &required_path(AUTHORIZATION_ENV)?,
                &repo_root,
                "authorization receipt",
            )?
        } else {
            existing_external_path(
                &required_path(AUTHORIZATION_ENV)?,
                &repo_root,
                "authorization receipt",
            )?
        };
        if authorization_path == preflight_path
            || authorization_path == output_root
            || preflight_path == output_root
            || authorization_path.starts_with(&output_root)
            || preflight_path.starts_with(&output_root)
        {
            return Err("successor authorization, preflight, and output paths must be distinct".into());
        }
        Ok(Self {
            repo_root,
            manifest_bytes,
            suite_bytes,
            preflight_bytes,
            preflight,
            authorization_path,
            output_root,
        })
    }

    fn protocol(&self) -> ValidatedProtocol<'_> {
        parse_and_validate_protocol(&self.manifest_bytes, &self.suite_bytes)
            .expect("validated successor protocol remains valid")
    }

    fn validate_current_authority(&self) -> Result<(), String> {
        let config = crate::configuration_persistence::load_provider_config();
        self.validate_provider(&config)
    }

    fn validate_provider(&self, provider: &ProviderConfig) -> Result<(), String> {
        validate_current_preflight_authority(
            &self.protocol(),
            &self.preflight,
            &self.repo_root,
            provider,
            &self.output_root,
        )
    }
}

struct SuccessorCellLedger<'a> {
    journal: &'a mut SuccessorExecutionJournal,
    ordinal: usize,
    active_arm: Option<ArmKindV1>,
}

impl ProductRunLedger for SuccessorCellLedger<'_> {
    fn begin_product(
        &mut self,
        label: &str,
        constraint: Option<AgentExecutionConstraint>,
    ) -> Result<(), String> {
        if self.active_arm.is_some() {
            return Err("successor product ledger already has an active arm".into());
        }
        let kind = match constraint {
            Some(AgentExecutionConstraint::MatchedDirect) => ArmKindV1::Direct,
            Some(AgentExecutionConstraint::MatchedWorkflow) => ArmKindV1::Workflow,
            _ => return Err("successor execution requires a frozen matched-route arm".into()),
        };
        let kind_label = match kind {
            ArmKindV1::Direct => "direct",
            ArmKindV1::Workflow => "workflow",
        };
        let reservation_sha256 = sha256_hex(
            format!(
                "cindx.collaboration-successor-arm-reservation.v1\0{}\0{}\0{}",
                self.ordinal, kind_label, label
            )
            .as_bytes(),
        );
        self.journal
            .reserve_arm(self.ordinal, kind, reservation_sha256)?;
        self.active_arm = Some(kind);
        Ok(())
    }

    fn complete_product(
        &mut self,
        run: &RawRun,
        receipt: &ProductRunReceipt,
    ) -> Result<(), String> {
        let kind = self
            .active_arm
            .take()
            .ok_or_else(|| "successor product ledger has no active arm".to_string())?;
        self.journal.record_arm_terminal(
            self.ordinal,
            kind,
            receipt.terminal_status.clone(),
            successor_arm_evidence_sha256(run, receipt)?,
            successor_observed_usage(run),
        )
    }
}

fn successor_observed_usage(run: &RawRun) -> Option<ArmObservedUsageV1> {
    if !run.completed || run.terminal_status != "completed" {
        return None;
    }
    let trace = run.outcome_trace.as_ref()?;
    if trace.lifecycle.terminal_status != AgentOutcomeTerminalStatusV1::Completed
        || trace.terminal_resources.lineage.completeness()
            != AgentOutcomeUsageCompletenessV1::Complete
    {
        return None;
    }
    let mut agent_turns = None;
    for event in &run.collaboration_learning_events {
        if event.kind == EventKind::ModelRequestFinished
            && event.summary == "Agent model turn finished"
        {
            let observed = event.metadata.get("run_agent_turns")?.parse::<u64>().ok()?;
            agent_turns = Some(agent_turns.map_or(observed, |current: u64| current.max(observed)));
        }
    }
    Some(ArmObservedUsageV1 {
        duration_ms: run.metrics.latency_ms,
        model_calls: u64::try_from(run.metrics.model_calls).ok()?,
        tool_calls: u64::try_from(run.metrics.tool_calls).ok()?,
        agent_turns: agent_turns?,
        physical_model_attempts: trace.terminal_resources.lineage.physical_model_attempts,
        total_tokens: trace.terminal_resources.lineage.total_tokens,
    })
}

fn successor_arm_evidence_sha256(
    run: &RawRun,
    receipt: &ProductRunReceipt,
) -> Result<String, String> {
    let terminal_commit_key = run
        .outcome_trace
        .as_ref()
        .map(|trace| trace.lifecycle.terminal_commit_key.as_str());
    let encoded = serde_json::to_vec(&(
        "cindx.collaboration-successor-arm-evidence.v1",
        receipt,
        terminal_commit_key,
        &run.metrics,
    ))
    .map_err(|error| format!("failed to hash successor arm evidence: {error}"))?;
    Ok(sha256_hex(&encoded))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CellStageResult {
    Continue,
    Ready(String),
    Frozen(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ControllerTerminal {
    Ready(String),
    Frozen(String),
}

fn run_fixed_three_cell_controller(
    mut run_cell: impl FnMut(usize) -> Result<CellStageResult, String>,
) -> Result<ControllerTerminal, String> {
    for ordinal in 1..=3 {
        match (ordinal, run_cell(ordinal)?) {
            (1 | 2, CellStageResult::Continue) => {}
            (3, CellStageResult::Ready(digest)) => {
                return Ok(ControllerTerminal::Ready(digest));
            }
            (3, CellStageResult::Frozen(reason)) => {
                return Ok(ControllerTerminal::Frozen(reason));
            }
            _ => return Err("successor cell returned an out-of-stage decision".into()),
        }
    }
    Err("successor controller omitted its terminal decision".into())
}

fn execute_fixed_campaign(
    inputs: &StaticInputs,
    provider: ProviderConfig,
    journal: &mut SuccessorExecutionJournal,
) -> Result<(), String> {
    let protocol = inputs.protocol();
    let (baseline_policy, candidate_policy, config) = frozen_learning_plan()?;
    let holdout_reservation = frozen_holdout_reservation(&protocol, &inputs.preflight, &candidate_policy)?;
    let candidate = CollaborationLearningCandidateV1::snapshot_with_holdout_reservations(
        candidate_policy.clone(),
        Some(baseline_policy.clone()),
        std::slice::from_ref(&holdout_reservation),
        &config,
        sha256_hex(
            format!(
                "cindx.collaboration-successor-proposer.v1\0{}\0{}",
                inputs.preflight.source_commit_sha256, PROTOCOL_ID
            )
            .as_bytes(),
        ),
        1,
    )
    .map_err(|error| error.to_string())?;

    super::super::install_eval_crypto_provider();
    let suite_root = inputs.output_root.join("evaluation");
    fs::create_dir(&suite_root)
        .map_err(|error| format!("failed to create successor evaluation root: {error}"))?;
    preflight_matched_workspaces(&suite_root, &protocol.suite)?;
    let _evaluation_data_environment = EvaluationDataEnvironment::install(&suite_root);
    let evaluation_database = activate_evaluation_data_root(&suite_root)?;
    let sidecars = SidecarConfig::default();
    crate::sidecar_runtime::apply_sidecar_env(&sidecars);
    let mut runtime_provider = provider;
    runtime_provider.prompt_evolution_enabled = false;
    let app = build_evaluation_app(
        runtime_provider.clone(),
        sidecars,
        &suite_root.join("workspace"),
        &evaluation_database,
    )?;
    let state = app.state::<crate::app_state::AppState>();
    let capture_authority = CollaborationLearningFrozenCaptureAuthority::new(
        inputs.preflight.source_commit_sha256.clone(),
        inputs.preflight.cohort_sha256.clone(),
    )?;
    let mut execution_index = 1usize;
    let mut candidate = Some(candidate);
    let mut holdout_reservation = Some(holdout_reservation);
    let mut evidence = None;
    let terminal = run_fixed_three_cell_controller(|ordinal| match ordinal {
        1 => {
            let baseline = execute_cell(
                inputs,
                &protocol.suite,
                &app,
                &state,
                &runtime_provider,
                &evaluation_database,
                &suite_root,
                &capture_authority,
                ordinal,
                baseline_policy.clone(),
                &mut execution_index,
                journal,
            )?;
            baseline.assess_positive_seed_pair(&config).map_err(|error| {
                freeze_error(journal, "baseline_non_positive", error.to_string())
            })?;
            evidence = Some(
                CollaborationLearningEvidenceSetV1::new_with_holdout_reservations(
                    candidate
                        .take()
                        .ok_or_else(|| "successor candidate was already consumed".to_string())?,
                    baseline,
                    config.clone(),
                    vec![holdout_reservation.take().ok_or_else(|| {
                        "successor holdout reservation was already consumed".to_string()
                    })?],
                )
                .map_err(|error| error.to_string())?,
            );
            Ok(CellStageResult::Continue)
        }
        2 => {
            let candidate_train = execute_cell(
                inputs,
                &protocol.suite,
                &app,
                &state,
                &runtime_provider,
                &evaluation_database,
                &suite_root,
                &capture_authority,
                ordinal,
                candidate_policy.clone(),
                &mut execution_index,
                journal,
            )?;
            candidate_train.assess_positive_pair(&config).map_err(|error| {
                freeze_error(journal, "candidate_non_positive", error.to_string())
            })?;
            evidence
                .as_mut()
                .ok_or_else(|| "successor candidate ran before its baseline".to_string())?
                .append_pair(candidate_train)
                .map_err(|error| error.to_string())?;
            Ok(CellStageResult::Continue)
        }
        3 => {
            let holdout = execute_cell(
                inputs,
                &protocol.suite,
                &app,
                &state,
                &runtime_provider,
                &evaluation_database,
                &suite_root,
                &capture_authority,
                ordinal,
                candidate_policy.clone(),
                &mut execution_index,
                journal,
            )?;
            let evidence = evidence
                .as_mut()
                .ok_or_else(|| "successor holdout ran before candidate eligibility".to_string())?;
            evidence
                .append_pair(holdout)
                .map_err(|error| error.to_string())?;
            let aggregate = evidence.aggregate().map_err(|error| error.to_string())?;
            match aggregate.status() {
                CollaborationLearningAggregateStatusV1::ReadyForReview => Ok(
                    CellStageResult::Ready(aggregate.review_evidence_sha256().to_string()),
                ),
                CollaborationLearningAggregateStatusV1::Frozen => Ok(CellStageResult::Frozen(
                    format!("{:?}", aggregate.freeze_reason()),
                )),
                CollaborationLearningAggregateStatusV1::Collecting => {
                    journal.freeze("aggregate_incomplete", TerminalDispositionV1::Censored)?;
                    Err("successor aggregate remained incomplete after the frozen matrix".into())
                }
            }
        }
        _ => Err("successor controller requested a cell outside the frozen matrix".into()),
    })?;
    match terminal {
        ControllerTerminal::Ready(evidence_sha256) => {
            journal.finish_ready_for_independent_review(evidence_sha256.clone())?;
            eprintln!(
                "[collaboration-successor-execute] status=ready_for_independent_review evidence={} runs=6 production_promotion=false",
                evidence_sha256
            );
            Ok(())
        }
        ControllerTerminal::Frozen(reason) => {
            journal.freeze("aggregate_frozen", TerminalDispositionV1::Frozen)?;
            Err(format!("successor collaboration type froze: {reason}"))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_cell(
    inputs: &StaticInputs,
    suite: &RealworldSuite,
    app: &tauri::App<tauri::Wry>,
    state: &tauri::State<'_, crate::app_state::AppState>,
    provider: &ProviderConfig,
    evaluation_database: &Path,
    suite_root: &Path,
    capture_authority: &CollaborationLearningFrozenCaptureAuthority,
    ordinal: usize,
    policy: CollaborationLearningPolicyV1,
    execution_index: &mut usize,
    journal: &mut SuccessorExecutionJournal,
) -> Result<CollaborationLearningPairV1, String> {
    let protocol = inputs.protocol();
    let cell = protocol
        .manifest
        .matrix
        .cells
        .get(ordinal.saturating_sub(1))
        .filter(|cell| cell.ordinal == ordinal)
        .ok_or_else(|| format!("missing frozen successor cell {ordinal}"))?;
    let case = suite
        .cases
        .iter()
        .find(|case| case.id == cell.case_id)
        .ok_or_else(|| format!("missing frozen successor case {}", cell.case_id))?;
    journal.reserve_cell(ordinal)?;
    let mut ledger = SuccessorCellLedger {
        journal,
        ordinal,
        active_arm: None,
    };
    let matched = execute_collaboration_learning_successor_pair(
        app,
        state,
        provider,
        evaluation_database,
        suite_root,
        case,
        campaign_split(cell)?,
        cell.replicate,
        ordinal - 1,
        execution_index,
        &inputs.preflight.suite_sha256,
        policy,
        &mut ledger,
    )?;
    project_validate_and_commit(inputs, capture_authority, ordinal, matched, journal)
}

fn project_validate_and_commit(
    inputs: &StaticInputs,
    capture_authority: &CollaborationLearningFrozenCaptureAuthority,
    ordinal: usize,
    matched: MatchedRoutePairRun,
    journal: &mut SuccessorExecutionJournal,
) -> Result<CollaborationLearningPairV1, String> {
    let pair = matched.project_collaboration_learning_pair(capture_authority)?;
    super::preflight::validate_observed_pair(
        &inputs.manifest_bytes,
        &inputs.suite_bytes,
        &inputs.preflight_bytes,
        ordinal,
        &matched,
        &pair,
    )?;
    let bytes = serde_json::to_vec(&pair)
        .map_err(|error| format!("failed to encode successor observed pair: {error}"))?;
    let path = inputs
        .output_root
        .join(format!("cell-{ordinal:02}-observed-pair.json"));
    super::preflight::write_new_private_file_atomically(&path, &bytes)?;
    journal.commit_pair(ordinal, pair.digest().to_string())?;
    journal.commit_cell(ordinal, sha256_hex(&bytes))?;
    Ok(pair)
}

fn frozen_learning_plan(
) -> Result<
    (
        CollaborationLearningPolicyV1,
        CollaborationLearningPolicyV1,
        CollaborationLearningConfigV1,
    ),
    String,
> {
    let baseline = CollaborationLearningPolicyV1::seed(
        CollaborationSpecialistInvocationV1::OneReadOnlySpecialist,
        COLLABORATION_CONTEXT_BUDGET_COMPACT_BPS,
        CollaborationVerificationV1::PlanRequiredOnly,
        CollaborationRepairV1::FailFast,
    )
    .map_err(|error| error.to_string())?;
    let candidate = CollaborationLearningPolicyV1::candidate(
        &baseline,
        CollaborationSpecialistInvocationV1::OneReadOnlySpecialist,
        COLLABORATION_CONTEXT_BUDGET_EXPANDED_BPS,
        CollaborationVerificationV1::PlanRequiredOnly,
        CollaborationRepairV1::FailFast,
    )
    .map_err(|error| error.to_string())?;
    let config = CollaborationLearningConfigV1::freeze(1, 1, 1, 2_500, 1, 1)
        .map_err(|error| error.to_string())?;
    Ok((baseline, candidate, config))
}

fn frozen_holdout_reservation(
    protocol: &ValidatedProtocol<'_>,
    receipt: &SuccessorPreflightReceipt,
    candidate: &CollaborationLearningPolicyV1,
) -> Result<CollaborationLearningHoldoutReservationV1, String> {
    let cell = protocol
        .manifest
        .matrix
        .cells
        .iter()
        .find(|cell| cell.split == "holdout")
        .ok_or_else(|| "successor protocol omitted its holdout cell".to_string())?;
    let materialized = receipt
        .cells
        .iter()
        .find(|materialized| materialized.ordinal == cell.ordinal)
        .ok_or_else(|| "successor preflight omitted its holdout materialization".to_string())?;
    CollaborationLearningHoldoutReservationV1::freeze(
        receipt.source_commit_sha256.clone(),
        receipt.suite_sha256.clone(),
        cell.case_input_sha256.clone(),
        materialized.workspace_prestate_sha256.clone(),
        receipt.provider.provider_identity_sha256.clone(),
        receipt.provider.capture_model_pool_sha256.clone(),
        receipt.outcome_budget_sha256.clone(),
        receipt.cohort_sha256.clone(),
        CollaborationLearningSplitV1::Holdout,
        u16::try_from(cell.replicate)
            .map_err(|_| "holdout replicate exceeds its fixed bound".to_string())?,
        match cell.arm_order.as_str() {
            "direct_first" => agent_application::CollaborationLearningArmOrderV1::DirectFirst,
            "workflow_first" => agent_application::CollaborationLearningArmOrderV1::WorkflowFirst,
            _ => return Err("holdout arm order is invalid".into()),
        },
        candidate.policy_sha256.clone(),
    )
    .map_err(|error| error.to_string())
}

fn campaign_split(cell: &super::FrozenCell) -> Result<CampaignSplit, String> {
    match cell.split.as_str() {
        "train" => Ok(CampaignSplit::Train),
        "holdout" => Ok(CampaignSplit::Test),
        _ => Err(format!("successor cell {} has an invalid split", cell.ordinal)),
    }
}

fn freeze_error(
    journal: &mut SuccessorExecutionJournal,
    reason: &str,
    detail: String,
) -> String {
    let _ = journal.freeze(reason, TerminalDispositionV1::Frozen);
    detail
}

fn require_authorize_arguments(args: impl IntoIterator<Item = OsString>) -> Result<(), String> {
    let args = args.into_iter().collect::<Vec<_>>();
    if args.len() != 3
        || args[1] != AUTHORIZE_FLAG
        || args[2] != PROTOCOL_ID
    {
        return Err(format!(
            "collaboration successor authorize requires exactly `{AUTHORIZE_FLAG} {PROTOCOL_ID}`"
        ));
    }
    Ok(())
}

fn authorization_nonce(now_ms: u64, authorization_path: &Path, output_root: &Path) -> String {
    sha256_hex(
        format!(
            "cindx.collaboration-successor-authorization-nonce.v1\0{now_ms}\0{}\0{}\0{}",
            authorization_path.display(),
            output_root.display(),
            uuid::Uuid::now_v7()
        )
        .as_bytes(),
    )
}

fn authorize_runner_bytes() -> Result<Vec<u8>, String> {
    let authorize_executable = std::env::current_exe()
        .map_err(|error| format!("failed to locate successor authorize binary: {error}"))?;
    let directory = authorize_executable
        .parent()
        .ok_or_else(|| "successor authorize binary has no parent directory".to_string())?;
    let execute_name = format!(
        "cindx-collaboration-successor-execute{}",
        std::env::consts::EXE_SUFFIX
    );
    read_exact_runner_binary(&directory.join(execute_name))
}

fn execute_runner_bytes() -> Result<Vec<u8>, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("failed to locate successor execute binary: {error}"))?;
    let expected_name = format!(
        "cindx-collaboration-successor-execute{}",
        std::env::consts::EXE_SUFFIX
    );
    if executable.file_name().and_then(|value| value.to_str()) != Some(expected_name.as_str()) {
        return Err("successor execute binary has an unexpected fixed name".into());
    }
    read_exact_runner_binary(&executable)
}

fn read_exact_runner_binary(path: &Path) -> Result<Vec<u8>, String> {
    let link_metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("failed to inspect successor execute binary: {error}"))?;
    if link_metadata.file_type().is_symlink() || !link_metadata.file_type().is_file() {
        return Err("successor execute binary must be a regular non-symlink file".into());
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("failed to resolve successor execute binary: {error}"))?;
    let metadata = fs::metadata(&canonical)
        .map_err(|error| format!("failed to stat successor execute binary: {error}"))?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_RUNNER_BYTES {
        return Err("successor execute binary size is outside its fixed bound".into());
    }
    let bytes = fs::read(&canonical)
        .map_err(|error| format!("failed to read successor execute binary: {error}"))?;
    if u64::try_from(bytes.len()).ok() != Some(metadata.len()) {
        return Err("successor execute binary changed while it was read".into());
    }
    Ok(bytes)
}

fn repository_root() -> Result<PathBuf, String> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .map_err(|error| format!("failed to locate repository root: {error}"))
}

fn required_path(variable: &str) -> Result<PathBuf, String> {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .ok_or_else(|| format!("{variable} is required"))
}

fn existing_external_path(path: &Path, repo_root: &Path, label: &str) -> Result<PathBuf, String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err(format!("{label} must be a normalized absolute path"));
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("failed to resolve {label}: {error}"))?;
    if canonical.starts_with(repo_root) {
        return Err(format!("{label} must remain outside the repository"));
    }
    Ok(canonical)
}

#[cfg(test)]
#[path = "collaboration_successor_runner_tests.rs"]
mod tests;
