use crate::app_state::AppState;
use crate::configuration_models::ProviderConfig;
use crate::configuration_persistence::clone_provider_config;
use crate::direct_judge_runtime::guardian_or_verifier_attribution;
use crate::event_persistence::append_event;
use crate::project_session_persistence::metadata_with_context;
use crate::runtime_values::current_time_millis;
use agent_core::{
    EventKind, Message, MessageRole, Metadata, ModelRole, PermissionDecision, PermissionRequest,
    PermissionResolution, PermissionRisk,
};
use agent_storage::{SqliteStore, StorageError};

/// Hard wall-clock bound for one guardian review. The waiting side gives up
/// after this deadline regardless of the review call's state; the review call
/// itself also carries the same no-progress bound.
pub(crate) const GUARDIAN_REVIEW_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

const GUARDIAN_MAX_FIELD_CHARS: usize = 2_000;
const GUARDIAN_MAX_CONTEXT_CHARS: usize = 3_000;
const GUARDIAN_CONTEXT_MESSAGE_LIMIT: usize = 4;
const GUARDIAN_CONTEXT_MESSAGE_CHARS: usize = 800;

// Shadow dispositions recorded on the run's permission events. Every value is
// observability-only: no disposition ever fails the task, it either
// auto-approves (`guardian_allowed`) or falls back to the user prompt.
pub(crate) const GUARDIAN_DISPOSITION_ALLOWED: &str = "guardian_allowed";
pub(crate) const GUARDIAN_DISPOSITION_DENIED: &str = "guardian_denied";
pub(crate) const GUARDIAN_DISPOSITION_TIMEOUT: &str = "guardian_timeout";
pub(crate) const GUARDIAN_DISPOSITION_MALFORMED: &str = "guardian_malformed";
pub(crate) const GUARDIAN_DISPOSITION_UNAVAILABLE: &str = "guardian_unavailable";
pub(crate) const GUARDIAN_DISPOSITION_DISABLED: &str = "guardian_disabled";
pub(crate) const GUARDIAN_DISPOSITION_DESTRUCTIVE_SKIPPED: &str = "guardian_destructive_skipped";
pub(crate) const GUARDIAN_DISPOSITION_INELIGIBLE_COMMAND: &str = "guardian_ineligible_command";
pub(crate) const GUARDIAN_DISPOSITION_NO_DISTINCT_REVIEWER: &str = "guardian_no_distinct_reviewer";

const GUARDIAN_RESOLVED_BY: &str = "guardian-auto-approval";

#[derive(Debug)]
pub(crate) struct GuardianReviewPlan {
    pub(crate) model: String,
    pub(crate) prompt: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GuardianDecision {
    Allow,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GuardianReview {
    pub(crate) decision: GuardianDecision,
    pub(crate) reason: String,
}

/// Why a guardian review could not produce a verdict. Every failure falls back
/// to the ordinary user prompt (fail-closed, never fails the task).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GuardianReviewFailure {
    Unavailable,
    Timeout,
    Malformed,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum GuardianGateOutcome {
    /// The guardian explicitly allowed the action; the caller may resolve the
    /// request as allow-once.
    AutoApprove { review: GuardianReview },
    /// Deny, timeout, malformed answer, or unavailable reviewer: the request
    /// stays pending for the user. Carries the recorded disposition.
    FallBackToPrompt { disposition: &'static str },
}

/// Eligibility fold for one guardian review. Returns the reviewer model to use
/// or the skip disposition proving why the guardian must not run. Destructive
/// risk is never auto-approved, and the reviewer must be model-distinct from
/// the run's executor model.
pub(crate) fn guardian_review_eligibility(
    enabled: bool,
    risk: &PermissionRisk,
    executor_model: &str,
    reviewer_model: &str,
) -> Result<String, &'static str> {
    if !enabled {
        return Err(GUARDIAN_DISPOSITION_DISABLED);
    }
    if *risk == PermissionRisk::Destructive {
        return Err(GUARDIAN_DISPOSITION_DESTRUCTIVE_SKIPPED);
    }
    let reviewer = reviewer_model.trim();
    if reviewer.is_empty() || reviewer == executor_model.trim() {
        return Err(GUARDIAN_DISPOSITION_NO_DISTINCT_REVIEWER);
    }
    Ok(reviewer.to_string())
}

fn bound_field(value: &str, limit: usize) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= limit {
        trimmed.to_string()
    } else {
        let bounded: String = trimmed.chars().take(limit).collect();
        format!("{bounded}…[truncated]")
    }
}

/// Bounded recent-context excerpt for the guardian prompt: the last few
/// user/assistant/tool messages, each individually truncated.
pub(crate) fn guardian_context_excerpt(messages: &[Message]) -> String {
    let mut excerpt = String::new();
    let recent = messages
        .iter()
        .filter(|message| {
            matches!(
                message.role,
                MessageRole::User | MessageRole::Assistant | MessageRole::Tool
            )
        })
        .rev()
        .take(GUARDIAN_CONTEXT_MESSAGE_LIMIT);
    let mut lines = Vec::new();
    for message in recent {
        let role = match message.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            _ => "tool",
        };
        lines.push(format!(
            "{role}: {}",
            bound_field(&message.content, GUARDIAN_CONTEXT_MESSAGE_CHARS)
        ));
    }
    for line in lines.into_iter().rev() {
        if !excerpt.is_empty() {
            excerpt.push('\n');
        }
        excerpt.push_str(&line);
    }
    if excerpt.chars().count() > GUARDIAN_MAX_CONTEXT_CHARS {
        let bounded: String = excerpt.chars().take(GUARDIAN_MAX_CONTEXT_CHARS).collect();
        return format!("{bounded}…[truncated]");
    }
    excerpt
}

