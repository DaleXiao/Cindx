use super::*;
use orchestrator::{AdaptiveWorkflow, AdaptiveWorkflowStep};
use tools::encode_input;

fn test_conductor_harness(models: Vec<String>, agent_budget: usize) -> ConductorHarness {
    let routing = RoutingContext::from_prompt("Test the conductor", Vec::new());
    ConductorHarness::new(ConductorRequest {
        workflow_id: "test-workflow".to_string(),
        objective: "Test the conductor".to_string(),
        recent_context: String::new(),
        effort: "pro".to_string(),
        policy: "best_of_n".to_string(),
        conductor_model: "conductor-model".to_string(),
        worker_models: models,
        role_hints: ConductorRoleHints {
            planner: "planner-a".to_string(),
            executor: "planner-a".to_string(),
            reviewer: "reviewer-b".to_string(),
            synthesizer: "summary-c".to_string(),
        },
        budget: WorkflowBudget {
            max_steps: adaptive_workflow_step_budget(agent_budget),
            max_models: agent_budget,
            max_model_turns_per_step: DEFAULT_COLLABORATION_WORKER_TURNS,
            max_tool_calls_per_step: MAX_COLLABORATION_WORKER_TOOL_CALLS,
            max_output_tokens_per_step: COLLABORATION_MAX_OUTPUT_TOKENS as usize,
        },
        execution_contract: ConductorExecutionContract::from_routing(
            &routing,
            "pro",
            OrchestrationPolicy::BestOfN {
                candidates: agent_budget,
            },
        ),
        prior_hint: None,
        prompt_evolution_enabled: true,
        prompt_genome: ConductorPromptGenome::seed_for_effort("pro"),
    })
}

#[test]
fn collaboration_candidate_quorum_matches_effort_contract() {
    assert_eq!(collaboration_candidate_quorum(0, "fast"), 0);
    assert_eq!(collaboration_candidate_quorum(1, "auto"), 1);
    assert_eq!(collaboration_candidate_quorum(3, "fast"), 1);
    assert_eq!(collaboration_candidate_quorum(2, "auto"), 2);
    assert_eq!(collaboration_candidate_quorum(4, "auto"), 3);
    assert_eq!(collaboration_candidate_quorum(3, "pro"), 2);
    assert_eq!(collaboration_candidate_quorum(4, "pro"), 3);

    assert_eq!(
        collaboration_candidate_quorum_grace("fast"),
        Duration::from_millis(100)
    );
    assert_eq!(
        collaboration_candidate_quorum_grace("auto"),
        Duration::from_millis(400)
    );
    assert_eq!(
        collaboration_candidate_quorum_grace("pro"),
        Duration::from_millis(1_500)
    );
}

#[test]
fn arbiter_failure_handoff_preserves_candidate_work_for_executor() {
    let candidates = vec![
        ("model-a".to_string(), "first grounded proposal".to_string()),
        (
            "model-b".to_string(),
            "second independent proposal".to_string(),
        ),
    ];
    let handoff = collaboration_candidate_handoff("solve the task", &candidates, "timeout");
    assert!(handoff.contains("INTERNAL TEAM HANDOFF"));
    assert!(handoff.contains("first grounded proposal"));
    assert!(handoff.contains("second independent proposal"));
    assert!(handoff.contains("timeout"));
}

#[test]
fn safety_and_steer_collaboration_errors_block_stale_executor_context() {
    assert!(collaboration_error_blocks_executor(&format!(
        "{WORKFLOW_SAFETY_ERROR_PREFIX} unsafe output"
    )));
    assert!(collaboration_error_blocks_executor(
        COLLABORATION_STEER_INTERRUPTED
    ));
    assert!(!collaboration_error_blocks_executor(&format!(
        "{WORKFLOW_RESUMABLE_ERROR_PREFIX} provider timeout"
    )));
    assert!(!collaboration_error_blocks_executor("arbiter unavailable"));
}

#[test]
fn pending_steer_interrupts_collaboration_without_stopping_the_run() {
    let control = Arc::new(AgentRunControl::new("pro"));
    assert!(!collaboration_run_should_interrupt(&control));

    assert_eq!(control.request_steer("queue-steer"), Ok(true));
    assert!(control.has_pending_steer());
    assert!(collaboration_run_should_interrupt(&control));
    assert!(!agent_run_should_stop(&control));

    let pending = control.take_pending_steers();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].queue_id, "queue-steer");
    assert!(!collaboration_run_should_interrupt(&control));
}

fn test_message(role: MessageRole, content: impl Into<String>) -> Message {
    Message {
        role,
        content: content.into(),
        metadata: Metadata::new(),
    }
}

fn single_step_workflow_plan(max_model_turns: usize) -> WorkflowPlanIr {
    WorkflowPlanIr::from_adaptive_with_profile(
        "worker-budget-test",
        "Inspect the workspace",
        "pro",
        "best_of_n",
        "planner",
        "seed-pro-v1",
        &AdaptiveWorkflow {
            steps: vec![AdaptiveWorkflowStep {
                id: "inspect".to_string(),
                role: "worker".to_string(),
                model: "worker".to_string(),
                subtask: "Inspect bounded evidence".to_string(),
                access: Vec::new(),
            }],
        },
        WorkflowBudget {
            max_steps: 1,
            max_models: 1,
            max_model_turns_per_step: max_model_turns,
            max_tool_calls_per_step: 6,
            max_output_tokens_per_step: 4_096,
        },
    )
}

#[test]
fn tool_registry_cache_is_versioned_and_bounded() {
    let mut cache = ToolRegistryCache::default();
    let active_root = PathBuf::from("/tmp/cindx-tool-cache-active");
    cache.insert(active_root.clone(), 7, ToolRegistry::new());

    assert!(cache.get(&active_root, 7).is_some());
    assert!(cache.get(&active_root, 8).is_none());

    for index in 0..TOOL_REGISTRY_CACHE_LIMIT {
        cache.insert(
            PathBuf::from(format!("/tmp/cindx-tool-cache-{index}")),
            7,
            ToolRegistry::new(),
        );
    }
    assert_eq!(cache.entries.len(), TOOL_REGISTRY_CACHE_LIMIT);

    cache.clear();
    assert!(cache.entries.is_empty());
}

#[test]
fn collaboration_tool_worker_reserves_a_terminal_answer_turn() {
    let mut runtime = start_agent_loop(
        TaskId("worker-finalization".to_string()),
        "Inspect evidence",
        AgentRuntimeConfig {
            max_turns: collaboration_worker_runtime_turn_limit(3, true),
        },
    );

    assert_eq!(runtime.max_turns, 4);
    runtime.turn = 2;
    assert!(!prepare_collaboration_worker_turn(&mut runtime, true, 3));
    runtime.turn = 3;
    assert!(prepare_collaboration_worker_turn(&mut runtime, true, 3));
    assert_eq!(
        runtime
            .messages
            .last()
            .and_then(|message| message.metadata.get("kind"))
            .map(String::as_str),
        Some("collaboration_worker_finalization")
    );
    let message_count = runtime.messages.len();
    assert!(prepare_collaboration_worker_turn(&mut runtime, true, 3));
    assert_eq!(runtime.messages.len(), message_count);
}

#[test]
fn workflow_continuation_reaches_worker_turns_and_attempts() {
    let plan = single_step_workflow_plan(3);
    let mut checkpoint = WorkflowExecutionCheckpoint::new("resume-worker", plan.clone(), 10);
    let genome = ConductorPromptGenome::seed_for_effort("pro");

    assert_eq!(effective_workflow_model_turn_budget(&plan, &checkpoint), 3);
    assert_eq!(
        effective_workflow_step_attempt_budget(&genome, &checkpoint),
        3
    );

    checkpoint.continue_with_budget(3, 20);

    assert_eq!(effective_workflow_model_turn_budget(&plan, &checkpoint), 6);
    assert_eq!(
        effective_workflow_step_attempt_budget(&genome, &checkpoint),
        6
    );
    checkpoint
        .begin_step_with_attempt_limit("inspect", "worker", 6, 30)
        .expect("continued attempt budget should reach the worker");
}

#[test]
fn running_workflow_attempt_resumes_without_consuming_another_attempt() {
    let plan = single_step_workflow_plan(3);
    let mut checkpoint = WorkflowExecutionCheckpoint::new("resume-running", plan, 10);

    assert!(
        ensure_adaptive_step_attempt_started(&mut checkpoint, "inspect", "worker", 3, 20,)
            .expect("first attempt should start")
    );
    assert!(
        !ensure_adaptive_step_attempt_started(&mut checkpoint, "inspect", "worker", 3, 30,)
            .expect("running attempt should resume")
    );
    assert_eq!(checkpoint.steps["inspect"].attempts, 1);

    checkpoint
        .fail_step("inspect", "transport failed", 40)
        .expect("attempt should fail");
    assert!(
        ensure_adaptive_step_attempt_started(&mut checkpoint, "inspect", "worker-alt", 3, 50,)
            .expect("failed attempt should restart")
    );
    assert_eq!(checkpoint.steps["inspect"].attempts, 2);
    assert_eq!(checkpoint.steps["inspect"].model, "worker-alt");
}

#[test]
fn adaptive_recovery_model_obeys_policy_and_rotates_alternates() {
    let models = vec![
        "worker-a".to_string(),
        "worker-b".to_string(),
        "worker-c".to_string(),
    ];

    assert_eq!(
        adaptive_recovery_model(
            "inspect",
            "worker-a",
            2,
            &models,
            PromptRetryPolicy::AlternateModel,
            Some("failed"),
        )
        .expect("alternate should exist"),
        "worker-b"
    );
    assert_eq!(
        adaptive_recovery_model(
            "inspect",
            "worker-a",
            3,
            &models,
            PromptRetryPolicy::AlternateModel,
            Some("failed"),
        )
        .expect("second alternate should exist"),
        "worker-c"
    );
    assert_eq!(
        adaptive_recovery_model(
            "inspect",
            "worker-a",
            2,
            &models,
            PromptRetryPolicy::SameModel,
            Some("failed"),
        )
        .expect("same-model retry should remain available"),
        "worker-a"
    );
    assert!(adaptive_recovery_model(
        "inspect",
        "worker-a",
        2,
        &models,
        PromptRetryPolicy::FailFast,
        Some("failed"),
    )
    .expect_err("fail-fast should reject recovery")
    .contains("fail-fast"));
}

#[test]
fn adaptive_layer_failure_preserves_every_failed_step() {
    let error = adaptive_layer_failure_error(&[
        "step inspect failed after recovery: timeout".to_string(),
        "step verify failed after recovery: invalid response".to_string(),
    ])
    .expect("layer failures should be resumable");

    assert!(error.starts_with(WORKFLOW_RESUMABLE_ERROR_PREFIX));
    assert!(error.contains("step inspect"));
    assert!(error.contains("step verify"));
    assert!(adaptive_layer_failure_error(&[]).is_none());
}

#[test]
fn adaptive_partial_handoff_preserves_completed_branches_without_claiming_completion() {
    let outputs = BTreeMap::from([
        (
            "inspect".to_string(),
            "Found the failing module and evidence A.".to_string(),
        ),
        (
            "design".to_string(),
            "Proposed a bounded repair with test B.".to_string(),
        ),
    ]);
    let handoff = adaptive_partial_work_handoff(
        "repair the project",
        &outputs,
        &["step verify failed after recovery: timeout".to_string()],
    )
    .expect("completed branches should produce a recoverable handoff");

    assert!(handoff.contains("INTERNAL PARTIAL WORKFLOW HANDOFF"));
    assert!(handoff.contains("Found the failing module"));
    assert!(handoff.contains("Proposed a bounded repair"));
    assert!(handoff.contains("step verify failed"));
    assert!(adaptive_partial_work_handoff(
        "repair the project",
        &BTreeMap::new(),
        &["failed".to_string()]
    )
    .is_none());
}

#[test]
fn anytime_best_known_output_tracks_verification_and_rejection() {
    let plan = single_step_workflow_plan(2);
    let mut checkpoint = WorkflowExecutionCheckpoint::new("resume", plan, 1);
    let mut controller = AnytimeController::new(AnytimeControllerConfig {
        max_parallelism: 2,
        min_successful_candidates: 1,
        max_candidates: 3,
        min_usable_quality_bps: 4_500,
        stop_policy: ConductorStopPolicy::FirstVerified,
        min_team_uplift_bps: 0,
        min_distinct_contributions: 0,
        requires_synthesis: false,
        verification_required: false,
    });
    controller
        .register(AnytimeCandidate::direct_anchor(DIRECT_ANCHOR_CANDIDATE_ID))
        .unwrap();
    controller
        .register(AnytimeCandidate::workflow("inspect", Vec::new(), 7_000))
        .unwrap();
    controller.mark_running(DIRECT_ANCHOR_CANDIDATE_ID).unwrap();
    controller
        .observe(
            DIRECT_ANCHOR_CANDIDATE_ID,
            AnytimeVerdict {
                quality_bps: 6_000,
                confidence_bps: 5_500,
                constraint_coverage_bps: 6_000,
                evidence_count: 0,
                safety_violations: 0,
                deliverable: true,
                verified: false,
                anchor_uplift_bps: None,
            },
        )
        .unwrap();
    controller.mark_running("inspect").unwrap();
    controller
        .observe(
            "inspect",
            AnytimeVerdict {
                quality_bps: 7_500,
                confidence_bps: 8_000,
                constraint_coverage_bps: 8_000,
                evidence_count: 3,
                safety_violations: 0,
                deliverable: true,
                verified: true,
                anchor_uplift_bps: None,
            },
        )
        .unwrap();
    checkpoint
        .anytime_outputs
        .insert(DIRECT_ANCHOR_CANDIDATE_ID.to_string(), "anchor".to_string());
    checkpoint
        .anytime_outputs
        .insert("inspect".to_string(), "verified workflow".to_string());

    let (candidate_id, output, verdict) =
        anytime_best_known_output(&controller, &checkpoint).unwrap();
    assert_eq!(candidate_id, "inspect");
    assert_eq!(output, "verified workflow");
    assert!(verdict.verified);

    controller
        .revise(
            "inspect",
            AnytimeVerdict {
                safety_violations: 1,
                ..verdict
            },
        )
        .unwrap();
    let (candidate_id, output, _) = anytime_best_known_output(&controller, &checkpoint).unwrap();
    assert_eq!(candidate_id, DIRECT_ANCHOR_CANDIDATE_ID);
    assert_eq!(output, "anchor");
}

#[test]
fn partial_handoff_enters_the_anytime_frontier_and_checkpoint() {
    let plan = single_step_workflow_plan(2);
    let mut checkpoint = WorkflowExecutionCheckpoint::new("resume", plan, 1);
    let mut controller = AnytimeController::new(AnytimeControllerConfig {
        max_parallelism: 2,
        min_successful_candidates: 1,
        max_candidates: 2,
        min_usable_quality_bps: 4_500,
        stop_policy: ConductorStopPolicy::Quorum,
        min_team_uplift_bps: 0,
        min_distinct_contributions: 0,
        requires_synthesis: false,
        verification_required: false,
    });
    controller
        .register(AnytimeCandidate::direct_anchor(DIRECT_ANCHOR_CANDIDATE_ID))
        .unwrap();
    controller.mark_running(DIRECT_ANCHOR_CANDIDATE_ID).unwrap();
    controller
        .observe(
            DIRECT_ANCHOR_CANDIDATE_ID,
            AnytimeVerdict {
                quality_bps: 6_000,
                confidence_bps: 5_500,
                constraint_coverage_bps: 6_000,
                evidence_count: 0,
                safety_violations: 0,
                deliverable: true,
                verified: false,
                anchor_uplift_bps: None,
            },
        )
        .unwrap();
    checkpoint
        .anytime_outputs
        .insert(DIRECT_ANCHOR_CANDIDATE_ID.to_string(), "anchor".to_string());

    register_partial_handoff_candidate(
        &mut controller,
        &mut checkpoint,
        "partial evidence handoff",
        2,
        1,
        4,
    )
    .unwrap();

    let (candidate_id, output, verdict) =
        anytime_best_known_output(&controller, &checkpoint).unwrap();
    assert_eq!(candidate_id, PARTIAL_HANDOFF_CANDIDATE_ID);
    assert_eq!(output, "partial evidence handoff");
    assert_eq!(verdict.evidence_count, 4);
    assert!(!verdict.verified);
    assert!(!checkpoint.anytime_controller_json.is_empty());
}

#[test]
fn adaptive_quality_gate_is_bounded_and_requires_safe_passing_score() {
    assert_eq!(
        adaptive_quality_repair_budget(PromptVerification::Minimal),
        0
    );
    assert_eq!(
        adaptive_quality_repair_budget(PromptVerification::Evidence),
        1
    );
    assert_eq!(
        adaptive_quality_repair_budget(PromptVerification::Adversarial),
        2
    );

    let mut gate = CollaborationQualityPayload {
        pass: true,
        score: ADAPTIVE_QUALITY_PASS_SCORE,
        issues: Vec::new(),
        safety_violations: 0,
    };
    assert!(adaptive_quality_gate_passes(&gate));
    gate.score = ADAPTIVE_QUALITY_PASS_SCORE - 0.01;
    assert!(!adaptive_quality_gate_passes(&gate));
    gate.score = 1.0;
    gate.safety_violations = 1;
    assert!(!adaptive_quality_gate_passes(&gate));
    gate.safety_violations = 0;
    gate.score = 1.01;
    assert!(!adaptive_quality_gate_passes(&gate));
    gate.score = f32::NAN;
    assert!(!adaptive_quality_gate_passes(&gate));
}

#[test]
fn adaptive_quality_search_preserves_the_safest_highest_scoring_anchor() {
    let anchor = CollaborationQualityPayload {
        pass: false,
        score: 0.74,
        issues: vec!["one remaining issue".to_string()],
        safety_violations: 0,
    };
    let regressed_repair = CollaborationQualityPayload {
        pass: true,
        score: 0.96,
        issues: Vec::new(),
        safety_violations: 1,
    };
    assert!(!adaptive_quality_candidate_is_better(
        &regressed_repair,
        &anchor
    ));

    let improved_repair = CollaborationQualityPayload {
        pass: true,
        score: ADAPTIVE_QUALITY_PASS_SCORE,
        issues: Vec::new(),
        safety_violations: 0,
    };
    assert!(adaptive_quality_candidate_is_better(
        &improved_repair,
        &anchor
    ));
}

#[test]
fn adaptive_quality_handoff_preserves_issues_and_fails_closed_on_safety() {
    let unresolved = AdaptiveQualityGateResult {
        output: "candidate guidance".to_string(),
        score: 0.61,
        safety_violations: 0,
        passed: false,
        issues: vec!["verify the generated artifact".to_string()],
    };
    let handoff = adaptive_quality_handoff(&unresolved).expect("safe issues should be delegated");
    assert!(handoff.contains("INTERNAL QUALITY HANDOFF"));
    assert!(handoff.contains("verify the generated artifact"));
    assert!(handoff.contains("candidate guidance"));

    let passed = AdaptiveQualityGateResult {
        passed: true,
        issues: Vec::new(),
        score: 0.9,
        ..unresolved
    };
    assert_eq!(
        adaptive_quality_handoff(&passed).expect("passing guidance should flow through"),
        "candidate guidance"
    );

    let unsafe_result = AdaptiveQualityGateResult {
        safety_violations: 1,
        ..passed
    };
    let error = adaptive_quality_handoff(&unsafe_result)
        .expect_err("safety violations must stop the workflow");
    assert!(error.starts_with(WORKFLOW_SAFETY_ERROR_PREFIX));
}

#[test]
fn transient_provider_failures_are_retryable_but_invalid_requests_are_not() {
    let broken_pipe = ModelError::new("failed to configure curl: Broken pipe (os error 32)");
    let timeout =
        ModelError::new("model stream timed out after 180 seconds without receiving data");
    let invalid = ModelError::with_status(
        400,
        "invalid_request_error: Unexpected item type in content",
    );
    assert!(broken_pipe.is_retryable());
    assert!(timeout.is_retryable());
    assert!(!invalid.is_retryable());
    assert_eq!(
        exhausted_model_transport_error_stop_reason(&broken_pipe),
        Some(RunStopReason::ProviderUnavailable)
    );
    assert_eq!(exhausted_model_transport_error_stop_reason(&invalid), None);
}

#[test]
fn context_estimate_accounts_for_multibyte_text_and_prompt_reserve() {
    let ascii = test_message(MessageRole::User, "abcdefgh");
    let chinese = test_message(MessageRole::User, "你好世界你好世界");
    assert!(estimate_message_tokens(&chinese) > estimate_message_tokens(&ascii));

    let mut image_message = test_message(MessageRole::User, "inspect these images");
    image_message.metadata.insert(
        "image_paths".to_string(),
        "/tmp/one.png\n/tmp/two.png".to_string(),
    );
    assert!(
        estimate_message_tokens(&image_message)
            >= estimate_text_tokens_for_context("inspect these images") + 2_048
    );

    let large_history = vec![test_message(MessageRole::User, "a".repeat(220_000))];
    let plan = session_compaction_plan(&large_history, 100_000);
    assert!(plan.should_compact);
    assert!(plan.estimated_request_tokens > plan.estimated_history_tokens);
}

#[test]
fn context_checkpoint_reuse_has_bounded_hysteresis() {
    let mut history = (0..40)
        .map(|index| {
            test_message(
                if index % 2 == 0 {
                    MessageRole::User
                } else {
                    MessageRole::Assistant
                },
                format!("message {index}"),
            )
        })
        .collect::<Vec<_>>();
    let checkpoint = ValidatedContextCheckpoint {
        text: "verified checkpoint".to_string(),
        covered_messages: 8,
    };
    let plan = session_compaction_plan(&history, 32_000);
    assert!(context_checkpoint_is_within_reuse_window(
        &checkpoint,
        &history,
        plan,
        32_000,
    ));

    history.extend(
        (40..58).map(|index| test_message(MessageRole::Assistant, format!("message {index}"))),
    );
    let plan = session_compaction_plan(&history, 32_000);
    assert!(!context_checkpoint_is_within_reuse_window(
        &checkpoint,
        &history,
        plan,
        32_000,
    ));
}

#[test]
fn recent_context_starts_on_a_complete_user_turn() {
    let history = vec![
        test_message(MessageRole::User, "old request"),
        test_message(MessageRole::Assistant, "old answer"),
        test_message(MessageRole::Tool, "old tool evidence"),
        test_message(MessageRole::User, "latest request"),
        test_message(MessageRole::Assistant, "latest answer"),
    ];
    let budget = estimate_message_tokens(&history[3]) + estimate_message_tokens(&history[4]);
    let (start, tokens) = recent_history_start(&history, budget);

    assert_eq!(start, 3);
    assert!(is_user_turn_start(&history[start]));
    assert_eq!(tokens, budget);

    let (narrow_start, _) = recent_history_start(
        &history,
        estimate_message_tokens(history.last().expect("latest message")),
    );
    assert_eq!(narrow_start, 3);
}

#[test]
fn context_checkpoint_events_stop_at_the_covered_message_prefix() {
    let session_id = "session-prefix";
    let event = |sequence: u64, kind: EventKind, role: Option<&str>, content: Option<&str>| {
        let mut metadata = [("session_id".to_string(), session_id.to_string())]
            .into_iter()
            .collect::<Metadata>();
        if let Some(role) = role {
            metadata.insert("role".to_string(), role.to_string());
        }
        if let Some(content) = content {
            metadata.insert("content".to_string(), content.to_string());
        }
        Event {
            id: EventId(format!("event-{sequence}")),
            task_id: phase16_task_id(),
            timestamp_ms: sequence,
            sequence,
            kind,
            summary: format!("event {sequence}"),
            metadata,
        }
    };
    let events = vec![
        event(1, EventKind::MessageAdded, Some("user"), Some("first")),
        event(2, EventKind::ToolCallFinished, None, None),
        event(
            3,
            EventKind::MessageAdded,
            Some("assistant"),
            Some("answer"),
        ),
        event(4, EventKind::TaskStatusChanged, None, None),
        event(5, EventKind::MessageAdded, Some("user"), Some("retained")),
    ];

    let covered = context_events_for_covered_history_prefix(&events, 2);

    assert_eq!(covered.len(), 3);
    assert_eq!(covered.last().map(|event| event.sequence), Some(3));
    assert!(covered
        .iter()
        .all(|event| event.metadata.get("content").map(String::as_str) != Some("retained")));
}

#[test]
fn installed_app_data_is_user_scoped_and_overrideable() {
    let home = PathBuf::from("/Users/new-cindx-user");
    let expected = home
        .join("Library")
        .join("Application Support")
        .join("Cindx");
    assert_eq!(app_data_root_for(None, Some(home)), expected);

    let override_root = PathBuf::from("/tmp/cindx-portable-data");
    assert_eq!(
        app_data_root_for(Some(override_root.clone()), None),
        override_root
    );
    assert_eq!(database_path(), app_data_root().join("state.sqlite3"));
}

