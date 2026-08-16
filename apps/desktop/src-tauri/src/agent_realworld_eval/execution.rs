use super::http_fixture::HttpFixture;
use super::receipts::{model_receipts_from_metadata, ResolvedBudgetReceipt};
use super::runtime::{
    collect_event_metrics, event_sequence_floor, run_product_task_with_execution_constraint,
};
use super::setup::{
    add_recall_session, configure_run_project, seed_memory_fixture_for_case, SetupFailure,
    SetupFailureCode, SetupFailureStage,
};
use super::verification::{case_input_sha256, direct_prompt, resolved_objective, verify_case};
use super::{
    configured_models, directory_size, elapsed_ms, failed_run, metadata_u64, process_resident_kib,
    EventMetrics, ExecutionCell, FailedRunDetails, RawRun, RealworldCase, RuntimeMetrics,
    Treatment,
};
use crate::agent_execution_constraint::{AgentExecutionConstraint, MatchedRoutePlanAnchor};
use crate::app_state::AppState;
use crate::collaboration_execution::complete_collaboration_model_with_control;
use crate::collaboration_learning_eval_runtime::CollaborationLearningEvalPolicyInput;
use crate::configuration_models::ProviderConfig;
use crate::knowledge_commands::index_workspace_rag_blocking;
use crate::runtime_values::unique_id;
use crate::view_models::RagOperationInput;
use agent_core::ModelRole;
use agent_rag::RAG_INDEX_CANCELLED;
use agent_runtime::{AgentRunControl, RunBudget};
use orchestrator::{sha256_hex, FrozenPromptProfileSnapshot};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

pub(super) struct CaseExecutionInput<'a> {
    pub(super) case: &'a RealworldCase,
    pub(super) treatment: Treatment,
    pub(super) replicate: u32,
    pub(super) root: &'a Path,
    pub(super) execution: &'a ExecutionCell,
    pub(super) frozen_profile: Option<&'a FrozenPromptProfileSnapshot>,
    pub(super) project_scope: Option<&'a str>,
    pub(super) run_budget: Option<RunBudget>,
    pub(super) execution_constraint: Option<AgentExecutionConstraint>,
    pub(super) matched_route_plan_anchor: Option<&'a MatchedRoutePlanAnchor>,
    pub(super) collaboration_learning_policy: Option<&'a CollaborationLearningEvalPolicyInput>,
}