/// The guardian review prompt: objective, tool, arguments, risk level, and a
/// bounded recent-context excerpt, with a strict single-line JSON contract.
pub(crate) fn guardian_review_prompt(
    objective: &str,
    action: &str,
    input_json: &str,
    risk_label: &str,
    context_excerpt: &str,
) -> String {
    format!(
        "You are the permission guardian for a desktop agent. Decide whether one pending tool call should be auto-approved without asking the user.\n\
         Approve only when the call clearly serves the stated objective, matches the recent context, and its consequences are bounded and reversible. \
         Deny when in doubt: a denial here does not reject the task, it only routes the decision back to the user.\n\n\
         Task objective:\n{}\n\n\
         Recent context:\n{}\n\n\
         Pending tool call:\n- tool: {}\n- risk level: {}\n- arguments (JSON):\n{}\n\n\
         Reply with EXACTLY one line of JSON and nothing else: {{\"decision\":\"allow\",\"reason\":\"...\"}} or {{\"decision\":\"deny\",\"reason\":\"...\"}}",
        bound_field(objective, GUARDIAN_MAX_FIELD_CHARS),
        if context_excerpt.trim().is_empty() {
            "(none)".to_string()
        } else {
            context_excerpt.to_string()
        },
        bound_field(action, 200),
        bound_field(risk_label, 100),
        bound_field(input_json, GUARDIAN_MAX_FIELD_CHARS),
    )
}

/// Strict parser for the guardian's answer: one JSON object with a `decision`
/// of exactly `allow` or `deny` plus a `reason` string. Anything else is
/// malformed and falls back to the user prompt.
pub(crate) fn parse_guardian_review(output: &str) -> Result<GuardianReview, String> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return Err("guardian review is empty".to_string());
    }
    if trimmed.lines().count() > 1 {
        return Err("guardian review must be single-line JSON".to_string());
    }
    let value: serde_json::Value = serde_json::from_str(trimmed)
        .map_err(|error| format!("guardian review is not valid JSON: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "guardian review must be a JSON object".to_string())?;
    let decision = object
        .get("decision")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "guardian review is missing its decision".to_string())?;
    let reason = object
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "guardian review is missing its reason".to_string())?
        .to_string();
    match decision {
        "allow" => Ok(GuardianReview {
            decision: GuardianDecision::Allow,
            reason,
        }),
        "deny" => Ok(GuardianReview {
            decision: GuardianDecision::Deny,
            reason,
        }),
        other => Err(format!("guardian review decision is unsupported: {other}")),
    }
}

