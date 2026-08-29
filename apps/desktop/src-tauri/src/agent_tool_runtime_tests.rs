use super::*;
use crate::agent_tool_runtime::{
    approval_policy_allows_auto_grant, evaluate_agent_tool_permission,
    pending_permission_matches_exact_invocation, AgentToolPermissionGateOutcome,
};
use agent_core::{PermissionDecision, PermissionResolution, ToolCallId};

fn run_context_for(session_id: &str, agent_run_id: &str, prompt_contract_epoch: u64) -> Metadata {
    [
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), session_id.to_string()),
        ("agent_run_id".to_string(), agent_run_id.to_string()),
        ("steer_epoch".to_string(), prompt_contract_epoch.to_string()),
        (
            "prompt_contract_epoch".to_string(),
            prompt_contract_epoch.to_string(),
        ),
    ]
    .into_iter()
    .collect()
}

fn run_context(session_id: &str) -> Metadata {
    run_context_for(session_id, "run-a", 0)
}

fn invocation() -> ToolInvocation {
    ToolInvocation {
        id: ToolCallId("call-a".to_string()),
        task_id: phase16_task_id(),
        tool_name: "file.write".to_string(),
        input_json: r#"{"path":"notes.md","content":"safe"}"#.to_string(),
        proposed_by_model: "test".to_string(),
        metadata: Metadata::new(),
    }
}

fn permission_request(invocation: &ToolInvocation) -> PermissionRequest {
    PermissionRequest {
        id: PermissionRequestId(String::new()),
        task_id: invocation.task_id.clone(),
        risk: PermissionRisk::Write,
        action: invocation.tool_name.clone(),
        reason: "Write the requested file".to_string(),
        scope: "notes.md".to_string(),
        metadata: Metadata::new(),
    }
}

fn pending_match_candidate(invocation: &ToolInvocation, context: &Metadata) -> PermissionRequest {
    let mut request = permission_request(invocation);
    request
        .metadata
        .insert("tool_call_id".to_string(), invocation.id.0.clone());
    request
        .metadata
        .insert("tool_name".to_string(), invocation.tool_name.clone());
    request.metadata.insert(
        "tool_input_fingerprint".to_string(),
        tool_input_fingerprint(&invocation.tool_name, &invocation.input_json),
    );
    request.metadata.extend(context.clone());
    request
}

#[test]
fn pending_permission_match_requires_exact_capability_and_invocation_identity() {
    let invocation = invocation();
    let candidate = pending_match_candidate(&invocation, &run_context("session-a"));
    assert!(pending_permission_matches_exact_invocation(
        &candidate, &candidate
    ));

    let mut changed = candidate.clone();
    changed.action = "file.delete".to_string();
    assert!(!pending_permission_matches_exact_invocation(
        &changed, &candidate
    ));
    let mut changed = candidate.clone();
    changed.risk = PermissionRisk::Destructive;
    assert!(!pending_permission_matches_exact_invocation(
        &changed, &candidate
    ));
    let mut changed = candidate.clone();
    changed.scope = "other.md".to_string();
    assert!(!pending_permission_matches_exact_invocation(
        &changed, &candidate
    ));
    for key in [
        "tool_call_id",
        "tool_name",
        "tool_input_fingerprint",
        "project_id",
        "session_id",
        "agent_run_id",
        "collaboration_id",
        "steer_epoch",
        "prompt_contract_epoch",
    ] {
        let mut changed = candidate.clone();
        changed
            .metadata
            .insert(key.to_string(), "other".to_string());
        assert!(
            !pending_permission_matches_exact_invocation(&changed, &candidate),
            "{key} must be part of the exact pending identity"
        );
    }
}