pub(super) fn execute_case(
    app: &tauri::App<tauri::Wry>,
    state: &tauri::State<'_, AppState>,
    provider: &ProviderConfig,
    evaluation_database: &Path,
    input: CaseExecutionInput<'_>,
) -> RawRun {
    let CaseExecutionInput {
        case,
        treatment,
        replicate,
        root,
        execution,
        frozen_profile,
        project_scope,
        run_budget,
        execution_constraint,
        matched_route_plan_anchor,
        collaboration_learning_policy,
    } = input;
    let input_sha256 = case_input_sha256(case);
    let started = Instant::now();
    let http_fixture = if case.objective.contains("{{BROWSER_URL}}") {
        match HttpFixture::start(root, &input_sha256) {
            Ok(fixture) => Some(fixture),
            Err(error) => {
                return failed_run(
                    case,
                    treatment,
                    replicate,
                    execution,
                    FailedRunDetails {
                        input_sha256,
                        error,
                        started,
                        setup_failure: SetupFailure::new(
                            SetupFailureStage::ProjectConfiguration,
                            SetupFailureCode::Configuration,
                            false,
                        ),
                        setup_latency_ms: 0,
                    },
                )
            }
        }
    } else {
        None
    };
    let browser_url = http_fixture.as_ref().map(HttpFixture::url);
    if treatment.is_oracle_reference() {
        let prompt = match direct_prompt(case, root, browser_url) {
            Ok(prompt) => prompt,
            Err(error) => {
                return failed_run(
                    case,
                    treatment,
                    replicate,
                    execution,
                    FailedRunDetails {
                        input_sha256,
                        error,
                        started,
                        setup_failure: SetupFailure::new(
                            SetupFailureStage::ProjectConfiguration,
                            SetupFailureCode::Configuration,
                            false,
                        ),
                        setup_latency_ms: 0,
                    },
                )
            }
        };
        let control = Arc::new(AgentRunControl::with_budget(RunBudget::for_effort("fast")));
        let completion = complete_collaboration_model_with_control(
            provider.clone(),
            ModelRole::Executor,
            provider.model.clone(),
            "You are the frozen direct baseline. You have no tools and cannot change external state. Answer only from the supplied fixture evidence; never claim that a file or command was executed."
                .to_string(),
            prompt,
            Some(control),
            |_| {},
        );
        let output = completion.content.unwrap_or_default();
        let completed = completion.error.is_none() && !output.trim().is_empty();
        let fixture_receipt = http_fixture.as_ref().map(HttpFixture::receipt);
        let verification = verify_case(
            case,
            treatment,
            root,
            &output,
            &[],
            fixture_receipt.as_ref(),
            0,
        );
        let model_responses = usize::from(completion.error.is_none());
        let (model_receipts, evidence_error) = if model_responses == 0 {
            (Vec::new(), None)
        } else {
            match model_receipts_from_metadata(&completion.usage) {
                Ok(receipts)
                    if receipts.len() == model_responses
                        && receipts
                            .iter()
                            .all(|receipt| receipt.receipt_status == "observed") =>
                {
                    (receipts, None)
                }
                Ok(receipts) => (
                    receipts,
                    Some("direct provider identity evidence is incomplete".to_string()),
                ),
                Err(error) => (Vec::new(), Some(error)),
            }
        };
        return RawRun {
            execution_index: execution.execution_index,
            treatment_position: execution.treatment_position,
            replicate,
            case_id: case.id.clone(),
            category: case.category.clone(),
            treatment,
            product_mechanism_exercised: false,
            completed,
            terminal_status: if completed {
                "completed".to_string()
            } else {
                "failed".to_string()
            },
            configured_models: vec![provider.model.clone()],
            tools_used: Vec::new(),
            fixture_receipt,
            tool_receipts: Vec::new(),
            memory_records_after_seed: None,
            memory_seed_sha256: None,
            input_sha256,
            output_sha256: sha256_hex(output.as_bytes()),
            output,
            error: completion.error,
            evidence_error,
            setup_failure: None,
            resolved_budget: ResolvedBudgetReceipt::for_treatment(treatment),
            strategy_receipt: None,
            direct_finalizer_execution: None,
            direct_finalizer_evidence_error: None,
            memory_evaluation_receipt: None,
            model_receipts,
            metrics: RuntimeMetrics {
                latency_ms: completion.latency_ms,
                model_calls: 1,
                model_responses,
                prompt_tokens: metadata_u64(&completion.usage, "prompt_tokens"),
                completion_tokens: metadata_u64(&completion.usage, "completion_tokens"),
                total_tokens: metadata_u64(&completion.usage, "total_tokens"),
                resident_kib_after: process_resident_kib(),
                workspace_bytes_after: directory_size(root),
                ..RuntimeMetrics::default()
            },
            verification,
        };
    }

    let key = format!("r{replicate}-{}-{}", case.id, treatment.label());
    let (project_id, mut session_id) = match configure_run_project(state, root, &key, project_scope)
    {
        Ok(value) => value,
        Err(error) => {
            return failed_run(
                case,
                treatment,
                replicate,
                execution,
                FailedRunDetails {
                    input_sha256,
                    error,
                    started,
                    setup_failure: SetupFailure::new(
                        SetupFailureStage::ProjectConfiguration,
                        SetupFailureCode::Configuration,
                        false,
                    ),
                    setup_latency_ms: 0,
                },
            )
        }
    };
    let objective = match resolved_objective(case, root, browser_url) {
        Ok(objective) => objective,
        Err(error) => {
            return failed_run(
                case,
                treatment,
                replicate,
                execution,
                FailedRunDetails {
                    input_sha256,
                    error,
                    started,
                    setup_failure: SetupFailure::new(
                        SetupFailureStage::ProjectConfiguration,
                        SetupFailureCode::Configuration,
                        false,
                    ),
                    setup_latency_ms: 0,
                },
            )
        }
    };
    let mut setup_latency_ms = 0_u64;
    let mut memory_records_after_seed = None;
    let mut memory_seed_sha256 = None;
    if case.index_workspace {
        let setup_started = Instant::now();
        let index_result = index_workspace_rag_blocking(
            app.handle(),
            RagOperationInput {
                operation_id: unique_id("realworld-index"),
            },
        );
        setup_latency_ms = setup_latency_ms.saturating_add(elapsed_ms(setup_started));
        if let Err(error) = index_result {
            let setup_failure = if error == RAG_INDEX_CANCELLED {
                SetupFailure::new(
                    SetupFailureStage::WorkspaceIndex,
                    SetupFailureCode::Transient,
                    true,
                )
            } else {
                SetupFailure::new(
                    SetupFailureStage::WorkspaceIndex,
                    SetupFailureCode::Index,
                    false,
                )
            };
            return failed_run(
                case,
                treatment,
                replicate,
                execution,
                FailedRunDetails {
                    input_sha256,
                    error,
                    started,
                    setup_failure,
                    setup_latency_ms,
                },
            );
        }
    }
    if let Some(seed_prompt) = case.seed_memory_prompt.as_deref() {
        let setup_started = Instant::now();
        let seed_result = seed_memory_fixture_for_case(
            state,
            evaluation_database,
            &project_id,
            &session_id,
            seed_prompt,
        );
        setup_latency_ms = setup_latency_ms.saturating_add(elapsed_ms(setup_started));
        memory_records_after_seed = match seed_result {
            Ok(receipt) => {
                memory_seed_sha256 = Some(receipt.projection_sha256);
                Some(receipt.record_count)
            }
            Err((setup_failure, error)) => {
                return failed_run(
                    case,
                    treatment,
                    replicate,
                    execution,
                    FailedRunDetails {
                        input_sha256,
                        error,
                        started,
                        setup_failure,
                        setup_latency_ms,
                    },
                )
            }
        };
        let session_started = Instant::now();
        session_id = match add_recall_session(state, &project_id, &key) {
            Ok(session_id) => session_id,
            Err(error) => {
                setup_latency_ms = setup_latency_ms.saturating_add(elapsed_ms(session_started));
                return failed_run(
                    case,
                    treatment,
                    replicate,
                    execution,
                    FailedRunDetails {
                        input_sha256,
                        error,
                        started,
                        setup_failure: SetupFailure::new(
                            SetupFailureStage::RecallSession,
                            SetupFailureCode::Configuration,
                            false,
                        ),
                        setup_latency_ms,
                    },
                );
            }
        };
        setup_latency_ms = setup_latency_ms.saturating_add(elapsed_ms(session_started));
    }

    let sequence_floor = match event_sequence_floor(state) {
        Ok(sequence) => sequence,
        Err(error) => {
            return failed_run(
                case,
                treatment,
                replicate,
                execution,
                FailedRunDetails {
                    input_sha256,
                    error,
                    started,
                    setup_failure: SetupFailure::new(
                        SetupFailureStage::ProjectConfiguration,
                        SetupFailureCode::Configuration,
                        false,
                    ),
                    setup_latency_ms,
                },
            )
        }
    };
    let product = run_product_task_with_execution_constraint(
        app.handle(),
        state,
        &session_id,
        &objective,
        treatment,
        case.permission_policy,
        run_budget,
        execution_constraint,
        matched_route_plan_anchor,
        collaboration_learning_policy,
    );
    let output = product.state.latest_answer.clone().unwrap_or_default();
    let mut event_metrics = match collect_event_metrics(
        state,
        &session_id,
        treatment,
        frozen_profile,
        sequence_floor,
        root,
        run_budget,
        execution_constraint,
    ) {
        Ok(metrics) => metrics,
        Err(error) => EventMetrics {
            evidence_errors: vec![error],
            ..EventMetrics::default()
        },
    };
    let tools = std::mem::take(&mut event_metrics.tools)
        .into_iter()
        .collect::<Vec<_>>();
    let tool_receipts = std::mem::take(&mut event_metrics.tool_receipts);
    let fixture_receipt = http_fixture.as_ref().map(HttpFixture::receipt);
    let denied_permissions = product
        .denied_permissions
        .max(event_metrics.denied_permissions);
    let verification = verify_case(
        case,
        treatment,
        root,
        &output,
        &tool_receipts,
        fixture_receipt.as_ref(),
        denied_permissions,
    );
    let evidence_error = (!event_metrics.evidence_errors.is_empty())
        .then(|| event_metrics.evidence_errors.join(" | "));
    RawRun {
        execution_index: execution.execution_index,
        treatment_position: execution.treatment_position,
        replicate,
        case_id: case.id.clone(),
        category: case.category.clone(),
        treatment,
        product_mechanism_exercised: true,
        completed: product.state.status == "completed",
        terminal_status: product.state.status.clone(),
        configured_models: configured_models(provider).into_values().collect(),
        tools_used: tools,
        fixture_receipt,
        tool_receipts,
        memory_records_after_seed,
        memory_seed_sha256,
        input_sha256,
        output_sha256: sha256_hex(output.as_bytes()),
        output,
        error: product.error.or(product.state.last_error.clone()),
        evidence_error,
        setup_failure: None,
        resolved_budget: event_metrics.resolved_budget.unwrap_or_else(|| {
            run_budget
                .map(ResolvedBudgetReceipt::from_budget)
                .unwrap_or_else(|| ResolvedBudgetReceipt::for_treatment(treatment))
        }),
        strategy_receipt: event_metrics.strategy_receipt,
        direct_finalizer_execution: event_metrics.direct_finalizer_execution,
        direct_finalizer_evidence_error: event_metrics.direct_finalizer_evidence_error,
        memory_evaluation_receipt: event_metrics.memory_evaluation_receipt,
        model_receipts: event_metrics.model_receipts,
        metrics: RuntimeMetrics {
            latency_ms: elapsed_ms(started).saturating_sub(setup_latency_ms),
            setup_latency_ms,
            model_calls: event_metrics.model_calls,
            model_responses: event_metrics.model_responses,
            tool_calls: event_metrics.tool_calls,
            tool_succeeded: event_metrics.tool_succeeded,
            tool_failed: event_metrics.tool_failed,
            tool_cancelled: event_metrics.tool_cancelled,
            tool_denied: event_metrics.tool_denied,
            tool_incomplete: event_metrics.tool_incomplete,
            tool_superseded: event_metrics.tool_superseded,
            tool_invalid: event_metrics.tool_invalid,
            permission_requests: product
                .permission_requests
                .max(event_metrics.permission_requests),
            denied_permissions,
            recovery_events: event_metrics.recovery_events,
            prompt_tokens: event_metrics.prompt_tokens,
            completion_tokens: event_metrics.completion_tokens,
            total_tokens: event_metrics.total_tokens,
            context_tokens_used: product.state.context_tokens_used,
            resident_kib_after: process_resident_kib(),
            workspace_bytes_after: directory_size(root),
        },
        verification,
    }
}