#[test]
fn attachment_paths_stay_inside_project_managed_storage() {
    let root = std::env::temp_dir().join(format!(
        "cindx-attachment-test-{}-{}",
        std::process::id(),
        current_time_millis()
    ));
    let attachment_dir = root.join(".cindx/attachments/session-a");
    fs::create_dir_all(&attachment_dir).expect("attachment directory should exist");
    let attachment = attachment_dir.join("image.png");
    fs::write(&attachment, b"png").expect("attachment should write");
    let outside = root.join("outside.png");
    fs::write(&outside, b"png").expect("outside fixture should write");

    assert_eq!(
        validated_attachment_path(&root, &attachment.display().to_string())
            .expect("managed attachment should validate"),
        fs::canonicalize(&attachment).expect("attachment should resolve")
    );
    assert!(validated_attachment_path(&root, &outside.display().to_string()).is_err());
    assert_eq!(safe_attachment_name("../nested/screen.png"), "screen.png");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn user_message_projection_preserves_attachment_metadata() {
    let attachment = AgentAttachmentView {
        id: "attachment-1".to_string(),
        name: "screen.png".to_string(),
        path: "/tmp/cindx/screen.png".to_string(),
        mime_type: "image/png".to_string(),
        size_bytes: 1_024,
    };
    let mut metadata = [
        ("role".to_string(), "user".to_string()),
        ("content".to_string(), "Review this screenshot".to_string()),
        ("queue_id".to_string(), "steer-queue-1".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    add_attachment_metadata(&mut metadata, std::slice::from_ref(&attachment));
    let event = Event {
        id: EventId("message-with-attachment".to_string()),
        task_id: phase16_task_id(),
        sequence: 9,
        timestamp_ms: 100,
        kind: EventKind::MessageAdded,
        summary: "user message".to_string(),
        metadata,
    };

    let message = message_view_from_event(&event).expect("message should project");
    assert_eq!(message.attachments.len(), 1);
    assert_eq!(message.attachments[0].id, attachment.id);
    assert_eq!(message.attachments[0].name, attachment.name);
    assert_eq!(message.attachments[0].path, attachment.path);
    assert_eq!(message.attachments[0].mime_type, attachment.mime_type);
    assert_eq!(message.attachments[0].size_bytes, attachment.size_bytes);
    assert_eq!(message.queue_id.as_deref(), Some("steer-queue-1"));
}

#[test]
fn assistant_reasoning_control_only_message_is_sanitized_for_chat() {
    let event = Event {
        id: EventId("assistant-reasoning-control".to_string()),
        task_id: phase16_task_id(),
        sequence: 10,
        timestamp_ms: 100,
        kind: EventKind::MessageAdded,
        summary: "assistant message".to_string(),
        metadata: [
            ("role".to_string(), "assistant".to_string()),
            ("content".to_string(), "</think>".to_string()),
            ("raw_tool_calls_json".to_string(), "[]".to_string()),
        ]
        .into_iter()
        .collect(),
    };

    let chat_message = message_view_from_event(&event).expect("message should project");
    assert_eq!(chat_message.content, "");
    let transcript_message = message_from_event(&event).expect("tool turn should remain");
    assert_eq!(transcript_message.content, "");
    assert!(transcript_message
        .metadata
        .contains_key("raw_tool_calls_json"));
}

#[test]
fn assistant_dsml_tool_protocol_is_sanitized_for_chat_and_transcript() {
    let event = Event {
        id: EventId("assistant-dsml-tool-protocol".to_string()),
        task_id: phase16_task_id(),
        sequence: 11,
        timestamp_ms: 100,
        kind: EventKind::MessageAdded,
        summary: "assistant message".to_string(),
        metadata: [
            ("role".to_string(), "assistant".to_string()),
            (
                "content".to_string(),
                concat!(
                    "<｜DSML｜tool_calls>",
                    "<｜DSML｜invoke name=\"shell_run\">",
                    "<｜DSML｜parameter name=\"command\" string=\"true\">pwd</｜DSML｜parameter>",
                    "</｜DSML｜invoke>",
                    "</｜DSML｜tool_calls>"
                )
                .to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    };

    let chat_message = message_view_from_event(&event).expect("message should project");
    assert_eq!(chat_message.content, "");
    let transcript_message = message_from_event(&event).expect("message should remain");
    assert_eq!(transcript_message.content, "");
}

#[test]
fn computer_screenshot_becomes_the_primary_visual_artifact() {
    let root = std::env::temp_dir().join(format!(
        "cindx-screenshot-test-{}-{}",
        std::process::id(),
        current_time_millis()
    ));
    fs::create_dir_all(root.join(".cindx/computer-actions"))
        .expect("computer action directory should exist");
    fs::write(root.join(".cindx/computer-actions/request.json"), b"{}")
        .expect("request fixture should write");
    fs::write(root.join(".cindx/computer-actions/screen.png"), b"png")
        .expect("screenshot fixture should write");
    let mut result = ToolResult::text(
        agent_core::ToolCallId("computer-test".to_string()),
        ToolOutcomeStatus::Succeeded,
        "captured",
        [
            (
                "artifact_path".to_string(),
                ".cindx/computer-actions/request.json".to_string(),
            ),
            (
                "screenshot_path".to_string(),
                ".cindx/computer-actions/screen.png".to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    );

    materialize_tool_result_artifacts(&mut result, &root).expect("artifacts should materialize");

    assert!(result
        .metadata
        .get("artifact_path")
        .is_some_and(|path| path.ends_with("screen.png")));
    assert_eq!(tool_result_image_paths(&result).len(), 1);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn repeated_image_outputs_keep_distinct_immutable_versions() {
    let root = std::env::temp_dir().join(format!(
        "cindx-versioned-image-test-{}-{}",
        std::process::id(),
        current_time_millis()
    ));
    let relative_path = "generated-images/cat.png";
    let source = root.join(relative_path);
    fs::create_dir_all(source.parent().expect("image parent should exist"))
        .expect("image directory should be created");

    let materialize = |call_id: &str, bytes: &[u8]| {
        fs::write(&source, bytes).expect("image version should write");
        let mut result = ToolResult::text(
            agent_core::ToolCallId(call_id.to_string()),
            ToolOutcomeStatus::Succeeded,
            "generated",
            [("artifact_path".to_string(), relative_path.to_string())]
                .into_iter()
                .collect(),
        );
        result.artifacts.push(ToolArtifact {
            path: relative_path.to_string(),
            mime_type: Some("image/png".to_string()),
            title: Some("Generated image".to_string()),
        });
        materialize_tool_result_artifacts(&mut result, &root)
            .expect("image artifact should materialize");
        result
    };

    let first = materialize("image-call-one", b"version-one");
    let first_snapshot = first.metadata["artifact_path"].clone();
    let second = materialize("image-call-two", b"version-two");
    let second_snapshot = second.metadata["artifact_path"].clone();

    assert_eq!(first.metadata["source_path"], relative_path);
    assert_eq!(second.metadata["source_path"], relative_path);
    assert_ne!(first_snapshot, second_snapshot);
    assert_eq!(
        fs::read(root.join(&first_snapshot)).expect("first snapshot should remain readable"),
        b"version-one"
    );
    assert_eq!(
        fs::read(root.join(&second_snapshot)).expect("second snapshot should remain readable"),
        b"version-two"
    );
    assert_eq!(first.artifacts.len(), 1);
    assert_eq!(second.artifacts.len(), 1);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn oversized_structured_tool_output_is_materialized_without_inline_duplication() {
    let root = std::env::temp_dir().join(format!(
        "cindx-structured-output-test-{}-{}",
        std::process::id(),
        current_time_millis()
    ));
    fs::create_dir_all(&root).expect("test workspace should exist");
    let structured = serde_json::json!({ "payload": "x".repeat(300 * 1024) }).to_string();
    let mut result = ToolResult::text(
        agent_core::ToolCallId("call/unsafe".to_string()),
        ToolOutcomeStatus::Succeeded,
        "bounded preview",
        Metadata::new(),
    );
    result.structured_output_json = Some(structured.clone());

    materialize_tool_result_artifacts(&mut result, &root)
        .expect("structured output should materialize");

    let path = result
        .metadata
        .get("structured_output_path")
        .expect("structured output should expose its artifact path");
    assert!(path.ends_with("call_unsafe-structured.json"));
    assert_eq!(
        fs::read_to_string(path).expect("structured artifact should read"),
        structured
    );
    assert!(result
        .structured_output_json
        .as_deref()
        .is_some_and(|value| value.len() < 512));
    assert!(result
        .metadata
        .get("structured_output")
        .is_some_and(|value| value.len() < 512));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn runtime_status_exposes_expected_modes() {
    let status = runtime_status_for_root(workspace_root());

    assert_eq!(status.app_version, env!("CARGO_PKG_VERSION"));
    assert!(status
        .orchestration_modes
        .contains(&"plan_execute_review".to_string()));
    assert!(status.registered_tools.contains(&"shell.run".to_string()));
}

#[test]
fn quit_confirmation_preference_requires_explicit_suppression() {
    assert!(quit_confirmation_suppressed_text(
        "theme=system\nskip_quit_confirmation=true\n"
    ));
    assert!(!quit_confirmation_suppressed_text(
        "skip_quit_confirmation=false\n"
    ));
}

#[test]
fn phase3_mock_permission_round_trips_through_store() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let state = request_mock_permission_in_store(&mut store).expect("request should save");

    assert_eq!(state.permissions.len(), 1);
    assert_eq!(state.permissions[0].status, "pending");

    let request_id = state.permissions[0].id.clone();
    let resolved = resolve_permission_in_store(&mut store, &request_id, "deny")
        .expect("resolution should save");

    assert_eq!(resolved.permissions.len(), 1);
    assert_eq!(resolved.permissions[0].status, "resolved");
    assert_eq!(resolved.permissions[0].decision.as_deref(), Some("deny"));
    assert!(resolved
        .timeline
        .iter()
        .any(|entry| entry.label == "Permission resolved"));
}

#[test]
fn session_read_model_advances_from_only_new_events() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let session_id = "session-read-model";
    let run_id = "run-read-model";
    let context = [
        ("project_id".to_string(), "project-a".to_string()),
        ("project_name".to_string(), "Project A".to_string()),
        ("session_id".to_string(), session_id.to_string()),
        ("session_name".to_string(), "Read model".to_string()),
        ("agent_run_id".to_string(), run_id.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();

    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        metadata_with_context(
            [
                ("prompt".to_string(), "Inspect the workspace".to_string()),
                ("context_window_tokens".to_string(), "128000".to_string()),
            ]
            .into_iter()
            .collect(),
            &context,
        ),
    )
    .expect("run start should append");
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "Inspect the workspace",
        context.clone(),
    )
    .expect("user message should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::ModelRequestFinished,
        "Agent model turn finished",
        metadata_with_context(
            [("prompt_tokens".to_string(), "640".to_string())]
                .into_iter()
                .collect(),
            &context,
        ),
    )
    .expect("model turn should append");

    let initial = load_agent_session_read_model(&mut store, session_id)
        .expect("initial read model should build");
    assert_eq!(initial.event_count, 3);
    assert_eq!(initial.state.turn_count, 1);
    assert_eq!(initial.state.context_tokens_used, 640);
    assert_eq!(initial.state.status, "running");

    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::Assistant,
        "Workspace inspected",
        context.clone(),
    )
    .expect("assistant message should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        context,
    )
    .expect("completion should append");

    let updated = load_agent_session_read_model(&mut store, session_id)
        .expect("read model should apply the delta");
    assert_eq!(updated.event_count, 5);
    assert_eq!(updated.revision, initial.revision + 2);
    assert_eq!(updated.state.status, "completed");
    assert_eq!(
        updated.state.latest_answer.as_deref(),
        Some("Workspace inspected")
    );
    assert!(updated.state.can_retry);

    let persisted = store
        .load_read_model(AGENT_SESSION_READ_MODEL_NAMESPACE, session_id)
        .expect("persisted read model should load")
        .expect("persisted read model should exist");
    assert_eq!(persisted.revision, updated.revision);
}

#[test]
fn project_memory_read_model_persists_deduplicated_cross_session_requirements() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    for (session_id, run_id) in [
        ("session-memory-a", "run-memory-a"),
        ("session-memory-b", "run-memory-b"),
    ] {
        let context = [
            ("project_id".to_string(), "project-memory".to_string()),
            ("session_id".to_string(), session_id.to_string()),
            ("agent_run_id".to_string(), run_id.to_string()),
        ]
        .into_iter()
        .collect::<Metadata>();
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task started",
            context.clone(),
        )
        .expect("run should start");
        append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            MessageRole::User,
            "Keep effort selection scoped to each session",
            context.clone(),
        )
        .expect("user requirement should append");
        append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            MessageRole::Assistant,
            "Effort is now stored per session.",
            context.clone(),
        )
        .expect("assistant outcome should append");
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task completed",
            context,
        )
        .expect("run should complete");
    }

    let ledger = load_project_memory_ledger(&mut store, "project-memory")
        .expect("memory read model should build");
    assert_eq!(ledger.project_id, "project-memory");
    assert_eq!(
        ledger
            .records
            .iter()
            .filter(|record| record.kind == agent_memory::MemoryKind::Requirement)
            .count(),
        1
    );
    let requirement = ledger
        .records
        .iter()
        .find(|record| record.kind == agent_memory::MemoryKind::Requirement)
        .expect("requirement memory should exist");
    assert_eq!(requirement.source_session_ids.len(), 2);
    let requirement_id = requirement.id.clone();
    let persisted = store
        .load_read_model(AGENT_MEMORY_READ_MODEL_NAMESPACE, "project-memory")
        .expect("memory read model should load")
        .expect("memory read model should exist");
    assert_eq!(persisted.revision, ledger.revision);

    let use_context = [
        ("project_id".to_string(), "project-memory".to_string()),
        ("session_id".to_string(), "session-memory-c".to_string()),
        ("agent_run_id".to_string(), "run-memory-c".to_string()),
        ("memory_ids".to_string(), requirement_id.clone()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let used = record_project_memory_observed_use(
        &mut store,
        &phase16_task_id(),
        &use_context,
        "Kept effort selection scoped to each session.",
    )
    .expect("memory utilization should persist");
    assert_eq!(used, 1);
    let updated = load_project_memory_ledger(&mut store, "project-memory")
        .expect("updated memory ledger should load");
    assert_eq!(
        updated
            .records
            .iter()
            .find(|record| record.id == requirement_id)
            .map(|record| record.observed_use_count),
        Some(1)
    );
}

#[test]
fn project_memory_read_model_ignores_unrelated_project_events() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let project_a = [
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), "session-a".to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "Keep project A requirements isolated",
        project_a.clone(),
    )
    .expect("project A message should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        project_a,
    )
    .expect("project A run should complete");
    let initial =
        load_project_memory_ledger(&mut store, "project-a").expect("project A ledger should build");

    let project_b = [
        ("project_id".to_string(), "project-b".to_string()),
        ("session_id".to_string(), "session-b".to_string()),
        ("agent_run_id".to_string(), "run-b".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "Unrelated project B requirement",
        project_b.clone(),
    )
    .expect("project B message should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        project_b,
    )
    .expect("project B run should complete");

    let unchanged = load_project_memory_ledger(&mut store, "project-a")
        .expect("project A ledger should remain current");
    assert_eq!(unchanged.revision, initial.revision);
    assert_eq!(unchanged.event_count, initial.event_count);
    assert_eq!(unchanged.records, initial.records);
}

#[test]
fn project_memory_feedback_keeps_project_scoped_revision() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let project_a = [
        ("project_id".to_string(), "project-feedback-a".to_string()),
        ("session_id".to_string(), "session-feedback-a".to_string()),
        ("agent_run_id".to_string(), "run-feedback-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "Keep memory feedback isolated by project",
        project_a.clone(),
    )
    .expect("project A message should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        project_a,
    )
    .expect("project A run should complete");
    let initial = load_project_memory_ledger(&mut store, "project-feedback-a")
        .expect("project A ledger should build");
    let memory_id = initial
        .records
        .iter()
        .find(|record| record.kind == agent_memory::MemoryKind::Requirement)
        .expect("requirement memory should exist")
        .id
        .clone();

    let project_b = [
        ("project_id".to_string(), "project-feedback-b".to_string()),
        ("session_id".to_string(), "session-feedback-b".to_string()),
        ("agent_run_id".to_string(), "run-feedback-b".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "Unrelated project B requirement",
        project_b,
    )
    .expect("project B message should append");

    let use_context = [
        ("project_id".to_string(), "project-feedback-a".to_string()),
        ("session_id".to_string(), "session-feedback-a-2".to_string()),
        ("agent_run_id".to_string(), "run-feedback-a-2".to_string()),
        ("memory_ids".to_string(), memory_id.clone()),
    ]
    .into_iter()
    .collect::<Metadata>();
    assert_eq!(
        record_project_memory_observed_use(
            &mut store,
            &phase16_task_id(),
            &use_context,
            "Kept memory feedback isolated by project.",
        )
        .expect("memory feedback should persist"),
        1
    );

    let persisted = store
        .load_read_model(AGENT_MEMORY_READ_MODEL_NAMESPACE, "project-feedback-a")
        .expect("memory read model should load")
        .expect("memory read model should exist");
    let scoped_revision = store
        .event_revision_by_metadata(&phase16_task_id(), "project_id", "project-feedback-a")
        .expect("project revision should load");
    assert_eq!(persisted.revision, scoped_revision.latest_sequence);
    let updated = load_project_memory_ledger(&mut store, "project-feedback-a")
        .expect("project A ledger should remain incremental");
    assert_eq!(updated.event_count, scoped_revision.event_count);
    assert_eq!(
        updated
            .records
            .iter()
            .find(|record| record.id == memory_id)
            .map(|record| record.observed_use_count),
        Some(1)
    );
}

#[test]
fn project_memory_projection_persists_and_searches_real_lancedb_vectors() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = [
        (
            "project_id".to_string(),
            "project-vector-memory".to_string(),
        ),
        (
            "session_id".to_string(),
            "session-vector-memory".to_string(),
        ),
        ("agent_run_id".to_string(), "run-vector-memory".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "Keep the inspector frosted and translucent",
        context.clone(),
    )
    .expect("user requirement should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        context,
    )
    .expect("run should complete");
    let ledger = load_project_memory_ledger(&mut store, "project-vector-memory")
        .expect("memory ledger should build");
    let root = std::env::temp_dir().join(format!(
        "cindx-memory-vector-test-{}-{}",
        std::process::id(),
        current_time_millis()
    ));

    let fallback = refresh_project_memory_vector_index(&root, &ProviderConfig::default(), &ledger)
        .expect("memory vectors should persist");
    assert!(fallback.is_none());
    let database_path = memory_lancedb_database_path_for(&root, "project-vector-memory");
    assert!(lancedb_index_exists(&database_path));
    let results = search_lancedb_index(
        &database_path,
        &local_query_embedding("frosted translucent inspector"),
        4,
    )
    .expect("memory vectors should search");
    assert_eq!(
        results.first().map(|result| result.chunk.id.as_str()),
        Some(ledger.records[0].id.as_str())
    );
    let manifest = load_memory_vector_manifest(&memory_lancedb_manifest_path_for(
        &root,
        "project-vector-memory",
    ))
    .expect("manifest should load")
    .expect("manifest should exist");
    assert_eq!(manifest.embedding_backend, "local");
    assert_eq!(manifest.record_count, ledger.records.len());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn project_memory_never_persists_raw_secrets() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = [
        (
            "project_id".to_string(),
            "project-memory-secret".to_string(),
        ),
        (
            "session_id".to_string(),
            "session-memory-secret".to_string(),
        ),
        ("agent_run_id".to_string(), "run-memory-secret".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "Always use sk-1234567890abcdef for this project",
        context.clone(),
    )
    .expect("redacted message should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        context,
    )
    .expect("run should complete");

    let ledger = load_project_memory_ledger(&mut store, "project-memory-secret")
        .expect("memory ledger should build");
    let payload = serde_json::to_string(&ledger).expect("memory ledger should serialize");

    assert!(!payload.contains("sk-1234567890abcdef"));
    assert!(payload.contains("[REDACTED]"));
}

#[test]
fn session_history_page_reports_a_stable_older_cursor() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let session_id = "session-history-page";
    for index in 0..7 {
        append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            if index % 2 == 0 {
                MessageRole::User
            } else {
                MessageRole::Assistant
            },
            &format!("message-{index}"),
            [("session_id".to_string(), session_id.to_string())]
                .into_iter()
                .collect(),
        )
        .expect("message should append");
    }

    let latest = store
        .list_by_task_and_metadata_before(&phase16_task_id(), "session_id", session_id, u64::MAX, 3)
        .expect("latest page should load");
    assert_eq!(
        latest
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        vec![5, 6, 7]
    );

    let older = store
        .list_by_task_and_metadata_before(
            &phase16_task_id(),
            "session_id",
            session_id,
            latest[0].sequence,
            3,
        )
        .expect("older page should load");
    assert_eq!(
        older.iter().map(|event| event.sequence).collect::<Vec<_>>(),
        vec![2, 3, 4]
    );
    assert!(store
        .has_task_metadata_event_before(
            &phase16_task_id(),
            "session_id",
            session_id,
            older[0].sequence,
        )
        .expect("older cursor should be checked"));
}

#[test]
fn routing_telemetry_read_model_deduplicates_completed_runs() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let run_context = [
        ("session_id".to_string(), "session-router".to_string()),
        ("agent_run_id".to_string(), "run-router-1".to_string()),
        ("task_class".to_string(), "coding".to_string()),
        (
            "collaboration_policy".to_string(),
            "plan_execute_review".to_string(),
        ),
        ("requested_policy".to_string(), "auto_router".to_string()),
        ("agent_model".to_string(), "model-a".to_string()),
        ("routing_signature".to_string(), "coding:3".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        run_context.clone(),
    )
    .expect("run should start");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::ModelRequestFinished,
        "Agent model turn finished",
        metadata_with_context(
            [("total_tokens".to_string(), "900".to_string())]
                .into_iter()
                .collect(),
            &run_context,
        ),
    )
    .expect("model telemetry should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        run_context.clone(),
    )
    .expect("run should complete");

    let initial =
        load_routing_telemetry_read_model(&mut store).expect("routing read model should build");
    assert_eq!(initial.len(), 1);
    assert_eq!(initial[0].selected_model, "model-a");
    assert_eq!(initial[0].cost_proxy, 900);

    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "unrelated follow-up",
        [("session_id".to_string(), "session-router".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("message should append");
    let updated =
        load_routing_telemetry_read_model(&mut store).expect("routing read model should advance");
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].selected_model, "model-a");
}

#[test]
fn read_only_runtime_snapshots_do_not_write_projection_caches() {
    let root = std::env::temp_dir().join(format!(
        "cindx-read-only-snapshot-test-{}-{}",
        std::process::id(),
        current_time_millis()
    ));
    fs::create_dir_all(&root).expect("snapshot test directory should exist");
    let database = root.join("state.sqlite3");
    drop(SqliteStore::open(&database).expect("writable store should initialize"));

    let mut store = SqliteStore::open_read_only(&database).expect("read-only store should open");
    assert!(load_routing_telemetry_read_model_snapshot(&mut store)
        .expect("routing snapshot should remain read-only")
        .is_empty());
    assert!(
        load_project_memory_ledger_snapshot(&mut store, "project-read-only")
            .expect("memory snapshot should remain read-only")
            .records
            .is_empty()
    );
    assert!(
        load_agent_session_read_model_snapshot(&store, "session-read-only")
            .expect("session snapshot should remain read-only")
            .state
            .messages
            .is_empty()
    );

    drop(store);
    fs::remove_dir_all(root).expect("snapshot test directory should be removed");
}

#[test]
fn prompt_evolution_read_model_only_indexes_evaluation_evidence() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let genome = ConductorPromptGenome::seed_for_effort("auto");
    let observation = PromptEvolutionObservation {
        profile_id: genome.id.clone(),
        evaluation_id: "pair-1".to_string(),
        case_id: "case-1".to_string(),
        opponent_profile_id: Some("challenger".to_string()),
        task_class: "coding".to_string(),
        split: PromptEvaluationSplit::Train,
        mode: PromptEvaluationMode::PairedShadow,
        format_valid: true,
        succeeded: true,
        quality_score: 0.9,
        latency_ms: 800,
        total_tokens: 500,
        estimated_cost_microusd: 0,
        safety_violations: 0,
        relative_reward: Some(0.2),
        step_credits: Vec::new(),
        reflection_packet: None,
    };
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Conductor pairwise evaluation",
        [
            ("prompt_effort".to_string(), "auto".to_string()),
            (
                "prompt_genome".to_string(),
                serde_json::to_string(&genome).expect("genome should serialize"),
            ),
            (
                "prompt_observation".to_string(),
                serde_json::to_string(&observation).expect("observation should serialize"),
            ),
        ]
        .into_iter()
        .collect(),
    )
    .expect("evaluation should append");

    let initial = load_prompt_evolution_read_model(&mut store)
        .expect("prompt evolution read model should build");
    assert_eq!(initial.genomes.len(), 1);
    assert_eq!(initial.observations.len(), 1);

    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "ordinary conversation",
        [("session_id".to_string(), "session-a".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("message should append");
    let updated = load_prompt_evolution_read_model(&mut store)
        .expect("prompt evolution read model should advance");
    assert_eq!(updated.genomes.len(), 1);
    assert_eq!(updated.observations.len(), 1);
    assert!(updated.revision > initial.revision);
}

#[test]
fn prompt_evolution_read_model_indexes_dataset_readiness_without_case_content() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Conductor offline dataset selected",
        [
            ("prompt_effort".to_string(), "auto".to_string()),
            ("project_id".to_string(), "project-a".to_string()),
            ("dataset_sha256".to_string(), "dataset-v1".to_string()),
            ("dataset_case_count".to_string(), "2".to_string()),
            ("dataset_train_count".to_string(), "2".to_string()),
            ("dataset_holdout_count".to_string(), "0".to_string()),
            (
                "dataset_status".to_string(),
                "insufficient_cases".to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    )
    .expect("dataset readiness should append");

    let initial = load_prompt_evolution_read_model(&mut store)
        .expect("prompt evolution read model should build");
    let key = prompt_dataset_key("auto", "project-a");
    let dataset = initial
        .datasets
        .get(&key)
        .expect("dataset readiness should be indexed");
    assert_eq!(dataset.case_count, 2);
    assert_eq!(dataset.status, "insufficient_cases");
    assert!(dataset.selected_case_id.is_none());

    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Conductor offline dataset selected",
        [
            ("prompt_effort".to_string(), "auto".to_string()),
            ("project_id".to_string(), "project-a".to_string()),
            ("dataset_sha256".to_string(), "dataset-v2".to_string()),
            ("dataset_case_count".to_string(), "3".to_string()),
            ("dataset_train_count".to_string(), "2".to_string()),
            ("dataset_holdout_count".to_string(), "1".to_string()),
            ("dataset_status".to_string(), "ready".to_string()),
            ("selected_case_id".to_string(), "case-3".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("updated dataset readiness should append");

    let updated = load_prompt_evolution_read_model(&mut store)
        .expect("prompt evolution read model should advance");
    let dataset = updated
        .datasets
        .get(&key)
        .expect("latest dataset readiness should replace the prior snapshot");
    assert_eq!(dataset.digest, "dataset-v2");
    assert_eq!(dataset.case_count, 3);
    assert_eq!(dataset.selected_case_id.as_deref(), Some("case-3"));
}

#[test]
fn prompt_evolution_readiness_reports_each_scientific_gate() {
    assert_eq!(
        prompt_evolution_readiness(false, true, 0, 0, 0, 0, false, "stable"),
        "not_applicable"
    );
    assert_eq!(
        prompt_evolution_readiness(true, true, 2, 0, 0, 0, false, "stable"),
        "collecting_dataset"
    );
    assert_eq!(
        prompt_evolution_readiness(true, true, 3, 2, 0, 0, false, "stable"),
        "collecting_train_evidence"
    );
    assert_eq!(
        prompt_evolution_readiness(true, true, 3, 3, 3, 0, false, "stable"),
        "collecting_holdout_evidence"
    );
    assert_eq!(
        prompt_evolution_readiness(true, true, 3, 3, 4, 0, false, "stable"),
        "selecting_frontier"
    );
    assert_eq!(
        prompt_evolution_readiness(true, true, 3, 3, 4, 1, false, "canary"),
        "canary"
    );
}

#[test]
fn prompt_evolution_read_model_restores_an_atomic_observation_pair() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let genome_a = ConductorPromptGenome::seed_for_effort("auto");
    let genome_b = ConductorPromptGenome {
        id: "atomic-challenger".to_string(),
        ..genome_a.clone()
    };
    let observation_a = PromptEvolutionObservation {
        profile_id: genome_a.id.clone(),
        evaluation_id: "atomic-pair".to_string(),
        case_id: "atomic-case".to_string(),
        opponent_profile_id: Some(genome_b.id.clone()),
        task_class: "coding".to_string(),
        split: PromptEvaluationSplit::Holdout,
        mode: PromptEvaluationMode::ReplayExecution,
        format_valid: true,
        succeeded: true,
        quality_score: 0.8,
        latency_ms: 100,
        total_tokens: 200,
        estimated_cost_microusd: 0,
        safety_violations: 0,
        relative_reward: Some(0.2),
        step_credits: Vec::new(),
        reflection_packet: None,
    };
    let observation_b = PromptEvolutionObservation {
        profile_id: genome_b.id.clone(),
        opponent_profile_id: Some(genome_a.id.clone()),
        quality_score: 0.6,
        relative_reward: Some(-0.2),
        ..observation_a.clone()
    };
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Conductor pairwise evaluation",
        [
            ("prompt_effort".to_string(), "auto".to_string()),
            (
                "prompt_genomes".to_string(),
                serde_json::to_string(&[&genome_a, &genome_b]).expect("genomes should serialize"),
            ),
            (
                "prompt_observations".to_string(),
                serde_json::to_string(&[&observation_a, &observation_b])
                    .expect("observations should serialize"),
            ),
        ]
        .into_iter()
        .collect(),
    )
    .expect("atomic pair should append");

    let model = load_prompt_evolution_read_model(&mut store).expect("atomic pair should rebuild");
    assert_eq!(model.genomes.len(), 2);
    assert_eq!(model.observations.len(), 2);
    assert!(model.observations.iter().all(|(_, observation)| {
        observation.evaluation_id == "atomic-pair"
            && observation.mode == PromptEvaluationMode::ReplayExecution
    }));
}

#[test]
fn prompt_evolution_evidence_counts_ignore_legacy_plan_only_modes() {
    let profile_id = "seed-auto-v1";
    let observation =
        |evaluation_id: &str, mode: PromptEvaluationMode| PromptEvolutionObservation {
            profile_id: profile_id.to_string(),
            evaluation_id: evaluation_id.to_string(),
            case_id: evaluation_id.to_string(),
            opponent_profile_id: Some("challenger".to_string()),
            task_class: "coding".to_string(),
            split: if mode.is_replay() {
                PromptEvaluationSplit::Holdout
            } else {
                PromptEvaluationSplit::Train
            },
            mode,
            format_valid: true,
            succeeded: true,
            quality_score: 0.9,
            latency_ms: 100,
            total_tokens: 100,
            estimated_cost_microusd: 0,
            safety_violations: 0,
            relative_reward: Some(0.2),
            step_credits: Vec::new(),
            reflection_packet: None,
        };
    let observations = vec![
        observation("legacy-paired", PromptEvaluationMode::PairedShadow),
        observation("legacy-replay", PromptEvaluationMode::ReplayHoldout),
        observation("train", PromptEvaluationMode::PairedExecution),
        observation("train", PromptEvaluationMode::PairedExecution),
        observation("holdout", PromptEvaluationMode::ReplayExecution),
        observation("holdout", PromptEvaluationMode::ReplayExecution),
    ];

    assert_eq!(
        prompt_profile_evidence_counts(&observations, profile_id),
        (1, 1)
    );
}

#[test]
fn prompt_evolution_direct_evidence_does_not_reuse_a_weaker_opponent() {
    let observation = |evaluation_id: &str, opponent: &str| PromptEvolutionObservation {
        profile_id: "candidate".to_string(),
        evaluation_id: evaluation_id.to_string(),
        case_id: evaluation_id.to_string(),
        opponent_profile_id: Some(opponent.to_string()),
        task_class: "coding".to_string(),
        split: PromptEvaluationSplit::Holdout,
        mode: PromptEvaluationMode::ReplayExecution,
        format_valid: true,
        succeeded: true,
        quality_score: 0.9,
        latency_ms: 100,
        total_tokens: 100,
        estimated_cost_microusd: 0,
        safety_violations: 0,
        relative_reward: Some(0.4),
        step_credits: Vec::new(),
        reflection_packet: None,
    };
    let observations = vec![
        observation("weak-1", "weak-profile"),
        observation("weak-2", "weak-profile"),
        observation("stable-1", "stable-profile"),
        observation("stable-1", "stable-profile"),
    ];

    assert_eq!(
        prompt_direct_profile_evidence_counts(&observations, "candidate", "stable-profile"),
        (0, 1)
    );
}

#[test]
fn prompt_instance_pareto_seeds_are_stable_across_event_order() {
    let genome = ConductorPromptGenome::seed_for_effort("auto");
    let observation = |evaluation_id: &str, case_id: &str| PromptEvolutionObservation {
        profile_id: genome.id.clone(),
        evaluation_id: evaluation_id.to_string(),
        case_id: case_id.to_string(),
        opponent_profile_id: Some("challenger".to_string()),
        task_class: "coding".to_string(),
        split: PromptEvaluationSplit::Holdout,
        mode: PromptEvaluationMode::ReplayExecution,
        format_valid: true,
        succeeded: true,
        quality_score: 0.9,
        latency_ms: 100,
        total_tokens: 100,
        estimated_cost_microusd: 0,
        safety_violations: 0,
        relative_reward: Some(0.2),
        step_credits: Vec::new(),
        reflection_packet: None,
    };
    let forward = vec![
        observation("replay-a", "case-a"),
        observation("replay-b", "case-b"),
    ];
    let reversed = forward.iter().rev().cloned().collect::<Vec<_>>();
    let seeds = |observations: &[PromptEvolutionObservation]| {
        prompt_instance_pareto_scores(std::slice::from_ref(&genome), observations)
            .into_iter()
            .map(|score| (score.run_id, score.seed))
            .collect::<BTreeMap<_, _>>()
    };

    assert_eq!(seeds(&forward), seeds(&reversed));
    assert_ne!(seeds(&forward)["replay-a"], seeds(&forward)["replay-b"]);
}

#[test]
fn prompt_rollout_advances_by_evidence_and_rolls_back_on_regression() {
    let stable = ConductorPromptGenome::seed_for_effort("auto");
    let mut candidate = stable.clone();
    candidate.id = "candidate-auto".to_string();
    candidate.generation = 1;
    let mut model = PromptEvolutionReadModel {
        schema: PROMPT_EVOLUTION_READ_MODEL_NAMESPACE.to_string(),
        revision: 0,
        event_count: 0,
        genomes: Vec::new(),
        observations: Vec::new(),
        rollouts: BTreeMap::new(),
        datasets: BTreeMap::new(),
    };
    let evaluation = |comparisons| PromptEvolutionEvaluation {
        population: vec![stable.clone(), candidate.clone()],
        observations: Vec::new(),
        frontier_ids: [candidate.id.clone()].into_iter().collect(),
        champion_id: Some(candidate.id.clone()),
        champion_score: Some(0.8),
        champion_confidence: Some(PromptPromotionConfidence {
            comparisons,
            wins: comparisons,
            losses: 0,
            ties: 0,
            observed_win_rate: 1.0,
            wilson_lower_bound: 0.6,
        }),
        status: "exploring".to_string(),
        freeze_reason: None,
        stagnant_generations: 0,
        evaluated_generations: 1,
        next_mode: "explore".to_string(),
        next_profile: candidate.clone(),
        mutation_parent: None,
        mutation_trajectories: Vec::new(),
    };

    let direct_observation =
        |index: usize, split: PromptEvaluationSplit| PromptEvolutionObservation {
            profile_id: candidate.id.clone(),
            evaluation_id: format!("direct-stable-{index}"),
            case_id: format!("direct-case-{index}"),
            opponent_profile_id: Some(stable.id.clone()),
            task_class: "coding".to_string(),
            split,
            mode: match split {
                PromptEvaluationSplit::Train => PromptEvaluationMode::PairedExecution,
                PromptEvaluationSplit::Holdout => PromptEvaluationMode::ReplayExecution,
            },
            format_valid: true,
            succeeded: true,
            quality_score: 0.9,
            latency_ms: 100,
            total_tokens: 100,
            estimated_cost_microusd: 0,
            safety_violations: 0,
            relative_reward: Some(0.4),
            step_credits: Vec::new(),
            reflection_packet: None,
        };
    for index in 0..3 {
        model.observations.push((
            "auto".to_string(),
            direct_observation(index, PromptEvaluationSplit::Train),
        ));
    }
    for index in 0..4 {
        model.observations.push((
            "auto".to_string(),
            direct_observation(index + 3, PromptEvaluationSplit::Holdout),
        ));
    }

    let started = reconcile_prompt_rollout(&mut model, "auto", &evaluation(4));
    assert_eq!(started.canary_profile_id.as_deref(), Some("candidate-auto"));
    assert_eq!(started.canary_percent, 10);

    model.observations.push((
        "auto".to_string(),
        PromptEvolutionObservation {
            profile_id: candidate.id.clone(),
            evaluation_id: "live-canary-1".to_string(),
            case_id: "live-canary-1".to_string(),
            opponent_profile_id: None,
            task_class: "coding".to_string(),
            split: PromptEvaluationSplit::Train,
            mode: PromptEvaluationMode::Live,
            format_valid: true,
            succeeded: true,
            quality_score: 0.9,
            latency_ms: 100,
            total_tokens: 100,
            estimated_cost_microusd: 0,
            safety_violations: 0,
            relative_reward: None,
            step_credits: Vec::new(),
            reflection_packet: None,
        },
    ));
    for index in 0..2 {
        model.observations.push((
            "auto".to_string(),
            direct_observation(index + 7, PromptEvaluationSplit::Holdout),
        ));
    }
    let advanced = reconcile_prompt_rollout(&mut model, "auto", &evaluation(6));
    assert_eq!(advanced.canary_percent, 25);

    model.observations.push((
        "auto".to_string(),
        PromptEvolutionObservation {
            profile_id: candidate.id.clone(),
            evaluation_id: "live-canary-unsafe".to_string(),
            case_id: "live-canary-unsafe".to_string(),
            opponent_profile_id: None,
            task_class: "coding".to_string(),
            split: PromptEvaluationSplit::Train,
            mode: PromptEvaluationMode::Live,
            format_valid: true,
            succeeded: false,
            quality_score: 0.0,
            latency_ms: 100,
            total_tokens: 100,
            estimated_cost_microusd: 0,
            safety_violations: 1,
            relative_reward: None,
            step_credits: Vec::new(),
            reflection_packet: None,
        },
    ));
    let rolled_back = reconcile_prompt_rollout(&mut model, "auto", &evaluation(8));
    assert_eq!(rolled_back.status, "rolled_back");
    assert!(rolled_back.canary_profile_id.is_none());
    assert_eq!(rolled_back.rollback_count, 1);
    assert_eq!(rolled_back.stable_profile_id, stable.id);
}

#[test]
fn prompt_rollout_rebuilds_from_durable_events() {
    let event = Event {
        id: EventId("rollout-update".to_string()),
        task_id: phase16_task_id(),
        sequence: 7,
        timestamp_ms: 70,
        kind: EventKind::TaskStatusChanged,
        summary: "Conductor prompt rollout updated".to_string(),
        metadata: [
            ("prompt_effort".to_string(), "auto".to_string()),
            ("stable_profile".to_string(), "stable-auto".to_string()),
            ("canary_profile".to_string(), "candidate-auto".to_string()),
            ("canary_percent".to_string(), "25".to_string()),
            ("rollout_status".to_string(), "canary".to_string()),
            (
                "rollout_reason".to_string(),
                "canary_stage_advanced".to_string(),
            ),
            ("promotion_confidence".to_string(), "0.61".to_string()),
            ("evidence_checkpoint".to_string(), "8".to_string()),
            ("live_checkpoint".to_string(), "3".to_string()),
            ("rollback_count".to_string(), "2".to_string()),
        ]
        .into_iter()
        .collect(),
    };

    let model = build_prompt_evolution_read_model(&[event], 7, 1);
    let rollout = model
        .rollouts
        .get("auto")
        .expect("durable rollout should rebuild");

    assert_eq!(rollout.stable_profile_id, "stable-auto");
    assert_eq!(rollout.canary_profile_id.as_deref(), Some("candidate-auto"));
    assert_eq!(rollout.canary_percent, 25);
    assert_eq!(rollout.evidence_checkpoint, 8);
    assert_eq!(rollout.live_checkpoint, 3);
    assert_eq!(rollout.rollback_count, 2);
    assert_eq!(rollout.promotion_confidence, Some(0.61));
}

#[test]
fn pending_review_state_identifies_the_related_session() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let project_sessions = ProjectSessionConfig::default_for_root(&workspace_root());
    let session = project_sessions
        .active_session()
        .expect("default session should exist");
    let project = project_sessions
        .active_project()
        .expect("default project should exist");
    let request = PermissionRequest {
        id: PermissionRequestId("agent-review-1".to_string()),
        task_id: phase16_task_id(),
        risk: PermissionRisk::Execute,
        action: "shell.run".to_string(),
        reason: "Run project checks.".to_string(),
        scope: ".".to_string(),
        metadata: [
            ("phase".to_string(), "16".to_string()),
            ("session_id".to_string(), session.id.clone()),
            ("session_name".to_string(), session.name.clone()),
            ("project_id".to_string(), project.id.clone()),
            ("project_name".to_string(), project.name.clone()),
            ("tool_input".to_string(), "command=cargo test".to_string()),
        ]
        .into_iter()
        .collect(),
    };
    store
        .save_permission_request(request, current_time_millis())
        .expect("request should save");

    let state =
        permission_review_state(&store, &project_sessions).expect("review state should load");

    assert_eq!(state.pending.len(), 1);
    assert_eq!(state.pending[0].source, "agent");
    assert_eq!(
        state.pending[0].session_id.as_deref(),
        Some(session.id.as_str())
    );
    assert_eq!(
        state.pending[0].session_name.as_deref(),
        Some(session.name.as_str())
    );
    assert!(state.pending[0].can_allow_session);
    assert!(state.pending[0].input.contains("cargo test"));
}

#[test]
fn phase4_state_includes_provider_config_and_messages() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let config = ProviderConfig {
        api_key: "secret".to_string(),
        ..ProviderConfig::default()
    };
    append_message_event(
        &mut store,
        &phase4_task_id(),
        MessageRole::User,
        "hello model",
    )
    .expect("message should append");

    let state = phase4_state(&mut store, &config, None).expect("state should load");

    assert!(state.provider.api_key_set);
    assert_eq!(state.messages.len(), 1);
    assert_eq!(state.messages[0].content, "hello model");
}