#[test]
fn pending_permission_gate_coalesces_only_exact_canonical_call_and_lineage() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let invocation = invocation();
    let context = run_context("session-a");

    for input_json in [
        r#"{"path":"notes.md","content":"safe"}"#,
        r#"{"content":"safe","path":"notes.md"}"#,
    ] {
        let mut equivalent = invocation.clone();
        equivalent.input_json = input_json.to_string();
        assert_eq!(
            evaluate_agent_tool_permission(
                &mut store,
                &phase16_task_id(),
                &context,
                Some("session-a"),
                &equivalent,
                permission_request(&equivalent),
                "strict",
            )
            .expect("equivalent request should reach the pending gate"),
            AgentToolPermissionGateOutcome::Pending
        );
    }

    let mut different_call = invocation.clone();
    different_call.id = ToolCallId("call-b".to_string());
    evaluate_agent_tool_permission(
        &mut store,
        &phase16_task_id(),
        &context,
        Some("session-a"),
        &different_call,
        permission_request(&different_call),
        "strict",
    )
    .expect("a different call id should remain independently permissioned");

    let next_epoch = run_context_for("session-a", "run-a", 1);
    evaluate_agent_tool_permission(
        &mut store,
        &phase16_task_id(),
        &next_epoch,
        Some("session-a"),
        &invocation,
        permission_request(&invocation),
        "strict",
    )
    .expect("a different prompt contract epoch should remain independent");

    let next_run = run_context_for("session-a", "run-b", 0);
    evaluate_agent_tool_permission(
        &mut store,
        &phase16_task_id(),
        &next_run,
        Some("session-a"),
        &invocation,
        permission_request(&invocation),
        "strict",
    )
    .expect("a different run should remain independent");

    let next_session = run_context_for("session-b", "run-a", 0);
    evaluate_agent_tool_permission(
        &mut store,
        &phase16_task_id(),
        &next_session,
        Some("session-b"),
        &invocation,
        permission_request(&invocation),
        "strict",
    )
    .expect("a different session should remain independent");

    let events = store
        .list_by_task(&phase16_task_id())
        .expect("permission events should be readable");
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == EventKind::PermissionRequested)
            .count(),
        5,
        "the canonical duplicate must not create a sixth permission event"
    );
    assert_eq!(
        pending_agent_permissions_for_run(
            &store,
            &phase16_task_id(),
            Some("session-a"),
            Some("run-a")
        )
        .expect("run-a permissions should be queryable")
        .len(),
        3
    );
}

#[test]
fn production_permission_gate_blocks_then_reuses_only_the_resolved_session_capability() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let invocation = invocation();
    let context = run_context("session-a");

    let first = evaluate_agent_tool_permission(
        &mut store,
        &phase16_task_id(),
        &context,
        Some("session-a"),
        &invocation,
        permission_request(&invocation),
        "strict",
    )
    .expect("permission gate should persist a pending request");
    assert_eq!(first, AgentToolPermissionGateOutcome::Pending);

    let pending = pending_agent_permissions_for_run(
        &store,
        &phase16_task_id(),
        Some("session-a"),
        Some("run-a"),
    )
    .expect("pending permission should be queryable");
    assert_eq!(pending.len(), 1);
    store
        .resolve_permission(PermissionResolution {
            request_id: pending[0].id.clone(),
            decision: PermissionDecision::AllowForSession,
            resolved_at_ms: current_time_millis(),
            resolved_by: "test".to_string(),
        })
        .expect("permission should resolve");

    let reused = evaluate_agent_tool_permission(
        &mut store,
        &phase16_task_id(),
        &context,
        Some("session-a"),
        &invocation,
        permission_request(&invocation),
        "strict",
    )
    .expect("the exact resolved capability should be reusable");
    assert_eq!(reused, AgentToolPermissionGateOutcome::Reused);

    let other_run_context = run_context("session-b");
    let other_session = evaluate_agent_tool_permission(
        &mut store,
        &phase16_task_id(),
        &other_run_context,
        Some("session-b"),
        &invocation,
        permission_request(&invocation),
        "strict",
    )
    .expect("another session should receive its own pending request");
    assert_eq!(other_session, AgentToolPermissionGateOutcome::Pending);
}

