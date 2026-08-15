use crate::agent_execution_constraint::AgentExecutionConstraint;
use crate::app_state::AppState;
use crate::event_persistence::append_event;
use crate::project_session_persistence::metadata_with_context;
#[cfg(test)]
use agent_application::COLLABORATION_CONTEXT_BUDGET_COMPACT_BPS;
use agent_application::{
    CollaborationLearningPolicyV1, CollaborationRepairV1, CollaborationSpecialistInvocationV1,
    CollaborationVerificationV1, COLLABORATION_LEARNING_ASSIGNMENT_SCHEMA,
    COLLABORATION_LEARNING_ASSIGNMENT_SCHEMA_METADATA_KEY,
    COLLABORATION_LEARNING_ASSIGNMENT_SUMMARY,
    COLLABORATION_LEARNING_CASE_BINDING_SHA256_METADATA_KEY,
    COLLABORATION_LEARNING_CONTEXT_BUDGET_BPS_METADATA_KEY,
    COLLABORATION_LEARNING_CONTEXT_BYTES_METADATA_KEY,
    COLLABORATION_LEARNING_CONTEXT_COMMITTED_SUMMARY,
    COLLABORATION_LEARNING_CONTEXT_PAYLOAD_SHA256_METADATA_KEY,
    COLLABORATION_LEARNING_CONTEXT_RECEIPT_SCHEMA,
    COLLABORATION_LEARNING_CONTEXT_SCHEMA_METADATA_KEY,
    COLLABORATION_LEARNING_PLAN_REQUIRED_INDEPENDENT_VERIFIER_METADATA_KEY,
    COLLABORATION_LEARNING_POLICY_JSON_METADATA_KEY,
    COLLABORATION_LEARNING_POLICY_SHA256_METADATA_KEY,
    COLLABORATION_LEARNING_SPECIALIST_MODEL_METADATA_KEY,
    COLLABORATION_LEARNING_SPECIALIST_OUTPUT_KIND_METADATA_KEY,
    COLLABORATION_LEARNING_SPECIALIST_REPAIR_MODEL_METADATA_KEY,
    COLLABORATION_LEARNING_SPECIALIST_STEP_ID_METADATA_KEY,
    COLLABORATION_LEARNING_VERIFIER_MODEL_METADATA_KEY,
    COLLABORATION_LEARNING_VERIFIER_REPAIR_MODEL_METADATA_KEY,
    COLLABORATION_LEARNING_VERIFIER_STEP_ID_METADATA_KEY,
    COLLABORATION_LEARNING_WORKER_TURN_ORDINAL_METADATA_KEY,
};
use agent_core::{EventKind, Metadata, TaskId};
use agent_storage::{SqliteStore, StorageError};
use orchestrator::{WorkflowOutputKind, WorkflowPlanIr};
use serde::{Deserialize, Serialize};

const EVAL_POLICY_CONTEXT_KEY: &str = "collaboration_learning_eval_policy";
const AGENT_RUN_ID: &str = "agent_run_id";
const STEER_EPOCH: &str = "steer_epoch";
const SEMANTIC_PLAN_SHA256: &str = "execution_plan_semantic_sha256";
const REQUEST_PAYLOAD_SHA256: &str = "request_payload_sha256";