#[test]
fn redacts_sensitive_values_in_new_and_existing_events() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_message_event(
        &mut store,
        &phase4_task_id(),
        MessageRole::Tool,
        "api_key=secret-value\nBearer token-value\nsk-1234567890abcdef",
    )
    .expect("message should append");
    store
        .append(Event {
            id: EventId("legacy-secret".to_string()),
            task_id: phase4_task_id(),
            sequence: 2,
            timestamp_ms: 20,
            kind: EventKind::ToolCallFinished,
            summary: "legacy secret".to_string(),
            metadata: [("output".to_string(), "password=old-secret".to_string())]
                .into_iter()
                .collect(),
        })
        .expect("legacy event should append");

    let updated = redact_persisted_events(&mut store).expect("history should redact");
    let events = store
        .list_by_task(&phase4_task_id())
        .expect("events should load");
    let rendered = events
        .iter()
        .flat_map(|event| event.metadata.values())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(updated, 1);
    assert!(!rendered.contains("secret-value"));
    assert!(!rendered.contains("token-value"));
    assert!(!rendered.contains("1234567890abcdef"));
    assert!(!rendered.contains("old-secret"));
    assert!(rendered.contains("[REDACTED]"));
}

#[test]
fn redacting_tool_call_metadata_preserves_nested_json() {
    let arguments = serde_json::json!({
        "command": "cat provider.conf | sed -E 's/(key|token|secret|api[_-]?key)=.*/[REDACTED]/g'",
        "api_key": "secret-value"
    })
    .to_string();
    let raw_tool_calls = serde_json::json!([{
        "id": "call_1",
        "type": "function",
        "function": {
            "name": "shell_run",
            "arguments": arguments
        }
    }])
    .to_string();
    let metadata = [("raw_tool_calls_json".to_string(), raw_tool_calls)]
        .into_iter()
        .collect();

    let redacted = redact_metadata(&metadata);
    let parsed: serde_json::Value = serde_json::from_str(
        redacted
            .get("raw_tool_calls_json")
            .expect("tool calls should remain present"),
    )
    .expect("tool calls should remain valid JSON");
    let nested: serde_json::Value = serde_json::from_str(
        parsed[0]["function"]["arguments"]
            .as_str()
            .expect("arguments should remain a JSON string"),
    )
    .expect("tool arguments should remain valid JSON");

    assert_eq!(nested["api_key"], "[REDACTED]");
    assert!(nested["command"].as_str().is_some());
    assert!(!redacted["raw_tool_calls_json"].contains("secret-value"));
}

#[test]
fn event_redaction_marker_records_completed_migration() {
    let root = temp_test_root("cindx-event-redaction-marker");
    assert!(!event_redaction_complete(&root));

    mark_event_redaction_complete(&root).expect("marker should persist");

    assert!(event_redaction_complete(&root));
    assert_eq!(
        fs::read_to_string(event_redaction_marker_path(&root)).expect("marker should be readable"),
        "events-redaction-v1\n"
    );
    fs::remove_dir_all(root).expect("marker fixture should be removed");
}

#[test]
fn tool_event_metadata_is_compacted_before_persistence() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_tool_finished_event(
        &mut store,
        &phase16_task_id(),
        "call-large",
        "browser.capture",
        "completed",
        &"output".repeat(20_000),
        [("structured_output".to_string(), "structured".repeat(20_000))]
            .into_iter()
            .collect(),
        None,
    )
    .expect("tool event should append");

    let event = store
        .list_by_task(&phase16_task_id())
        .expect("events should load")
        .pop()
        .expect("tool event should exist");
    assert!(event.metadata["output"].len() <= PERSISTED_TOOL_EVENT_OUTPUT_PREVIEW_BYTES + 3);
    assert_eq!(event.metadata["output_omitted"], "true");
    assert_eq!(event.metadata["result_structured_output_omitted"], "true");
    assert!(event.metadata["result_structured_output"].starts_with("[omitted:"));
}

#[test]
fn persisted_tool_event_compaction_rewrites_legacy_payloads() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    store
        .append(Event {
            id: EventId("legacy-large-tool-event".to_string()),
            task_id: phase16_task_id(),
            sequence: 1,
            timestamp_ms: 1,
            kind: EventKind::ToolCallFinished,
            summary: "legacy tool output".to_string(),
            metadata: [
                ("session_id".to_string(), "session-a".to_string()),
                ("output".to_string(), "legacy-output".repeat(30_000)),
            ]
            .into_iter()
            .collect(),
        })
        .expect("legacy event should append");

    assert_eq!(
        compact_persisted_tool_event_metadata(&mut store).expect("legacy metadata should compact"),
        1
    );
    let event = store
        .event_by_id("legacy-large-tool-event")
        .expect("event should load")
        .expect("event should exist");
    assert_eq!(event.metadata["session_id"], "session-a");
    assert_eq!(event.metadata["output_omitted"], "true");
    assert!(event.metadata["output"].len() <= PERSISTED_TOOL_EVENT_OUTPUT_PREVIEW_BYTES + 3);
}

#[test]
fn provider_config_input_preserves_existing_key_when_blank() {
    let mut config = ProviderConfig {
        api_key: "existing".to_string(),
        ..ProviderConfig::default()
    };

    apply_provider_config_input(
        &mut config,
        ProviderConfigInput {
            base_url: "https://example.test/v1".to_string(),
            api_key: "".to_string(),
            model: "model-a".to_string(),
            conductor_model: "".to_string(),
            planner_model: "".to_string(),
            executor_model: "".to_string(),
            reviewer_model: "".to_string(),
            summarizer_model: "".to_string(),
            embedding_model: "".to_string(),
            image_model: "image-model-a".to_string(),
            image_endpoint: "https://images.example.test/v1".to_string(),
            collaboration_policy: "auto_router".to_string(),
            prompt_evolution_enabled: true,
            context_window_tokens: 128_000,
            agent_system_prompt: "Be concise.\nUse Chinese when asked.".to_string(),
        },
    );

    assert_eq!(config.api_key, "existing");
    assert_eq!(config.model_for_conductor(), "model-a");
    assert_eq!(config.executor_model, "model-a");
    assert_eq!(config.collaboration_policy, "auto_router");
    assert_eq!(config.context_window_tokens, 128_000);
    assert_eq!(
        config.agent_system_prompt,
        "Be concise.\nUse Chinese when asked."
    );
    assert_eq!(config.image_model, "image-model-a");
    assert_eq!(config.image_endpoint, "https://images.example.test/v1");
}

#[test]
fn legacy_provider_config_inherits_conductor_from_planner() {
    let migrated = provider_config_from_text(
        "model=default-a\nplanner_model=planner-b\nexecutor_model=executor-c\n",
    );
    assert_eq!(migrated.model_for_conductor(), "planner-b");

    let explicit = provider_config_from_text(
        "model=default-a\nconductor_model=conductor-z\nplanner_model=planner-b\n",
    );
    assert_eq!(explicit.model_for_conductor(), "conductor-z");
}

#[test]
fn dashscope_provider_migrates_the_openai_embedding_default() {
    let migrated = provider_config_from_text(
        "base_url=https://dashscope.aliyuncs.com/compatible-mode/v1\n\
             model=qwen-plus\n\
             embedding_model=text-embedding-3-small\n",
    );
    assert_eq!(
        migrated.model_for_role(&ModelRole::Embedder),
        DASHSCOPE_DEFAULT_EMBEDDING_MODEL
    );

    let explicit = provider_config_from_text(
        "base_url=https://dashscope.aliyuncs.com/compatible-mode/v1\n\
             embedding_model=custom-embedding-model\n",
    );
    assert_eq!(
        explicit.model_for_role(&ModelRole::Embedder),
        "custom-embedding-model"
    );

    let openai = ProviderConfig::default();
    assert_eq!(
        openai.model_for_role(&ModelRole::Embedder),
        OPENAI_DEFAULT_EMBEDDING_MODEL
    );
}

#[test]
fn ensemble_uses_distinct_role_models_in_stable_order() {
    let config = ProviderConfig {
        model: "default".to_string(),
        conductor_model: "conductor-z".to_string(),
        planner_model: "planner-a".to_string(),
        executor_model: "executor-b".to_string(),
        reviewer_model: "reviewer-c".to_string(),
        summarizer_model: "summary-d".to_string(),
        ..ProviderConfig::default()
    };

    assert_eq!(
        collaboration_candidate_models(&config, 3),
        vec![
            "planner-a".to_string(),
            "executor-b".to_string(),
            "reviewer-c".to_string(),
        ]
    );
    assert_eq!(config.model_for_conductor(), "conductor-z");
    let oversized_pool = collaboration_candidate_models(&config, 5);
    assert_eq!(oversized_pool.len(), MAX_ADAPTIVE_WORKFLOW_AGENTS);
    assert!(!oversized_pool.contains(&"conductor-z".to_string()));

    let worker_models = collaboration_candidate_models(&config, 3);
    let hints = collaboration_role_hints(&config, &worker_models);
    assert_eq!(hints.planner, "planner-a");
    assert_eq!(hints.executor, "executor-b");
    assert_eq!(hints.reviewer, "reviewer-c");
    assert_eq!(hints.synthesizer, "planner-a");
    assert!([
        &hints.planner,
        &hints.executor,
        &hints.reviewer,
        &hints.synthesizer,
    ]
    .iter()
    .all(|model| worker_models.contains(model)));

    let coverage = workflow_role_coverage(
        &AdaptiveWorkflow {
            steps: vec![
                AdaptiveWorkflowStep {
                    id: "plan".to_string(),
                    role: "thinker".to_string(),
                    model: hints.planner.clone(),
                    subtask: "plan".to_string(),
                    access: Vec::new(),
                },
                AdaptiveWorkflowStep {
                    id: "execute".to_string(),
                    role: "worker".to_string(),
                    model: hints.executor.clone(),
                    subtask: "execute".to_string(),
                    access: Vec::new(),
                },
                AdaptiveWorkflowStep {
                    id: "review".to_string(),
                    role: "verifier".to_string(),
                    model: hints.reviewer.clone(),
                    subtask: "review".to_string(),
                    access: vec!["plan".to_string(), "execute".to_string()],
                },
                AdaptiveWorkflowStep {
                    id: "synthesize".to_string(),
                    role: "synthesizer".to_string(),
                    model: hints.synthesizer.clone(),
                    subtask: "synthesize".to_string(),
                    access: vec![
                        "plan".to_string(),
                        "execute".to_string(),
                        "review".to_string(),
                    ],
                },
            ],
        },
        &hints,
    );
    assert_eq!(coverage.aligned_steps, 4);
    assert_eq!(coverage.independent_models, 2);
    assert_eq!(coverage.verifier_steps, 1);
    assert_eq!(coverage.synthesizer_steps, 1);
    assert!(coverage.cross_reviewed);
}

#[test]
fn pro_role_budget_does_not_collapse_when_roles_share_one_model() {
    let models = vec!["shared-frontier-model".to_string()];
    let budget = collaboration_agent_budget(3);
    let fallback = collaboration_fallback_models(&models, budget);

    assert_eq!(budget, 3);
    assert_eq!(fallback.len(), 3);
    assert!(fallback
        .iter()
        .all(|model| model == "shared-frontier-model"));
    assert_eq!(adaptive_workflow_step_budget(budget), 5);
}

#[test]
fn effort_and_router_select_the_primary_model_without_collapsing_to_executor() {
    let config = ProviderConfig {
        model: "default-a".to_string(),
        executor_model: "executor-b".to_string(),
        ..ProviderConfig::default()
    };

    assert_eq!(
        config.model_for_agent_policy(&OrchestrationPolicy::Single),
        "default-a"
    );
    assert_eq!(
        config.model_for_agent_policy(&OrchestrationPolicy::PlanExecuteReview),
        "executor-b"
    );

    let run_context = [
        ("collaboration_policy".to_string(), "single".to_string()),
        ("agent_model".to_string(), "routed-c".to_string()),
    ]
    .into_iter()
    .collect();
    assert_eq!(agent_model_for_run(&config, &run_context), "routed-c");

    let policy = OrchestrationPolicy::Single;
    let mut decision = RuleBasedRouter.route(&RoutingContext::from_prompt(
        "hello",
        model_candidates_for_config(&config),
    ));
    assert_eq!(
        agent_model_for_effort(&config, AgentEffort::Auto, &policy, &decision),
        "default-a"
    );
    decision.model = "routed-c".to_string();
    decision
        .metadata
        .insert("router".to_string(), "rule_based_v2".to_string());
    assert_eq!(
        agent_model_for_effort(&config, AgentEffort::Auto, &policy, &decision),
        "routed-c"
    );
    assert_eq!(
        agent_model_for_effort(&config, AgentEffort::Fast, &policy, &decision),
        "default-a"
    );
    assert_eq!(
        agent_model_for_effort(
            &config,
            AgentEffort::Pro,
            &OrchestrationPolicy::BestOfN { candidates: 3 },
            &decision,
        ),
        "executor-b"
    );
}

#[test]
fn agent_effort_maps_to_bounded_policies() {
    assert_eq!(
        AgentEffort::parse("fast").requested_policy(),
        OrchestrationPolicy::Single
    );
    assert_eq!(
        AgentEffort::parse("auto").requested_policy(),
        OrchestrationPolicy::AutoRouter
    );
    assert_eq!(
        AgentEffort::parse("pro").requested_policy(),
        OrchestrationPolicy::BestOfN { candidates: 3 }
    );
    assert_eq!(AgentEffort::parse("unknown"), AgentEffort::Auto);
}

#[test]
fn pro_uses_bounded_collaboration_for_simple_requests_only() {
    let mut context = RoutingContext::from_prompt("重新来一次", Vec::new());

    assert_eq!(collaboration_profile(AgentEffort::Pro, &context), "bounded");
    assert_eq!(
        collaboration_profile(AgentEffort::Auto, &context),
        "adaptive"
    );

    context.needs_multi_model = true;
    assert_eq!(
        collaboration_profile(AgentEffort::Pro, &context),
        "adaptive"
    );
}

#[test]
fn retry_recovers_effort_from_the_active_run() {
    let event = Event {
        id: EventId("run-start".to_string()),
        task_id: phase16_task_id(),
        sequence: 1,
        timestamp_ms: 1,
        kind: EventKind::TaskStatusChanged,
        summary: "Agent task started".to_string(),
        metadata: [("agent_effort".to_string(), "pro".to_string())]
            .into_iter()
            .collect(),
    };

    assert_eq!(agent_effort_from_active_events(&[event]), AgentEffort::Pro);
    assert_eq!(agent_effort_from_active_events(&[]), AgentEffort::Auto);
}