/// Fail-closed fold of one review attempt into the gate outcome: only an
/// explicit `allow` auto-approves; every other result falls back to the user
/// prompt.
pub(crate) fn guardian_gate_outcome(
    review: Result<GuardianReview, GuardianReviewFailure>,
) -> GuardianGateOutcome {
    match review {
        Ok(review) if review.decision == GuardianDecision::Allow => {
            GuardianGateOutcome::AutoApprove { review }
        }
        Ok(_) => GuardianGateOutcome::FallBackToPrompt {
            disposition: GUARDIAN_DISPOSITION_DENIED,
        },
        Err(GuardianReviewFailure::Unavailable) => GuardianGateOutcome::FallBackToPrompt {
            disposition: GUARDIAN_DISPOSITION_UNAVAILABLE,
        },
        Err(GuardianReviewFailure::Timeout) => GuardianGateOutcome::FallBackToPrompt {
            disposition: GUARDIAN_DISPOSITION_TIMEOUT,
        },
        Err(GuardianReviewFailure::Malformed) => GuardianGateOutcome::FallBackToPrompt {
            disposition: GUARDIAN_DISPOSITION_MALFORMED,
        },
    }
}

/// Plan one guardian review for a pending permission request, or report the
/// skip disposition. Pure; the caller owns dispatch.
pub(crate) fn plan_guardian_review(
    config: &ProviderConfig,
    risk: &PermissionRisk,
    executor_model: &str,
    objective: &str,
    request: &PermissionRequest,
    context_excerpt: &str,
) -> Result<GuardianReviewPlan, &'static str> {
    let reviewer = guardian_review_eligibility(
        config.guardian_auto_approval,
        risk,
        executor_model,
        config.model_for_role(&ModelRole::Reviewer).as_str(),
    )?;
    // Commands that are ineligible for policy auto-approval (unrecognized
    // executables, positional script files) are equally ineligible for
    // guardian auto-approval: the guardian must never approve what the
    // approval policy itself would still prompt for.
    if request
        .metadata
        .get("auto_grant_eligible")
        .map(String::as_str)
        == Some("false")
    {
        return Err(GUARDIAN_DISPOSITION_INELIGIBLE_COMMAND);
    }
    let input_json = request
        .metadata
        .get("tool_input")
        .map(String::as_str)
        .unwrap_or_default();
    let prompt = guardian_review_prompt(
        objective,
        &request.action,
        input_json,
        crate::permission_service::permission_risk_label(risk),
        context_excerpt,
    );
    Ok(GuardianReviewPlan {
        model: reviewer,
        prompt,
    })
}