pub(crate) const COLLABORATION_LEARNING_OUTER_REQUEST_ID_METADATA_KEY: &str = "request_id";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CollaborationLearningEvalPolicyInput {
    policy: CollaborationLearningPolicyV1,
    case_binding_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedEvalPolicyInput {
    policy_json: String,
    policy_sha256: String,
    case_binding_sha256: String,
}

impl CollaborationLearningEvalPolicyInput {
    pub(crate) fn matched_direct(case_binding_sha256: impl Into<String>) -> Result<Self, String> {
        Self::new(
            CollaborationLearningPolicyV1::seed(
                CollaborationSpecialistInvocationV1::DirectOwnerOnly,
                0,
                CollaborationVerificationV1::PlanRequiredOnly,
                CollaborationRepairV1::FailFast,
            )
            .map_err(|error| error.to_string())?,
            case_binding_sha256,
        )
    }

    #[cfg(test)]
    pub(crate) fn matched_workflow(case_binding_sha256: impl Into<String>) -> Result<Self, String> {
        Self::matched_workflow_policy(
            CollaborationLearningPolicyV1::seed(
                CollaborationSpecialistInvocationV1::OneReadOnlySpecialist,
                COLLABORATION_CONTEXT_BUDGET_COMPACT_BPS,
                CollaborationVerificationV1::PlanRequiredOnly,
                CollaborationRepairV1::FailFast,
            )
            .map_err(|error| error.to_string())?,
            case_binding_sha256,
        )
    }

    pub(crate) fn matched_workflow_policy(
        policy: CollaborationLearningPolicyV1,
        case_binding_sha256: impl Into<String>,
    ) -> Result<Self, String> {
        if policy.specialist_invocation
            != CollaborationSpecialistInvocationV1::OneReadOnlySpecialist
        {
            return Err(
                "collaboration learning candidate must retain Workflow topology".to_string(),
            );
        }
        Self::new(policy, case_binding_sha256)
    }

    fn new(
        policy: CollaborationLearningPolicyV1,
        case_binding_sha256: impl Into<String>,
    ) -> Result<Self, String> {
        policy.validate().map_err(|error| error.to_string())?;
        if policy.repair != CollaborationRepairV1::FailFast {
            return Err("real-world collaboration learning must fail fast".to_string());
        }
        let case_binding_sha256 = case_binding_sha256.into();
        validate_sha256(&case_binding_sha256, "case binding")?;
        Ok(Self {
            policy,
            case_binding_sha256,
        })
    }

    pub(crate) fn install_into(
        &self,
        run_context: &mut Metadata,
        execution_constraint: AgentExecutionConstraint,
    ) -> Result<(), String> {
        let expected_invocation = match execution_constraint {
            AgentExecutionConstraint::MatchedDirect => {
                CollaborationSpecialistInvocationV1::DirectOwnerOnly
            }
            AgentExecutionConstraint::MatchedWorkflow => {
                CollaborationSpecialistInvocationV1::OneReadOnlySpecialist
            }
            _ => {
                return Err(
                    "collaboration learning policy requires a matched route constraint".to_string(),
                )
            }
        };
        if self.policy.specialist_invocation != expected_invocation {
            return Err(
                "collaboration learning policy topology disagrees with execution constraint"
                    .to_string(),
            );
        }
        let persisted = PersistedEvalPolicyInput {
            policy_json: self.policy.to_json().map_err(|error| error.to_string())?,
            policy_sha256: self.policy.policy_sha256.clone(),
            case_binding_sha256: self.case_binding_sha256.clone(),
        };
        run_context.insert(
            EVAL_POLICY_CONTEXT_KEY.to_string(),
            serde_json::to_string(&persisted)
                .map_err(|error| format!("collaboration learning policy input failed: {error}"))?,
        );
        Ok(())
    }
}

pub(crate) fn install_policy_input(
    run_context: &mut Metadata,
    input: Option<&CollaborationLearningEvalPolicyInput>,
    execution_constraint: AgentExecutionConstraint,
) -> Result<(), String> {
    run_context.remove(EVAL_POLICY_CONTEXT_KEY);
    if let Some(input) = input {
        input.install_into(run_context, execution_constraint)?;
    }
    Ok(())
}

pub(crate) fn append_direct_assignment_if_enabled(
    store: &mut SqliteStore,
    task_id: &TaskId,
    run_context: &Metadata,
) -> Result<(), StorageError> {
    let Some(input) = restore_policy_input(run_context).map_err(StorageError::new)? else {
        return Ok(());
    };
    if input.policy.specialist_invocation != CollaborationSpecialistInvocationV1::DirectOwnerOnly {
        return Ok(());
    }
    let metadata =
        assignment_metadata(&input, run_context, None, false).map_err(StorageError::new)?;
    append_assignment_once(store, task_id, run_context, metadata)
}

pub(crate) fn append_workflow_assignment_if_enabled(
    store: &mut SqliteStore,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    workflow_plan: &WorkflowPlanIr,
    plan_required_independent_verifier: bool,
) -> Result<(), StorageError> {
    let Some(input) = restore_policy_input(run_context).map_err(StorageError::new)? else {
        return Ok(());
    };
    if input.policy.specialist_invocation
        != CollaborationSpecialistInvocationV1::OneReadOnlySpecialist
    {
        return Ok(());
    }
    let metadata = assignment_metadata(
        &input,
        run_context,
        Some((collaboration_id, workflow_plan)),
        plan_required_independent_verifier,
    )
    .map_err(StorageError::new)?;
    append_assignment_once(store, task_id, run_context, metadata)
}

pub(crate) fn effective_worker_context_window_tokens(
    run_context: &Metadata,
    configured_context_window_tokens: u64,
) -> Result<u64, String> {
    let Some(input) = restore_policy_input(run_context)? else {
        return Ok(configured_context_window_tokens);
    };
    if input.policy.specialist_invocation
        != CollaborationSpecialistInvocationV1::OneReadOnlySpecialist
    {
        return Err("Owner-only collaboration policy cannot dispatch a worker".to_string());
    }
    let scaled = u128::from(configured_context_window_tokens)
        .saturating_mul(u128::from(input.policy.context_budget_bps))
        / 10_000;
    Ok(u64::try_from(scaled).unwrap_or(u64::MAX).max(1))
}

pub(crate) fn policy_input_is_enabled(run_context: &Metadata) -> Result<bool, String> {
    restore_policy_input(run_context).map(|input| input.is_some())
}

pub(crate) fn effective_workflow_step_attempts(
    run_context: &Metadata,
    configured_attempts: usize,
) -> Result<usize, String> {
    let Some(input) = restore_policy_input(run_context)? else {
        return Ok(configured_attempts);
    };
    match input.policy.repair {
        CollaborationRepairV1::FailFast => Ok(configured_attempts.min(1)),
        _ => Err("real-world collaboration learning repair policy is unsupported".to_string()),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn record_context_committed_if_enabled(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    workflow_step_id: &str,
    request_id: &str,
    stage: &str,
    model: &str,
    worker_turn_ordinal: usize,
    request_payload_sha256: &str,
    request_body_bytes: usize,
) -> Result<(), String> {
    let Some(input) = restore_policy_input(run_context)? else {
        return Ok(());
    };
    if input.policy.specialist_invocation
        != CollaborationSpecialistInvocationV1::OneReadOnlySpecialist
    {
        return Err("Owner-only collaboration policy cannot commit worker context".to_string());
    }
    if collaboration_id.trim().is_empty()
        || workflow_step_id.trim().is_empty()
        || request_id.trim().is_empty()
        || stage.trim().is_empty()
        || model.trim().is_empty()
        || worker_turn_ordinal == 0
        || request_body_bytes == 0
    {
        return Err("collaboration learning context identity is incomplete".to_string());
    }
    validate_sha256(request_payload_sha256, "prepared request payload")?;
    let metadata = metadata_with_context(
        [
            ("collaboration_id".to_string(), collaboration_id.to_string()),
            ("workflow_step_id".to_string(), workflow_step_id.to_string()),
            (
                COLLABORATION_LEARNING_OUTER_REQUEST_ID_METADATA_KEY.to_string(),
                request_id.to_string(),
            ),
            ("stage".to_string(), stage.to_string()),
            ("model".to_string(), model.to_string()),
            (
                COLLABORATION_LEARNING_WORKER_TURN_ORDINAL_METADATA_KEY.to_string(),
                worker_turn_ordinal.to_string(),
            ),
            (
                COLLABORATION_LEARNING_CONTEXT_SCHEMA_METADATA_KEY.to_string(),
                COLLABORATION_LEARNING_CONTEXT_RECEIPT_SCHEMA.to_string(),
            ),
            (
                COLLABORATION_LEARNING_CONTEXT_BUDGET_BPS_METADATA_KEY.to_string(),
                input.policy.context_budget_bps.to_string(),
            ),
            (
                COLLABORATION_LEARNING_CONTEXT_PAYLOAD_SHA256_METADATA_KEY.to_string(),
                request_payload_sha256.to_string(),
            ),
            (
                COLLABORATION_LEARNING_CONTEXT_BYTES_METADATA_KEY.to_string(),
                request_body_bytes.to_string(),
            ),
            (
                REQUEST_PAYLOAD_SHA256.to_string(),
                request_payload_sha256.to_string(),
            ),
        ]
        .into_iter()
        .collect(),
        run_context,
    );
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        task_id,
        EventKind::TaskStatusChanged,
        COLLABORATION_LEARNING_CONTEXT_COMMITTED_SUMMARY,
        metadata,
    )
    .map_err(|error| error.to_string())
}

fn restore_policy_input(
    run_context: &Metadata,
) -> Result<Option<CollaborationLearningEvalPolicyInput>, String> {
    let Some(encoded) = run_context.get(EVAL_POLICY_CONTEXT_KEY) else {
        return Ok(None);
    };
    let persisted = serde_json::from_str::<PersistedEvalPolicyInput>(encoded)
        .map_err(|error| format!("collaboration learning policy input is invalid: {error}"))?;
    validate_sha256(&persisted.case_binding_sha256, "case binding")?;
    let policy = CollaborationLearningPolicyV1::from_json(&persisted.policy_json)
        .map_err(|error| error.to_string())?;
    if policy.to_json().map_err(|error| error.to_string())? != persisted.policy_json
        || policy.policy_sha256 != persisted.policy_sha256
        || policy.repair != CollaborationRepairV1::FailFast
    {
        return Err("collaboration learning policy input is not canonical".to_string());
    }
    Ok(Some(CollaborationLearningEvalPolicyInput {
        policy,
        case_binding_sha256: persisted.case_binding_sha256,
    }))
}

fn assignment_metadata(
    input: &CollaborationLearningEvalPolicyInput,
    run_context: &Metadata,
    workflow: Option<(&str, &WorkflowPlanIr)>,
    plan_required_independent_verifier: bool,
) -> Result<Metadata, String> {
    let policy_json = input.policy.to_json().map_err(|error| error.to_string())?;
    let semantic_plan = required(run_context, SEMANTIC_PLAN_SHA256, "semantic plan")?;
    validate_sha256(semantic_plan, "semantic plan")?;
    required(run_context, AGENT_RUN_ID, "agent run id")?;
    required(run_context, STEER_EPOCH, "steer epoch")?
        .parse::<u64>()
        .map_err(|_| "collaboration learning steer epoch is invalid".to_string())?;
    let mut metadata = [
        (
            COLLABORATION_LEARNING_ASSIGNMENT_SCHEMA_METADATA_KEY.to_string(),
            COLLABORATION_LEARNING_ASSIGNMENT_SCHEMA.to_string(),
        ),
        (
            COLLABORATION_LEARNING_POLICY_JSON_METADATA_KEY.to_string(),
            policy_json,
        ),
        (
            COLLABORATION_LEARNING_POLICY_SHA256_METADATA_KEY.to_string(),
            input.policy.policy_sha256.clone(),
        ),
        (
            COLLABORATION_LEARNING_CASE_BINDING_SHA256_METADATA_KEY.to_string(),
            input.case_binding_sha256.clone(),
        ),
        (
            COLLABORATION_LEARNING_PLAN_REQUIRED_INDEPENDENT_VERIFIER_METADATA_KEY.to_string(),
            plan_required_independent_verifier.to_string(),
        ),
    ]
    .into_iter()
    .collect::<Metadata>();
    match (input.policy.specialist_invocation, workflow) {
        (CollaborationSpecialistInvocationV1::DirectOwnerOnly, None) => {
            if plan_required_independent_verifier {
                return Err("Owner-only assignment cannot require a verifier".to_string());
            }
        }
        (
            CollaborationSpecialistInvocationV1::OneReadOnlySpecialist,
            Some((collaboration_id, workflow_plan)),
        ) => {
            if collaboration_id.trim().is_empty() {
                return Err("workflow assignment requires a collaboration id".to_string());
            }
            if workflow_plan.parallel_read_only_specialists {
                return Err(
                    "OneReadOnlySpecialist assignments cannot use the two-specialist read-only graph"
                        .to_string(),
                );
            }
            workflow_plan.validate_owner_execution_graph(plan_required_independent_verifier)?;
            if input.policy.verification == CollaborationVerificationV1::AlwaysIndependent
                && !workflow_plan
                    .steps
                    .iter()
                    .any(|step| step.contract.output_kind == WorkflowOutputKind::Verification)
            {
                return Err(
                    "AlwaysIndependent policy requires a materialized verifier step".to_string(),
                );
            }
            let specialist = workflow_plan
                .steps
                .iter()
                .find(|step| {
                    matches!(
                        step.contract.output_kind,
                        WorkflowOutputKind::Analysis | WorkflowOutputKind::Evidence
                    )
                })
                .ok_or_else(|| "workflow assignment is missing its Specialist".to_string())?;
            metadata.extend([
                ("collaboration_id".to_string(), collaboration_id.to_string()),
                (
                    COLLABORATION_LEARNING_SPECIALIST_STEP_ID_METADATA_KEY.to_string(),
                    specialist.id.clone(),
                ),
                (
                    COLLABORATION_LEARNING_SPECIALIST_OUTPUT_KIND_METADATA_KEY.to_string(),
                    workflow_output_kind_label(&specialist.contract.output_kind).to_string(),
                ),
                (
                    COLLABORATION_LEARNING_SPECIALIST_MODEL_METADATA_KEY.to_string(),
                    specialist.model.clone(),
                ),
            ]);
            if let Some(verifier) = workflow_plan
                .steps
                .iter()
                .find(|step| step.contract.output_kind == WorkflowOutputKind::Verification)
            {
                metadata.extend([
                    (
                        COLLABORATION_LEARNING_VERIFIER_STEP_ID_METADATA_KEY.to_string(),
                        verifier.id.clone(),
                    ),
                    (
                        COLLABORATION_LEARNING_VERIFIER_MODEL_METADATA_KEY.to_string(),
                        verifier.model.clone(),
                    ),
                ]);
            }
        }
        _ => {
            return Err(
                "collaboration learning assignment topology is incomplete or inconsistent"
                    .to_string(),
            )
        }
    }
    Ok(metadata_with_context(metadata, run_context))
}

fn append_assignment_once(
    store: &mut SqliteStore,
    task_id: &TaskId,
    run_context: &Metadata,
    metadata: Metadata,
) -> Result<(), StorageError> {
    let run_id = required(run_context, AGENT_RUN_ID, "agent run id").map_err(StorageError::new)?;
    let epoch = required(run_context, STEER_EPOCH, "steer epoch").map_err(StorageError::new)?;
    let assignments = store
        .list_by_task_and_metadata(task_id, AGENT_RUN_ID, run_id)?
        .into_iter()
        .filter(|event| {
            event.summary == COLLABORATION_LEARNING_ASSIGNMENT_SUMMARY
                || event
                    .metadata
                    .contains_key(COLLABORATION_LEARNING_ASSIGNMENT_SCHEMA_METADATA_KEY)
        })
        .filter(|event| event.metadata.get(STEER_EPOCH).map(String::as_str) == Some(epoch))
        .collect::<Vec<_>>();
    match assignments.as_slice() {
        [] => append_event(
            store,
            task_id,
            EventKind::TaskStatusChanged,
            COLLABORATION_LEARNING_ASSIGNMENT_SUMMARY,
            metadata,
        ),
        [existing] if assignment_matches(&existing.metadata, &metadata) => Ok(()),
        _ => Err(StorageError::new(
            "collaboration learning assignment conflicts with durable trace",
        )),
    }
}

fn assignment_matches(existing: &Metadata, expected: &Metadata) -> bool {
    let reserved = [
        COLLABORATION_LEARNING_ASSIGNMENT_SCHEMA_METADATA_KEY,
        COLLABORATION_LEARNING_POLICY_JSON_METADATA_KEY,
        COLLABORATION_LEARNING_POLICY_SHA256_METADATA_KEY,
        COLLABORATION_LEARNING_CASE_BINDING_SHA256_METADATA_KEY,
        COLLABORATION_LEARNING_PLAN_REQUIRED_INDEPENDENT_VERIFIER_METADATA_KEY,
        COLLABORATION_LEARNING_SPECIALIST_STEP_ID_METADATA_KEY,
        COLLABORATION_LEARNING_SPECIALIST_OUTPUT_KIND_METADATA_KEY,
        COLLABORATION_LEARNING_SPECIALIST_MODEL_METADATA_KEY,
        COLLABORATION_LEARNING_SPECIALIST_REPAIR_MODEL_METADATA_KEY,
        COLLABORATION_LEARNING_VERIFIER_STEP_ID_METADATA_KEY,
        COLLABORATION_LEARNING_VERIFIER_MODEL_METADATA_KEY,
        COLLABORATION_LEARNING_VERIFIER_REPAIR_MODEL_METADATA_KEY,
        "collaboration_id",
        AGENT_RUN_ID,
        STEER_EPOCH,
        SEMANTIC_PLAN_SHA256,
    ];
    reserved
        .into_iter()
        .all(|key| existing.get(key) == expected.get(key))
}

fn workflow_output_kind_label(kind: &WorkflowOutputKind) -> &'static str {
    match kind {
        WorkflowOutputKind::Analysis => "analysis",
        WorkflowOutputKind::Evidence => "evidence",
        WorkflowOutputKind::Verification => "verification",
        WorkflowOutputKind::Synthesis => "synthesis",
    }
}

fn required<'a>(metadata: &'a Metadata, key: &str, label: &str) -> Result<&'a str, String> {
    metadata
        .get(key)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("collaboration learning {label} is missing"))
}