#[test]
fn approval_policy_auto_grant_is_policy_scoped_and_never_destructive() {
    for policy in ["strict", "", "unknown", "SESSION"] {
        assert!(
            !approval_policy_allows_auto_grant(policy, &PermissionRisk::Write),
            "{policy} must never auto-approve"
        );
        assert!(!approval_policy_allows_auto_grant(
            policy,
            &PermissionRisk::Destructive
        ));
    }
    for policy in ["session", "all"] {
        for risk in [
            PermissionRisk::Read,
            PermissionRisk::Write,
            PermissionRisk::Execute,
            PermissionRisk::Network,
            PermissionRisk::Sensitive,
        ] {
            assert!(
                approval_policy_allows_auto_grant(policy, &risk),
                "{policy} should auto-approve non-destructive risk {risk:?}"
            );
        }
        // Safety floor: destructive risk always prompts, under every policy.
        assert!(!approval_policy_allows_auto_grant(
            policy,
            &PermissionRisk::Destructive
        ));
    }
}

#[test]
fn session_and_all_policies_auto_approve_non_destructive_requests_only() {
    for policy in ["session", "all"] {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let invocation = invocation();
        let context = run_context("session-a");

        let outcome = evaluate_agent_tool_permission(
            &mut store,
            &phase16_task_id(),
            &context,
            Some("session-a"),
            &invocation,
            permission_request(&invocation),
            policy,
        )
        .expect("the approval policy gate should evaluate");
        assert_eq!(outcome, AgentToolPermissionGateOutcome::Reused);
        assert!(
            pending_agent_permissions_for_run(
                &store,
                &phase16_task_id(),
                Some("session-a"),
                Some("run-a"),
            )
            .expect("permissions should be queryable")
            .is_empty(),
            "an auto-approved request must not leave a pending permission behind"
        );

        let events = store
            .list_by_task(&phase16_task_id())
            .expect("permission events should be readable");
        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind == EventKind::PermissionRequested)
                .count(),
            0,
            "an auto-approved request must not record a permission prompt"
        );
        let resolved = events
            .iter()
            .filter(|event| event.kind == EventKind::PermissionResolved)
            .collect::<Vec<_>>();
        assert_eq!(resolved.len(), 1);
        assert_eq!(
            resolved[0].metadata.get("decision").map(String::as_str),
            Some(format!("auto_{policy}").as_str()),
            "the automatic decision must be auditable per policy"
        );
        assert_eq!(
            resolved[0]
                .metadata
                .get("approval_policy")
                .map(String::as_str),
            Some(policy)
        );

        // Destructive requests keep pausing for manual approval even under
        // the permissive policies.
        let mut destructive = permission_request(&invocation);
        destructive.risk = PermissionRisk::Destructive;
        let destructive_outcome = evaluate_agent_tool_permission(
            &mut store,
            &phase16_task_id(),
            &context,
            Some("session-a"),
            &invocation,
            destructive,
            policy,
        )
        .expect("destructive requests must reach the ordinary gate");
        assert_eq!(destructive_outcome, AgentToolPermissionGateOutcome::Pending);
        assert_eq!(
            pending_agent_permissions_for_run(
                &store,
                &phase16_task_id(),
                Some("session-a"),
                Some("run-a"),
            )
            .expect("permissions should be queryable")
            .len(),
            1,
            "a destructive request under {policy} must stay pending for the user"
        );
    }
}

#[test]
fn strict_policy_keeps_prompting_without_reusing_policy_grants() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let invocation = invocation();
    let context = run_context("session-a");

    // Even after a session-policy run auto-approved the same request, a
    // strict evaluation still prompts: policy auto-approvals write no
    // reusable grant.
    evaluate_agent_tool_permission(
        &mut store,
        &phase16_task_id(),
        &context,
        Some("session-a"),
        &invocation,
        permission_request(&invocation),
        "session",
    )
    .expect("the session policy gate should evaluate");
    let strict = evaluate_agent_tool_permission(
        &mut store,
        &phase16_task_id(),
        &context,
        Some("session-a"),
        &invocation,
        permission_request(&invocation),
        "strict",
    )
    .expect("strict evaluation must reach the ordinary gate");
    assert_eq!(strict, AgentToolPermissionGateOutcome::Pending);
}