fn guardian_disposition_event_rows(
    store: &mut SqliteStore,
    request: &PermissionRequest,
    run_context: &Metadata,
    disposition: &str,
    detail: Option<&str>,
) -> Result<(), StorageError> {
    let mut metadata = [
        ("permission_id".to_string(), request.id.0.clone()),
        ("tool".to_string(), request.action.clone()),
        (
            "risk".to_string(),
            crate::permission_service::permission_risk_label(&request.risk).to_string(),
        ),
        ("guardian_disposition".to_string(), disposition.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    if let Some(detail) = detail {
        metadata.insert("guardian_reason".to_string(), bound_field(detail, 500));
    }
    append_event(
        store,
        &request.task_id,
        EventKind::TaskStatusChanged,
        "Guardian permission review recorded",
        metadata_with_context(metadata, run_context),
    )
}

fn record_guardian_disposition_event(
    state: &tauri::State<'_, AppState>,
    request: &PermissionRequest,
    run_context: &Metadata,
    disposition: &str,
    detail: Option<&str>,
) -> Result<(), String> {
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    guardian_disposition_event_rows(&mut store, request, run_context, disposition, detail)
        .map_err(|error| error.to_string())
}

fn persist_guardian_approval_rows(
    store: &mut SqliteStore,
    request: &PermissionRequest,
    run_context: &Metadata,
    reason: &str,
) -> Result<(), StorageError> {
    let resolution = PermissionResolution {
        request_id: request.id.clone(),
        decision: PermissionDecision::AllowOnce,
        resolved_at_ms: current_time_millis(),
        resolved_by: GUARDIAN_RESOLVED_BY.to_string(),
    };
    store.with_immediate_transaction(|store| {
        store.resolve_permission_in_transaction(&resolution)?;
        append_event(
            store,
            &request.task_id,
            EventKind::PermissionResolved,
            "Permission approved",
            metadata_with_context(
                [
                    ("permission_id".to_string(), request.id.0.clone()),
                    ("decision".to_string(), "allow_once".to_string()),
                    ("tool".to_string(), request.action.clone()),
                    ("resolved_by".to_string(), GUARDIAN_RESOLVED_BY.to_string()),
                    (
                        "guardian_disposition".to_string(),
                        GUARDIAN_DISPOSITION_ALLOWED.to_string(),
                    ),
                    ("guardian_reason".to_string(), bound_field(reason, 500)),
                ]
                .into_iter()
                .collect(),
                run_context,
            ),
        )
    })
}

fn persist_guardian_approval(
    state: &tauri::State<'_, AppState>,
    request: &PermissionRequest,
    run_context: &Metadata,
    reason: &str,
) -> Result<(), String> {
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    persist_guardian_approval_rows(&mut store, request, run_context, reason)
        .map_err(|error| error.to_string())
}

fn dispatch_guardian_review(
    app: &tauri::AppHandle,
    config: &ProviderConfig,
    request: &PermissionRequest,
    run_context: &Metadata,
    plan: &GuardianReviewPlan,
) -> Result<GuardianReview, GuardianReviewFailure> {
    let (sender, receiver) = std::sync::mpsc::channel::<Result<String, String>>();
    let thread_app = app.clone();
    let thread_config = config.clone();
    let thread_task_id = request.task_id.clone();
    let thread_run_context = run_context.clone();
    let thread_model = plan.model.clone();
    let thread_prompt = plan.prompt.clone();
    std::thread::spawn(move || {
        use tauri::Manager as _;
        let state = thread_app.state::<AppState>();
        let limits = crate::collaboration_execution::CollaborationCallLimits {
            no_progress_timeout: Some(GUARDIAN_REVIEW_TIMEOUT),
            ..Default::default()
        };
        let result = crate::collaboration_stage_runtime::run_collaboration_stage_with_limits(
            &state,
            &thread_config,
            &thread_task_id,
            &thread_run_context,
            "guardian-permission-review",
            "guardian_permission_review",
            ModelRole::Reviewer,
            &thread_model,
            thread_prompt,
            guardian_or_verifier_attribution(ModelRole::Reviewer),
            limits,
        );
        let _ = sender.send(result);
    });
    let output = match receiver.recv_timeout(GUARDIAN_REVIEW_TIMEOUT) {
        Ok(Ok(output)) => output,
        Ok(Err(_)) => return Err(GuardianReviewFailure::Unavailable),
        Err(_) => return Err(GuardianReviewFailure::Timeout),
    };
    parse_guardian_review(&output).map_err(|_| GuardianReviewFailure::Malformed)
}

/// Guardian gate for one permission request that is already persisted as
/// pending (the run would otherwise pause for the user). Returns `true` only
/// when the guardian explicitly allowed the action and the approval was
/// durably recorded; every other outcome leaves the request pending for the
/// ordinary user prompt. With the toggle off this performs no model call and
/// records nothing.
pub(crate) fn guardian_auto_approve_pending_permission(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    run_context: &Metadata,
    request: &PermissionRequest,
    objective: &str,
    context_excerpt: &str,
) -> Result<bool, String> {
    let config = clone_provider_config(state)?;
    if !config.guardian_auto_approval {
        return Ok(false);
    }
    let executor_model = run_context
        .get("agent_model")
        .map(String::as_str)
        .unwrap_or(config.executor_model.as_str());
    let plan = match plan_guardian_review(
        &config,
        &request.risk,
        executor_model,
        objective,
        request,
        context_excerpt,
    ) {
        Ok(plan) => plan,
        Err(disposition) => {
            record_guardian_disposition_event(state, request, run_context, disposition, None)?;
            return Ok(false);
        }
    };
    let review = dispatch_guardian_review(app, &config, request, run_context, &plan);
    match guardian_gate_outcome(review) {
        GuardianGateOutcome::AutoApprove { review } => {
            persist_guardian_approval(state, request, run_context, &review.reason)?;
            Ok(true)
        }
        GuardianGateOutcome::FallBackToPrompt { disposition } => {
            record_guardian_disposition_event(state, request, run_context, disposition, None)?;
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::TaskId;

    fn provider_config_with_guardian(
        enabled: bool,
        reviewer: &str,
        executor: &str,
    ) -> ProviderConfig {
        ProviderConfig {
            guardian_auto_approval: enabled,
            reviewer_model: reviewer.to_string(),
            executor_model: executor.to_string(),
            ..Default::default()
        }
    }

    fn shell_permission_request(command: &str) -> PermissionRequest {
        PermissionRequest {
            id: agent_core::PermissionRequestId("perm-guardian".to_string()),
            task_id: TaskId("task".to_string()),
            risk: PermissionRisk::Execute,
            action: "shell.run".to_string(),
            reason: "run a command".to_string(),
            scope: ".".to_string(),
            metadata: [
                ("command".to_string(), command.to_string()),
                (
                    "tool_input".to_string(),
                    format!("{{\"command\":\"{command}\"}}"),
                ),
            ]
            .into_iter()
            .collect(),
        }
    }

    #[test]
    fn guardian_is_not_planned_when_disabled_or_destructive_or_same_model() {
        let request = shell_permission_request("cargo test");

        // Default off: no plan, no disposition-specific eligibility.
        let off = provider_config_with_guardian(false, "reviewer-model", "executor-model");
        assert_eq!(
            plan_guardian_review(
                &off,
                &request.risk,
                "executor-model",
                "objective",
                &request,
                ""
            )
            .unwrap_err(),
            GUARDIAN_DISPOSITION_DISABLED
        );

        // Destructive risk is never auto-approved even when enabled.
        let enabled = provider_config_with_guardian(true, "reviewer-model", "executor-model");
        let mut destructive = request.clone();
        destructive.risk = PermissionRisk::Destructive;
        assert_eq!(
            plan_guardian_review(
                &enabled,
                &destructive.risk,
                "executor-model",
                "objective",
                &destructive,
                ""
            )
            .unwrap_err(),
            GUARDIAN_DISPOSITION_DESTRUCTIVE_SKIPPED
        );

        // The reviewer must be model-distinct from the executor.
        let same_model = provider_config_with_guardian(true, "shared-model", "shared-model");
        assert_eq!(
            plan_guardian_review(
                &same_model,
                &request.risk,
                "shared-model",
                "objective",
                &request,
                ""
            )
            .unwrap_err(),
            GUARDIAN_DISPOSITION_NO_DISTINCT_REVIEWER
        );

        // Distinct reviewer plans a review bound to the reviewer model.
        let plan = plan_guardian_review(
            &enabled,
            &request.risk,
            "executor-model",
            "ship the feature",
            &request,
            "user: run the tests",
        )
        .expect("eligible guardian should plan");
        assert_eq!(plan.model, "reviewer-model");
        assert!(plan.prompt.contains("ship the feature"));
        assert!(plan.prompt.contains("shell.run"));
        assert!(plan.prompt.contains("cargo test"));
        assert!(plan.prompt.contains("execute"));
        assert!(plan.prompt.contains("user: run the tests"));
        assert!(plan.prompt.contains("\"decision\""));
    }

    #[test]
    fn guardian_never_reviews_commands_the_policy_would_still_prompt_for() {
        let enabled = provider_config_with_guardian(true, "reviewer-model", "executor-model");
        // An unrecognized executable is classified auto_grant_eligible=false
        // (see the tools crate classification), so the guardian must skip it
        // even though it is Execute risk and the guardian is enabled with a
        // distinct reviewer.
        let mut ineligible = shell_permission_request("./target/debug/mystery-tool --run");
        ineligible
            .metadata
            .insert("auto_grant_eligible".to_string(), "false".to_string());
        assert_eq!(
            plan_guardian_review(
                &enabled,
                &ineligible.risk,
                "executor-model",
                "objective",
                &ineligible,
                ""
            )
            .unwrap_err(),
            GUARDIAN_DISPOSITION_INELIGIBLE_COMMAND
        );

        // A known command keeps its guardian eligibility.
        let eligible = shell_permission_request("cargo test");
        assert!(plan_guardian_review(
            &enabled,
            &eligible.risk,
            "executor-model",
            "objective",
            &eligible,
            ""
        )
        .is_ok());
    }

    #[test]
    fn guardian_review_prompt_bounds_oversized_fields() {
        let huge_objective = "x".repeat(GUARDIAN_MAX_FIELD_CHARS + 100);
        let huge_input = "y".repeat(GUARDIAN_MAX_FIELD_CHARS + 100);
        let prompt =
            guardian_review_prompt(&huge_objective, "shell.run", &huge_input, "execute", "");
        assert!(prompt.contains("…[truncated]"));
        assert!(!prompt.contains(&"x".repeat(GUARDIAN_MAX_FIELD_CHARS + 100)));
        assert!(prompt.contains("(none)"));
    }

    #[test]
    fn guardian_context_excerpt_is_bounded_and_ordered() {
        let messages = (0..10)
            .map(|index| Message {
                role: if index % 2 == 0 {
                    MessageRole::User
                } else {
                    MessageRole::Assistant
                },
                content: format!("message {index}"),
                metadata: Metadata::new(),
            })
            .collect::<Vec<_>>();
        let excerpt = guardian_context_excerpt(&messages);
        let lines = excerpt.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), GUARDIAN_CONTEXT_MESSAGE_LIMIT);
        assert!(lines[0].contains("message 6"));
        assert!(lines[3].contains("message 9"));
        assert!(!excerpt.contains("message 5"));
    }

    #[test]
    fn guardian_review_parsing_accepts_only_strict_single_line_json() {
        let allow = parse_guardian_review("{\"decision\":\"allow\",\"reason\":\"bounded\"}")
            .expect("allow should parse");
        assert_eq!(allow.decision, GuardianDecision::Allow);
        assert_eq!(allow.reason, "bounded");

        let deny = parse_guardian_review("  {\"decision\":\"deny\",\"reason\":\"risky\"}  ")
            .expect("deny should parse");
        assert_eq!(deny.decision, GuardianDecision::Deny);

        for malformed in [
            "",
            "   ",
            "allow",
            "{\"decision\":\"allow\"}",
            "{\"decision\":\"maybe\",\"reason\":\"x\"}",
            "{\"decision\":\"ALLOW\",\"reason\":\"x\"}",
            "[\"allow\"]",
            "{\"decision\":\"allow\",\"reason\":\"x\"}\ntrailing",
        ] {
            assert!(
                parse_guardian_review(malformed).is_err(),
                "must reject malformed guardian output: {malformed:?}"
            );
        }
    }

    #[test]
    fn guardian_gate_outcome_only_auto_approves_explicit_allow() {
        let allow = Ok(GuardianReview {
            decision: GuardianDecision::Allow,
            reason: "ok".to_string(),
        });
        assert!(matches!(
            guardian_gate_outcome(allow),
            GuardianGateOutcome::AutoApprove { .. }
        ));

        let deny = Ok(GuardianReview {
            decision: GuardianDecision::Deny,
            reason: "no".to_string(),
        });
        assert_eq!(
            guardian_gate_outcome(deny),
            GuardianGateOutcome::FallBackToPrompt {
                disposition: GUARDIAN_DISPOSITION_DENIED,
            }
        );
        assert_eq!(
            guardian_gate_outcome(Err(GuardianReviewFailure::Timeout)),
            GuardianGateOutcome::FallBackToPrompt {
                disposition: GUARDIAN_DISPOSITION_TIMEOUT,
            }
        );
        assert_eq!(
            guardian_gate_outcome(Err(GuardianReviewFailure::Malformed)),
            GuardianGateOutcome::FallBackToPrompt {
                disposition: GUARDIAN_DISPOSITION_MALFORMED,
            }
        );
        assert_eq!(
            guardian_gate_outcome(Err(GuardianReviewFailure::Unavailable)),
            GuardianGateOutcome::FallBackToPrompt {
                disposition: GUARDIAN_DISPOSITION_UNAVAILABLE,
            }
        );
    }

    #[test]
    fn guardian_eligibility_never_auto_approves_destructive_even_when_enabled() {
        for risk in [
            PermissionRisk::Read,
            PermissionRisk::Write,
            PermissionRisk::Execute,
            PermissionRisk::Network,
            PermissionRisk::Sensitive,
        ] {
            assert!(
                guardian_review_eligibility(true, &risk, "executor", "reviewer").is_ok(),
                "non-destructive risk {risk:?} should stay eligible"
            );
        }
        assert_eq!(
            guardian_review_eligibility(true, &PermissionRisk::Destructive, "executor", "reviewer"),
            Err(GUARDIAN_DISPOSITION_DESTRUCTIVE_SKIPPED)
        );
    }

    #[test]
    fn guardian_approval_records_allow_once_resolution_and_disposition_event() {
        use agent_core::{PermissionRequestId, TaskId};
        use agent_storage::{EventStore, PermissionStore};

        let mut store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("task-guardian".to_string());
        let request = PermissionRequest {
            id: PermissionRequestId("perm-guardian-approval".to_string()),
            task_id: task_id.clone(),
            risk: PermissionRisk::Execute,
            action: "shell.run".to_string(),
            reason: "run a command".to_string(),
            scope: ".".to_string(),
            metadata: [
                ("session_id".to_string(), "session-a".to_string()),
                ("command".to_string(), "cargo test".to_string()),
            ]
            .into_iter()
            .collect(),
        };
        store
            .save_permission_request(request.clone(), 100)
            .expect("pending request should save");

        persist_guardian_approval_rows(&mut store, &request, &Metadata::new(), "bounded")
            .expect("guardian approval should persist");

        let audits = store.list_permission_audits().expect("audits should load");
        assert_eq!(audits.len(), 1);
        let resolution = audits[0]
            .resolution
            .as_ref()
            .expect("guardian approval should resolve the request");
        assert_eq!(resolution.decision, PermissionDecision::AllowOnce);
        assert_eq!(resolution.resolved_by, GUARDIAN_RESOLVED_BY);

        let events = store.list_by_task(&task_id).expect("events should load");
        let resolved = events
            .iter()
            .find(|event| event.kind == EventKind::PermissionResolved)
            .expect("approval should record a resolution event");
        assert_eq!(
            resolved
                .metadata
                .get("guardian_disposition")
                .map(String::as_str),
            Some(GUARDIAN_DISPOSITION_ALLOWED)
        );
        assert_eq!(
            resolved.metadata.get("resolved_by").map(String::as_str),
            Some(GUARDIAN_RESOLVED_BY)
        );
        assert_eq!(
            resolved.metadata.get("decision").map(String::as_str),
            Some("allow_once")
        );

        // The guardian never creates a reusable session capability.
        assert!(!store
            .has_session_permission_capability(&task_id, "session-a", &request, true, true)
            .expect("capability query should succeed"));
    }

    #[test]
    fn guardian_fallback_dispositions_land_on_the_event_stream() {
        use agent_core::{PermissionRequestId, TaskId};
        use agent_storage::{EventStore, PermissionStore};

        let mut store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("task-guardian-fallback".to_string());
        let request = PermissionRequest {
            id: PermissionRequestId("perm-guardian-fallback".to_string()),
            task_id: task_id.clone(),
            risk: PermissionRisk::Execute,
            action: "shell.run".to_string(),
            reason: "run a command".to_string(),
            scope: ".".to_string(),
            metadata: [
                ("session_id".to_string(), "session-a".to_string()),
                ("command".to_string(), "cargo test".to_string()),
            ]
            .into_iter()
            .collect(),
        };
        store
            .save_permission_request(request.clone(), 100)
            .expect("pending request should save");

        for disposition in [
            GUARDIAN_DISPOSITION_DENIED,
            GUARDIAN_DISPOSITION_TIMEOUT,
            GUARDIAN_DISPOSITION_MALFORMED,
            GUARDIAN_DISPOSITION_UNAVAILABLE,
            GUARDIAN_DISPOSITION_DESTRUCTIVE_SKIPPED,
        ] {
            guardian_disposition_event_rows(
                &mut store,
                &request,
                &Metadata::new(),
                disposition,
                None,
            )
            .expect("disposition event should persist");
        }

        let events = store.list_by_task(&task_id).expect("events should load");
        let recorded = events
            .iter()
            .filter(|event| event.summary == "Guardian permission review recorded")
            .collect::<Vec<_>>();
        assert_eq!(recorded.len(), 5);
        let observed = recorded
            .iter()
            .map(|event| {
                event
                    .metadata
                    .get("guardian_disposition")
                    .map(String::as_str)
                    .unwrap_or_default()
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            observed,
            [
                GUARDIAN_DISPOSITION_DENIED,
                GUARDIAN_DISPOSITION_TIMEOUT,
                GUARDIAN_DISPOSITION_MALFORMED,
                GUARDIAN_DISPOSITION_UNAVAILABLE,
                GUARDIAN_DISPOSITION_DESTRUCTIVE_SKIPPED,
            ]
            .into_iter()
            .collect()
        );

        // A fallback never resolves the pending request.
        let audits = store.list_permission_audits().expect("audits should load");
        assert!(audits[0].resolution.is_none());
    }
}