fn validate_sha256(value: &str, label: &str) -> Result<(), String> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(format!(
            "collaboration learning {label} is not a SHA-256 digest"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orchestrator::{
        WorkflowBudget, WorkflowPlanStep, WorkflowStepContract, WorkflowToolPolicy,
        WORKFLOW_IR_SCHEMA,
    };

    fn context() -> Metadata {
        [
            (AGENT_RUN_ID.to_string(), "run-1".to_string()),
            (STEER_EPOCH.to_string(), "0".to_string()),
            (SEMANTIC_PLAN_SHA256.to_string(), "a".repeat(64)),
        ]
        .into_iter()
        .collect()
    }

    fn step(
        id: &str,
        model: &str,
        access: Vec<&str>,
        output_kind: WorkflowOutputKind,
        tool_policy: WorkflowToolPolicy,
    ) -> WorkflowPlanStep {
        let access = access.into_iter().map(str::to_string).collect::<Vec<_>>();
        WorkflowPlanStep {
            id: id.to_string(),
            role: id.to_string(),
            model: model.to_string(),
            subtask: id.to_string(),
            access: access.clone(),
            tool_policy,
            contract: WorkflowStepContract {
                input_steps: access,
                output_kind,
                ..WorkflowStepContract::default()
            },
        }
    }

    fn plan(with_verifier: bool) -> WorkflowPlanIr {
        let mut steps = vec![step(
            "specialist",
            "reasoning-model",
            Vec::new(),
            WorkflowOutputKind::Evidence,
            WorkflowToolPolicy::ReadOnlyEvidence,
        )];
        if with_verifier {
            steps.push(step(
                "verifier",
                "verifier-model",
                vec!["specialist"],
                WorkflowOutputKind::Verification,
                WorkflowToolPolicy::None,
            ));
        }
        steps.push(step(
            "owner-handoff",
            "owner-model",
            vec![if with_verifier {
                "verifier"
            } else {
                "specialist"
            }],
            WorkflowOutputKind::Synthesis,
            WorkflowToolPolicy::None,
        ));
        WorkflowPlanIr {
            schema: WORKFLOW_IR_SCHEMA.to_string(),
            workflow_id: "workflow-1".to_string(),
            objective: "verify the change".to_string(),
            effort: "pro".to_string(),
            policy: "adaptive".to_string(),
            coordinator_model: "owner-model".to_string(),
            prompt_profile: "baseline".to_string(),
            parallel_read_only_specialists: false,
            steps,
            budget: WorkflowBudget {
                max_steps: 3,
                max_models: 3,
                max_model_turns_per_step: 1,
                max_tool_calls_per_step: 1,
                max_output_tokens_per_step: 1_024,
            },
        }
    }

    #[test]
    fn agent_collaboration_learning_offline_adapter_contract_explicit_policy_rejects_a_mismatched_route_topology(
    ) {
        let input = CollaborationLearningEvalPolicyInput::matched_direct("c".repeat(64))
            .expect("direct policy");
        let mut run_context = context();

        assert!(input
            .install_into(&mut run_context, AgentExecutionConstraint::MatchedWorkflow)
            .is_err());
        assert!(!run_context.contains_key(EVAL_POLICY_CONTEXT_KEY));
    }

    #[test]
    fn agent_collaboration_learning_offline_adapter_contract_compact_policy_halves_the_actual_worker_context_window(
    ) {
        let input = CollaborationLearningEvalPolicyInput::matched_workflow("c".repeat(64))
            .expect("workflow policy");
        let mut run_context = context();
        input
            .install_into(&mut run_context, AgentExecutionConstraint::MatchedWorkflow)
            .expect("install policy");

        assert_eq!(
            effective_worker_context_window_tokens(&run_context, 128_000),
            Ok(64_000)
        );
        assert_eq!(effective_workflow_step_attempts(&run_context, 3), Ok(1));
        assert_eq!(
            effective_worker_context_window_tokens(&Metadata::new(), 128_000),
            Ok(128_000)
        );
    }

    #[test]
    fn agent_collaboration_learning_offline_adapter_contract_accepts_bounded_workflow_candidates() {
        let parent = CollaborationLearningPolicyV1::seed(
            CollaborationSpecialistInvocationV1::OneReadOnlySpecialist,
            COLLABORATION_CONTEXT_BUDGET_COMPACT_BPS,
            CollaborationVerificationV1::PlanRequiredOnly,
            CollaborationRepairV1::FailFast,
        )
        .expect("parent policy");
        let expanded = CollaborationLearningPolicyV1::candidate(
            &parent,
            CollaborationSpecialistInvocationV1::OneReadOnlySpecialist,
            agent_application::COLLABORATION_CONTEXT_BUDGET_EXPANDED_BPS,
            CollaborationVerificationV1::PlanRequiredOnly,
            CollaborationRepairV1::FailFast,
        )
        .expect("expanded candidate");
        let independent = CollaborationLearningPolicyV1::candidate(
            &parent,
            CollaborationSpecialistInvocationV1::OneReadOnlySpecialist,
            COLLABORATION_CONTEXT_BUDGET_COMPACT_BPS,
            CollaborationVerificationV1::AlwaysIndependent,
            CollaborationRepairV1::FailFast,
        )
        .expect("verification candidate");

        assert!(CollaborationLearningEvalPolicyInput::matched_workflow_policy(
            expanded,
            "c".repeat(64),
        )
        .is_ok());
        let input = CollaborationLearningEvalPolicyInput::matched_workflow_policy(
            independent,
            "c".repeat(64),
        )
        .expect("independent policy");
        assert!(assignment_metadata(
            &input,
            &context(),
            Some(("collaboration-1", &plan(true))),
            true,
        )
        .is_ok());
    }

    #[test]
    fn agent_collaboration_learning_offline_adapter_contract_assignments_are_derived_from_the_materialized_owner_graph(
    ) {
        let input = CollaborationLearningEvalPolicyInput::matched_workflow("c".repeat(64))
            .expect("workflow policy");
        let mut run_context = context();
        input
            .install_into(&mut run_context, AgentExecutionConstraint::MatchedWorkflow)
            .expect("install policy");
        let metadata = assignment_metadata(
            &restore_policy_input(&run_context)
                .expect("restore")
                .expect("policy"),
            &run_context,
            Some(("collaboration-1", &plan(true))),
            true,
        )
        .expect("assignment");

        assert_eq!(
            metadata
                .get(COLLABORATION_LEARNING_SPECIALIST_STEP_ID_METADATA_KEY)
                .map(String::as_str),
            Some("specialist")
        );
        assert_eq!(
            metadata
                .get(COLLABORATION_LEARNING_SPECIALIST_OUTPUT_KIND_METADATA_KEY)
                .map(String::as_str),
            Some("evidence")
        );
        assert_eq!(
            metadata
                .get(COLLABORATION_LEARNING_VERIFIER_STEP_ID_METADATA_KEY)
                .map(String::as_str),
            Some("verifier")
        );
        assert!(!metadata.contains_key(COLLABORATION_LEARNING_SPECIALIST_REPAIR_MODEL_METADATA_KEY));
        assert!(!metadata.contains_key(COLLABORATION_LEARNING_VERIFIER_REPAIR_MODEL_METADATA_KEY));
    }

    #[test]
    fn agent_collaboration_learning_offline_adapter_contract_direct_assignment_is_owner_only() {
        let input = CollaborationLearningEvalPolicyInput::matched_direct("c".repeat(64))
            .expect("direct policy");
        let mut run_context = context();
        input
            .install_into(&mut run_context, AgentExecutionConstraint::MatchedDirect)
            .expect("install policy");
        let metadata = assignment_metadata(
            &restore_policy_input(&run_context)
                .expect("restore")
                .expect("policy"),
            &run_context,
            None,
            false,
        )
        .expect("assignment");

        assert_eq!(
            metadata
                .get(COLLABORATION_LEARNING_ASSIGNMENT_SCHEMA_METADATA_KEY)
                .map(String::as_str),
            Some(COLLABORATION_LEARNING_ASSIGNMENT_SCHEMA)
        );
        assert!(!metadata.contains_key("collaboration_id"));
        assert!(!metadata.contains_key(COLLABORATION_LEARNING_SPECIALIST_STEP_ID_METADATA_KEY));
        assert!(!metadata.contains_key(COLLABORATION_LEARNING_VERIFIER_STEP_ID_METADATA_KEY));
    }
}