#[test]
fn adaptive_coordinator_parses_bounded_dependency_workflow() {
    let response = r#"```json
        {"steps":[
          {"id":"independent-a","role":"thinker","model":"planner-a","subtask":"Analyze one path","access":[]},
          {"id":"independent-b","role":"worker","model":"reviewer-b","subtask":"Challenge assumptions","access":[]},
          {"id":"verify","role":"verifier","model":"summary-c","subtask":"Verify both paths","access":["independent-a","independent-b"]},
          {"id":"final","role":"synthesizer","model":"summary-c","subtask":"Synthesize guidance","access":["independent-a","independent-b","verify"]}
        ]}
        ```"#;
    let models = vec![
        "planner-a".to_string(),
        "reviewer-b".to_string(),
        "summary-c".to_string(),
    ];

    let workflow = test_conductor_harness(models, 3)
        .parse_plan(response)
        .expect("workflow should parse")
        .adaptive_workflow();

    assert_eq!(workflow.steps.len(), 4);
    assert_eq!(
        workflow.steps[3].access,
        vec![
            "independent-a".to_string(),
            "independent-b".to_string(),
            "verify".to_string()
        ]
    );
    assert_eq!(
        adaptive_workflow_layers(&workflow).expect("layers should build"),
        vec![vec![0, 1], vec![2], vec![3]]
    );
}

#[test]
fn adaptive_coordinator_rejects_models_outside_the_configured_pool() {
    let response = r#"{"steps":[
          {"id":"first","role":"thinker","model":"configured","subtask":"Analyze","access":[]},
          {"id":"second","role":"worker","model":"configured","subtask":"Challenge","access":[]},
          {"id":"verify","role":"verifier","model":"configured","subtask":"Verify","access":["first","second"]},
          {"id":"final","role":"synthesizer","model":"unconfigured","subtask":"Synthesize","access":["first","second","verify"]}
        ]}"#;

    let error = test_conductor_harness(vec!["configured".to_string()], 3)
        .parse_plan(response)
        .expect_err("unknown model should be rejected");

    assert!(error.contains("unknown model"));
}

#[test]
fn adaptive_coordinator_accepts_five_steps_with_three_reused_models() {
    let response = r#"{"steps":[
          {"id":"a","role":"thinker","model":"planner-a","subtask":"Independent approach","access":[]},
          {"id":"b","role":"worker","model":"reviewer-b","subtask":"Independent challenge","access":[]},
          {"id":"verify","role":"verifier","model":"summary-c","subtask":"Verify both","access":["a","b"]},
          {"id":"refine","role":"worker","model":"planner-a","subtask":"Refine verified work","access":["verify"]},
          {"id":"final","role":"synthesizer","model":"reviewer-b","subtask":"Synthesize","access":["b","refine"]}
        ]}"#;
    let models = vec![
        "planner-a".to_string(),
        "reviewer-b".to_string(),
        "summary-c".to_string(),
    ];

    let workflow = test_conductor_harness(models, 3)
        .parse_plan(response)
        .expect("five-step plan should parse")
        .adaptive_workflow();

    assert_eq!(workflow.steps.len(), 5);
    assert_eq!(
        adaptive_workflow_layers(&workflow).expect("layers should build"),
        vec![vec![0, 1], vec![2], vec![3], vec![4]]
    );
}

#[test]
fn conductor_result_separates_worker_claims_from_tool_evidence() {
    let result = collaboration_step_result(
        "inspect",
        "model-a",
        "The config probably uses model A.",
        &[CollaborationEvidence {
            source_step: "worker_1".to_string(),
            tool_call_id: "call-1".to_string(),
            tool_name: "file.read".to_string(),
            request: "path=config.toml".to_string(),
            status: "succeeded".to_string(),
            output: "model = B".to_string(),
        }],
    );

    assert!(result.contains("treat as a proposal until supported"));
    assert!(result.contains("Tool evidence ledger"));
    assert!(result.contains("source=worker_1 call=call-1 tool=file.read status=succeeded"));
    assert!(result.contains("model = B"));
}

#[test]
fn conductor_merges_authorized_evidence_and_deduplicates_provenance() {
    let a = CollaborationEvidence {
        source_step: "worker_a".to_string(),
        tool_call_id: "call-1".to_string(),
        tool_name: "file.read".to_string(),
        request: "path=a.txt".to_string(),
        status: "succeeded".to_string(),
        output: "A".to_string(),
    };
    let b = CollaborationEvidence {
        source_step: "worker_b".to_string(),
        tool_call_id: "call-1".to_string(),
        tool_name: "file.read".to_string(),
        request: "path=b.txt".to_string(),
        status: "succeeded".to_string(),
        output: "B".to_string(),
    };
    let inherited = [
        ("a".to_string(), vec![a.clone()]),
        ("b".to_string(), vec![b.clone(), a.clone()]),
    ]
    .into_iter()
    .collect();

    let merged =
        merge_collaboration_evidence(&["a".to_string(), "b".to_string()], &inherited, &[b]);

    assert_eq!(merged.len(), 2);
    assert_eq!(merged[0].source_step, "worker_a");
    assert_eq!(merged[1].source_step, "worker_b");
}

#[test]
fn conductor_bounds_checkpoint_evidence_and_preserves_current_step_observations() {
    let inherited = (0..40)
        .map(|index| CollaborationEvidence {
            source_step: "source".to_string(),
            tool_call_id: format!("inherited-{index}"),
            tool_name: "file.read".to_string(),
            request: "r".repeat(3_000),
            status: "succeeded".to_string(),
            output: "o".repeat(3_000),
        })
        .collect::<Vec<_>>();
    let own = (0..4)
        .map(|index| CollaborationEvidence {
            source_step: "current".to_string(),
            tool_call_id: format!("own-{index}"),
            tool_name: "file.read".to_string(),
            request: "request".to_string(),
            status: "succeeded".to_string(),
            output: "current observation".to_string(),
        })
        .collect::<Vec<_>>();
    let evidence_by_step = BTreeMap::from([("source".to_string(), inherited)]);

    let merged = merge_collaboration_evidence(&["source".to_string()], &evidence_by_step, &own);

    assert_eq!(merged.len(), 32);
    assert_eq!(
        merged
            .iter()
            .filter(|entry| entry.source_step == "current")
            .count(),
        own.len()
    );
    assert!(merged
        .iter()
        .all(|entry| entry.request.chars().count() <= 2_012));
    assert!(merged
        .iter()
        .all(|entry| entry.output.chars().count() <= 2_012));
}

#[test]
fn collaboration_stages_have_visible_timeline_labels() {
    let event = Event {
        id: EventId("candidate-event".to_string()),
        task_id: phase16_task_id(),
        sequence: 1,
        timestamp_ms: 1,
        kind: EventKind::ModelRequestStarted,
        summary: "Collaboration candidate_2 started".to_string(),
        metadata: [
            ("collaboration_id".to_string(), "collab-1".to_string()),
            ("stage".to_string(), "candidate_2".to_string()),
        ]
        .into_iter()
        .collect(),
    };

    assert_eq!(timeline_event_label(&event), "Candidate 2");
    assert_eq!(
        collaboration_stage_display_label("coordinator"),
        "Conductor"
    );
    assert_eq!(
        collaboration_stage_display_label("conductor_plan"),
        "Conductor"
    );
    assert_eq!(
        collaboration_stage_display_label("conductor_repair"),
        "Conductor repair"
    );
    assert_eq!(collaboration_stage_display_label("worker_3"), "Worker 3");
    assert_eq!(collaboration_stage_display_label("arbiter"), "Arbiter");
    assert_eq!(
        collaboration_stage_display_label("synthesizer"),
        "Synthesis"
    );
}

#[test]
fn agent_system_prompt_config_encoding_preserves_multiline_unicode() {
    let prompt = "你是 Cindx。\n先检查事实，再执行。";
    let encoded = config_hex_encode(prompt);

    assert_eq!(config_hex_decode(&encoded).as_deref(), Some(prompt));
    assert!(config_hex_decode("not-hex").is_none());
}

#[test]
fn personalization_is_normalized_and_applied_to_agent_instructions() {
    let config = normalized_personalization_config(PersonalizationConfig {
        preferred_name: "  Dale\nAdmin  ".to_string(),
        response_tone: "direct".to_string(),
        response_length: "concise".to_string(),
    });
    let instructions = personalized_agent_instructions(&config, "Use Chinese when asked.");

    assert_eq!(config.preferred_name, "DaleAdmin");
    assert!(instructions.contains("The user's preferred name is \"DaleAdmin\""));
    assert!(instructions.contains("answer with \"DaleAdmin\""));
    assert!(instructions.contains("direct, factual tone"));
    assert!(instructions.contains("Keep responses concise"));
    assert!(instructions.starts_with("Use Chinese when asked."));
    assert!(instructions
        .ends_with("Keep responses concise unless more detail is necessary for correctness."));
}

#[test]
fn invalid_personalization_options_fall_back_to_safe_defaults() {
    let config = normalized_personalization_config(PersonalizationConfig {
        preferred_name: String::new(),
        response_tone: "unknown".to_string(),
        response_length: "unbounded".to_string(),
    });

    assert_eq!(config.response_tone, "natural");
    assert_eq!(config.response_length, "balanced");
}

#[test]
fn agent_runtime_context_includes_the_time_computed_for_the_user_turn() {
    let context = [(
        "current_time".to_string(),
        "2026-07-11 10:30 CST".to_string(),
    )]
    .into_iter()
    .collect();
    let runtime_context =
        agent_runtime_context_for_run(&context).expect("time context should exist");

    assert!(runtime_context.contains("Current date and time: 2026-07-11 10:30 CST"));
    assert!(runtime_context.contains("authoritative for this turn"));
}

#[test]
fn collaboration_prompt_keeps_core_user_instructions_and_runtime_context() {
    let context = [(
        "current_time".to_string(),
        "2026-07-11 10:30 CST".to_string(),
    )]
    .into_iter()
    .collect();
    let prompt = collaboration_system_prompt_for_run("Answer in Chinese.", &context);

    assert!(prompt.starts_with("You are Cindx"));
    assert!(prompt.contains("<user_instructions>\nAnswer in Chinese."));
    assert!(prompt.contains("<runtime_context>"));
    assert!(prompt.contains("Current date and time: 2026-07-11 10:30 CST"));
}

#[test]
fn legacy_default_prompt_migrates_to_empty_custom_instructions() {
    assert!(ProviderConfig::default().agent_system_prompt.is_empty());
    assert!(normalized_agent_instructions("").is_empty());
    assert!(normalized_agent_instructions(LEGACY_AGENT_SYSTEM_PROMPT).is_empty());
    assert_eq!(
        normalized_agent_instructions("  Prefer concise answers.  "),
        "Prefer concise answers."
    );
}

#[test]
fn agent_state_reports_context_usage_and_hides_internal_drafts() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        [
            ("prompt".to_string(), "inspect".to_string()),
            ("context_window_tokens".to_string(), "100000".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("start should append");
    append_message_event(&mut store, &phase16_task_id(), MessageRole::User, "inspect")
        .expect("user message should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::ModelRequestFinished,
        "Agent model turn finished",
        [("prompt_tokens".to_string(), "25000".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("usage should append");
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::Assistant,
        "internal draft",
        [("internal".to_string(), "true".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("draft should append");
    append_message_event(
        &mut store,
        &phase16_task_id(),
        MessageRole::Assistant,
        "final answer",
    )
    .expect("final should append");

    let state = agent_state(&store, None).expect("state should load");

    assert_eq!(state.context_tokens_used, 25_000);
    assert_eq!(state.context_window_tokens, 100_000);
    assert_eq!(state.context_remaining_percent, 75.0);
    assert!(!state.context_usage_estimated);
    assert_eq!(state.messages.len(), 2);
    assert_eq!(state.messages[1].content, "final answer");
}

#[test]
fn phase5_state_lists_tool_results() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let root = workspace_root();
    execute_tool_invocation(
        &mut store,
        ToolInvocation {
            id: agent_core::ToolCallId("tool-1".to_string()),
            task_id: phase5_task_id(),
            tool_name: "file.list".to_string(),
            input_json: encode_input(&[("path", ".")]),
            proposed_by_model: "test".to_string(),
            metadata: Metadata::new(),
        },
        &root,
        None,
    )
    .expect("tool should execute");

    let state = phase5_state(&store, None, &root).expect("state should load");

    assert!(state.tools.iter().any(|tool| tool.name == "file.write"));
    assert_eq!(state.results.len(), 1);
    assert_eq!(state.results[0].tool_name, "file.list");
}

#[test]
fn phase5_state_lists_pending_tool_approvals() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let root = workspace_root();
    let invocation = ToolInvocation {
        id: agent_core::ToolCallId("tool-2".to_string()),
        task_id: phase5_task_id(),
        tool_name: "file.write".to_string(),
        input_json: encode_input(&[("path", ".cindx/phase5-test.txt"), ("content", "ok")]),
        proposed_by_model: "test".to_string(),
        metadata: Metadata::new(),
    };
    let registry = ToolRegistry::with_workspace_tools(root.clone());
    let mut request = registry
        .get("file.write")
        .expect("tool should exist")
        .permission_request(&invocation)
        .expect("write should request permission");
    request.id = PermissionRequestId("perm-phase5".to_string());
    request
        .metadata
        .insert("phase".to_string(), "5".to_string());
    request
        .metadata
        .insert("tool_input".to_string(), invocation.input_json);
    request
        .metadata
        .insert("tool_call_id".to_string(), invocation.id.0);
    request
        .metadata
        .insert("tool_name".to_string(), invocation.tool_name);
    store
        .save_permission_request(request, 123)
        .expect("request should save");

    let state = phase5_state(&store, None, &root).expect("state should load");

    assert_eq!(state.pending_approvals.len(), 1);
    assert_eq!(state.pending_approvals[0].tool_name, "file.write");
}

#[test]
fn phase6_state_lists_orchestration_steps() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase6_task_id(),
        EventKind::ModelRequestFinished,
        "planner step finished",
        [
            ("orchestration_id".to_string(), "orch-1".to_string()),
            ("policy".to_string(), "plan_execute_review".to_string()),
            ("step_index".to_string(), "0".to_string()),
            ("role".to_string(), "planner".to_string()),
            ("model".to_string(), "model-a".to_string()),
            ("latency_ms".to_string(), "12".to_string()),
            ("output".to_string(), "Plan first.".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("event should append");

    let state = phase6_state(&store, None).expect("state should load");

    assert_eq!(state.steps.len(), 1);
    assert_eq!(state.steps[0].policy, "plan_execute_review");
    assert_eq!(state.steps[0].role, "planner");
    assert_eq!(state.steps[0].output, "Plan first.");
}

#[test]
fn workspace_index_falls_back_to_local_embeddings_when_cloud_fails() {
    struct FailingEmbedder;

    impl RagEmbedder for FailingEmbedder {
        fn embed_texts(&mut self, _texts: &[String]) -> Result<EmbeddingBatch, RagError> {
            Err(RagError::new("configured embedding model is unavailable"))
        }
    }

    let root = temp_test_root("phase7-cloud-fallback");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(
        root.join("notes.md"),
        "# Cindx\n\nLocal indexing remains available when cloud embeddings fail.",
    )
    .expect("fixture should write");

    let (index, backend, model, fallback_error) = index_workspace_with_cloud_fallback(
        &root,
        IndexOptions::default(),
        &mut FailingEmbedder,
        "missing-cloud-model",
    )
    .expect("local fallback should build the index");

    assert_eq!(backend, "local-fallback");
    assert!(model.starts_with("local-hash-"));
    assert_eq!(
        fallback_error.as_deref(),
        Some("configured embedding model is unavailable")
    );
    assert!(!index.chunks.is_empty());
    assert!(index
        .chunks
        .iter()
        .all(|chunk| chunk.embedding_provider == "local"));
}

#[test]
fn phase7_state_reports_rag_stats() {
    let root = temp_test_root("phase7-stats");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(
        root.join("notes.md"),
        "# Cindx\n\nThe RAG index stores line-level provenance.",
    )
    .expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    let mut adapter = FileRagAdapter::open(root.join(".cindx").join("rag-index.tsv"))
        .expect("adapter should open");
    adapter.replace_all(index).expect("index should persist");
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase7_task_id(),
        EventKind::RetrievalPerformed,
        "Workspace indexed for RAG",
        [
            ("action".to_string(), "index".to_string()),
            ("files_indexed".to_string(), "1".to_string()),
            ("chunks_indexed".to_string(), "1".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("event should append");

    let state = phase7_state(
        &store,
        &adapter,
        MemoryStatsView::default(),
        Vec::new(),
        None,
        empty_graph_state(),
        None,
        None,
    )
    .expect("state should load");

    assert_eq!(state.stats.files_indexed, 1);
    assert_eq!(state.stats.chunks_indexed, 1);
    assert!(state
        .timeline
        .iter()
        .any(|entry| entry.label == "Retrieval"));
}

#[test]
fn rag_sources_include_line_ranges() {
    let root = temp_test_root("phase7-sources");
    fs::create_dir_all(root.join("docs")).expect("temp docs should exist");
    fs::write(
        root.join("docs").join("rag.md"),
        "Intro\nSemantic retrieval should cite exact source lines.\nDone",
    )
    .expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    let mut adapter = FileRagAdapter::open(root.join(".cindx").join("rag-index.tsv"))
        .expect("adapter should open");
    adapter.replace_all(index).expect("index should persist");

    let results = adapter
        .search("semantic retrieval source lines", 3)
        .expect("search should run");
    let sources = rag_sources_from_results(&results);

    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].path, "docs/rag.md");
    assert_eq!(sources[0].start_line, 1);
    assert_eq!(sources[0].end_line, 3);
    assert!(sources[0].score > 0.0);
}

#[test]
fn context_state_builds_and_persists_restore_pack() {
    let root = temp_test_root("phase15-context");
    fs::create_dir_all(&root).expect("temp root should exist");
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_message_event(
        &mut store,
        &phase4_task_id(),
        MessageRole::User,
        "Continue the MVP context manager",
    )
    .expect("message should append");
    append_event(
        &mut store,
        &phase7_task_id(),
        EventKind::RetrievalPerformed,
        "RAG search completed",
        [
            ("action".to_string(), "search".to_string()),
            ("query".to_string(), "context compression".to_string()),
            ("selected_count".to_string(), "2".to_string()),
            (
                "retrieval_mode".to_string(),
                "four_way_parallel".to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    )
    .expect("retrieval should append");

    let run_context = Metadata::new();
    let preview =
        context_state(&store, &root, &run_context, None, None).expect("state should load");
    let checkpoint = preview.checkpoint.expect("checkpoint should exist");

    assert_eq!(
        checkpoint.current_goal.as_deref(),
        Some("Continue the MVP context manager")
    );
    assert!(checkpoint.path.is_none());
    assert!(checkpoint.restore_pack.contains("## Current Goal"));
    assert!(checkpoint.restore_pack.contains("four_way_parallel"));

    let events = collect_context_events(&store, &run_context).expect("events should collect");
    let pack = build_restore_context_pack(build_session_checkpoint_at(
        &events,
        CheckpointOptions::default(),
        777,
    ));
    let checkpoint_path =
        write_context_checkpoint(&root, None, &pack.text, None).expect("checkpoint should write");
    append_event(
        &mut store,
        &phase15_task_id(),
        EventKind::TaskStatusChanged,
        "Context checkpoint compacted",
        [
            ("checkpoint_id".to_string(), pack.checkpoint.id.clone()),
            (
                "context_checkpoint_path".to_string(),
                checkpoint_path.display().to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    )
    .expect("compact event should append");

    let compacted =
        context_state(&store, &root, &run_context, Some(pack), None).expect("state should load");
    let checkpoint = compacted.checkpoint.expect("checkpoint should exist");
    let expected_path = checkpoint_path.display().to_string();

    assert_eq!(checkpoint.path.as_deref(), Some(expected_path.as_str()));
    assert!(fs::read_to_string(checkpoint_path)
        .expect("checkpoint should be readable")
        .contains("Cindx Context Checkpoint"));
    assert!(compacted
        .timeline
        .iter()
        .any(|entry| entry.detail.contains("Context checkpoint compacted")));
}

#[test]
fn context_events_and_checkpoint_paths_are_session_scoped() {
    let root = temp_test_root("phase15-session-scope");
    fs::create_dir_all(&root).expect("temp root should exist");
    let mut store = SqliteStore::in_memory().expect("store should open");
    for (session_id, content) in [("session-a", "alpha"), ("session-b", "beta")] {
        append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            MessageRole::User,
            content,
            [
                ("project_id".to_string(), "project-a".to_string()),
                ("session_id".to_string(), session_id.to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .expect("message should append");
    }
    let run_context = [
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), "session-a".to_string()),
    ]
    .into_iter()
    .collect();

    let events = collect_context_events(&store, &run_context).expect("events should collect");

    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].metadata.get("content").map(String::as_str),
        Some("alpha")
    );
    assert_ne!(
        context_checkpoint_path_for_session(&root, Some("session-a")),
        context_checkpoint_path_for_session(&root, Some("session-b"))
    );
}

#[test]
fn context_checkpoint_coverage_restores_only_the_verified_prefix() {
    let root = temp_test_root("context-checkpoint-coverage");
    let history = vec![
        test_message(MessageRole::User, "first requirement"),
        test_message(MessageRole::Assistant, "first answer"),
        test_message(MessageRole::User, "second requirement"),
        test_message(MessageRole::Assistant, "second answer"),
    ];
    let path = write_context_checkpoint(
        &root,
        Some("session-a"),
        "verified checkpoint",
        Some(ContextCheckpointCoverage {
            history: &history,
            covered_messages: 2,
        }),
    )
    .expect("checkpoint should write");

    let checkpoint = read_validated_context_checkpoint(&root, Some("session-a"), &history)
        .expect("coverage should validate");
    assert_eq!(checkpoint.covered_messages, 2);
    let restored = history_with_context_checkpoint(&history, checkpoint, &path)
        .expect("history should restore");

    assert_eq!(restored.len(), 3);
    assert_eq!(restored[1].content, "second requirement");
    assert_eq!(restored[2].content, "second answer");
    assert_eq!(
        restored[0]
            .metadata
            .get("covered_messages")
            .map(String::as_str),
        Some("2")
    );
}

#[test]
fn legacy_context_checkpoint_never_truncates_history() {
    let root = temp_test_root("legacy-context-checkpoint");
    let history = vec![test_message(MessageRole::User, "keep this verbatim")];
    write_context_checkpoint(&root, Some("session-a"), "legacy checkpoint", None)
        .expect("legacy checkpoint should write");

    assert!(read_validated_context_checkpoint(&root, Some("session-a"), &history,).is_none());
}

#[test]
fn context_checkpoint_rejects_changed_history_prefix() {
    let root = temp_test_root("stale-context-prefix");
    let history = vec![
        test_message(MessageRole::User, "original requirement"),
        test_message(MessageRole::Assistant, "original answer"),
        test_message(MessageRole::User, "uncovered tail"),
    ];
    write_context_checkpoint(
        &root,
        Some("session-a"),
        "checkpoint",
        Some(ContextCheckpointCoverage {
            history: &history,
            covered_messages: 2,
        }),
    )
    .expect("checkpoint should write");
    let mut changed = history.clone();
    changed[0].content = "edited requirement".to_string();

    assert!(read_validated_context_checkpoint(&root, Some("session-a"), &changed,).is_none());
    assert!(read_validated_context_checkpoint(&root, Some("session-a"), &history[..2],).is_none());
}

#[test]
fn context_checkpoint_rejects_manifest_content_mismatch() {
    let root = temp_test_root("stale-context-content");
    let history = vec![test_message(MessageRole::User, "requirement")];
    let path = write_context_checkpoint(
        &root,
        Some("session-a"),
        "checkpoint",
        Some(ContextCheckpointCoverage {
            history: &history,
            covered_messages: 1,
        }),
    )
    .expect("checkpoint should write");
    fs::write(path, "different checkpoint").expect("checkpoint should mutate");

    assert!(read_validated_context_checkpoint(&root, Some("session-a"), &history,).is_none());
}

#[test]
fn context_governor_preserves_canonical_history_and_latest_tool_round() {
    let tools = vec![ToolSpec::builtin(
        "file.read",
        "file",
        "Read a workspace file",
        ToolRisk::ReadOnly,
        r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#,
    )];
    let mut runtime = start_agent_loop(
        TaskId("context-governor-test".to_string()),
        "current goal: finish the verified implementation",
        AgentRuntimeConfig { max_turns: 24 },
    );
    let current_user = runtime.messages.pop().expect("current user should exist");
    runtime.messages.push(Message {
        role: MessageRole::System,
        content: "artifact ".repeat(8_000),
        metadata: [("kind".to_string(), "artifact_manifest".to_string())]
            .into_iter()
            .collect(),
    });
    runtime.messages.push(test_message(
        MessageRole::User,
        "旧的中文需求".repeat(5_000),
    ));
    runtime.messages.push(test_message(
        MessageRole::Assistant,
        "old answer ".repeat(5_000),
    ));
    runtime.messages.push(current_user);
    for index in 0..5 {
        runtime.messages.push(Message {
                role: MessageRole::Assistant,
                content: format!("tool round {index}"),
                metadata: [(
                    "raw_tool_calls_json".to_string(),
                    format!(
                        r#"[{{"id":"call-{index}","type":"function","function":{{"name":"file_read","arguments":"{{\"path\":\"file-{index}\"}}"}}}}]"#
                    ),
                )]
                .into_iter()
                .collect(),
            });
        runtime.messages.push(Message {
            role: MessageRole::Tool,
            content: if index == 4 {
                "latest verified evidence".to_string()
            } else {
                "older evidence ".repeat(4_000)
            },
            metadata: [("tool_call_id".to_string(), format!("call-{index}"))]
                .into_iter()
                .collect(),
        });
    }
    let canonical_history = runtime.messages.clone();

    let (request, report) =
        model_request_for_turn_with_context_budget(&runtime, &tools, None, None, 16_384, 2_048);

    assert!(report.applied);
    assert!(report.hard_limit_satisfied);
    assert!(report.omitted_messages > 0);
    assert!(request.messages.iter().any(|message| {
        message
            .content
            .contains("current goal: finish the verified implementation")
    }));
    assert!(request.messages.iter().any(|message| {
        message.metadata.get("kind").map(String::as_str) == Some("artifact_manifest")
    }));
    let assistant_index = request
        .messages
        .iter()
        .position(|message| {
            message
                .metadata
                .get("raw_tool_calls_json")
                .is_some_and(|value| value.contains("call-4"))
        })
        .expect("latest assistant tool call should remain");
    let tool_index = request
        .messages
        .iter()
        .position(|message| {
            message.metadata.get("tool_call_id").map(String::as_str) == Some("call-4")
        })
        .expect("latest tool evidence should remain");
    assert!(assistant_index < tool_index);
    assert_eq!(runtime.messages, canonical_history);
}

#[test]
fn routing_telemetry_is_reconstructed_from_completed_runs() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let run_context = [
        ("agent_run_id".to_string(), "run-1".to_string()),
        ("task_class".to_string(), "coding".to_string()),
        (
            "collaboration_policy".to_string(),
            "plan_execute_review".to_string(),
        ),
        ("requested_policy".to_string(), "auto_router".to_string()),
        ("router_model".to_string(), "model-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        run_context.clone(),
    )
    .expect("start should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::RetrievalPerformed,
        "RAG agent_context completed",
        run_context.clone(),
    )
    .expect("retrieval should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::ModelRequestFinished,
        "Agent model turn finished",
        metadata_with_context(
            [("total_tokens".to_string(), "120".to_string())]
                .into_iter()
                .collect(),
            &run_context,
        ),
    )
    .expect("model completion should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        run_context,
    )
    .expect("completion should append");
    let events = store
        .list_by_task(&phase16_task_id())
        .expect("events should load");

    let telemetry = routing_telemetry_from_events(&events);

    assert_eq!(telemetry.len(), 1);
    assert_eq!(telemetry[0].task_class, TaskClass::Coding);
    assert_eq!(telemetry[0].outcome, RoutingOutcome::Succeeded);
    assert_eq!(telemetry[0].cost_proxy, 120);
    assert_eq!(telemetry[0].retrieval_count, 1);
}

#[test]
fn routing_telemetry_excludes_unverified_workspace_completions() {
    let run_context = [
        ("agent_run_id".to_string(), "run-unverified".to_string()),
        ("task_class".to_string(), "coding".to_string()),
        (
            "collaboration_policy".to_string(),
            "plan_execute_review".to_string(),
        ),
        ("requested_policy".to_string(), "auto_router".to_string()),
        ("router_model".to_string(), "model-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        run_context.clone(),
    )
    .expect("start should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        metadata_with_context(
            [
                (
                    "completion_evidence".to_string(),
                    "unverified_mutation".to_string(),
                ),
                ("routing_learning_eligible".to_string(), "false".to_string()),
            ]
            .into_iter()
            .collect(),
            &run_context,
        ),
    )
    .expect("completion should append");

    let events = store
        .list_by_task(&phase16_task_id())
        .expect("events should load");
    assert!(routing_telemetry_from_events(&events).is_empty());
}

#[test]
fn routing_telemetry_uses_collaboration_quality_gate_as_outcome() {
    let run_context = [
        ("agent_run_id".to_string(), "run-low-quality".to_string()),
        ("task_class".to_string(), "research".to_string()),
        ("collaboration_policy".to_string(), "best_of_n".to_string()),
        ("requested_policy".to_string(), "auto_router".to_string()),
        ("router_model".to_string(), "model-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        run_context.clone(),
    )
    .expect("start should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Collaboration quality gate evaluated",
        metadata_with_context(
            [
                ("quality_pass".to_string(), "false".to_string()),
                ("quality_score".to_string(), "0.61".to_string()),
                ("safety_violations".to_string(), "0".to_string()),
            ]
            .into_iter()
            .collect(),
            &run_context,
        ),
    )
    .expect("quality gate should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        metadata_with_context(
            [("routing_learning_eligible".to_string(), "true".to_string())]
                .into_iter()
                .collect(),
            &run_context,
        ),
    )
    .expect("completion should append");

    let events = store
        .list_by_task(&phase16_task_id())
        .expect("events should load");
    let telemetry = routing_telemetry_from_events(&events);
    assert_eq!(telemetry.len(), 1);
    assert_eq!(telemetry[0].outcome, RoutingOutcome::Failed);
    assert_eq!(telemetry[0].quality_score, Some(0.61));
    assert_eq!(telemetry[0].verification_passed, Some(false));
}

#[test]
fn routing_telemetry_excludes_unverified_anytime_delivery() {
    let run_context = [
        ("agent_run_id".to_string(), "run-anytime-draft".to_string()),
        ("task_class".to_string(), "research".to_string()),
        ("collaboration_policy".to_string(), "best_of_n".to_string()),
        ("requested_policy".to_string(), "auto_router".to_string()),
        ("router_model".to_string(), "model-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        run_context.clone(),
    )
    .expect("start should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Collaboration workflow completed",
        metadata_with_context(
            [
                (
                    "anytime_selected_candidate".to_string(),
                    DIRECT_ANCHOR_CANDIDATE_ID.to_string(),
                ),
                ("anytime_selected_verified".to_string(), "false".to_string()),
                (
                    "anytime_selected_quality_bps".to_string(),
                    "6500".to_string(),
                ),
                (
                    "anytime_routing_learning_eligible".to_string(),
                    "false".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
            &run_context,
        ),
    )
    .expect("workflow completion should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        metadata_with_context(
            [("routing_learning_eligible".to_string(), "true".to_string())]
                .into_iter()
                .collect(),
            &run_context,
        ),
    )
    .expect("completion should append");

    let events = store
        .list_by_task(&phase16_task_id())
        .expect("events should load");
    assert!(routing_telemetry_from_events(&events).is_empty());
}

#[test]
fn collaboration_result_frontier_brief_is_bounded_and_excludes_executor_duplicate() {
    let control = AgentRunControl::new("pro");
    control.record_best_known_result(
        "executor",
        "executor answer",
        ResultQuality::Grounded,
        2,
        false,
        true,
    );
    for index in 0..5 {
        control.record_best_known_result(
            &format!("candidate_{index}"),
            &format!("candidate proposal {index}"),
            ResultQuality::Substantive,
            index,
            false,
            false,
        );
    }

    let brief = collaboration_result_frontier_brief(&control, "executor answer");

    assert!(!brief.contains("executor answer"));
    assert_eq!(brief.matches("Candidate ").count(), 3);
    assert!(brief.contains("candidate proposal 4"));
}

#[test]
fn completion_learning_signal_requires_post_mutation_verification() {
    let mut runtime = start_agent_loop(
        phase16_task_id(),
        "update the workspace",
        AgentRuntimeConfig::default(),
    );
    assert_eq!(completion_learning_signal(&runtime), ("non_mutating", true));

    runtime.successful_mutations = 1;
    assert_eq!(
        completion_learning_signal(&runtime),
        ("unverified_mutation", false)
    );

    runtime.verified_after_last_mutation = true;
    assert_eq!(
        completion_learning_signal(&runtime),
        ("verified_mutation", true)
    );
}

#[test]
fn workflow_telemetry_restores_versioned_plan_and_quality() {
    let models = vec!["planner".to_string(), "reviewer".to_string()];
    let workflow = AdaptiveWorkflow {
        steps: vec![
            AdaptiveWorkflowStep {
                id: "first".to_string(),
                role: "thinker".to_string(),
                model: "planner".to_string(),
                subtask: "independent analysis".to_string(),
                access: Vec::new(),
            },
            AdaptiveWorkflowStep {
                id: "second".to_string(),
                role: "worker".to_string(),
                model: "reviewer".to_string(),
                subtask: "alternative analysis".to_string(),
                access: Vec::new(),
            },
            AdaptiveWorkflowStep {
                id: "final".to_string(),
                role: "synthesizer".to_string(),
                model: "planner".to_string(),
                subtask: "synthesize both branches".to_string(),
                access: vec!["first".to_string(), "second".to_string()],
            },
        ],
    };
    let plan = WorkflowPlanIr::from_adaptive(
        "collab-1",
        "Compare approaches",
        "pro",
        "best_of_n",
        "planner",
        &workflow,
        WorkflowBudget {
            max_steps: 3,
            max_models: 2,
            max_model_turns_per_step: 5,
            max_tool_calls_per_step: 6,
            max_output_tokens_per_step: 4_096,
        },
    );
    let context = [
        ("collaboration_id".to_string(), "collab-1".to_string()),
        ("task_class".to_string(), "research".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let mut events = vec![
        Event {
            id: EventId("planned".to_string()),
            task_id: phase16_task_id(),
            sequence: 1,
            timestamp_ms: 100,
            kind: EventKind::TaskStatusChanged,
            summary: "Collaboration workflow planned".to_string(),
            metadata: metadata_with_context(
                [("workflow_ir".to_string(), plan.to_json().unwrap())]
                    .into_iter()
                    .collect(),
                &context,
            ),
        },
        Event {
            id: EventId("model".to_string()),
            task_id: phase16_task_id(),
            sequence: 2,
            timestamp_ms: 200,
            kind: EventKind::ModelRequestFinished,
            summary: "Collaboration worker finished".to_string(),
            metadata: metadata_with_context(
                [("total_tokens".to_string(), "640".to_string())]
                    .into_iter()
                    .collect(),
                &context,
            ),
        },
        Event {
            id: EventId("quality".to_string()),
            task_id: phase16_task_id(),
            sequence: 3,
            timestamp_ms: 250,
            kind: EventKind::TaskStatusChanged,
            summary: "Collaboration quality gate evaluated".to_string(),
            metadata: metadata_with_context(
                [("quality_score".to_string(), "0.875".to_string())]
                    .into_iter()
                    .collect(),
                &context,
            ),
        },
        Event {
            id: EventId("completed".to_string()),
            task_id: phase16_task_id(),
            sequence: 4,
            timestamp_ms: 500,
            kind: EventKind::TaskStatusChanged,
            summary: "Collaboration workflow completed".to_string(),
            metadata: metadata_with_context(
                [("fallback_used".to_string(), "false".to_string())]
                    .into_iter()
                    .collect(),
                &context,
            ),
        },
    ];

    let telemetry = workflow_execution_telemetry_from_events(&events, &models);

    assert_eq!(telemetry.len(), 1);
    assert_eq!(telemetry[0].plan.schema, WORKFLOW_IR_SCHEMA);
    assert_eq!(telemetry[0].task_class, TaskClass::Research);
    assert_eq!(telemetry[0].quality_score, Some(0.875));
    assert_eq!(telemetry[0].latency_ms, 400);
    assert_eq!(telemetry[0].total_tokens, 640);
    assert!(telemetry[0].succeeded);

    events.last_mut().unwrap().metadata.insert(
        "anytime_prompt_learning_eligible".to_string(),
        "false".to_string(),
    );
    assert!(workflow_execution_telemetry_from_events(&events, &models).is_empty());
}

#[test]
fn workflow_checkpoint_resume_is_scoped_to_the_latest_user_turn_and_terminal_snapshot() {
    let models = vec!["planner".to_string()];
    let prompt = "Implement and verify the change";
    let plan = WorkflowPlanIr::from_adaptive(
        "collab-resume",
        prompt,
        "pro",
        "best_of_n",
        "planner",
        &AdaptiveWorkflow {
            steps: vec![AdaptiveWorkflowStep {
                id: "final".to_string(),
                role: "synthesizer".to_string(),
                model: "planner".to_string(),
                subtask: "produce verified guidance".to_string(),
                access: Vec::new(),
            }],
        },
        WorkflowBudget {
            max_steps: 1,
            max_models: 1,
            max_model_turns_per_step: 2,
            max_tool_calls_per_step: 2,
            max_output_tokens_per_step: 2_048,
        },
    );
    let mut events = vec![Event {
        id: EventId("user-1".to_string()),
        task_id: phase16_task_id(),
        sequence: 1,
        timestamp_ms: 100,
        kind: EventKind::MessageAdded,
        summary: "user message".to_string(),
        metadata: [
            ("session_id".to_string(), "session-a".to_string()),
            ("role".to_string(), "user".to_string()),
            ("content".to_string(), prompt.to_string()),
        ]
        .into_iter()
        .collect(),
    }];
    let resume_key =
        workflow_resume_key_from_events(&events, Some("session-a"), prompt, "pro", "best_of_n");
    let mut checkpoint = WorkflowExecutionCheckpoint::new(&resume_key, plan, 110);
    events.push(Event {
        id: EventId("checkpoint-running".to_string()),
        task_id: phase16_task_id(),
        sequence: 2,
        timestamp_ms: 120,
        kind: EventKind::TaskStatusChanged,
        summary: "Collaboration workflow checkpoint created".to_string(),
        metadata: [
            ("workflow_resume_key".to_string(), resume_key.clone()),
            (
                "workflow_checkpoint".to_string(),
                checkpoint.to_json().unwrap(),
            ),
        ]
        .into_iter()
        .collect(),
    });
    assert!(
        resumable_workflow_checkpoint_from_events(&events, &resume_key, prompt, &models,).is_some()
    );

    checkpoint.begin_step("final", "planner", 130).unwrap();
    checkpoint
        .complete_step(
            "final",
            "planner",
            "draft".to_string(),
            "[]".to_string(),
            140,
        )
        .unwrap();
    checkpoint.finalize("verified".to_string(), 150).unwrap();
    events.push(Event {
        id: EventId("checkpoint-final".to_string()),
        task_id: phase16_task_id(),
        sequence: 3,
        timestamp_ms: 150,
        kind: EventKind::TaskStatusChanged,
        summary: "Collaboration workflow checkpoint finalized".to_string(),
        metadata: [
            ("workflow_resume_key".to_string(), resume_key.clone()),
            (
                "workflow_checkpoint".to_string(),
                checkpoint.to_json().unwrap(),
            ),
        ]
        .into_iter()
        .collect(),
    });
    assert!(
        resumable_workflow_checkpoint_from_events(&events, &resume_key, prompt, &models,).is_none()
    );

    events.push(Event {
        id: EventId("continuation-replay".to_string()),
        task_id: phase16_task_id(),
        sequence: 4,
        timestamp_ms: 155,
        kind: EventKind::MessageAdded,
        summary: "user message".to_string(),
        metadata: [
            ("session_id".to_string(), "session-a".to_string()),
            ("role".to_string(), "user".to_string()),
            ("content".to_string(), prompt.to_string()),
            ("continuation_replay".to_string(), "true".to_string()),
        ]
        .into_iter()
        .collect(),
    });
    let continuation_key =
        workflow_resume_key_from_events(&events, Some("session-a"), prompt, "pro", "best_of_n");
    assert_eq!(resume_key, continuation_key);

    events.push(Event {
        id: EventId("user-2".to_string()),
        task_id: phase16_task_id(),
        sequence: 5,
        timestamp_ms: 160,
        kind: EventKind::MessageAdded,
        summary: "user message".to_string(),
        metadata: [
            ("session_id".to_string(), "session-a".to_string()),
            ("role".to_string(), "user".to_string()),
            ("content".to_string(), prompt.to_string()),
        ]
        .into_iter()
        .collect(),
    });
    let repeated_prompt_key =
        workflow_resume_key_from_events(&events, Some("session-a"), prompt, "pro", "best_of_n");
    assert_ne!(resume_key, repeated_prompt_key);
}

#[test]
fn prompt_evolution_uses_holdout_results_to_select_a_new_generation() {
    let seed = ConductorPromptGenome::seed_for_effort("auto");
    let genome_json = serde_json::to_string(&seed).expect("genome should serialize");
    let mut events = Vec::new();
    for run_index in 0..6u64 {
        let collaboration_id = format!("evolution-{run_index}");
        let plan = WorkflowPlanIr::from_adaptive_with_profile(
            collaboration_id.clone(),
            "Implement and verify a change",
            "auto",
            "best_of_n",
            "planner",
            seed.id.clone(),
            &AdaptiveWorkflow {
                steps: vec![AdaptiveWorkflowStep {
                    id: "final".to_string(),
                    role: "synthesizer".to_string(),
                    model: "planner".to_string(),
                    subtask: "produce the verified result".to_string(),
                    access: Vec::new(),
                }],
            },
            WorkflowBudget {
                max_steps: 3,
                max_models: 2,
                max_model_turns_per_step: 5,
                max_tool_calls_per_step: 6,
                max_output_tokens_per_step: 4_096,
            },
        );
        let context = [
            ("collaboration_id".to_string(), collaboration_id),
            ("prompt_profile".to_string(), seed.id.clone()),
            ("prompt_effort".to_string(), "auto".to_string()),
            ("prompt_genome".to_string(), genome_json.clone()),
            ("task_class".to_string(), "coding".to_string()),
        ]
        .into_iter()
        .collect::<Metadata>();
        let sequence = run_index * 4 + 1;
        events.push(Event {
            id: EventId(format!("selected-{run_index}")),
            task_id: phase16_task_id(),
            sequence,
            timestamp_ms: 1_000 + run_index * 100,
            kind: EventKind::TaskStatusChanged,
            summary: "Conductor prompt profile selected".to_string(),
            metadata: context.clone(),
        });
        events.push(Event {
            id: EventId(format!("planned-{run_index}")),
            task_id: phase16_task_id(),
            sequence: sequence + 1,
            timestamp_ms: 1_020 + run_index * 100,
            kind: EventKind::TaskStatusChanged,
            summary: "Collaboration workflow planned".to_string(),
            metadata: metadata_with_context(
                [("workflow_ir".to_string(), plan.to_json().unwrap())]
                    .into_iter()
                    .collect(),
                &context,
            ),
        });
        events.push(Event {
            id: EventId(format!("quality-{run_index}")),
            task_id: phase16_task_id(),
            sequence: sequence + 2,
            timestamp_ms: 1_040 + run_index * 100,
            kind: EventKind::TaskStatusChanged,
            summary: "Collaboration quality gate evaluated".to_string(),
            metadata: metadata_with_context(
                [("quality_score".to_string(), "0.9".to_string())]
                    .into_iter()
                    .collect(),
                &context,
            ),
        });
        events.push(Event {
            id: EventId(format!("completed-{run_index}")),
            task_id: phase16_task_id(),
            sequence: sequence + 3,
            timestamp_ms: 1_080 + run_index * 100,
            kind: EventKind::TaskStatusChanged,
            summary: "Collaboration workflow completed".to_string(),
            metadata: context,
        });
    }

    let live_only =
        evaluate_prompt_evolution(&events, "auto").expect("live prompt outcomes should evaluate");
    assert!(!live_only.frontier_ids.contains(&seed.id));
    assert!(live_only.champion_id.is_none());

    for evaluation_index in 0..8u64 {
        let mode = if evaluation_index < 4 {
            PromptEvaluationMode::PairedExecution
        } else {
            PromptEvaluationMode::ReplayExecution
        };
        let split = if mode.is_replay() {
            PromptEvaluationSplit::Holdout
        } else {
            PromptEvaluationSplit::Train
        };
        let reflection_packet = (mode == PromptEvaluationMode::PairedExecution).then(|| {
            AgentEvaluationReflectionPacket {
                suite_id: "runtime-prompt-evolution".to_string(),
                suite_version: 2,
                case_id: format!("case-{evaluation_index}"),
                category: "coding".to_string(),
                run_id: format!("pair-{evaluation_index}"),
                seed: evaluation_index,
                candidate_id: seed.id.clone(),
                candidate_fingerprint: "seed-fingerprint".to_string(),
                model_fingerprints: BTreeMap::new(),
                input: "Implement and verify a change".to_string(),
                steps: Vec::new(),
                final_output: "verified".to_string(),
                verifier: AgentEvaluationVerifierOutcome {
                    source: AgentEvaluationEvidenceSource::Judge,
                    passed: true,
                    score: 0.9,
                    checks: Vec::new(),
                },
                actionable_feedback: ActionableSideInformation {
                    summary: "preserve verification coverage".to_string(),
                    ..ActionableSideInformation::default()
                },
            }
        });
        let observation = PromptEvolutionObservation {
            profile_id: seed.id.clone(),
            evaluation_id: format!("pair-{evaluation_index}"),
            case_id: format!("case-{evaluation_index}"),
            opponent_profile_id: Some("baseline-opponent".to_string()),
            task_class: "coding".to_string(),
            split,
            mode,
            format_valid: true,
            succeeded: true,
            quality_score: 0.9,
            latency_ms: 1_000,
            total_tokens: 800,
            estimated_cost_microusd: 0,
            safety_violations: 0,
            relative_reward: Some(0.2),
            step_credits: vec![PromptStepCredit {
                step_id: "final".to_string(),
                role: "synthesizer".to_string(),
                succeeded: true,
                attempts: 1,
                evidence_count: 1,
                latency_ms: 1_000,
                total_tokens: 800,
                credit: 0.9,
            }],
            reflection_packet,
        };
        events.push(Event {
            id: EventId(format!("pair-event-{evaluation_index}")),
            task_id: phase16_task_id(),
            sequence: 100 + evaluation_index,
            timestamp_ms: 10_000 + evaluation_index * 100,
            kind: EventKind::TaskStatusChanged,
            summary: "Conductor pairwise evaluation".to_string(),
            metadata: [
                ("prompt_effort".to_string(), "auto".to_string()),
                (
                    "prompt_observation".to_string(),
                    serde_json::to_string(&observation).unwrap(),
                ),
            ]
            .into_iter()
            .collect(),
        });
    }

    let evaluation = evaluate_prompt_evolution(&events, "auto")
        .expect("paired and replay prompt outcomes should evaluate");
    let seed_observations = evaluation
        .observations
        .iter()
        .filter(|observation| observation.profile_id == seed.id)
        .collect::<Vec<_>>();

    assert_eq!(seed_observations.len(), 14);
    assert_eq!(
        seed_observations
            .iter()
            .filter(|observation| observation.mode == PromptEvaluationMode::PairedExecution)
            .count(),
        4
    );
    assert_eq!(
        seed_observations
            .iter()
            .filter(|observation| observation.mode == PromptEvaluationMode::ReplayExecution)
            .count(),
        4
    );
    assert!(evaluation.frontier_ids.contains(&seed.id));
    assert_eq!(evaluation.next_profile.generation, 1);
    assert_ne!(evaluation.next_profile.id, seed.id);
    assert_eq!(
        evaluation
            .mutation_parent
            .as_ref()
            .map(|genome| genome.id.as_str()),
        Some(seed.id.as_str())
    );
    assert_eq!(evaluation.mutation_trajectories.len(), 4);
    assert!(evaluation
        .mutation_trajectories
        .iter()
        .all(|packet| packet.candidate_id == seed.id));
}

#[test]
fn replay_holdout_uses_only_a_different_completed_workflow() {
    let workflow = |id: &str, objective: &str| {
        WorkflowPlanIr::from_adaptive_with_profile(
            id.to_string(),
            objective,
            "auto",
            "best_of_n",
            "planner",
            "seed-auto-v1",
            &AdaptiveWorkflow {
                steps: vec![AdaptiveWorkflowStep {
                    id: "final".to_string(),
                    role: "synthesizer".to_string(),
                    model: "planner".to_string(),
                    subtask: "finish".to_string(),
                    access: Vec::new(),
                }],
            },
            WorkflowBudget {
                max_steps: 4,
                max_models: 2,
                max_model_turns_per_step: 3,
                max_tool_calls_per_step: 4,
                max_output_tokens_per_step: 2_048,
            },
        )
    };
    let planned = |sequence, id: &str, objective: &str| Event {
        id: EventId(format!("planned-{id}")),
        task_id: phase16_task_id(),
        sequence,
        timestamp_ms: sequence * 10,
        kind: EventKind::TaskStatusChanged,
        summary: "Collaboration workflow planned".to_string(),
        metadata: [
            ("collaboration_id".to_string(), id.to_string()),
            (
                "workflow_ir".to_string(),
                workflow(id, objective).to_json().unwrap(),
            ),
            ("task_class".to_string(), "coding".to_string()),
        ]
        .into_iter()
        .collect(),
    };
    let completed = |sequence, id: &str| Event {
        id: EventId(format!("completed-{id}")),
        task_id: phase16_task_id(),
        sequence,
        timestamp_ms: sequence * 10,
        kind: EventKind::TaskStatusChanged,
        summary: "Collaboration workflow completed".to_string(),
        metadata: [("collaboration_id".to_string(), id.to_string())]
            .into_iter()
            .collect(),
    };
    let events = vec![
        planned(1, "current", "Current task"),
        completed(2, "current"),
        planned(3, "holdout", "Older completed task"),
        completed(4, "holdout"),
        planned(5, "failed", "Failed historical task"),
    ];

    let replay = prompt_replay_case(&events, "Current task", 0).unwrap();

    assert_eq!(replay.objective, "Older completed task");
    assert_eq!(replay.task_class, "coding");
}

#[test]
fn offline_prompt_dataset_is_project_scoped_deterministic_and_split_stable() {
    let run_events =
        |sequence: u64,
         run_id: &str,
         project_id: &str,
         objective: &str,
         terminal_summary: Option<&str>| {
            let metadata = [
                ("agent_run_id".to_string(), run_id.to_string()),
                ("project_id".to_string(), project_id.to_string()),
                ("session_id".to_string(), format!("session-{run_id}")),
                ("task_class".to_string(), "coding".to_string()),
                ("prompt".to_string(), objective.to_string()),
            ]
            .into_iter()
            .collect::<Metadata>();
            let mut events = vec![Event {
                id: EventId(format!("started-{run_id}")),
                task_id: phase16_task_id(),
                sequence,
                timestamp_ms: sequence * 10,
                kind: EventKind::TaskStatusChanged,
                summary: "Agent task started".to_string(),
                metadata: metadata.clone(),
            }];
            if let Some(terminal_summary) = terminal_summary {
                events.push(Event {
                    id: EventId(format!("completed-{run_id}")),
                    task_id: phase16_task_id(),
                    sequence: sequence + 1,
                    timestamp_ms: (sequence + 1) * 10,
                    kind: EventKind::TaskStatusChanged,
                    summary: terminal_summary.to_string(),
                    metadata,
                });
            }
            events
        };
    let mut events = Vec::new();
    events.extend(run_events(
        1,
        "a",
        "project-a",
        "Audit the agent loop",
        Some("Agent task completed"),
    ));
    events.extend(run_events(
        3,
        "b",
        "project-a",
        "Improve retrieval fusion",
        Some("Agent task completed"),
    ));
    events.extend(run_events(
        5,
        "c",
        "project-a",
        "Verify queue steering",
        Some("Agent task completed"),
    ));
    events.extend(run_events(
        7,
        "failed",
        "project-a",
        "Recover a failed long-running task",
        Some("Agent task failed"),
    ));
    events.extend(run_events(
        9,
        "cancelled",
        "project-a",
        "Preserve state after cancellation",
        Some("Agent task cancelled"),
    ));
    events.extend(run_events(
        11,
        "other",
        "project-b",
        "Unrelated project",
        Some("Agent task completed"),
    ));
    events.extend(run_events(
        13,
        "incomplete",
        "project-a",
        "Incomplete run",
        None,
    ));

    let first = prompt_offline_dataset(&events, "project-a");
    let second = prompt_offline_dataset(&events, "project-a");

    assert_eq!(first, second);
    assert_eq!(first.len(), 5);
    assert!(first.iter().all(|case| case.project_id == "project-a"));
    assert!(
        first
            .iter()
            .filter(|case| case.split == PromptEvaluationSplit::Train)
            .count()
            >= 2
    );
    assert!(first
        .iter()
        .any(|case| case.split == PromptEvaluationSplit::Holdout));
    assert!(!first.iter().any(|case| case.objective == "Incomplete run"));
    assert!(first
        .iter()
        .any(|case| case.objective == "Recover a failed long-running task"));
    assert!(first
        .iter()
        .any(|case| case.objective == "Preserve state after cancellation"));

    let split_manifest = first
        .iter()
        .map(|case| (case.id.clone(), case.split))
        .collect::<BTreeMap<_, _>>();
    events.push(Event {
        id: EventId("offline-split-snapshot".to_string()),
        task_id: phase16_task_id(),
        sequence: 15,
        timestamp_ms: 150,
        kind: EventKind::TaskStatusChanged,
        summary: "Conductor offline dataset selected".to_string(),
        metadata: [
            ("project_id".to_string(), "project-a".to_string()),
            (
                "dataset_split_manifest".to_string(),
                serde_json::to_string(&split_manifest).unwrap(),
            ),
        ]
        .into_iter()
        .collect(),
    });
    events.extend(run_events(
        16,
        "d",
        "project-a",
        "Check recovery checkpoints",
        Some("Agent task completed"),
    ));
    events.extend(run_events(
        18,
        "e",
        "project-a",
        "Review quality gates",
        Some("Agent task completed"),
    ));

    let grown = prompt_offline_dataset(&events, "project-a");
    for original in &first {
        assert_eq!(
            grown
                .iter()
                .find(|case| case.id == original.id)
                .map(|case| case.split),
            Some(original.split),
            "existing offline split must not drift as the dataset grows"
        );
    }
}

#[test]
fn replay_holdout_accepts_a_completed_bounded_collaboration() {
    let profile = ConductorPromptGenome::seed_for_effort("pro");
    let profile_event = Event {
        id: EventId("bounded-profile".to_string()),
        task_id: phase16_task_id(),
        sequence: 1,
        timestamp_ms: 100,
        kind: EventKind::TaskStatusChanged,
        summary: "Conductor prompt profile selected".to_string(),
        metadata: [
            ("collaboration_id".to_string(), "bounded-1".to_string()),
            ("collaboration_profile".to_string(), "bounded".to_string()),
            ("agent_run_id".to_string(), "run-1".to_string()),
            ("prompt_effort".to_string(), "pro".to_string()),
            ("prompt_profile".to_string(), profile.id.clone()),
            (
                "prompt_genome".to_string(),
                serde_json::to_string(&profile).unwrap(),
            ),
            (
                "prompt_objective".to_string(),
                "Completed bounded task".to_string(),
            ),
            ("task_class".to_string(), "coding".to_string()),
        ]
        .into_iter()
        .collect(),
    };
    let terminal = Event {
        id: EventId("bounded-terminal".to_string()),
        task_id: phase16_task_id(),
        sequence: 2,
        timestamp_ms: 200,
        kind: EventKind::TaskStatusChanged,
        summary: "Agent task completed".to_string(),
        metadata: [("agent_run_id".to_string(), "run-1".to_string())]
            .into_iter()
            .collect(),
    };
    let events = vec![profile_event, terminal];

    let replay = prompt_replay_case(&events, "Current task", 0).unwrap();
    let model = build_prompt_evolution_read_model(&events, 2, 2);

    assert_eq!(replay.objective, "Completed bounded task");
    assert_eq!(replay.task_class, "coding");
    assert_eq!(model.genomes.len(), 1);
    assert_eq!(model.observations.len(), 1);
    assert!(model.observations[0].1.format_valid);
}

#[test]
fn bidirectional_pairwise_judging_normalizes_position_and_merges_feedback() {
    let feedback = |summary: &str, change: &str| ActionableSideInformation {
        summary: summary.to_string(),
        suggested_changes: vec![change.to_string()],
        ..ActionableSideInformation::default()
    };
    let forward = PromptPairwiseEvaluationPayload {
        score_a: 0.8,
        score_b: 0.4,
        safety_violations_a: 0,
        safety_violations_b: 1,
        step_scores_a: [("inspect".to_string(), 0.8)].into_iter().collect(),
        step_scores_b: [("inspect".to_string(), 0.4)].into_iter().collect(),
        feedback_a: feedback("forward A", "preserve evidence"),
        feedback_b: feedback("forward B", "fix verification"),
    };
    let reverse = PromptPairwiseEvaluationPayload {
        score_a: 0.2,
        score_b: 0.6,
        safety_violations_a: 0,
        safety_violations_b: 2,
        step_scores_a: [("inspect".to_string(), 0.2)].into_iter().collect(),
        step_scores_b: [("inspect".to_string(), 0.6)].into_iter().collect(),
        feedback_a: feedback("reverse B", "fix verification"),
        feedback_b: feedback("reverse A", "reduce unsupported claims"),
    };

    let aggregate =
        aggregate_prompt_pairwise_payloads(forward, reverse_prompt_pairwise_payload(reverse));

    assert!((aggregate.score_a - 0.7).abs() < f64::EPSILON * 8.0);
    assert!((aggregate.score_b - 0.3).abs() < f64::EPSILON * 8.0);
    assert_eq!(aggregate.safety_violations_a, 2);
    assert_eq!(aggregate.safety_violations_b, 1);
    assert_eq!(aggregate.step_scores_a.get("inspect"), Some(&0.7));
    assert!(aggregate.feedback_a.summary.contains("forward A"));
    assert!(aggregate.feedback_a.summary.contains("reverse A"));
    assert_eq!(aggregate.feedback_b.suggested_changes.len(), 1);
}

#[test]
fn pairwise_observation_keeps_relative_and_per_step_credit() {
    let profile = ConductorPromptGenome::seed_for_effort("auto");
    let opponent_profile = ConductorPromptGenome {
        id: "opponent".to_string(),
        ..profile.clone()
    };
    let candidate_plan = PromptPlanCandidate {
        genome: profile,
        plan: Some(WorkflowPlanIr::from_adaptive_with_profile(
            "candidate",
            "Verify a change",
            "auto",
            "best_of_n",
            "planner",
            "seed-auto-v1",
            &AdaptiveWorkflow {
                steps: vec![AdaptiveWorkflowStep {
                    id: "verify".to_string(),
                    role: "reviewer".to_string(),
                    model: "planner".to_string(),
                    subtask: "verify".to_string(),
                    access: Vec::new(),
                }],
            },
            WorkflowBudget {
                max_steps: 4,
                max_models: 2,
                max_model_turns_per_step: 3,
                max_tool_calls_per_step: 4,
                max_output_tokens_per_step: 2_048,
            },
        )),
        raw_output: String::new(),
        latency_ms: 120,
        total_tokens: 80,
    };
    let opponent_plan = PromptPlanCandidate {
        genome: opponent_profile,
        plan: candidate_plan.plan.clone(),
        raw_output: String::new(),
        latency_ms: 140,
        total_tokens: 90,
    };
    let candidate = PromptExecutionCandidate {
        plan: candidate_plan,
        execution: PromptWorkflowExecution {
            succeeded: true,
            final_output: "verified".to_string(),
            steps: vec![PromptExecutionStep {
                id: "verify".to_string(),
                role: "reviewer".to_string(),
                model: "reviewer-model".to_string(),
                prompt: "verify the result".to_string(),
                attempts: 1,
                succeeded: true,
                output: "verified".to_string(),
                tool_calls: Vec::new(),
                errors: Vec::new(),
                latency_ms: 100,
                total_tokens: 60,
                evidence_count: 1,
            }],
            latency_ms: 100,
            total_tokens: 60,
        },
    };
    let opponent = PromptExecutionCandidate {
        plan: opponent_plan,
        execution: PromptWorkflowExecution {
            succeeded: true,
            final_output: "reviewed".to_string(),
            steps: vec![PromptExecutionStep {
                id: "verify".to_string(),
                role: "reviewer".to_string(),
                model: "reviewer-model".to_string(),
                prompt: "review the result".to_string(),
                attempts: 1,
                succeeded: true,
                output: "reviewed".to_string(),
                tool_calls: Vec::new(),
                errors: Vec::new(),
                latency_ms: 120,
                total_tokens: 70,
                evidence_count: 1,
            }],
            latency_ms: 120,
            total_tokens: 70,
        },
    };
    let observation = prompt_pairwise_observation(
        &candidate,
        &opponent,
        "Fix the project and run tests",
        "pair-1",
        "coding",
        PromptEvaluationSplit::Train,
        PromptEvaluationMode::PairedShadow,
        0.85,
        0.55,
        0,
        &[("verify".to_string(), 0.78)].into_iter().collect(),
        ActionableSideInformation {
            summary: "candidate verified more completely".to_string(),
            ..ActionableSideInformation::default()
        },
        &[],
    );

    assert!((observation.relative_reward.unwrap_or_default() - 0.3).abs() < f64::EPSILON * 4.0);
    assert_eq!(observation.step_credits.len(), 1);
    assert_eq!(observation.step_credits[0].step_id, "verify");
    assert_eq!(observation.step_credits[0].credit, 0.78);
    assert!(observation.reflection_packet.is_none());

    let secret = "evaluation-secret-token".to_string();
    let bearer = "Bearer runtime-reflection-token";
    let mut traced_candidate = candidate.clone();
    traced_candidate.execution.steps[0].prompt =
        format!("inspect with {secret}\nAuthorization: {bearer}");
    traced_candidate.execution.steps[0].tool_calls = vec![AgentEvaluationToolTrace {
        tool: "file.read".to_string(),
        request: format!("{{\"token\":\"{secret}\"}}"),
        response: format!("verified with {secret}\n{bearer}"),
        error: None,
    }];
    let traced = prompt_pairwise_observation(
        &traced_candidate,
        &opponent,
        &format!("Fix the project using {secret}"),
        "pair-2",
        "coding",
        PromptEvaluationSplit::Train,
        PromptEvaluationMode::PairedExecution,
        0.85,
        0.55,
        0,
        &[("verify".to_string(), 0.78)].into_iter().collect(),
        ActionableSideInformation {
            summary: format!("verified without exposing {secret}"),
            ..ActionableSideInformation::default()
        },
        std::slice::from_ref(&secret),
    );
    let packet = traced
        .reflection_packet
        .expect("executed feedback should produce a reflection packet");
    let encoded = serde_json::to_string(&packet).expect("packet should serialize");
    assert!(!encoded.contains(&secret));
    assert!(!encoded.contains("runtime-reflection-token"));
    assert!(encoded.contains("[REDACTED]"));
    assert_eq!(packet.steps[0].tool_calls.len(), 1);
}

#[test]
fn agent_runtime_turn_budget_tracks_effort_and_extends_on_resume() {
    let control = AgentRunControl::new("pro");
    let config = control.runtime_config();
    assert_eq!(config.max_turns, 384);

    let mut runtime = start_agent_loop(phase16_task_id(), "continue", config);
    runtime.turn = 23;
    control.extend_runtime_budget(&mut runtime);

    assert_eq!(runtime.max_turns, 407);
}

#[test]
fn agent_run_budget_extends_only_after_material_progress_and_stops_cycles() {
    let control = AgentRunControl::new("fast");
    for call in 1..=6 {
        assert_eq!(control.begin_model_call("executor"), Ok(call));
    }
    assert!(control.record_observation("model_result", "executor", "new model wording"));
    assert_eq!(
        control.begin_model_call("executor"),
        Err(RunStopReason::ModelCallBudgetExceeded)
    );

    let control = AgentRunControl::new("fast");
    for call in 1..=6 {
        assert_eq!(control.begin_model_call("executor"), Ok(call));
    }
    assert!(control.record_checkpoint("tool_result", "file.read", "new evidence"));
    assert_eq!(control.begin_model_call("executor"), Ok(7));
    assert!(!control.record_checkpoint("tool_result", "file.read", "new evidence"));

    let cycle_control = AgentRunControl::new("fast");
    for input in ["a", "b", "a", "b", "a", "b", "a"] {
        assert!(cycle_control
            .begin_tool_call("executor", "file.read", input)
            .is_ok());
    }
    assert_eq!(
        cycle_control.begin_tool_call("executor", "file.read", "b"),
        Err(RunStopReason::RepeatedAction)
    );
}

#[test]
fn agent_runtime_never_completes_with_an_empty_model_answer() {
    let mut runtime = start_agent_loop(
        phase16_task_id(),
        "complete the task",
        AgentRuntimeConfig { max_turns: 6 },
    );
    let empty_response = || model_provider::ModelResponse {
        message: Message {
            role: MessageRole::Assistant,
            content: String::new(),
            metadata: Metadata::new(),
        },
        raw_tool_calls_json: None,
        tool_calls: Vec::new(),
        metadata: Metadata::new(),
    };

    assert!(matches!(
        advance_with_model_response(&mut runtime, empty_response(), &[]),
        AgentAdvance::Retry { .. }
    ));
    assert!(matches!(
        advance_with_model_response(&mut runtime, empty_response(), &[]),
        AgentAdvance::Retry { .. }
    ));
    assert!(matches!(
        advance_with_model_response(&mut runtime, empty_response(), &[]),
        AgentAdvance::Failed { .. }
    ));
}

#[test]
fn image_generation_run_cannot_complete_without_the_configured_tool() {
    let mut runtime = start_agent_loop(
        phase16_task_id(),
        "生成一张图片",
        AgentRuntimeConfig { max_turns: 6 },
    );
    let run_context = [("image_generation_required".to_string(), "true".to_string())]
        .into_iter()
        .collect();
    assert!(!required_image_generation_satisfied(&runtime, &run_context));

    append_tool_observation(
        &mut runtime,
        agent_core::ToolCallId("image-call".to_string()),
        "tool=image.generate\nstatus=succeeded\noutput=generated-images/cat.png",
    );

    assert!(required_image_generation_satisfied(&runtime, &run_context));
}

#[test]
fn conductor_evaluation_repairs_invalid_structure_before_scoring() {
    let genome = ConductorPromptGenome::seed_for_effort("fast");
    let routing = RoutingContext::from_prompt("Answer a focused question", Vec::new());
    let harness = ConductorHarness::new(ConductorRequest {
        workflow_id: "repair-evaluation".to_string(),
        objective: "Answer a focused question".to_string(),
        recent_context: String::new(),
        effort: "fast".to_string(),
        policy: "direct".to_string(),
        conductor_model: "planner".to_string(),
        worker_models: vec!["worker-a".to_string()],
        role_hints: ConductorRoleHints {
            planner: "worker-a".to_string(),
            executor: "worker-a".to_string(),
            reviewer: "worker-a".to_string(),
            synthesizer: "worker-a".to_string(),
        },
        budget: WorkflowBudget {
            max_steps: 2,
            max_models: 1,
            max_model_turns_per_step: 1,
            max_tool_calls_per_step: 0,
            max_output_tokens_per_step: 1_024,
        },
        execution_contract: ConductorExecutionContract::from_routing(
            &routing,
            "fast",
            OrchestrationPolicy::Single,
        ),
        prior_hint: None,
        prompt_evolution_enabled: true,
        prompt_genome: genome.clone(),
    });
    let mut calls = 0usize;
    let mut prompts = Vec::new();

    let candidate = evaluate_conductor_prompt_profile_with_runner(&harness, &genome, |prompt| {
        calls += 1;
        prompts.push(prompt);
        CollaborationCompletion {
            content: Some(if calls == 1 {
                "not a workflow".to_string()
            } else {
                r#"{"steps":[{"id":"final","role":"synthesizer","model":"worker-a","subtask":"answer directly","access":[]}]}"#.to_string()
            }),
            error: None,
            latency_ms: if calls == 1 { 7 } else { 11 },
            usage: [(
                "total_tokens".to_string(),
                if calls == 1 { "13" } else { "17" }.to_string(),
            )]
            .into_iter()
            .collect(),
            evidence: Vec::new(),
        }
    });

    assert!(candidate.plan.is_some());
    assert_eq!(calls, 2);
    assert_eq!(candidate.latency_ms, 18);
    assert_eq!(candidate.total_tokens, 30);
    assert!(prompts[1].contains("deterministic Cindx Harness"));
    assert!(prompts[1].contains("not a workflow"));
}

#[test]
fn conductor_evaluation_uses_a_collaborative_fallback_after_failed_repair() {
    let genome =
        ConductorPromptGenome::seed_for_effort("auto").with_effort_capability_floor("auto");
    let routing = RoutingContext::from_prompt(
        "Compare two implementation strategies with evidence",
        Vec::new(),
    );
    let harness = ConductorHarness::new(ConductorRequest {
        workflow_id: "fallback-evaluation".to_string(),
        objective: "Compare two implementation strategies with evidence".to_string(),
        recent_context: String::new(),
        effort: "auto".to_string(),
        policy: "best_of_n".to_string(),
        conductor_model: "planner".to_string(),
        worker_models: vec!["worker-a".to_string(), "worker-b".to_string()],
        role_hints: ConductorRoleHints {
            planner: "worker-a".to_string(),
            executor: "worker-b".to_string(),
            reviewer: "worker-b".to_string(),
            synthesizer: "worker-a".to_string(),
        },
        budget: WorkflowBudget {
            max_steps: 3,
            max_models: 2,
            max_model_turns_per_step: 2,
            max_tool_calls_per_step: 4,
            max_output_tokens_per_step: 2_048,
        },
        execution_contract: ConductorExecutionContract::from_routing(
            &routing,
            "auto",
            OrchestrationPolicy::BestOfN { candidates: 2 },
        ),
        prior_hint: None,
        prompt_evolution_enabled: true,
        prompt_genome: genome.clone(),
    });
    let mut calls = 0usize;

    let candidate = evaluate_conductor_prompt_profile_with_runner(&harness, &genome, |_| {
        calls += 1;
        CollaborationCompletion {
            content: Some("still not a workflow".to_string()),
            error: None,
            latency_ms: 5,
            usage: BTreeMap::new(),
            evidence: Vec::new(),
        }
    });

    let plan = candidate
        .plan
        .expect("failed conductor repair should use the deterministic graph");
    assert_eq!(calls, CONDUCTOR_MAX_ATTEMPTS);
    assert_eq!(plan.steps.len(), 3);
    assert_eq!(
        plan.steps
            .iter()
            .take(2)
            .filter(|step| step.access.is_empty())
            .count(),
        2
    );
    assert!(candidate
        .raw_output
        .contains("Deterministic harness fallback applied"));
}

#[test]
fn execution_arena_runs_dependencies_before_final_synthesis() {
    let profile = ConductorPromptGenome::seed_for_effort("auto");
    let plan = WorkflowPlanIr::from_adaptive_with_profile(
        "arena-candidate",
        "Investigate and summarize",
        "auto",
        "best_of_n",
        "planner",
        profile.id.clone(),
        &AdaptiveWorkflow {
            steps: vec![
                AdaptiveWorkflowStep {
                    id: "investigate".to_string(),
                    role: "worker".to_string(),
                    model: "worker-a".to_string(),
                    subtask: "investigate evidence".to_string(),
                    access: Vec::new(),
                },
                AdaptiveWorkflowStep {
                    id: "final".to_string(),
                    role: "synthesizer".to_string(),
                    model: "worker-b".to_string(),
                    subtask: "synthesize the result".to_string(),
                    access: vec!["investigate".to_string()],
                },
            ],
        },
        WorkflowBudget {
            max_steps: 4,
            max_models: 2,
            max_model_turns_per_step: 2,
            max_tool_calls_per_step: 1,
            max_output_tokens_per_step: 2_048,
        },
    );
    let prompts = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured = Arc::clone(&prompts);
    let runner: PromptEvaluationRunner = Arc::new(move |request| {
        captured
            .lock()
            .expect("prompt capture lock")
            .push(request.prompt);
        CollaborationCompletion {
            content: Some(if request.model == "worker-a" {
                "branch-output".to_string()
            } else {
                "final-output".to_string()
            }),
            error: None,
            latency_ms: 10,
            usage: [("total_tokens".to_string(), "20".to_string())]
                .into_iter()
                .collect(),
            evidence: Vec::new(),
        }
    });

    let candidate = execute_prompt_workflow_candidate_with_runner(
        "Investigate and summarize",
        PromptPlanCandidate {
            genome: profile,
            plan: Some(plan),
            raw_output: String::new(),
            latency_ms: 5,
            total_tokens: 10,
        },
        runner,
    );

    assert!(candidate.execution.succeeded);
    assert_eq!(candidate.execution.final_output, "final-output");
    assert_eq!(candidate.execution.steps.len(), 2);
    assert_eq!(candidate.execution.total_tokens, 40);
    let prompts = prompts.lock().expect("prompt capture lock");
    assert_eq!(prompts.len(), 2);
    assert!(prompts[1].contains("[investigate]\nbranch-output"));
}

#[test]
fn evaluation_sandbox_exposes_and_executes_only_read_only_workspace_tools() {
    let root = temp_test_root("cindx-evaluation-sandbox");
    fs::create_dir_all(&root).expect("sandbox root should be created");
    fs::write(root.join("evidence.txt"), "verified workspace evidence")
        .expect("sandbox fixture should be written");
    let registry = ToolRegistry::with_workspace_tools(root.clone());

    let tools = prompt_evaluation_tool_specs(
        &registry,
        "Inspect evidence.txt",
        32_000,
        &WorkflowToolPolicy::ReadOnlyExploration,
    );
    assert!(!tools.is_empty());
    assert!(tools.iter().all(|tool| tool.risk == ToolRisk::ReadOnly));
    assert!(tools.iter().any(|tool| tool.name == "file.read"));
    assert!(!tools.iter().any(|tool| {
        matches!(
            tool.risk,
            ToolRisk::WritesWorkspace
                | ToolRisk::ExecutesProcess
                | ToolRisk::UsesNetwork
                | ToolRisk::SensitiveContext
                | ToolRisk::Destructive
        )
    }));
    assert!(prompt_evaluation_tool_specs(
        &registry,
        "Inspect evidence.txt",
        32_000,
        &WorkflowToolPolicy::None,
    )
    .is_empty());

    let result = registry
        .get("file.read")
        .expect("read tool should exist")
        .execute(ToolInvocation {
            id: agent_core::ToolCallId("evaluation-read".to_string()),
            task_id: phase16_task_id(),
            tool_name: "file.read".to_string(),
            input_json: serde_json::json!({"path":"evidence.txt"}).to_string(),
            proposed_by_model: "test".to_string(),
            metadata: Metadata::new(),
        })
        .expect("read-only sandbox tool should execute");
    assert_eq!(result.output, "verified workspace evidence");
    fs::remove_dir_all(root).expect("sandbox fixture should be removed");
}

#[test]
#[ignore = "requires the user's configured provider and network access"]
fn provider_backed_evaluation_sandbox_reads_real_workspace_evidence() {
    let config = load_provider_config();
    assert!(config.is_ready(), "provider configuration is required");
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("repository root should resolve");
    let profile = ConductorPromptGenome::seed_for_effort("auto");
    let objective = "Use read-only workspace tools to inspect the root Cargo.toml. Report one workspace member and include the exact token crates/orchestrator. Do not guess.";
    let mut plan = WorkflowPlanIr::from_adaptive_with_profile(
        "provider-sandbox-smoke",
        objective,
        "auto",
        "single_worker",
        config.model_for_conductor(),
        profile.id.clone(),
        &AdaptiveWorkflow {
            steps: vec![AdaptiveWorkflowStep {
                id: "inspect".to_string(),
                role: "worker".to_string(),
                model: config.model_for_role(&ModelRole::Executor),
                subtask: "Read the root Cargo.toml and report a verified workspace member."
                    .to_string(),
                access: Vec::new(),
            }],
        },
        WorkflowBudget {
            max_steps: 1,
            max_models: 1,
            max_model_turns_per_step: 1,
            max_tool_calls_per_step: 4,
            max_output_tokens_per_step: 2_048,
        },
    );
    plan.steps[0].tool_policy = WorkflowToolPolicy::ReadOnlyEvidence;
    let candidate = execute_prompt_workflow_candidate(
        &config,
        &workspace_root,
        objective,
        PromptPlanCandidate {
            genome: profile,
            plan: Some(plan),
            raw_output: String::new(),
            latency_ms: 0,
            total_tokens: 0,
        },
        &Arc::new(AgentRunControl::new("pro")),
    );

    assert!(candidate.execution.succeeded);
    assert!(candidate
        .execution
        .final_output
        .to_ascii_lowercase()
        .contains("crates/orchestrator"));
    assert!(candidate.execution.steps[0]
        .tool_calls
        .iter()
        .any(|call| call.tool == "file.read"));
}

#[test]
#[ignore = "requires the user's configured provider and network access"]
fn provider_backed_paired_ablation_rewards_read_only_tool_evidence() {
    let config = load_provider_config();
    assert!(config.is_ready(), "provider configuration is required");
    let workspace_root = temp_test_root("cindx-provider-ablation");
    fs::create_dir_all(&workspace_root).expect("ablation sandbox should be created");
    let hidden_fact = format!("CINDX-VERIFY-{}", unique_id("fact"));
    fs::write(workspace_root.join("evidence.txt"), &hidden_fact)
        .expect("hidden evidence should be written");
    let objective = "The read-only workspace contains evidence.txt with one verification code. Report the exact code. Use workspace evidence when available and never guess.";
    let profile = ConductorPromptGenome::seed_for_effort("auto");
    let plan = |id: &str, tool_policy: WorkflowToolPolicy| {
        let mut plan = WorkflowPlanIr::from_adaptive_with_profile(
                id,
                objective,
                "auto",
                "single_worker",
                config.model_for_conductor(),
                profile.id.clone(),
                &AdaptiveWorkflow {
                    steps: vec![AdaptiveWorkflowStep {
                        id: "inspect".to_string(),
                        role: "worker".to_string(),
                        model: config.model_for_role(&ModelRole::Executor),
                        subtask: "Read evidence.txt when the sandbox exposes a read-only tool and report the exact code."
                            .to_string(),
                        access: Vec::new(),
                    }],
                },
                WorkflowBudget {
                    max_steps: 1,
                    max_models: 1,
                    max_model_turns_per_step: 3,
                    max_tool_calls_per_step: 4,
                    max_output_tokens_per_step: 2_048,
                },
            );
        plan.steps[0].tool_policy = tool_policy;
        plan
    };
    let run = |id: &str, tool_policy: WorkflowToolPolicy| {
        execute_prompt_workflow_candidate(
            &config,
            &workspace_root,
            objective,
            PromptPlanCandidate {
                genome: profile.clone(),
                plan: Some(plan(id, tool_policy)),
                raw_output: String::new(),
                latency_ms: 0,
                total_tokens: 0,
            },
            &Arc::new(AgentRunControl::new("pro")),
        )
    };
    let baseline = run("without-tools", WorkflowToolPolicy::None);
    let candidate = run("with-read-only-tools", WorkflowToolPolicy::ReadOnlyEvidence);
    let verifier = AgentEvaluationVerifier::ContainsAll {
        expected: vec![hidden_fact.clone()],
        case_sensitive: true,
    };
    let baseline_outcome = verifier.verify(&baseline.execution.final_output);
    let candidate_outcome = verifier.verify(&candidate.execution.final_output);
    fs::remove_dir_all(workspace_root).expect("ablation sandbox should be removed");

    assert!(!baseline_outcome.passed);
    assert!(candidate_outcome.passed);
    assert!(candidate.execution.steps[0]
        .tool_calls
        .iter()
        .any(|call| call.tool == "file.read" && call.response.contains(&hidden_fact)));
}

#[test]
#[ignore = "requires the user's configured provider and network access"]
fn provider_backed_gepa_reflection_repairs_a_disabled_tool_gene() {
    let config = load_provider_config();
    assert!(config.is_ready(), "provider configuration is required");
    let mut parent = ConductorPromptGenome::seed_for_effort("auto");
    parent.id = "reflection-parent-tools-disabled".to_string();
    parent.tool_policy = orchestrator::PromptToolPolicy::Disabled;
    parent.max_tool_calls_per_step = 0;
    let failure = AgentEvaluationReflectionPacket {
            suite_id: "provider-reflection-smoke".to_string(),
            suite_version: 2,
            case_id: "feedback-random-workspace-fact".to_string(),
            category: "tool-use".to_string(),
            run_id: "provider-reflection-run".to_string(),
            seed: 0,
            candidate_id: parent.id.clone(),
            candidate_fingerprint: "reflection-parent-fingerprint".to_string(),
            model_fingerprints: BTreeMap::from([(
                "worker".to_string(),
                config.model_for_role(&ModelRole::Executor),
            )]),
            input: "Read an unpredictable value from evidence.txt and report it exactly."
                .to_string(),
            steps: vec![AgentEvaluationTraceStep {
                step_id: "inspect".to_string(),
                role: "worker".to_string(),
                model: config.model_for_role(&ModelRole::Executor),
                prompt: "No tools are available; report the exact unpredictable file value."
                    .to_string(),
                output: "I cannot access evidence.txt without a workspace tool.".to_string(),
                tool_calls: Vec::new(),
                errors: vec!["required workspace evidence was unavailable".to_string()],
                latency_ms: 100,
                total_tokens: 50,
            }],
            final_output: "I cannot access evidence.txt without a workspace tool.".to_string(),
            verifier: AgentEvaluationVerifierOutcome {
                source: AgentEvaluationEvidenceSource::Deterministic,
                passed: false,
                score: 0.0,
                checks: vec![AgentEvaluationCheck {
                    id: "contains_unpredictable_value".to_string(),
                    passed: false,
                    detail: "the output omitted the exact value stored in evidence.txt".to_string(),
                }],
            },
            actionable_feedback: ActionableSideInformation {
                summary: "The harness disabled the only safe evidence path required by this task."
                    .to_string(),
                failed_constraints: vec![
                    "The worker could not inspect a required workspace file.".to_string(),
                ],
                errors: vec!["No read-only workspace tool was exposed.".to_string()],
                suggested_changes: vec![
                    "Enable the existing read-only evidence tool policy; do not add writes or network access."
                        .to_string(),
                ],
                ..ActionableSideInformation::default()
            },
        };
    let mutation_prompt = parent
        .reflective_mutation_prompt(&[failure])
        .expect("reflection prompt should build");
    let control = Arc::new(AgentRunControl::new("pro"));
    let completion = complete_collaboration_model_with_control(
        config.clone(),
        ModelRole::Planner,
        config.model_for_conductor(),
        collaboration_system_prompt_for_run(&config.agent_system_prompt, &Metadata::new()),
        mutation_prompt,
        Some(control),
        |_| {},
    );
    let response = completion
        .content
        .unwrap_or_else(|| panic!("reflection model failed: {:?}", completion.error));
    let mutation = match parent
        .learned_mutation_from_response(&response, "reflection-child-tools-enabled")
    {
        Ok(mutation) => mutation,
        Err(error) => {
            let repair = complete_collaboration_model_with_control(
                config.clone(),
                ModelRole::Planner,
                config.model_for_conductor(),
                collaboration_system_prompt_for_run(&config.agent_system_prompt, &Metadata::new()),
                parent.mutation_repair_prompt(&response, &error),
                Some(Arc::new(AgentRunControl::new("pro"))),
                |_| {},
            );
            let repaired_response = repair
                .content
                .unwrap_or_else(|| panic!("reflection repair model failed: {:?}", repair.error));
            parent
                .learned_mutation_from_response(
                    &repaired_response,
                    "reflection-child-tools-enabled",
                )
                .expect("repaired reflection should be a bounded genome mutation")
        }
    };

    assert_ne!(
        mutation.tool_policy,
        orchestrator::PromptToolPolicy::Disabled
    );
    assert!(mutation.effective_max_tool_calls_per_step() > 0);
    mutation.validate().expect("mutation should remain valid");
}

#[test]
#[ignore = "runs the full 30-case by 3-repeat provider-backed hidden gate"]
fn provider_backed_hidden_gate_compares_pre_gepa_and_read_only_sandbox() {
    let config = load_provider_config();
    assert!(config.is_ready(), "provider configuration is required");
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("repository root should resolve");
    let workspace_root = temp_test_root("cindx-provider-hidden-gate");
    let evidence_root = workspace_root.join("evidence");
    fs::create_dir_all(&evidence_root).expect("hidden evidence root should be created");
    let cases = (0..30usize)
            .map(|index| {
                let case_id = format!("provider-hidden-{index:02}");
                let category = match index % 3 {
                    0 => "coding",
                    1 => "research",
                    _ => "tool-use",
                }
                .to_string();
                let relative_path = format!("evidence/{case_id}.txt");
                let expected = format!("CINDX-HIDDEN-{}", unique_id("fact"));
                fs::write(workspace_root.join(&relative_path), &expected)
                    .expect("hidden evidence should be written");
                let objective = format!(
                    "The read-only workspace contains {relative_path} with one unpredictable verification code. Report the exact code. Use workspace evidence when available and never guess."
                );
                (case_id, category, relative_path, objective, expected)
            })
            .collect::<Vec<_>>();
    let dataset = orchestrator::AgentEvaluationDataset {
        schema: orchestrator::AGENT_EVALUATION_DATASET_SCHEMA.to_string(),
        suite_id: "core-agent-quality".to_string(),
        suite_version: 2,
        split: AgentEvaluationSplit::Test,
        description: "Runtime-generated provider-backed hidden workspace facts.".to_string(),
        cases: cases
            .iter()
            .map(
                |(case_id, category, _, objective, expected)| orchestrator::AgentEvaluationCase {
                    id: case_id.clone(),
                    category: category.clone(),
                    objective: objective.clone(),
                    verifier: AgentEvaluationVerifier::ContainsAll {
                        expected: vec![expected.clone()],
                        case_sensitive: true,
                    },
                    metadata: BTreeMap::from([(
                        "provenance".to_string(),
                        "runtime-random-fact".to_string(),
                    )]),
                },
            )
            .collect(),
    };
    dataset.validate().expect("hidden dataset should validate");
    let dataset_json =
        serde_json::to_string_pretty(&dataset).expect("hidden dataset should serialize");
    let dataset_sha256 = sha256_hex(dataset_json.as_bytes());
    let jobs = Arc::new(Mutex::new(
        (0..cases.len())
            .flat_map(|case_index| (0..3u64).map(move |seed| (case_index, seed)))
            .rev()
            .collect::<Vec<_>>(),
    ));
    let cases = Arc::new(cases);
    let records = Arc::new(Mutex::new((
        Vec::<AgentEvaluationCaseScore>::new(),
        Vec::<AgentEvaluationCaseScore>::new(),
    )));
    let baseline_id = "pre-gepa-no-evaluation-tools".to_string();
    let candidate_id = "gepa-read-only-sandbox".to_string();
    let baseline_fingerprint = sha256_hex(baseline_id.as_bytes());
    let candidate_profile = ConductorPromptGenome::seed_for_effort("auto");
    let candidate_fingerprint = sha256_hex(
        &serde_json::to_vec(&candidate_profile).expect("candidate genome should serialize"),
    );
    let concurrency = std::env::var("CINDX_EVAL_CONCURRENCY")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(4)
        .clamp(1, 8);
    let handles = (0..concurrency)
            .map(|_| {
                let config = config.clone();
                let workspace_root = workspace_root.clone();
                let jobs = Arc::clone(&jobs);
                let cases = Arc::clone(&cases);
                let records = Arc::clone(&records);
                let baseline_id = baseline_id.clone();
                let candidate_id = candidate_id.clone();
                let baseline_fingerprint = baseline_fingerprint.clone();
                let candidate_fingerprint = candidate_fingerprint.clone();
                let candidate_profile = candidate_profile.clone();
                std::thread::spawn(move || loop {
                    let Some((case_index, seed)) = jobs
                        .lock()
                        .expect("hidden job lock")
                        .pop()
                    else {
                        break;
                    };
                    let (case_id, category, relative_path, objective, expected) =
                        &cases[case_index];
                    let run = |workflow_id: &str, tool_policy: WorkflowToolPolicy| {
                        let mut plan = WorkflowPlanIr::from_adaptive_with_profile(
                            workflow_id,
                            objective,
                            "auto",
                            "single_worker",
                            config.model_for_conductor(),
                            candidate_profile.id.clone(),
                            &AdaptiveWorkflow {
                                steps: vec![AdaptiveWorkflowStep {
                                    id: "inspect".to_string(),
                                    role: "worker".to_string(),
                                    model: config.model_for_role(&ModelRole::Executor),
                                    subtask: format!(
                                        "Read {relative_path} when a read-only tool is exposed and report its exact unpredictable code."
                                    ),
                                    access: Vec::new(),
                                }],
                            },
                            WorkflowBudget {
                                max_steps: 1,
                                max_models: 1,
                                max_model_turns_per_step: 3,
                                max_tool_calls_per_step: 4,
                                max_output_tokens_per_step: 2_048,
                            },
                        );
                        plan.steps[0].tool_policy = tool_policy;
                        execute_prompt_workflow_candidate(
                            &config,
                            &workspace_root,
                            objective,
                            PromptPlanCandidate {
                                genome: candidate_profile.clone(),
                                plan: Some(plan),
                                raw_output: String::new(),
                                latency_ms: 0,
                                total_tokens: 0,
                            },
                            &Arc::new(AgentRunControl::new("pro")),
                        )
                    };
                    let baseline = run(
                        &format!("baseline-{case_index}-{seed}"),
                        WorkflowToolPolicy::None,
                    );
                    let candidate = run(
                        &format!("candidate-{case_index}-{seed}"),
                        WorkflowToolPolicy::ReadOnlyEvidence,
                    );
                    let verifier = AgentEvaluationVerifier::ContainsAll {
                        expected: vec![expected.clone()],
                        case_sensitive: true,
                    };
                    let baseline_outcome = verifier.verify(&baseline.execution.final_output);
                    let candidate_outcome = verifier.verify(&candidate.execution.final_output);
                    let record = |candidate_id: &str,
                                  candidate_fingerprint: &str,
                                  execution: &PromptWorkflowExecution,
                                  outcome: &AgentEvaluationVerifierOutcome| {
                        AgentEvaluationCaseScore {
                            suite_id: "core-agent-quality".to_string(),
                            suite_version: 2,
                            case_id: case_id.clone(),
                            category: category.clone(),
                            split: AgentEvaluationSplit::Test,
                            run_id: format!("{candidate_id}-{case_index}-{seed}"),
                            seed,
                            candidate_id: candidate_id.to_string(),
                            candidate_fingerprint: candidate_fingerprint.to_string(),
                            evidence_source: AgentEvaluationEvidenceSource::Deterministic,
                            score: if execution.succeeded {
                                outcome.score
                            } else {
                                0.0
                            },
                            verified_success: execution.succeeded && outcome.passed,
                            latency_ms: execution.latency_ms,
                            total_tokens: execution.total_tokens,
                            safety_violations: 0,
                        }
                    };
                    let mut records = records.lock().expect("hidden record lock");
                    records.0.push(record(
                        &baseline_id,
                        &baseline_fingerprint,
                        &baseline.execution,
                        &baseline_outcome,
                    ));
                    records.1.push(record(
                        &candidate_id,
                        &candidate_fingerprint,
                        &candidate.execution,
                        &candidate_outcome,
                    ));
                })
            })
            .collect::<Vec<_>>();
    for handle in handles {
        handle.join().expect("provider hidden worker should join");
    }
    let (mut baseline_records, mut candidate_records) = Arc::try_unwrap(records)
        .expect("hidden records should have one owner")
        .into_inner()
        .expect("hidden record lock should unwrap");
    let sort_records = |records: &mut Vec<AgentEvaluationCaseScore>| {
        records.sort_by(|left, right| {
            left.case_id
                .cmp(&right.case_id)
                .then(left.seed.cmp(&right.seed))
        });
    };
    sort_records(&mut baseline_records);
    sort_records(&mut candidate_records);
    let baseline_scores = orchestrator::AgentEvaluationScoreSet {
        schema: orchestrator::AGENT_EVALUATION_SCORE_SET_SCHEMA.to_string(),
        suite_id: "core-agent-quality".to_string(),
        suite_version: 2,
        split: AgentEvaluationSplit::Test,
        candidate_id: baseline_id,
        candidate_fingerprint: baseline_fingerprint,
        dataset_sha256: dataset_sha256.clone(),
        provenance: orchestrator::AgentEvaluationRunProvenance::ProviderBacked,
        records: baseline_records,
    };
    let candidate_scores = orchestrator::AgentEvaluationScoreSet {
        schema: orchestrator::AGENT_EVALUATION_SCORE_SET_SCHEMA.to_string(),
        suite_id: "core-agent-quality".to_string(),
        suite_version: 2,
        split: AgentEvaluationSplit::Test,
        candidate_id,
        candidate_fingerprint,
        dataset_sha256,
        provenance: orchestrator::AgentEvaluationRunProvenance::ProviderBacked,
        records: candidate_records,
    };
    let baseline = orchestrator::parse_agent_evaluation_baseline(
        &fs::read_to_string(repository_root.join("benchmarks/agent/evaluation-v2-baseline.json"))
            .expect("frozen evaluation baseline should load"),
    )
    .expect("frozen evaluation baseline should parse");
    let report = orchestrator::build_agent_evaluation_promotion_report(
        &baseline,
        &baseline_scores,
        &candidate_scores,
    )
    .expect("provider hidden report should build");
    let hidden_root = repository_root.join("benchmarks/agent/hidden");
    fs::create_dir_all(&hidden_root).expect("ignored hidden report root should exist");
    fs::write(hidden_root.join("provider-test.json"), dataset_json)
        .expect("provider hidden dataset should persist");
    fs::write(
        hidden_root.join("provider-baseline-scores.json"),
        serde_json::to_string_pretty(&baseline_scores).expect("baseline scores should serialize"),
    )
    .expect("baseline scores should persist");
    fs::write(
        hidden_root.join("provider-candidate-scores.json"),
        serde_json::to_string_pretty(&candidate_scores).expect("candidate scores should serialize"),
    )
    .expect("candidate scores should persist");
    fs::write(
        repository_root.join("target/evaluation-v2-provider-promotion.json"),
        serde_json::to_string_pretty(&report).expect("promotion report should serialize"),
    )
    .expect("promotion report should persist");
    fs::remove_dir_all(workspace_root).expect("provider hidden workspace should be removed");

    assert!(
        report.promotion_eligible,
        "provider hidden gate: {report:#?}"
    );
    assert_eq!(report.recommended_canary_percent, Some(10));
}

#[test]
fn evaluation_arena_applies_retry_and_alternate_model_genes() {
    let mut profile = ConductorPromptGenome::seed_for_effort("auto");
    profile.max_step_attempts = 2;
    profile.retry_policy = PromptRetryPolicy::AlternateModel;
    let mut plan = WorkflowPlanIr::from_adaptive_with_profile(
        "retry-candidate",
        "Investigate and summarize",
        "auto",
        "best_of_n",
        "planner",
        profile.id.clone(),
        &AdaptiveWorkflow {
            steps: vec![
                AdaptiveWorkflowStep {
                    id: "investigate".to_string(),
                    role: "worker".to_string(),
                    model: "worker-a".to_string(),
                    subtask: "investigate evidence".to_string(),
                    access: Vec::new(),
                },
                AdaptiveWorkflowStep {
                    id: "final".to_string(),
                    role: "synthesizer".to_string(),
                    model: "worker-b".to_string(),
                    subtask: "synthesize".to_string(),
                    access: vec!["investigate".to_string()],
                },
            ],
        },
        WorkflowBudget {
            max_steps: 4,
            max_models: 2,
            max_model_turns_per_step: 2,
            max_tool_calls_per_step: 4,
            max_output_tokens_per_step: 2_048,
        },
    );
    plan.steps[0].tool_policy = WorkflowToolPolicy::ReadOnlyEvidence;
    let requests = Arc::new(Mutex::new(Vec::<PromptEvaluationWorkerRequest>::new()));
    let captured = Arc::clone(&requests);
    let runner: PromptEvaluationRunner = Arc::new(move |request| {
        let should_fail = request.model == "worker-a";
        captured.lock().expect("request capture lock").push(request);
        CollaborationCompletion {
            content: (!should_fail).then(|| "recovered output".to_string()),
            error: should_fail.then(|| "worker-a failed".to_string()),
            latency_ms: 10,
            usage: [("total_tokens".to_string(), "20".to_string())]
                .into_iter()
                .collect(),
            evidence: Vec::new(),
        }
    });

    let candidate = execute_prompt_workflow_candidate_with_runner(
        "Investigate and summarize",
        PromptPlanCandidate {
            genome: profile,
            plan: Some(plan),
            raw_output: String::new(),
            latency_ms: 0,
            total_tokens: 0,
        },
        runner,
    );

    assert!(candidate.execution.succeeded);
    assert_eq!(candidate.execution.steps[0].attempts, 2);
    assert_eq!(candidate.execution.steps[0].model, "worker-b");
    assert_eq!(candidate.execution.steps[0].errors, vec!["worker-a failed"]);
    let requests = requests.lock().expect("request capture lock");
    assert_eq!(
        requests[0].tool_policy,
        WorkflowToolPolicy::ReadOnlyEvidence
    );
    assert_eq!(requests[0].max_model_turns, 2);
    assert_eq!(requests[0].max_tool_calls, 4);
    assert!(requests[1]
        .prompt
        .contains("Retry the same authorized evaluation node"));
}

#[test]
fn timeline_exposes_durable_workflow_progress() {
    let event = Event {
        id: EventId("checkpoint-progress".to_string()),
        task_id: phase16_task_id(),
        sequence: 1,
        timestamp_ms: 100,
        kind: EventKind::TaskStatusChanged,
        summary: "Collaboration workflow step checkpointed".to_string(),
        metadata: [
            (
                "workflow_checkpoint_schema".to_string(),
                WORKFLOW_CHECKPOINT_SCHEMA.to_string(),
            ),
            ("workflow_steps".to_string(), "5".to_string()),
            ("completed_steps".to_string(), "2".to_string()),
            ("step_id".to_string(), "review".to_string()),
            ("step_status".to_string(), "completed".to_string()),
            ("workflow_continuations".to_string(), "1".to_string()),
        ]
        .into_iter()
        .collect(),
    };

    let progress = timeline_workflow_progress(&event).unwrap();

    assert_eq!(progress.completed_steps, 2);
    assert_eq!(progress.total_steps, 5);
    assert_eq!(progress.current_step_id.as_deref(), Some("review"));
    assert_eq!(progress.continuations, 1);
    assert!(progress.recoverable);
}

#[test]
fn prompt_evolution_waits_for_the_final_agent_outcome() {
    let seed = ConductorPromptGenome::seed_for_effort("pro");
    let context = [
        ("collaboration_id".to_string(), "collab-final".to_string()),
        ("agent_run_id".to_string(), "run-final".to_string()),
        ("prompt_profile".to_string(), seed.id.clone()),
        ("prompt_effort".to_string(), "pro".to_string()),
        (
            "prompt_genome".to_string(),
            serde_json::to_string(&seed).unwrap(),
        ),
    ]
    .into_iter()
    .collect::<Metadata>();
    let mut events = vec![
        Event {
            id: EventId("profile".to_string()),
            task_id: phase16_task_id(),
            sequence: 1,
            timestamp_ms: 100,
            kind: EventKind::TaskStatusChanged,
            summary: "Conductor prompt profile selected".to_string(),
            metadata: context.clone(),
        },
        Event {
            id: EventId("collaboration-complete".to_string()),
            task_id: phase16_task_id(),
            sequence: 2,
            timestamp_ms: 200,
            kind: EventKind::TaskStatusChanged,
            summary: "Collaboration workflow completed".to_string(),
            metadata: context.clone(),
        },
    ];

    assert!(prompt_evolution_observations_from_events(&events).is_empty());
    events.push(Event {
        id: EventId("agent-failed".to_string()),
        task_id: phase16_task_id(),
        sequence: 3,
        timestamp_ms: 300,
        kind: EventKind::Error,
        summary: "Agent task failed".to_string(),
        metadata: context,
    });
    let observations = prompt_evolution_observations_from_events(&events);
    assert_eq!(observations.len(), 1);
    assert!(!observations[0].1.succeeded);
}

#[test]
fn prompt_evolution_penalizes_a_profile_when_anchor_was_delivered() {
    let seed = ConductorPromptGenome::seed_for_effort("pro");
    let context = [
        ("collaboration_id".to_string(), "collab-anchor".to_string()),
        ("agent_run_id".to_string(), "run-anchor".to_string()),
        ("prompt_profile".to_string(), seed.id.clone()),
        ("prompt_effort".to_string(), "pro".to_string()),
        (
            "prompt_genome".to_string(),
            serde_json::to_string(&seed).unwrap(),
        ),
        ("collaboration_profile".to_string(), "bounded".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let events = vec![
        Event {
            id: EventId("profile-anchor".to_string()),
            task_id: phase16_task_id(),
            sequence: 1,
            timestamp_ms: 100,
            kind: EventKind::TaskStatusChanged,
            summary: "Conductor prompt profile selected".to_string(),
            metadata: context.clone(),
        },
        Event {
            id: EventId("workflow-anchor".to_string()),
            task_id: phase16_task_id(),
            sequence: 2,
            timestamp_ms: 200,
            kind: EventKind::TaskStatusChanged,
            summary: "Collaboration workflow completed".to_string(),
            metadata: metadata_with_context(
                [
                    (
                        "anytime_selected_candidate".to_string(),
                        DIRECT_ANCHOR_CANDIDATE_ID.to_string(),
                    ),
                    (
                        "anytime_prompt_learning_eligible".to_string(),
                        "false".to_string(),
                    ),
                    ("anytime_team_uplift_bps".to_string(), "-800".to_string()),
                ]
                .into_iter()
                .collect(),
                &context,
            ),
        },
        Event {
            id: EventId("agent-anchor".to_string()),
            task_id: phase16_task_id(),
            sequence: 3,
            timestamp_ms: 300,
            kind: EventKind::TaskStatusChanged,
            summary: "Agent task completed".to_string(),
            metadata: metadata_with_context(
                [("routing_learning_eligible".to_string(), "true".to_string())]
                    .into_iter()
                    .collect(),
                &context,
            ),
        },
    ];

    let observations = prompt_evolution_observations_from_events(&events);
    assert_eq!(observations.len(), 1);
    assert!(!observations[0].1.succeeded);
    assert_eq!(observations[0].1.relative_reward, Some(-0.08));
}

#[test]
fn prompt_evolution_treats_user_cancellation_as_a_mild_negative_signal() {
    let seed = ConductorPromptGenome::seed_for_effort("pro");
    let context = [
        ("collaboration_id".to_string(), "collab-cancelled".to_string()),
        ("agent_run_id".to_string(), "run-cancelled".to_string()),
        ("prompt_profile".to_string(), seed.id.clone()),
        ("prompt_effort".to_string(), "pro".to_string()),
        (
            "prompt_genome".to_string(),
            serde_json::to_string(&seed).unwrap(),
        ),
        ("collaboration_profile".to_string(), "bounded".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let events = vec![
        Event {
            id: EventId("profile-cancelled".to_string()),
            task_id: phase16_task_id(),
            sequence: 1,
            timestamp_ms: 100,
            kind: EventKind::TaskStatusChanged,
            summary: "Conductor prompt profile selected".to_string(),
            metadata: context.clone(),
        },
        Event {
            id: EventId("workflow-cancelled".to_string()),
            task_id: phase16_task_id(),
            sequence: 2,
            timestamp_ms: 200,
            kind: EventKind::TaskStatusChanged,
            summary: "Collaboration workflow failed".to_string(),
            metadata: metadata_with_context(
                [("anytime_native_effort_success".to_string(), "false".to_string())]
                    .into_iter()
                    .collect(),
                &context,
            ),
        },
        Event {
            id: EventId("agent-cancelled".to_string()),
            task_id: phase16_task_id(),
            sequence: 3,
            timestamp_ms: 300,
            kind: EventKind::TaskStatusChanged,
            summary: "Agent task cancelled".to_string(),
            metadata: metadata_with_context(
                [("reason".to_string(), "user_cancelled".to_string())]
                    .into_iter()
                    .collect(),
                &context,
            ),
        },
    ];

    let observations = prompt_evolution_observations_from_events(&events);
    assert_eq!(observations.len(), 1);
    assert!(!observations[0].1.succeeded);
    assert_eq!(observations[0].1.relative_reward, Some(-0.25));
}

#[test]
fn scheduled_queue_progress_tracks_permission_and_completion() {
    let queue_id = "schedule-queue";
    let event = |sequence: u64, summary: &str, queue_action: Option<&str>| Event {
        id: EventId(format!("schedule-event-{sequence}")),
        task_id: phase16_task_id(),
        sequence,
        timestamp_ms: sequence * 100,
        kind: EventKind::TaskStatusChanged,
        summary: summary.to_string(),
        metadata: [
            ("queue_id".to_string(), queue_id.to_string()),
            ("session_id".to_string(), "session-a".to_string()),
        ]
        .into_iter()
        .chain(queue_action.map(|action| ("queue_action".to_string(), action.to_string())))
        .collect(),
    };
    let mut events = vec![
        event(1, "Agent message queued", Some("enqueue")),
        event(2, "Queued agent message started", Some("start")),
        event(3, "Agent task started", None),
        event(4, "Agent task waiting for permission", None),
    ];

    let waiting = schedule_queue_progress(&events, queue_id).unwrap();
    assert_eq!(waiting.status, "waiting_for_permission");
    assert_eq!(waiting.started_at_ms, Some(200));
    assert_eq!(
        latest_unfinished_agent_queue_id(&events).as_deref(),
        Some(queue_id)
    );

    events.push(event(5, "Agent task paused", None));
    let paused = schedule_queue_progress(&events, queue_id).unwrap();
    assert_eq!(paused.status, "paused");
    assert_eq!(paused.finished_at_ms, Some(500));
    assert!(latest_unfinished_agent_queue_id(&events).is_none());

    events.push(event(6, "Agent task retry started", None));
    events.push(event(7, "Agent task completed", None));
    let completed = schedule_queue_progress(&events, queue_id).unwrap();
    assert_eq!(completed.status, "completed");
    assert_eq!(completed.finished_at_ms, Some(700));
    assert!(latest_unfinished_agent_queue_id(&events).is_none());
}

#[test]
fn restored_scheduled_queue_is_retryable_instead_of_running() {
    let queue_id = "schedule-queue";
    let events = vec![
        Event {
            id: EventId("schedule-enqueue".to_string()),
            task_id: phase16_task_id(),
            sequence: 1,
            timestamp_ms: 100,
            kind: EventKind::TaskStatusChanged,
            summary: "Agent message queued".to_string(),
            metadata: [
                ("queue_id".to_string(), queue_id.to_string()),
                ("queue_action".to_string(), "enqueue".to_string()),
            ]
            .into_iter()
            .collect(),
        },
        Event {
            id: EventId("schedule-start".to_string()),
            task_id: phase16_task_id(),
            sequence: 2,
            timestamp_ms: 200,
            kind: EventKind::TaskStatusChanged,
            summary: "Queued agent message started".to_string(),
            metadata: [
                ("queue_id".to_string(), queue_id.to_string()),
                ("queue_action".to_string(), "start".to_string()),
            ]
            .into_iter()
            .collect(),
        },
        Event {
            id: EventId("schedule-restore".to_string()),
            task_id: phase16_task_id(),
            sequence: 3,
            timestamp_ms: 300,
            kind: EventKind::TaskStatusChanged,
            summary: "Queued agent message restored".to_string(),
            metadata: [
                ("queue_id".to_string(), queue_id.to_string()),
                ("queue_action".to_string(), "restore".to_string()),
            ]
            .into_iter()
            .collect(),
        },
    ];

    let progress = schedule_queue_progress(&events, queue_id).unwrap();
    assert_eq!(progress.status, "queued");
    assert_eq!(progress.started_at_ms, None);
    assert_eq!(progress.finished_at_ms, None);
}

#[test]
fn simple_greeting_skips_workspace_knowledge_retrieval() {
    let greeting = RoutingContext::from_prompt("你好", Vec::new());
    let capability_question = RoutingContext::from_prompt("你会不会写代码", Vec::new());
    let retrieval = RoutingContext::from_prompt("搜索项目文档里的 API 定义", Vec::new());

    assert!(!should_run_agent_knowledge_retrieval(&greeting));
    assert!(!should_run_agent_knowledge_retrieval(&capability_question));
    assert!(should_run_agent_knowledge_retrieval(&retrieval));
    assert!(!should_recall_agent_memory(&greeting, "你好"));
    assert!(!should_recall_agent_memory(
        &capability_question,
        "你会不会写代码"
    ));
    assert!(should_recall_agent_memory(
        &RoutingContext::from_prompt("继续上次的侧边栏修改", Vec::new()),
        "继续上次的侧边栏修改"
    ));
}

#[test]
fn coding_retrieval_mode_skips_graph_channels() {
    let root = temp_test_root("phase7-selective-retrieval");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("notes.md"), "Cindx selective retrieval source")
        .expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    let mut adapter = FileRagAdapter::open(root.join(".cindx").join("rag-index.tsv"))
        .expect("adapter should open");
    adapter.replace_all(index).expect("index should persist");
    replace_lancedb_index(lancedb_database_path_for(&root), adapter.index())
        .expect("LanceDB index should persist");
    let cancellation = Arc::new(AgentRunControl::new("auto"));

    let retrieval = run_parallel_retrieval(
        &root,
        &adapter,
        &ProviderConfig::default(),
        "selective retrieval source",
        4,
        "semantic_literal_parallel",
        None,
        &cancellation,
    )
    .expect("retrieval should run");

    assert_eq!(
        retrieval
            .trace
            .channels
            .iter()
            .map(|channel| channel.name.as_str())
            .collect::<Vec<_>>(),
        vec!["semantic_rag", "file_search"]
    );
}

#[test]
fn retrieval_keeps_file_evidence_when_semantic_channel_is_unavailable() {
    let root = temp_test_root("phase7-independent-channel-failure");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(
        root.join("notes.md"),
        "independent lexical fallback evidence",
    )
    .expect("fixture should write");
    let mut index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    for chunk in &mut index.chunks {
        chunk.embedding_provider = "cloud".to_string();
        chunk.embedding_model = "cloud-embedding".to_string();
    }
    let mut adapter = FileRagAdapter::open(root.join(".cindx").join("rag-index.tsv"))
        .expect("adapter should open");
    adapter.replace_all(index).expect("index should persist");
    let cancellation = Arc::new(AgentRunControl::new("auto"));

    let retrieval = run_parallel_retrieval(
        &root,
        &adapter,
        &ProviderConfig::default(),
        "independent lexical fallback evidence",
        4,
        "semantic_literal_parallel",
        None,
        &cancellation,
    )
    .expect("independent channels should degrade without failing the retrieval");

    let semantic = retrieval
        .trace
        .channels
        .iter()
        .find(|channel| channel.name == "semantic_rag")
        .expect("semantic channel");
    let file = retrieval
        .trace
        .channels
        .iter()
        .find(|channel| channel.name == "file_search")
        .expect("file channel");
    assert!(semantic.error.is_some());
    assert!(file.error.is_none());
    assert!(file.result_count > 0);
    assert!(!retrieval.results.is_empty());
}

#[test]
fn workspace_cache_ttl_advances_only_after_validation_or_index_change() {
    assert!(!workspace_knowledge_cache_needs_refresh(true, false));
    assert!(workspace_knowledge_cache_needs_refresh(false, false));
    assert!(workspace_knowledge_cache_needs_refresh(true, true));
}

#[test]
fn interactive_observation_tools_do_not_invalidate_workspace_knowledge() {
    assert!(!tool_may_mutate_workspace(
        "browser.click",
        &ToolRisk::UsesNetwork
    ));
    assert!(!tool_may_mutate_workspace(
        "computer.key",
        &ToolRisk::Destructive
    ));
    assert!(!tool_may_mutate_workspace("file.read", &ToolRisk::ReadOnly));
    assert!(tool_may_mutate_workspace(
        "file.write",
        &ToolRisk::WritesWorkspace
    ));
    assert!(tool_may_mutate_workspace(
        "shell.run",
        &ToolRisk::ExecutesProcess
    ));
}

#[test]
fn graph_walk_seed_fusion_includes_semantic_and_file_evidence() {
    let root = temp_test_root("phase7-graph-seeds");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("a.md"), "semantic source alpha").expect("fixture should write");
    fs::write(root.join("b.md"), "direct source beta").expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    let semantic = RagSearchResult {
        chunk: index.chunks[0].clone(),
        score: 0.9,
    };
    let file = RagSearchResult {
        chunk: index.chunks[1].clone(),
        score: 0.8,
    };
    let channels = vec![
        RetrievalChannelOutcome {
            name: "semantic_rag".to_string(),
            duration_ms: 1,
            results: vec![semantic.clone()],
            error: None,
        },
        RetrievalChannelOutcome {
            name: "graph_walk".to_string(),
            duration_ms: 1,
            results: Vec::new(),
            error: None,
        },
        RetrievalChannelOutcome {
            name: "file_search".to_string(),
            duration_ms: 1,
            results: vec![file.clone()],
            error: None,
        },
    ];

    let seeds = graph_walk_seed_results(&channels, 8);

    assert_eq!(seeds.len(), 2);
    assert_eq!(seeds[0].chunk.id, semantic.chunk.id);
    assert_eq!(seeds[1].chunk.id, file.chunk.id);
}

#[test]
fn graph_walk_enrichment_skips_duplicate_literal_seeds() {
    let root = temp_test_root("phase7-graph-enrichment-duplicate");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("a.md"), "shared graph seed").expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    let shared = RagSearchResult {
        chunk: index.chunks[0].clone(),
        score: 0.9,
    };
    let channels = vec![
        RetrievalChannelOutcome {
            name: "semantic_rag".to_string(),
            duration_ms: 1,
            results: vec![shared.clone()],
            error: None,
        },
        RetrievalChannelOutcome {
            name: "graph_walk".to_string(),
            duration_ms: 1,
            results: Vec::new(),
            error: None,
        },
        RetrievalChannelOutcome {
            name: "file_search".to_string(),
            duration_ms: 1,
            results: vec![shared],
            error: None,
        },
    ];
    let seeds = graph_walk_seed_results(&channels, 8);

    assert!(!graph_walk_has_novel_enrichment_seeds(&channels, &seeds));
}

#[test]
fn graph_walk_enrichment_runs_for_semantic_only_seed() {
    let root = temp_test_root("phase7-graph-enrichment-novel");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("a.md"), "literal graph seed").expect("fixture should write");
    fs::write(root.join("b.md"), "semantic graph seed").expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    let literal = RagSearchResult {
        chunk: index.chunks[0].clone(),
        score: 0.8,
    };
    let semantic = RagSearchResult {
        chunk: index.chunks[1].clone(),
        score: 0.9,
    };
    let channels = vec![
        RetrievalChannelOutcome {
            name: "semantic_rag".to_string(),
            duration_ms: 1,
            results: vec![semantic],
            error: None,
        },
        RetrievalChannelOutcome {
            name: "graph_walk".to_string(),
            duration_ms: 1,
            results: Vec::new(),
            error: None,
        },
        RetrievalChannelOutcome {
            name: "file_search".to_string(),
            duration_ms: 1,
            results: vec![literal],
            error: None,
        },
    ];
    let seeds = graph_walk_seed_results(&channels, 8);

    assert!(graph_walk_has_novel_enrichment_seeds(&channels, &seeds));
}

#[test]
fn complex_retrieval_runs_all_four_independent_channels() {
    let root = temp_test_root("phase7-four-way-retrieval");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(
        root.join("notes.md"),
        "Cindx graph retrieval connects workspace evidence",
    )
    .expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    index_graph_chunks(&root, &index.chunks).expect("graph should build");
    let mut adapter = FileRagAdapter::open(root.join(".cindx").join("rag-index.tsv"))
        .expect("adapter should open");
    adapter.replace_all(index).expect("index should persist");
    replace_lancedb_index(lancedb_database_path_for(&root), adapter.index())
        .expect("LanceDB index should persist");
    let cancellation = Arc::new(AgentRunControl::new("pro"));

    let retrieval = run_parallel_retrieval(
        &root,
        &adapter,
        &ProviderConfig::default(),
        "graph retrieval workspace evidence",
        4,
        "four_way_parallel",
        None,
        &cancellation,
    )
    .expect("retrieval should run");

    assert_eq!(
        retrieval
            .trace
            .channels
            .iter()
            .map(|channel| channel.name.as_str())
            .collect::<Vec<_>>(),
        vec!["semantic_rag", "graph_recall", "graph_walk", "file_search"]
    );
    assert!(retrieval
        .trace
        .channels
        .iter()
        .all(|channel| channel.error.is_none()));
    assert!(!retrieval.results.is_empty());
}

#[test]
fn graph_index_cancellation_preserves_previous_cache() {
    let root = temp_test_root("phase7-graph-cancel");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("a.md"), "graph source").expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    index_graph_chunks(&root, &index.chunks).expect("initial graph should build");
    let graph_path = graph_store_path_for(&root);
    let before = fs::read(&graph_path).expect("initial graph should persist");

    fs::write(root.join("b.md"), "replacement graph source")
        .expect("replacement fixture should write");
    let replacement =
        index_workspace(&root, IndexOptions::default()).expect("replacement index should build");

    let error = index_graph_chunks_cancellable(&root, &replacement.chunks, || true)
        .expect_err("graph indexing should cancel");

    assert_eq!(error, MODEL_REQUEST_CANCELLED);
    assert_eq!(
        fs::read(&graph_path).expect("previous graph should remain available"),
        before
    );
    assert!(
        fs::read_dir(graph_path.parent().expect("graph parent should exist"))
            .expect("graph directory should list")
            .all(|entry| !entry
                .expect("graph entry should load")
                .file_name()
                .to_string_lossy()
                .contains("graph-index"))
    );
}

#[test]
fn retrieval_fusion_deduplicates_and_preserves_channel_reasons() {
    let root = temp_test_root("phase7-fusion");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("a.md"), "fusion source alpha").expect("fixture should write");
    fs::write(root.join("b.md"), "fusion source beta").expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    let first = RagSearchResult {
        chunk: index.chunks[0].clone(),
        score: 0.9,
    };
    let second = RagSearchResult {
        chunk: index.chunks[1].clone(),
        score: 0.8,
    };
    let mut overlapping = first.clone();
    overlapping.chunk.id = "direct-file-evidence".to_string();
    overlapping.score = 1.2;
    let channels = vec![
        RetrievalChannelOutcome {
            name: "semantic_rag".to_string(),
            duration_ms: 2,
            results: vec![first.clone(), second],
            error: None,
        },
        RetrievalChannelOutcome {
            name: "file_search".to_string(),
            duration_ms: 1,
            results: vec![overlapping],
            error: None,
        },
    ];

    let (results, sources) = fuse_retrieval_channels(&channels, 8);

    assert_eq!(results.len(), 2);
    assert_eq!(sources.len(), 2);
    assert!(sources[0].reason.contains("semantic_rag"));
    assert!(sources[0].reason.contains("file_search"));
}

#[test]
fn retrieval_fusion_prefers_independent_consensus_over_one_channel_outlier() {
    let root = temp_test_root("phase7-calibrated-consensus");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("a.md"), "single channel outlier").expect("fixture should write");
    fs::write(root.join("b.md"), "independently corroborated evidence")
        .expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    let outlier = index
        .chunks
        .iter()
        .find(|chunk| chunk.path == "a.md")
        .expect("outlier chunk should exist")
        .clone();
    let corroborated = index
        .chunks
        .iter()
        .find(|chunk| chunk.path == "b.md")
        .expect("corroborated chunk should exist")
        .clone();
    let channels = vec![
        RetrievalChannelOutcome {
            name: "semantic_rag".to_string(),
            duration_ms: 1,
            results: vec![
                RagSearchResult {
                    chunk: outlier,
                    score: 100.0,
                },
                RagSearchResult {
                    chunk: corroborated.clone(),
                    score: 0.4,
                },
            ],
            error: None,
        },
        RetrievalChannelOutcome {
            name: "file_search".to_string(),
            duration_ms: 1,
            results: vec![RagSearchResult {
                chunk: corroborated,
                score: 0.5,
            }],
            error: None,
        },
    ];

    let (results, sources) = fuse_retrieval_channels(&channels, 4);

    assert_eq!(results[0].chunk.path, "b.md");
    assert!(sources[0].reason.contains("consensus:2"));
    assert!(results.iter().all(|result| result.score.is_finite()));
}

#[test]
fn retrieval_fusion_does_not_double_count_correlated_graph_routes() {
    let root = temp_test_root("phase7-calibrated-graph-family");
    fs::create_dir_all(&root).expect("temp root should exist");
    fs::write(root.join("a.md"), "graph-only evidence").expect("fixture should write");
    fs::write(root.join("b.md"), "semantic evidence").expect("fixture should write");
    let index = index_workspace(&root, IndexOptions::default()).expect("index should build");
    let graph = index
        .chunks
        .iter()
        .find(|chunk| chunk.path == "a.md")
        .expect("graph chunk should exist")
        .clone();
    let semantic = index
        .chunks
        .iter()
        .find(|chunk| chunk.path == "b.md")
        .expect("semantic chunk should exist")
        .clone();
    let channels = vec![
        RetrievalChannelOutcome {
            name: "graph_recall".to_string(),
            duration_ms: 1,
            results: vec![RagSearchResult {
                chunk: graph.clone(),
                score: 1.0,
            }],
            error: None,
        },
        RetrievalChannelOutcome {
            name: "graph_walk".to_string(),
            duration_ms: 1,
            results: vec![RagSearchResult {
                chunk: graph,
                score: 1.0,
            }],
            error: None,
        },
        RetrievalChannelOutcome {
            name: "semantic_rag".to_string(),
            duration_ms: 1,
            results: vec![RagSearchResult {
                chunk: semantic,
                score: 1.0,
            }],
            error: None,
        },
    ];

    let (results, sources) = fuse_retrieval_channels(&channels, 4);

    assert_eq!(results[0].chunk.path, "b.md");
    let graph_source = sources
        .iter()
        .find(|source| source.path == "a.md")
        .expect("graph source should remain available");
    assert!(!graph_source.reason.contains("consensus:"));
}

#[test]
fn phase8_state_lists_browser_observations() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase8_task_id(),
        EventKind::ToolCallFinished,
        "Browser text extracted",
        [
            ("tool_call_id".to_string(), "browser-1".to_string()),
            ("tool".to_string(), "browser.extract_text".to_string()),
            ("status".to_string(), "succeeded".to_string()),
            ("output".to_string(), "Example Domain".to_string()),
            ("result_url".to_string(), "https://example.com".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("event should append");

    let state = phase8_state(&store, None).expect("state should load");

    assert_eq!(state.observations.len(), 1);
    assert_eq!(state.observations[0].tool_name, "browser.extract_text");
    assert_eq!(
        state.observations[0].url.as_deref(),
        Some("https://example.com")
    );
}

#[test]
fn phase8_state_lists_pending_browser_approvals() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let invocation = ToolInvocation {
        id: agent_core::ToolCallId("browser-2".to_string()),
        task_id: phase8_task_id(),
        tool_name: "browser.capture".to_string(),
        input_json: encode_input(&[("url", "https://example.com")]),
        proposed_by_model: "test".to_string(),
        metadata: Metadata::new(),
    };
    let registry = ToolRegistry::with_workspace_tools(workspace_root());
    let mut request = registry
        .get("browser.capture")
        .expect("tool should exist")
        .permission_request(&invocation)
        .expect("browser capture should request permission");
    request.id = PermissionRequestId("perm-phase8".to_string());
    request
        .metadata
        .insert("phase".to_string(), "8".to_string());
    request
        .metadata
        .insert("tool_input".to_string(), invocation.input_json);
    request
        .metadata
        .insert("tool_call_id".to_string(), invocation.id.0);
    request
        .metadata
        .insert("tool_name".to_string(), invocation.tool_name);
    store
        .save_permission_request(request, 456)
        .expect("request should save");

    let state = phase8_state(&store, None).expect("state should load");

    assert_eq!(state.pending_approvals.len(), 1);
    assert_eq!(state.pending_approvals[0].tool_name, "browser.capture");
}

#[test]
fn agent_state_lists_pending_agent_approvals() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        [("prompt".to_string(), "write a file".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("start should append");
    let invocation = ToolInvocation {
        id: agent_core::ToolCallId("agent-tool-1".to_string()),
        task_id: phase16_task_id(),
        tool_name: "file.write".to_string(),
        input_json: encode_input(&[("path", ".cindx/agent-loop.txt"), ("content", "ok")]),
        proposed_by_model: "agent-loop".to_string(),
        metadata: Metadata::new(),
    };
    let registry = ToolRegistry::with_workspace_tools(workspace_root());
    let mut request = registry
        .get("file.write")
        .expect("tool should exist")
        .permission_request(&invocation)
        .expect("write should request permission");
    request.id = PermissionRequestId("perm-agent".to_string());
    request
        .metadata
        .insert("phase".to_string(), "16".to_string());
    request
        .metadata
        .insert("tool_input".to_string(), invocation.input_json);
    request
        .metadata
        .insert("tool_call_id".to_string(), invocation.id.0);
    request
        .metadata
        .insert("tool_name".to_string(), invocation.tool_name);
    request
        .metadata
        .insert("agent_prompt".to_string(), "write a file".to_string());
    store
        .save_permission_request(request, current_time_millis())
        .expect("request should save");

    let state = agent_state(&store, None).expect("state should load");

    assert_eq!(state.status, "waiting_for_permission");
    assert_eq!(state.pending_approvals.len(), 1);
    assert_eq!(state.pending_approvals[0].tool_name, "file.write");

    store
        .resolve_permission(PermissionResolution {
            request_id: PermissionRequestId("perm-agent".to_string()),
            decision: PermissionDecision::AllowOnce,
            resolved_at_ms: current_time_millis(),
            resolved_by: "local-user".to_string(),
        })
        .expect("permission should resolve");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task resumed after permission",
        Metadata::new(),
    )
    .expect("resume should append");

    let resumed = agent_state(&store, None).expect("resumed state should load");
    assert_eq!(resumed.status, "running");
    assert!(resumed.pending_approvals.is_empty());
}

#[test]
fn agent_transcript_restores_assistant_tool_and_tool_messages() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        [("prompt".to_string(), "read README".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("start should append");
    append_message_event(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "read README",
    )
    .expect("user message should append");
    append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            MessageRole::Assistant,
            "",
            [
                (
                    "raw_tool_calls_json".to_string(),
                    r#"[{"id":"call-1","type":"function","function":{"name":"file_read","arguments":"{\"input\":\"path=README.md\"}"}}]"#.to_string(),
                ),
                ("tool_call_count".to_string(), "1".to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .expect("assistant tool call should append");
    append_tool_message_event(
        &mut store,
        &phase16_task_id(),
        "call-1",
        "file.read",
        "succeeded",
        "tool=file.read\nstatus=succeeded\noutput=hello",
        None,
    )
    .expect("tool message should append");

    let events = store
        .list_by_task(&phase16_task_id())
        .expect("events should load");
    let transcript = agent_transcript_from_active_events(&active_agent_events(&events));

    assert_eq!(transcript.len(), 3);
    assert!(matches!(transcript[1].role, MessageRole::Assistant));
    assert!(transcript[1].metadata.contains_key("raw_tool_calls_json"));
    assert!(matches!(transcript[2].role, MessageRole::Tool));
    assert_eq!(
        transcript[2]
            .metadata
            .get("tool_call_id")
            .map(String::as_str),
        Some("call-1")
    );

    let state = agent_state(&store, None).expect("agent state should load");
    assert!(state.run_started_at_ms > 0);
    assert_eq!(state.messages.len(), 3);
    assert_eq!(state.messages[0].role, "user");
    assert_eq!(state.messages[2].role, "tool");
}

#[test]
fn agent_state_and_trace_are_isolated_by_session() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    for (session_id, prompt, answer) in [
        ("session-a", "inspect alpha", "alpha answer"),
        ("session-b", "inspect beta", "beta answer"),
        ("session-a", "follow up alpha", "second alpha answer"),
    ] {
        let context = [
            ("project_id".to_string(), "project-cindx".to_string()),
            ("project_name".to_string(), "Cindx".to_string()),
            ("session_id".to_string(), session_id.to_string()),
            ("session_name".to_string(), session_id.to_string()),
        ]
        .into_iter()
        .collect::<Metadata>();
        let mut start_metadata = context.clone();
        start_metadata.insert("prompt".to_string(), prompt.to_string());
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task started",
            start_metadata,
        )
        .expect("start should append");
        append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            MessageRole::User,
            prompt,
            context.clone(),
        )
        .expect("user message should append");
        append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            MessageRole::Assistant,
            answer,
            context,
        )
        .expect("assistant message should append");
    }

    let alpha =
        agent_state_for_session(&store, None, Some("session-a")).expect("alpha state should load");
    let beta =
        agent_state_for_session(&store, None, Some("session-b")).expect("beta state should load");
    let alpha_trace = agent_trace_state_for_session(&store, None, None, Some("session-a"))
        .expect("alpha trace should load");

    assert_eq!(alpha.session_id.as_deref(), Some("session-a"));
    assert_eq!(alpha.messages.len(), 4);
    assert_eq!(alpha.messages[1].content, "alpha answer");
    assert_eq!(alpha.messages[3].content, "second alpha answer");
    assert_eq!(beta.session_id.as_deref(), Some("session-b"));
    assert_eq!(beta.messages.len(), 2);
    assert_eq!(beta.messages[1].content, "beta answer");
    assert_eq!(alpha_trace.session_id.as_deref(), Some("session-a"));
    assert!(alpha_trace
        .turns
        .iter()
        .flat_map(|turn| turn.steps.iter())
        .all(|step| !step.detail.contains("beta")));
}

#[test]
fn interleaved_agent_runs_remain_isolated_by_session() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let alpha = [("session_id".to_string(), "session-a".to_string())]
        .into_iter()
        .collect::<Metadata>();
    let beta = [("session_id".to_string(), "session-b".to_string())]
        .into_iter()
        .collect::<Metadata>();

    for (summary, context) in [
        ("Agent task started", alpha.clone()),
        ("Agent task started", beta.clone()),
    ] {
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            summary,
            context,
        )
        .expect("run start should append");
    }
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::Assistant,
        "alpha finished after beta started",
        alpha.clone(),
    )
    .expect("alpha answer should append");
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::Assistant,
        "beta answer",
        beta.clone(),
    )
    .expect("beta answer should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        alpha,
    )
    .expect("alpha completion should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        beta,
    )
    .expect("beta completion should append");

    let alpha_state =
        agent_state_for_session(&store, None, Some("session-a")).expect("alpha state should load");
    let beta_state =
        agent_state_for_session(&store, None, Some("session-b")).expect("beta state should load");

    assert_eq!(alpha_state.status, "completed");
    assert_eq!(alpha_state.messages.len(), 1);
    assert_eq!(
        alpha_state.messages[0].content,
        "alpha finished after beta started"
    );
    assert_eq!(beta_state.status, "completed");
    assert_eq!(beta_state.messages.len(), 1);
    assert_eq!(beta_state.messages[0].content, "beta answer");
}

#[test]
fn cancelling_one_session_does_not_cancel_another() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    for session_id in ["session-a", "session-b"] {
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task started",
            [("session_id".to_string(), session_id.to_string())]
                .into_iter()
                .collect(),
        )
        .expect("run start should append");
    }
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task cancelled",
        [("session_id".to_string(), "session-a".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("cancellation should append");

    assert!(agent_task_is_cancelled(&mut store, Some("session-a"))
        .expect("alpha cancellation should load"));
    assert!(!agent_task_is_cancelled(&mut store, Some("session-b"))
        .expect("beta cancellation should load"));
}

#[test]
fn startup_recovery_preserves_unfinished_agent_runs_as_continuations() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    for (session_id, run_id) in [("session-a", "run-a"), ("session-b", "run-b")] {
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task started",
            [
                ("session_id".to_string(), session_id.to_string()),
                ("agent_run_id".to_string(), run_id.to_string()),
                ("prompt".to_string(), "finish the task".to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .expect("run start should append");
    }
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        [
            ("session_id".to_string(), "session-a".to_string()),
            ("agent_run_id".to_string(), "run-a".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("completion should append");

    assert_eq!(
        reconcile_interrupted_agent_runs(&mut store).expect("recovery should succeed"),
        1
    );
    let completed = agent_state_for_session(&store, None, Some("session-a"))
        .expect("completed state should load");
    let interrupted = agent_state_for_session(&store, None, Some("session-b"))
        .expect("interrupted state should load");

    assert_eq!(completed.status, "completed");
    assert_eq!(interrupted.status, "paused");
    assert!(interrupted.can_retry);
    assert!(interrupted.can_continue);
    assert!(!interrupted.can_cancel);
    assert!(interrupted.last_error.is_none());
    let interrupted_events = store
        .list_by_task_and_metadata_or_unscoped(&phase16_task_id(), "session_id", "session-b")
        .expect("interrupted events should load");
    let envelope = latest_agent_recovery_envelope(&interrupted_events)
        .expect("recovery envelope should persist");
    assert_eq!(envelope.schema, AGENT_RECOVERY_SCHEMA);
    assert_eq!(envelope.state, "paused");
    assert_eq!(envelope.reason, "app_restarted");
    assert_eq!(envelope.source_run_id, "run-b");
    assert_eq!(
        reconcile_interrupted_agent_runs(&mut store).expect("recovery should be idempotent"),
        0
    );
}

#[test]
fn startup_recovery_preserves_pending_permission_as_blocked() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = [
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), "session-a".to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
        ("agent_effort".to_string(), "pro".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let mut start = context.clone();
    start.insert("prompt".to_string(), "write the report".to_string());
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        start,
    )
    .expect("run should start");
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "write the report",
        context.clone(),
    )
    .expect("user message should append");

    let invocation = ToolInvocation {
        id: agent_core::ToolCallId("call-write".to_string()),
        task_id: phase16_task_id(),
        tool_name: "file.write".to_string(),
        input_json: encode_input(&[("path", "report.md"), ("content", "draft")]),
        proposed_by_model: "agent-loop".to_string(),
        metadata: context.clone(),
    };
    let registry = ToolRegistry::with_workspace_tools(workspace_root());
    let mut request = registry
        .get("file.write")
        .expect("write tool should exist")
        .permission_request(&invocation)
        .expect("write should require permission");
    request.id = PermissionRequestId("permission-write".to_string());
    request.task_id = phase16_task_id();
    request.metadata.extend(context.clone());
    request
        .metadata
        .insert("tool_call_id".to_string(), "call-write".to_string());
    request
        .metadata
        .insert("tool_name".to_string(), "file.write".to_string());
    store
        .save_permission_request(request, current_time_millis())
        .expect("permission should persist");

    assert_eq!(
        reconcile_interrupted_agent_runs(&mut store).expect("recovery should succeed"),
        1
    );
    let state = agent_state_for_session(&store, None, Some("session-a"))
        .expect("blocked state should load");
    assert_eq!(state.status, "waiting_for_permission");
    assert_eq!(state.pending_approvals.len(), 1);
    assert!(!state.can_continue);
    let events = store
        .list_by_task_and_metadata_or_unscoped(&phase16_task_id(), "session_id", "session-a")
        .expect("events should load");
    let envelope =
        latest_agent_recovery_envelope(&events).expect("blocked recovery envelope should persist");
    assert_eq!(envelope.state, "blocked");
    assert_eq!(envelope.effort, "pro");
    assert_eq!(
        reconcile_interrupted_agent_runs(&mut store).expect("recovery should be idempotent"),
        0
    );
}

#[test]
fn recovery_envelope_is_bound_to_the_latest_external_user_turn() {
    let context = [
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), "session-a".to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
        ("agent_effort".to_string(), "auto".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let mut events = vec![
        Event {
            id: EventId("start".to_string()),
            task_id: phase16_task_id(),
            sequence: 1,
            timestamp_ms: 10,
            kind: EventKind::TaskStatusChanged,
            summary: "Agent task started".to_string(),
            metadata: metadata_with_context(
                [("prompt".to_string(), "finish alpha".to_string())]
                    .into_iter()
                    .collect(),
                &context,
            ),
        },
        Event {
            id: EventId("user-alpha".to_string()),
            task_id: phase16_task_id(),
            sequence: 2,
            timestamp_ms: 20,
            kind: EventKind::MessageAdded,
            summary: "user message".to_string(),
            metadata: metadata_with_context(
                [
                    ("role".to_string(), "user".to_string()),
                    ("content".to_string(), "finish alpha".to_string()),
                ]
                .into_iter()
                .collect(),
                &context,
            ),
        },
    ];
    let envelope =
        build_agent_recovery_envelope(&events, &context, "paused", "deadline_exceeded", 30)
            .expect("envelope should build");
    assert!(recovery_envelope_matches_active_turn(
        &envelope, &events, &context
    ));

    events.push(Event {
        id: EventId("replay".to_string()),
        task_id: phase16_task_id(),
        sequence: 3,
        timestamp_ms: 30,
        kind: EventKind::MessageAdded,
        summary: "user message".to_string(),
        metadata: metadata_with_context(
            [
                ("role".to_string(), "user".to_string()),
                ("content".to_string(), "finish alpha".to_string()),
                ("continuation_replay".to_string(), "true".to_string()),
            ]
            .into_iter()
            .collect(),
            &context,
        ),
    });
    assert!(recovery_envelope_matches_active_turn(
        &envelope, &events, &context
    ));

    events.push(Event {
        id: EventId("user-beta".to_string()),
        task_id: phase16_task_id(),
        sequence: 4,
        timestamp_ms: 40,
        kind: EventKind::MessageAdded,
        summary: "user message".to_string(),
        metadata: metadata_with_context(
            [
                ("role".to_string(), "user".to_string()),
                ("content".to_string(), "start beta".to_string()),
            ]
            .into_iter()
            .collect(),
            &context,
        ),
    });
    assert!(!recovery_envelope_matches_active_turn(
        &envelope, &events, &context
    ));
}

#[test]
fn recovery_envelope_round_trips_the_kernel_task_checkpoint() {
    let context = [
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), "session-a".to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let events = vec![
        Event {
            id: EventId("start".to_string()),
            task_id: phase16_task_id(),
            sequence: 1,
            timestamp_ms: 10,
            kind: EventKind::TaskStatusChanged,
            summary: "Agent task started".to_string(),
            metadata: metadata_with_context(
                [("prompt".to_string(), "finish alpha".to_string())]
                    .into_iter()
                    .collect(),
                &context,
            ),
        },
        Event {
            id: EventId("user-alpha".to_string()),
            task_id: phase16_task_id(),
            sequence: 2,
            timestamp_ms: 20,
            kind: EventKind::MessageAdded,
            summary: "user message".to_string(),
            metadata: metadata_with_context(
                [
                    ("role".to_string(), "user".to_string()),
                    ("content".to_string(), "finish alpha".to_string()),
                ]
                .into_iter()
                .collect(),
                &context,
            ),
        },
    ];
    let mut runtime = start_agent_loop(
        phase16_task_id(),
        "finish alpha",
        AgentRuntimeConfig::default(),
    );
    runtime.turn = 3;
    runtime.successful_mutations = 1;
    let checkpoint = AgentTaskStateSnapshot::capture(&runtime);
    let envelope = build_agent_recovery_envelope_with_task_state(
        &events,
        &context,
        "paused",
        "deadline_exceeded",
        30,
        Some(&checkpoint),
    )
    .expect("envelope should build");
    let encoded = serde_json::to_string(&envelope).expect("envelope encodes");
    let decoded = serde_json::from_str::<AgentRecoveryEnvelope>(&encoded)
        .expect("envelope decodes");
    let restored = decoded
        .task_state
        .expect("task checkpoint persists")
        .restore("finish alpha", runtime.messages.clone())
        .expect("checkpoint restores");

    assert_eq!(restored.turn, 3);
    assert_eq!(restored.successful_mutations, 1);
}

#[test]
fn recovery_claim_is_single_use_and_recovered_if_restart_interrupts_claim() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = [
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), "session-a".to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
        ("agent_effort".to_string(), "pro".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        metadata_with_context(
            [("prompt".to_string(), "finish alpha".to_string())]
                .into_iter()
                .collect(),
            &context,
        ),
    )
    .expect("run should start");
    append_message_event_with_metadata(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "finish alpha",
        context.clone(),
    )
    .expect("user message should append");
    let events = store
        .list_by_task_and_metadata_or_unscoped(&phase16_task_id(), "session_id", "session-a")
        .expect("events should load");
    let recovery_metadata = agent_recovery_metadata(
        &events,
        &context,
        "paused",
        "deadline_exceeded",
        [("completion".to_string(), "partial".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("recovery metadata should build");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task paused",
        recovery_metadata,
    )
    .expect("pause should persist");

    let claimed =
        claim_agent_recovery_envelope(&mut store, &context, &["paused"], "user_continued")
            .expect("claim should succeed")
            .expect("checkpoint should exist");
    assert_eq!(claimed.attempts, 1);
    let duplicate =
        claim_agent_recovery_envelope(&mut store, &context, &["paused"], "user_continued")
            .expect_err("a claimed recovery must not be claimed twice");
    assert!(duplicate.contains("already claimed"));

    assert_eq!(
        reconcile_interrupted_agent_runs(&mut store)
            .expect("a restart should pause an interrupted claim"),
        1
    );
    let state = agent_state_for_session(&store, None, Some("session-a"))
        .expect("recovered state should load");
    assert_eq!(state.status, "paused");
    let events = store
        .list_by_task_and_metadata_or_unscoped(&phase16_task_id(), "session_id", "session-a")
        .expect("events should reload");
    let recovered =
        latest_agent_recovery_envelope(&events).expect("recovered checkpoint should persist");
    assert_eq!(recovered.state, "paused");
    assert_eq!(recovered.attempts, 1);
}

#[test]
fn recovery_transcript_marks_unfinished_tool_calls_unknown() {
    let mut events = vec![
        Event {
            id: EventId("user".to_string()),
            task_id: phase16_task_id(),
            sequence: 1,
            timestamp_ms: 10,
            kind: EventKind::MessageAdded,
            summary: "user message".to_string(),
            metadata: [
                ("role".to_string(), "user".to_string()),
                ("content".to_string(), "update report".to_string()),
            ]
            .into_iter()
            .collect(),
        },
        Event {
            id: EventId("assistant-tool".to_string()),
            task_id: phase16_task_id(),
            sequence: 2,
            timestamp_ms: 20,
            kind: EventKind::MessageAdded,
            summary: "assistant message".to_string(),
            metadata: [
                ("role".to_string(), "assistant".to_string()),
                ("content".to_string(), String::new()),
                ("tool_call_ids".to_string(), "call-write".to_string()),
                ("raw_tool_calls_json".to_string(), "[]".to_string()),
            ]
            .into_iter()
            .collect(),
        },
    ];
    let interrupted = recovery_safe_transcript(&events);
    assert_eq!(interrupted.len(), 3);
    assert_eq!(interrupted[2].role, MessageRole::Tool);
    assert_eq!(
        interrupted[2].metadata.get("status").map(String::as_str),
        Some("interrupted")
    );
    assert!(interrupted[2].content.contains("outcome as unknown"));

    events.push(Event {
        id: EventId("tool-result".to_string()),
        task_id: phase16_task_id(),
        sequence: 3,
        timestamp_ms: 30,
        kind: EventKind::MessageAdded,
        summary: "tool message".to_string(),
        metadata: [
            ("role".to_string(), "tool".to_string()),
            ("content".to_string(), "write completed".to_string()),
            ("tool_call_id".to_string(), "call-write".to_string()),
            ("status".to_string(), "succeeded".to_string()),
        ]
        .into_iter()
        .collect(),
    });
    let resolved = recovery_safe_transcript(&events);
    assert_eq!(resolved.len(), 3);
    assert_eq!(resolved[2].content, "write completed");
}

#[test]
fn queued_agent_message_ids_accept_only_bounded_client_ids() {
    assert_eq!(
        queued_agent_message_id(Some(" agent-queue-client-abc-123 ")),
        "agent-queue-client-abc-123"
    );
    assert!(!queued_agent_message_id(Some("queue-client-abc")).starts_with("queue-client-"));
    assert!(!queued_agent_message_id(Some("agent-queue-client-bad/id")).contains("bad/id"));
}

#[test]
fn queued_agent_messages_are_durable_ordered_and_session_scoped() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context_a = [("session_id".to_string(), "session-a".to_string())]
        .into_iter()
        .collect::<Metadata>();
    let context_b = [("session_id".to_string(), "session-b".to_string())]
        .into_iter()
        .collect::<Metadata>();
    let payload = |prompt: &str| QueuedAgentMessagePayload {
        prompt: prompt.to_string(),
        attachments: Vec::new(),
        effort: "auto".to_string(),
        current_time: "now".to_string(),
    };
    let first = payload("first");
    let second = payload("second");
    let other = payload("other session");
    append_agent_queue_event(
        &mut store,
        &context_a,
        "enqueue",
        "queue-a-1",
        "queue",
        10,
        Some(&first),
    )
    .expect("first message should queue");
    append_agent_queue_event(
        &mut store,
        &context_a,
        "enqueue",
        "queue-a-2",
        "queue",
        20,
        Some(&second),
    )
    .expect("second message should queue");
    append_agent_queue_event(
        &mut store,
        &context_b,
        "enqueue",
        "queue-b-1",
        "queue",
        5,
        Some(&other),
    )
    .expect("other session message should queue");
    append_agent_queue_event(
        &mut store,
        &context_a,
        "steer",
        "queue-a-2",
        "steer",
        20,
        None,
    )
    .expect("second message should steer");

    let events = store
        .list_by_task(&phase16_task_id())
        .expect("queue events should load");
    let pending = pending_queued_agent_messages(&events, "session-a");
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].view.id, "queue-a-2");
    assert_eq!(pending[0].view.mode, "steer");
    assert_eq!(pending[1].view.id, "queue-a-1");

    let state =
        agent_state_for_session(&store, None, Some("session-a")).expect("queued state should load");
    assert_eq!(state.session_id.as_deref(), Some("session-a"));
    assert_eq!(state.status, "idle");
    assert_eq!(state.queued_messages.len(), 2);
    assert!(state.timeline.is_empty());
    let edited = payload("first edited");
    append_agent_queue_event(
        &mut store,
        &context_a,
        "edit",
        "queue-a-1",
        "queue",
        10,
        Some(&edited),
    )
    .expect("first message should edit");
    append_agent_queue_event(
        &mut store,
        &context_a,
        "delete",
        "queue-a-2",
        "steer",
        20,
        None,
    )
    .expect("steered message should delete");
    let events = store
        .list_by_task(&phase16_task_id())
        .expect("edited queue events should load");
    let remaining = pending_queued_agent_messages(&events, "session-a");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].view.prompt, "first edited");
    assert_eq!(
        agent_state_for_session(&store, None, Some("session-b"))
            .expect("other queued state should load")
            .queued_messages
            .len(),
        1
    );
}

#[test]
fn queue_events_do_not_change_a_terminal_agent_status() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = [
        ("session_id".to_string(), "session-a".to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        metadata_with_context(
            [("prompt".to_string(), "first task".to_string())]
                .into_iter()
                .collect(),
            &context,
        ),
    )
    .expect("run should start");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        context.clone(),
    )
    .expect("run should complete");
    let queued = QueuedAgentMessagePayload {
        prompt: "next task".to_string(),
        attachments: Vec::new(),
        effort: "auto".to_string(),
        current_time: "now".to_string(),
    };
    append_agent_queue_event(
        &mut store,
        &context,
        "enqueue",
        "queue-next",
        "queue",
        20,
        Some(&queued),
    )
    .expect("next task should queue");

    let state = agent_state_for_session(&store, None, Some("session-a"))
        .expect("terminal queue state should load");
    assert_eq!(state.status, "completed");
    assert_eq!(state.queued_messages.len(), 1);
    assert_eq!(state.timeline.len(), 2);
}

#[test]
fn queued_messages_preserve_a_permission_waiting_run() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = [
        ("session_id".to_string(), "session-a".to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        metadata_with_context(
            [("prompt".to_string(), "inspect files".to_string())]
                .into_iter()
                .collect(),
            &context,
        ),
    )
    .expect("run should start");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task waiting for permission",
        context.clone(),
    )
    .expect("run should wait");
    let queued = QueuedAgentMessagePayload {
        prompt: "follow-up".to_string(),
        attachments: Vec::new(),
        effort: "auto".to_string(),
        current_time: "now".to_string(),
    };
    append_agent_queue_event(
        &mut store,
        &context,
        "enqueue",
        "queue-next",
        "queue",
        20,
        Some(&queued),
    )
    .expect("follow-up should queue");

    let state = agent_state_for_session(&store, None, Some("session-a"))
        .expect("waiting queue state should load");
    assert_eq!(state.status, "waiting_for_permission");
    assert!(state.can_cancel);
    assert_eq!(state.queued_messages.len(), 1);
}

#[test]
fn queued_agent_message_start_and_restore_are_replay_safe() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = [("session_id".to_string(), "session-a".to_string())]
        .into_iter()
        .collect::<Metadata>();
    let payload = QueuedAgentMessagePayload {
        prompt: "continue safely".to_string(),
        attachments: Vec::new(),
        effort: "pro".to_string(),
        current_time: "now".to_string(),
    };
    append_agent_queue_event(
        &mut store,
        &context,
        "enqueue",
        "queue-a",
        "queue",
        10,
        Some(&payload),
    )
    .expect("message should queue");
    append_agent_queue_event(&mut store, &context, "start", "queue-a", "queue", 10, None)
        .expect("message should start");
    let events = store
        .list_by_task(&phase16_task_id())
        .expect("queue events should load");
    assert!(pending_queued_agent_messages(&events, "session-a").is_empty());

    append_agent_queue_event(
        &mut store,
        &context,
        "restore",
        "queue-a",
        "queue",
        10,
        Some(&payload),
    )
    .expect("message should restore");
    let events = store
        .list_by_task(&phase16_task_id())
        .expect("restored queue events should load");
    let restored = pending_queued_agent_messages(&events, "session-a");
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].view.prompt, "continue safely");
    assert_eq!(restored[0].view.effort, "pro");
}

#[test]
fn queued_agent_messages_update_the_incremental_session_read_model() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let session_id = "session-queue-read-model";
    let context = [("session_id".to_string(), session_id.to_string())]
        .into_iter()
        .collect::<Metadata>();
    let mut payload = QueuedAgentMessagePayload {
        prompt: "first version".to_string(),
        attachments: Vec::new(),
        effort: "auto".to_string(),
        current_time: "now".to_string(),
    };
    append_agent_queue_event(
        &mut store,
        &context,
        "enqueue",
        "queue-a",
        "queue",
        10,
        Some(&payload),
    )
    .expect("message should queue");
    let initial = load_agent_session_read_model(&mut store, session_id)
        .expect("initial queue read model should build");
    assert_eq!(initial.state.queued_messages.len(), 1);
    assert_eq!(
        initial
            .queued_payloads
            .get("queue-a")
            .map(|payload| payload.prompt.as_str()),
        Some("first version")
    );

    payload.prompt = "edited version".to_string();
    append_agent_queue_event(
        &mut store,
        &context,
        "edit",
        "queue-a",
        "queue",
        10,
        Some(&payload),
    )
    .expect("message should edit");
    let edited = load_agent_session_read_model(&mut store, session_id)
        .expect("queue edit should apply incrementally");
    assert_eq!(edited.state.queued_messages[0].prompt, "edited version");
    let next = next_queued_agent_message_from_read_model(&mut store, session_id)
        .expect("next queue item should use the read model")
        .expect("next queue item should exist");
    assert_eq!(next.payload.prompt, "edited version");
    let (queued, can_cancel) =
        queued_agent_message_from_read_model(&mut store, session_id, "queue-a")
            .expect("queue action lookup should use the read model");
    let receipt =
        queued_agent_message_action_receipt(&store, session_id, "queue-a", queued, can_cancel)
            .expect("queue action receipt should use the compact revision");
    assert_eq!(
        receipt
            .message
            .as_ref()
            .map(|message| message.prompt.as_str()),
        Some("edited version")
    );
    assert!(!receipt.cancelled_active_run);

    append_agent_queue_event(&mut store, &context, "start", "queue-a", "queue", 10, None)
        .expect("message should start");
    let started = load_agent_session_read_model(&mut store, session_id)
        .expect("queue start should apply incrementally");
    assert!(started.state.queued_messages.is_empty());
    assert!(started.queued_payloads.is_empty());

    let run_context = [
        ("session_id".to_string(), session_id.to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
        ("queue_id".to_string(), "queue-a".to_string()),
    ]
    .into_iter()
    .collect();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        run_context,
    )
    .expect("queued run should start");
    let running = load_agent_session_read_model(&mut store, session_id)
        .expect("run identity should apply incrementally");
    assert_eq!(running.latest_run_queue_id.as_deref(), Some("queue-a"));
}

#[test]
fn partial_budget_completion_exposes_a_continuation() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = [
        ("session_id".to_string(), "session-a".to_string()),
        ("agent_run_id".to_string(), "run-a".to_string()),
        ("prompt".to_string(), "finish the long task".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        context.clone(),
    )
    .expect("run start should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        metadata_with_context(
            [
                ("completion".to_string(), "partial".to_string()),
                ("continuation_available".to_string(), "true".to_string()),
            ]
            .into_iter()
            .collect(),
            &context,
        ),
    )
    .expect("partial completion should append");

    let state =
        agent_state_for_session(&store, None, Some("session-a")).expect("agent state should load");
    assert_eq!(state.status, "completed");
    assert!(state.can_retry);
    assert!(state.can_continue);
}

#[test]
fn session_permission_grant_covers_non_destructive_requests_in_the_same_session() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let granted = PermissionRequest {
        id: PermissionRequestId("session-grant".to_string()),
        task_id: phase16_task_id(),
        risk: PermissionRisk::Execute,
        action: "shell.run".to_string(),
        reason: "run a command".to_string(),
        scope: ".".to_string(),
        metadata: [
            ("session_id".to_string(), "session-a".to_string()),
            ("agent_run_id".to_string(), "run-a".to_string()),
        ]
        .into_iter()
        .collect(),
    };
    store
        .save_permission_request(granted.clone(), 1)
        .expect("grant request should save");
    store
        .resolve_permission(PermissionResolution {
            request_id: granted.id.clone(),
            decision: PermissionDecision::AllowForSession,
            resolved_at_ms: 2,
            resolved_by: "local-user".to_string(),
        })
        .expect("grant should resolve");

    let mut next = granted.clone();
    next.id = PermissionRequestId("next-request".to_string());
    assert!(
        agent_session_permission_granted(&store, &phase16_task_id(), &next, Some("session-a"),)
            .expect("matching grant should load")
    );
    assert!(!agent_session_permission_granted(
        &store,
        &phase16_task_id(),
        &next,
        Some("session-b"),
    )
    .expect("other session should load"));
    next.action = "file.write".to_string();
    next.scope = "crates/tools".to_string();
    next.risk = PermissionRisk::Write;
    assert!(
        agent_session_permission_granted(&store, &phase16_task_id(), &next, Some("session-a"),)
            .expect("session grant should cover another non-destructive request")
    );
    next.risk = PermissionRisk::Destructive;
    assert!(!agent_session_permission_granted(
        &store,
        &phase16_task_id(),
        &next,
        Some("session-a"),
    )
    .expect("destructive grant should not persist"));
}

#[test]
fn pending_permissions_are_isolated_by_agent_run() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    for (id, run_id) in [("pending-old", "run-old"), ("pending-new", "run-new")] {
        store
            .save_permission_request(
                PermissionRequest {
                    id: PermissionRequestId(id.to_string()),
                    task_id: phase16_task_id(),
                    risk: PermissionRisk::Execute,
                    action: "shell.run".to_string(),
                    reason: "run a command".to_string(),
                    scope: ".".to_string(),
                    metadata: [
                        ("session_id".to_string(), "session-a".to_string()),
                        ("agent_run_id".to_string(), run_id.to_string()),
                    ]
                    .into_iter()
                    .collect(),
                },
                if run_id == "run-old" { 1 } else { 2 },
            )
            .expect("pending request should save");
    }

    let pending = pending_agent_permissions_for_run(
        &store,
        &phase16_task_id(),
        Some("session-a"),
        Some("run-new"),
    )
    .expect("pending requests should load");

    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id.0, "pending-new");
}

#[test]
fn agent_state_ignores_old_pending_approvals_after_new_run() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let mut request = PermissionRequest {
        id: PermissionRequestId("old-agent-perm".to_string()),
        task_id: phase16_task_id(),
        risk: PermissionRisk::Write,
        action: "file.write".to_string(),
        reason: "old run".to_string(),
        scope: ".".to_string(),
        metadata: [
            ("tool_call_id".to_string(), "old-call".to_string()),
            ("tool_name".to_string(), "file.write".to_string()),
            ("tool_input".to_string(), "path=old.txt".to_string()),
        ]
        .into_iter()
        .collect(),
    };
    request
        .metadata
        .insert("phase".to_string(), "16".to_string());
    store
        .save_permission_request(request, 1)
        .expect("old request should save");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        [("prompt".to_string(), "new task".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("start should append");
    append_message_event(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "new task",
    )
    .expect("user message should append");

    let state = agent_state(&store, None).expect("state should load");

    assert_eq!(state.status, "running");
    assert!(state.pending_approvals.is_empty());
    assert_eq!(state.transcript_messages, 1);
}

#[test]
fn agent_state_reports_cancelled_and_retryable() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        [("prompt".to_string(), "inspect".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("start should append");
    append_message_event(&mut store, &phase16_task_id(), MessageRole::User, "inspect")
        .expect("user message should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task cancelled",
        Metadata::new(),
    )
    .expect("cancel should append");

    let state = agent_state(&store, None).expect("state should load");

    assert_eq!(state.status, "cancelled");
    assert!(state.can_retry);
    assert!(!state.can_cancel);
}

#[test]
fn agent_trace_groups_steps_by_turn_and_exposes_details() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        [("prompt".to_string(), "read README".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("start should append");
    append_message_event(
        &mut store,
        &phase16_task_id(),
        MessageRole::User,
        "read README",
    )
    .expect("user message should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::ModelRequestStarted,
        "Agent model turn started",
        [
            ("request_id".to_string(), "agent-model-1".to_string()),
            ("turn".to_string(), "0".to_string()),
            ("model".to_string(), "model-a".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("model start should append");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::ModelRequestFinished,
        "Agent model turn finished",
        [
            ("request_id".to_string(), "agent-model-1".to_string()),
            ("latency_ms".to_string(), "42".to_string()),
            ("tool_calls".to_string(), "1".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("model finish should append");
    append_tool_finished_event(
        &mut store,
        &phase16_task_id(),
        "call-1",
        "file.write",
        "succeeded",
        "written ok",
        [("path".to_string(), "notes/result.md".to_string())]
            .into_iter()
            .collect(),
        None,
    )
    .expect("tool finish should append");

    let trace =
        agent_trace_state_for_session(&store, None, None, None).expect("trace should build");

    assert_eq!(trace.turn_count, 1);
    assert!(trace.step_count >= 5);
    assert!(trace.tool_call_count >= 1);
    assert!(trace.turns.iter().any(|turn| turn.index == 1
        && turn
            .steps
            .iter()
            .any(|step| { step.kind == "model" && step.latency_ms == Some(42) })));
    assert!(trace
        .turns
        .iter()
        .flat_map(|turn| turn.steps.iter())
        .any(|step| step.tool_name.as_deref() == Some("file.write")
            && step.output_preview.as_deref() == Some("written ok")
            && step.artifact_path.as_deref() == Some("notes/result.md")));
}

#[test]
fn agent_trace_reports_actual_collaboration_role_activity() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let context = [
        ("session_id".to_string(), "session-role-trace".to_string()),
        ("project_id".to_string(), "project-role-trace".to_string()),
        ("agent_run_id".to_string(), "run-role-trace".to_string()),
        (
            "collaboration_id".to_string(),
            "collab-role-trace".to_string(),
        ),
    ]
    .into_iter()
    .collect::<Metadata>();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        metadata_with_context(
            [("prompt".to_string(), "compare approaches".to_string())]
                .into_iter()
                .collect(),
            &context,
        ),
    )
    .expect("start should append");
    for (role, model, status, latency, first_token, tokens, evidence) in [
        (
            "planner",
            "model-planner",
            "completed",
            "120",
            "30",
            "80",
            "2",
        ),
        (
            "reviewer",
            "model-reviewer",
            "degraded",
            "90",
            "25",
            "40",
            "1",
        ),
    ] {
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::ModelRequestFinished,
            format!("Collaboration {role} finished"),
            metadata_with_context(
                [
                    ("role".to_string(), role.to_string()),
                    ("model".to_string(), model.to_string()),
                    ("status".to_string(), status.to_string()),
                    ("latency_ms".to_string(), latency.to_string()),
                    (
                        "first_token_latency_ms".to_string(),
                        first_token.to_string(),
                    ),
                    ("total_tokens".to_string(), tokens.to_string()),
                    ("evidence_count".to_string(), evidence.to_string()),
                ]
                .into_iter()
                .collect(),
                &context,
            ),
        )
        .expect("role event should append");
    }
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        context,
    )
    .expect("completion should append");

    let trace = agent_trace_state_for_session(&store, None, None, Some("session-role-trace"))
        .expect("trace should build");

    assert_eq!(trace.role_summaries.len(), 2);
    assert_eq!(trace.role_summaries[0].role, "planner");
    assert_eq!(trace.role_summaries[0].models, vec!["model-planner"]);
    assert_eq!(trace.role_summaries[0].completed, 1);
    assert_eq!(trace.role_summaries[0].latency_ms, 120);
    assert_eq!(trace.role_summaries[0].first_token_latency_ms, Some(30));
    assert_eq!(trace.role_summaries[1].role, "reviewer");
    assert_eq!(trace.role_summaries[1].degraded, 1);
    assert_eq!(trace.role_summaries[1].evidence_count, 1);
}

#[test]
fn agent_outputs_accumulate_versioned_files_across_session_runs() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    for (index, run_id) in ["run-one", "run-two"].into_iter().enumerate() {
        let context = [
            ("session_id".to_string(), "session-alpha".to_string()),
            ("agent_run_id".to_string(), run_id.to_string()),
        ]
        .into_iter()
        .collect();
        append_tool_finished_event(
            &mut store,
            &phase16_task_id(),
            &format!("call-{index}"),
            "file.write",
            "succeeded",
            "file written",
            [
                ("path".to_string(), "notes/result.md".to_string()),
                (
                    "source_path".to_string(),
                    "/workspace/notes/result.md".to_string(),
                ),
                (
                    "artifact_path".to_string(),
                    format!("/workspace/.cindx/output-history/{run_id}/result.md"),
                ),
            ]
            .into_iter()
            .collect(),
            Some(&context),
        )
        .expect("tool output should append");
    }
    let context = [
        ("session_id".to_string(), "session-alpha".to_string()),
        ("agent_run_id".to_string(), "run-two".to_string()),
    ]
    .into_iter()
    .collect();
    append_tool_finished_event(
        &mut store,
        &phase16_task_id(),
        "call-list",
        "file.list",
        "succeeded",
        "notes",
        [("path".to_string(), "notes".to_string())]
            .into_iter()
            .collect(),
        Some(&context),
    )
    .expect("read-only tool output should append");

    let events = agent_events_for_session(&store, &phase16_task_id(), Some("session-alpha"))
        .expect("session events should load");
    let outputs = agent_output_artifacts_from_events(&events);
    let manifest = artifact_manifest_message(&events).expect("manifest should exist");

    assert_eq!(outputs.len(), 2);
    assert_eq!(
        outputs
            .iter()
            .map(|output| output.version)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([1, 2])
    );
    assert!(outputs
        .iter()
        .all(|output| { output.source_path.as_deref() == Some("/workspace/notes/result.md") }));
    assert!(outputs
        .iter()
        .any(|output| output.run_id.as_deref() == Some("run-one")));
    assert!(outputs
        .iter()
        .any(|output| output.run_id.as_deref() == Some("run-two")));
    assert_eq!(
        manifest.metadata.get("kind").map(String::as_str),
        Some("artifact_manifest")
    );
    assert!(manifest.content.contains("/workspace/notes/result.md"));
    assert!(manifest.content.contains("run-one"));
    assert!(manifest.content.contains("run-two"));
    assert!(manifest.content.contains("latest_version\": 2"));
}

#[test]
fn agent_trace_export_writes_jsonl() {
    let root = temp_test_root("phase18-trace");
    fs::create_dir_all(&root).expect("temp root should exist");
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        [("prompt".to_string(), "inspect".to_string())]
            .into_iter()
            .collect(),
    )
    .expect("start should append");
    append_message_event(&mut store, &phase16_task_id(), MessageRole::User, "inspect")
        .expect("user message should append");

    let path = write_agent_trace_jsonl(&root, &store, None).expect("trace should export");
    let text = fs::read_to_string(path).expect("trace export should be readable");

    assert!(text.contains("\"trace_id\""));
    assert!(text.contains("\"task_id\":\"phase-16-agent-loop\""));
    assert!(text.contains("\"kind\":\"message\""));
}

#[test]
fn project_session_state_defaults_to_active_workspace() {
    let root = temp_test_root("phase20-projects");
    let config = ProjectSessionConfig::default_for_root(&root);
    let session_id = config.active_session_id.clone();
    let state = project_session_state(&config, None);

    assert_eq!(state.projects.len(), 1);
    assert_eq!(state.sessions.len(), 1);
    assert_eq!(state.projects[0].root, root.display().to_string());
    assert!(state.projects[0].active);
    assert!(state.sessions[0].active);
    assert_eq!(state.sessions[0].effort, "auto");
    assert_eq!(state.active_project_id, "project-cindx");
    assert_eq!(state.active_session_id, session_id);
    assert!(state.active_session_id.starts_with("sess_"));
}

#[test]
fn new_session_ids_are_unique_uuid_v7_values() {
    let first = new_session_id();
    let second = new_session_id();

    assert_ne!(first, second);
    for session_id in [first, second] {
        let value = session_id
            .strip_prefix("sess_")
            .expect("session id should use the opaque prefix");
        let uuid = uuid::Uuid::parse_str(value).expect("session id should contain a UUID");
        assert_eq!(uuid.get_version(), Some(uuid::Version::SortRand));
    }
}

#[test]
fn schedule_execution_sessions_stay_out_of_the_task_sidebar() {
    let root = temp_test_root("hidden-schedule-session");
    let mut config = ProjectSessionConfig::default_for_root(&root);
    let initial_session_id = config.active_session_id.clone();
    config.sessions.push(SessionRecord {
        id: "schedule-session-review".to_string(),
        project_id: "project-cindx".to_string(),
        name: "Review · Schedule".to_string(),
        detail: SCHEDULE_EXECUTION_SESSION_DETAIL.to_string(),
        effort: "auto".to_string(),
        title_state: SessionTitleState::Manual,
        seen_event_sequence: 0,
        created_at_ms: 2,
        updated_at_ms: 2,
        archived_at_ms: None,
    });

    let state = project_session_state(&config, None);
    assert_eq!(state.sessions.len(), 1);
    assert_eq!(state.sessions[0].id, initial_session_id);

    config.sessions.retain(is_schedule_execution_session);
    let replacement = ensure_open_session_for_project(&mut config, "project-cindx");
    assert_ne!(replacement, "schedule-session-review");
    assert!(config
        .sessions
        .iter()
        .any(|session| { session.id == replacement && !is_schedule_execution_session(session) }));
}

#[test]
fn session_effort_updates_only_the_selected_session() {
    let root = temp_test_root("session-effort");
    let mut config = ProjectSessionConfig::default_for_root(&root);
    let initial_session_id = config.active_session_id.clone();
    config.sessions.push(SessionRecord {
        id: "session-second".to_string(),
        project_id: config.projects[0].id.clone(),
        name: "Second".to_string(),
        detail: "timeline + chat".to_string(),
        effort: default_agent_effort(),
        title_state: SessionTitleState::Manual,
        seen_event_sequence: 0,
        created_at_ms: 2,
        updated_at_ms: 2,
        archived_at_ms: None,
    });

    assert!(update_session_effort(
        &mut config,
        &initial_session_id,
        "pro"
    ));
    assert_eq!(config.sessions[0].effort, "pro");
    assert_eq!(config.sessions[1].effort, "auto");
    assert!(!update_session_effort(&mut config, "missing", "high"));
}

#[test]
fn deleting_a_project_removes_its_sessions_and_selects_a_neighbor() {
    let root = temp_test_root("delete-project");
    let mut config = ProjectSessionConfig::default_for_root(&root);
    let initial_session_id = config.active_session_id.clone();
    config.projects.push(ProjectRecord {
        id: "project-next".to_string(),
        name: "Next".to_string(),
        root: root.join("next").display().to_string(),
        detail: "workspace project".to_string(),
        created_at_ms: 2,
        updated_at_ms: 2,
    });
    config.sessions.push(SessionRecord {
        id: "session-next".to_string(),
        project_id: "project-next".to_string(),
        name: "Next Session".to_string(),
        detail: "timeline + chat".to_string(),
        effort: "pro".to_string(),
        title_state: SessionTitleState::Manual,
        seen_event_sequence: 0,
        created_at_ms: 2,
        updated_at_ms: 2,
        archived_at_ms: None,
    });

    let (_, deleted_session_ids) = remove_project_from_config(&mut config, "project-cindx")
        .expect("project should be removed");

    assert_eq!(deleted_session_ids, vec![initial_session_id]);
    assert_eq!(config.projects.len(), 1);
    assert_eq!(config.sessions.len(), 1);
    assert_eq!(config.active_project_id, "project-next");
    assert_eq!(config.active_session_id, "session-next");
    assert_eq!(config.sessions[0].effort, "pro");
}

#[test]
fn deleting_the_last_project_leaves_an_empty_workspace() {
    let root = temp_test_root("delete-last-project");
    let mut config = ProjectSessionConfig::default_for_root(&root);

    remove_project_from_config(&mut config, "project-cindx").expect("project should be removed");

    assert!(config.projects.is_empty());
    assert!(config.sessions.is_empty());
    assert!(config.active_project_id.is_empty());
    assert!(config.active_session_id.is_empty());
}

#[test]
fn automatic_session_names_use_the_first_prompt() {
    assert!(is_automatic_session_name("Runtime Session"));
    assert!(is_automatic_session_name("New Session"));
    assert!(!is_automatic_session_name("Release planning"));
    assert_eq!(
        automatic_session_title("  Review   the project architecture and risks  "),
        "Review the project architecture and risks"
    );
    assert_eq!(
        automatic_session_title("请帮我修复 session 自动命名"),
        "修复 session 自动命名"
    );
    assert_eq!(
        automatic_session_title("请帮我分析 MBTI，重点区分 N/S？"),
        "分析 MBTI 重点区分 N/S"
    );
}

#[test]
fn semantic_session_titles_reject_raw_conversation_sentences() {
    let turns = vec![
        SessionTitleTurn {
            prompt: "你帮我画一个超时空要塞的三段变形机器人".to_string(),
            answer: "我会生成一张三段变形机器人设定图。".to_string(),
        },
        SessionTitleTurn {
            prompt: "这是高达，不是马克罗士，你重新画".to_string(),
            answer: "我会按超时空要塞 VF-1 的特征重新绘制。".to_string(),
        },
    ];

    assert!(generated_session_title_copies_conversation(
        "这是高达 不是马克罗士 你重新画",
        &turns
    ));
    assert_eq!(
        validated_generated_session_title("这是高达 不是马克罗士 你重新画", &turns),
        None
    );
    assert_eq!(
        validated_generated_session_title("重绘超时空要塞变形机器人", &turns),
        Some("重绘超时空要塞变形机器人".to_string())
    );
}

#[test]
fn concise_user_topic_can_already_be_a_valid_title() {
    let turns = vec![SessionTitleTurn {
        prompt: "Rust agent loop review".to_string(),
        answer: "I found two lifecycle races.".to_string(),
    }];

    assert!(!generated_session_title_copies_conversation(
        "Rust agent loop review",
        &turns
    ));
    assert_eq!(
        validated_generated_session_title("Rust agent loop review", &turns),
        Some("Rust agent loop review".to_string())
    );
}

#[test]
fn session_title_fallback_recovers_a_pending_conversation_without_copying_a_request() {
    let title_like_turns = vec![SessionTitleTurn {
        prompt: "用 mindmap 描述下 transformer 架构".to_string(),
        answer: "下面用思维导图展示 Transformer 的结构。".to_string(),
    }];
    assert_eq!(
        fallback_session_title(&title_like_turns),
        Some("用 mindmap 描述下 transformer 架构".to_string())
    );

    let request_turns = vec![SessionTitleTurn {
        prompt: "请帮我修复 session 自动命名".to_string(),
        answer: "我会检查标题生成与持久化链路。".to_string(),
    }];
    assert_eq!(
        fallback_session_title(&request_turns),
        Some("修复 session 自动命名".to_string())
    );
}

#[test]
fn session_title_state_retries_pending_and_repairs_legacy_prompt_copies() {
    let turns = vec![SessionTitleTurn {
        prompt: "这是高达，不是马克罗士，你重新画".to_string(),
        answer: "我会按超时空要塞的设定重新绘制。".to_string(),
    }];

    assert!(session_title_refinement_needed(
        SessionTitleState::Pending,
        "New Session",
        &turns
    ));
    assert!(session_title_refinement_needed(
        SessionTitleState::Automatic,
        "这是高达 不是马克罗士 你重新画",
        &turns
    ));
    assert!(!session_title_refinement_needed(
        SessionTitleState::Automatic,
        "重绘超时空要塞变形机器人",
        &turns
    ));
    assert!(!session_title_refinement_needed(
        SessionTitleState::Manual,
        "这是高达 不是马克罗士 你重新画",
        &turns
    ));
}

#[test]
fn generated_session_titles_are_clean_and_bounded() {
    assert_eq!(
        cleaned_generated_session_title("**标题：桌面宠物开发。**\nextra"),
        Some("桌面宠物开发".to_string())
    );
    assert_eq!(
        cleaned_generated_session_title("Title: Review repository architecture"),
        Some("Review repository architecture".to_string())
    );
    assert_eq!(cleaned_generated_session_title("New Session"), None);
    assert_eq!(cleaned_generated_session_title("你好，Dale！我是"), None);
    assert_eq!(cleaned_generated_session_title("我是 Cindx"), None);
}

#[test]
fn session_title_context_skips_greetings_and_uses_two_meaningful_turns() {
    let message = |sequence, role: &str, content: &str| ChatMessageView {
        sequence,
        role: role.to_string(),
        content: content.to_string(),
        timestamp_ms: sequence,
        run_id: None,
        queue_id: None,
        attachments: Vec::new(),
    };
    let messages = vec![
        message(1, "user", "你好"),
        message(2, "assistant", "你好，Dale！我是 Cindx。"),
        message(3, "user", "分析我们对话里体现出的 MBTI 倾向"),
        message(4, "assistant", "我会根据具体措辞分析倾向。"),
        message(5, "user", "重点区分 N/S，并给出直接证据"),
        message(6, "assistant", "N/S 的证据主要来自抽象与细节偏好。"),
    ];

    let turns = meaningful_session_title_turns(&messages);
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0].prompt, "分析我们对话里体现出的 MBTI 倾向");
    assert_eq!(turns[1].prompt, "重点区分 N/S，并给出直接证据");
    assert_eq!(turns[1].answer, "N/S 的证据主要来自抽象与细节偏好。");
}

#[test]
fn greeting_only_turns_do_not_claim_a_session_title() {
    for greeting in ["你好", "您好！", "hello", "Hi there"] {
        assert!(!is_meaningful_session_title_prompt(greeting));
    }
    assert!(is_meaningful_session_title_prompt(
        "你好，帮我审查 Rust agent loop"
    ));
    assert!(is_meaningful_session_title_prompt(
        "Hello World app architecture"
    ));
}

#[test]
fn generated_session_titles_do_not_overwrite_later_edits() {
    assert!(can_apply_generated_session_title(
        "Initial request title",
        42,
        "Initial request title",
        42
    ));
    assert!(!can_apply_generated_session_title(
        "My custom title",
        43,
        "Initial request title",
        42
    ));
    assert!(!can_apply_generated_session_title(
        "Initial request title",
        43,
        "Initial request title",
        42
    ));
}

#[test]
fn archived_active_session_gets_a_visible_replacement_and_can_be_listed() {
    let root = temp_test_root("archived-session");
    let mut config = ProjectSessionConfig::default_for_root(&root);
    config.sessions[0].archived_at_ms = Some(42);
    config.ensure_consistent(&root);

    let state = project_session_state(&config, None);
    let archived = state
        .sessions
        .iter()
        .find(|session| session.archived)
        .expect("archived session should remain recoverable");
    let active = state
        .sessions
        .iter()
        .find(|session| session.active)
        .expect("replacement session should be active");

    assert_eq!(archived.archived_at_ms, Some(42));
    assert_ne!(archived.id, active.id);
    assert!(!active.archived);
}

#[test]
fn archived_session_restore_does_not_revive_seen_activity() {
    let root = temp_test_root("archived-session-seen-activity");
    let mut config = ProjectSessionConfig::default_for_root(&root);
    let session_id = config.sessions[0].id.clone();
    let mut store = SqliteStore::in_memory().expect("store should open");
    let metadata = [("session_id".to_string(), session_id.clone())]
        .into_iter()
        .collect();
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        metadata,
    )
    .expect("completion should append");
    let latest_sequence = load_agent_session_read_model_snapshot(&store, &session_id)
        .expect("session model should load")
        .revision;

    config.sessions[0].seen_event_sequence = latest_sequence;
    config.sessions[0].archived_at_ms = Some(42);
    let archived = project_session_state_from_store(&config, &store, None)
        .expect("archived state should project");
    assert_eq!(archived.sessions[0].activity, "idle");
    assert!(!archived.sessions[0].unseen_result);

    config.sessions[0].archived_at_ms = None;
    let restored = project_session_state_from_store(&config, &store, None)
        .expect("restored state should project");
    assert_eq!(restored.sessions[0].activity, "idle");
    assert!(!restored.sessions[0].unseen_result);
}

#[test]
fn fork_names_are_unique_within_a_project() {
    let root = temp_test_root("fork-name");
    let mut config = ProjectSessionConfig::default_for_root(&root);
    let source = config.sessions[0].clone();
    assert_eq!(unique_fork_name(&config, &source), "Runtime Session Fork");
    config.sessions.push(SessionRecord {
        id: "fork-one".to_string(),
        project_id: source.project_id.clone(),
        name: "Runtime Session Fork".to_string(),
        detail: "fork".to_string(),
        effort: default_agent_effort(),
        title_state: SessionTitleState::Manual,
        seen_event_sequence: 0,
        created_at_ms: 1,
        updated_at_ms: 1,
        archived_at_ms: None,
    });

    assert_eq!(unique_fork_name(&config, &source), "Runtime Session Fork 2");
}

#[test]
fn agent_state_and_trace_expose_project_session_context() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        [
            ("prompt".to_string(), "inspect".to_string()),
            ("project_id".to_string(), "project-alpha".to_string()),
            ("project_name".to_string(), "Alpha".to_string()),
            ("session_id".to_string(), "session-alpha".to_string()),
            ("session_name".to_string(), "Alpha Session".to_string()),
        ]
        .into_iter()
        .collect(),
    )
    .expect("start should append");
    append_message_event(&mut store, &phase16_task_id(), MessageRole::User, "inspect")
        .expect("user message should append");

    let state = agent_state(&store, None).expect("state should load");
    let trace =
        agent_trace_state_for_session(&store, None, None, None).expect("trace should build");

    assert_eq!(state.project_id.as_deref(), Some("project-alpha"));
    assert_eq!(state.session_name.as_deref(), Some("Alpha Session"));
    assert_eq!(trace.project_name.as_deref(), Some("Alpha"));
    assert_eq!(trace.session_id.as_deref(), Some("session-alpha"));
}

#[test]
fn default_sidecar_state_reports_runtime_capabilities() {
    let config = SidecarConfig::default();
    let state = sidecar_state(&config, None);

    assert!(state.auto_configure);
    assert!(state.browser.exists);
    assert!(state.browser.healthy);
    assert!(state.computer.exists);
    assert!(state.computer.executable);
    if !state.computer.healthy {
        assert!(!state.computer.health_output.trim().is_empty());
    }
}

fn temp_test_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{name}-{}", unique_id("test")))
}
