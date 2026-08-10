use super::*;

pub(crate) fn adaptive_quality_repair_budget(verification: PromptVerification) -> usize {
    match verification {
        PromptVerification::Minimal => 0,
        PromptVerification::Evidence => ADAPTIVE_EVIDENCE_REPAIR_ATTEMPTS,
        PromptVerification::Adversarial => ADAPTIVE_ADVERSARIAL_REPAIR_ATTEMPTS,
    }
}

pub(crate) fn adaptive_quality_gate_passes(gate: &CollaborationQualityPayload) -> bool {
    gate.pass
        && gate.score.is_finite()
        && (0.0..=1.0).contains(&gate.score)
        && gate.score >= ADAPTIVE_QUALITY_PASS_SCORE
        && gate.safety_violations == 0
}

pub(crate) fn adaptive_quality_candidate_is_better(
    candidate: &CollaborationQualityPayload,
    anchor: &CollaborationQualityPayload,
) -> bool {
    if candidate.safety_violations != anchor.safety_violations {
        return candidate.safety_violations < anchor.safety_violations;
    }
    let candidate_passes = adaptive_quality_gate_passes(candidate);
    let anchor_passes = adaptive_quality_gate_passes(anchor);
    if candidate_passes != anchor_passes {
        return candidate_passes;
    }
    candidate.score.is_finite() && (!anchor.score.is_finite() || candidate.score > anchor.score)
}

pub(crate) fn adaptive_quality_result_quality(gate: &CollaborationQualityPayload) -> ResultQuality {
    if adaptive_quality_gate_passes(gate) {
        ResultQuality::Verified
    } else if gate.score >= ADAPTIVE_QUALITY_PASS_SCORE {
        ResultQuality::Grounded
    } else if gate.score >= 0.5 {
        ResultQuality::Substantive
    } else {
        ResultQuality::Draft
    }
}

pub(crate) fn adaptive_quality_handoff(gate: &AdaptiveQualityGateResult) -> Result<String, String> {
    if gate.safety_violations > 0 {
        return Err(format!(
            "{WORKFLOW_SAFETY_ERROR_PREFIX} adaptive quality gate found {} safety violation(s)",
            gate.safety_violations
        ));
    }
    if gate.passed {
        return Ok(gate.output.clone());
    }
    let issues = if gate.issues.is_empty() {
        "The independent quality review was unavailable or inconclusive.".to_string()
    } else {
        gate.issues.join("\n- ")
    };
    Ok(format!(
        "INTERNAL QUALITY HANDOFF: The adaptive team guidance did not yet pass its independent quality gate. The downstream executor, reviewer, and synthesizer must resolve every issue below, verify claims against tool evidence, and must not claim completion until the issues are closed. Do not expose this internal note to the user.\n\nUnresolved issues:\n- {}\n\nCandidate guidance:\n{}",
        issues,
        gate.output
    ))
}

fn distinct_quality_models(models: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut distinct = Vec::new();
    for model in models {
        if !model.trim().is_empty() && !distinct.iter().any(|existing| existing == &model) {
            distinct.push(model);
        }
    }
    distinct
}

pub(crate) fn adaptive_quality_reviewer_models(
    config: &ProviderConfig,
    participant_models: &BTreeSet<String>,
) -> Vec<String> {
    let candidates = distinct_quality_models([
        config.model_for_role(&ModelRole::Reviewer),
        config.model_for_role(&ModelRole::Summarizer),
        config.model_for_conductor(),
        config.model_for_role(&ModelRole::Planner),
        config.model_for_role(&ModelRole::Executor),
        config.model.clone(),
    ]);
    let (independent, overlapping): (Vec<_>, Vec<_>) = candidates
        .into_iter()
        .partition(|model| !participant_models.contains(model));
    independent.into_iter().chain(overlapping).collect()
}

fn adaptive_quality_participant_models(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
) -> BTreeSet<String> {
    let mut models = [
        config.model_for_conductor(),
        config.model_for_role(&ModelRole::Planner),
        config.model_for_role(&ModelRole::Executor),
        config.model.clone(),
    ]
    .into_iter()
    .filter(|model| !model.trim().is_empty())
    .collect::<BTreeSet<_>>();
    let stable_epoch = run_context_steer_epoch(run_context).to_string();
    let events = state.store.lock().ok().and_then(|store| {
        store
            .list_by_task_and_metadata(task_id, "collaboration_id", collaboration_id)
            .ok()
    });
    for event in events.into_iter().flatten().filter(|event| {
        matches!(
            event.kind,
            EventKind::ModelRequestStarted | EventKind::ModelRequestFinished
        ) && event.metadata.get("steer_epoch").map(String::as_str) == Some(stable_epoch.as_str())
            && !event.metadata.get("stage").is_some_and(|stage| {
                stage == "quality_gate" || stage.starts_with("quality_recheck_")
            })
    }) {
        if let Some(model) = event
            .metadata
            .get("model")
            .map(|model| model.trim())
            .filter(|model| !model.is_empty())
        {
            models.insert(model.to_string());
        }
    }
    models
}

fn adaptive_quality_repair_models(config: &ProviderConfig) -> Vec<String> {
    distinct_quality_models([
        config.model_for_role(&ModelRole::Summarizer),
        config.model_for_conductor(),
        config.model_for_role(&ModelRole::Executor),
    ])
}

#[allow(clippy::too_many_arguments)]
fn adaptive_quality_evaluator_receipt(
    config: &ProviderConfig,
    run_context: &Metadata,
    collaboration_id: &str,
    stage: &str,
    reviewer_model: &str,
    evaluated_artifact: &str,
    gate: &CollaborationQualityPayload,
    events: &[Event],
) -> Option<orchestrator::AutoTeacherEvaluatorReceiptV1> {
    let stable_epoch = run_context_steer_epoch(run_context).to_string();
    let stage_event = events.iter().rev().find(|event| {
        event.kind == EventKind::ModelRequestFinished
            && event.metadata.get("collaboration_id").map(String::as_str) == Some(collaboration_id)
            && event.metadata.get("stage").map(String::as_str) == Some(stage)
            && event.metadata.get("role").map(String::as_str) == Some("reviewer")
            && event.metadata.get("model").map(String::as_str) == Some(reviewer_model)
            && event.metadata.get("status").map(String::as_str) == Some("completed")
            && event.metadata.get("usage_source").map(String::as_str) == Some("provider")
            && event.metadata.contains_key("output")
            && event.metadata.get("steer_epoch").map(String::as_str) == Some(stable_epoch.as_str())
    })?;
    let request_id = stage_event.metadata.get("request_id")?.trim();
    let project_root = run_context.get("project_root")?.trim();
    if request_id.is_empty() || project_root.is_empty() {
        return None;
    }
    let evaluator = orchestrator::AutoTeacherProviderIdentityV1::new(
        &config.provider_id,
        &config.provider_resource,
        &config.base_url,
        reviewer_model,
    )
    .ok()?;
    let persisted_score = format!("{:.3}", gate.score.clamp(0.0, 1.0));
    let quality_score_bps = (persisted_score.parse::<f64>().ok()? * 10_000.0).round() as u16;
    let source_revision = crate::prompt_learning_runtime::prompt_source_revision().ok()?;
    let receipt = orchestrator::AutoTeacherEvaluatorReceiptV1 {
        schema: orchestrator::AUTO_TEACHER_EVALUATOR_RECEIPT_SCHEMA_V1.to_string(),
        protocol: orchestrator::AUTO_TEACHER_PROVIDER_REVIEW_PROTOCOL_V1.to_string(),
        evaluator,
        stage: stage.to_string(),
        request_id_sha256: sha256_hex(request_id.as_bytes()),
        stage_event_sha256: crate::prompt_learning_runtime::prompt_event_sha256(stage_event)
            .ok()?,
        evaluated_artifact_sha256: orchestrator::auto_teacher_evaluated_artifact_sha256(
            evaluated_artifact,
        )
        .ok()?,
        system_prompt_sha256: sha256_hex(
            collaboration_system_prompt_for_run(&config.agent_system_prompt, run_context)
                .as_bytes(),
        ),
        tool_contract_sha256:
            crate::prompt_learning_runtime::prompt_evaluation_tool_contract_sha256(
                std::path::Path::new(project_root),
            ),
        source_revision_sha256: sha256_hex(source_revision.as_bytes()),
        quality_score_bps,
        passed: gate.pass,
        safety_violations: gate.safety_violations,
    };
    receipt.validate().ok()?;
    Some(receipt)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn record_adaptive_quality_gate_event(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    stage: &str,
    reviewer_model: &str,
    evaluated_artifact: &str,
    gate: &CollaborationQualityPayload,
    review_index: usize,
    repair_budget: usize,
) {
    let events = state.store.lock().ok().and_then(|store| {
        store
            .list_by_task_and_metadata_before(
                task_id,
                "collaboration_id",
                collaboration_id,
                u64::MAX,
                8,
            )
            .ok()
    });
    let receipt = events
        .as_deref()
        .and_then(|events| {
            adaptive_quality_evaluator_receipt(
                config,
                run_context,
                collaboration_id,
                stage,
                reviewer_model,
                evaluated_artifact,
                gate,
                events,
            )
        })
        .and_then(|receipt| serde_json::to_string(&receipt).ok());
    let mut metadata = [
        ("collaboration_id".to_string(), collaboration_id.to_string()),
        ("quality_pass".to_string(), gate.pass.to_string()),
        (
            "quality_score".to_string(),
            format!("{:.3}", gate.score.clamp(0.0, 1.0)),
        ),
        (
            "quality_issues".to_string(),
            truncate_for_collaboration(&gate.issues.join(" | "), 4_000),
        ),
        (
            "safety_violations".to_string(),
            gate.safety_violations.to_string(),
        ),
        ("quality_review_index".to_string(), review_index.to_string()),
        (
            "quality_repair_budget".to_string(),
            repair_budget.to_string(),
        ),
    ]
    .into_iter()
    .collect::<Metadata>();
    if let Some(receipt) = receipt {
        metadata.insert(
            orchestrator::AUTO_TEACHER_EVALUATOR_RECEIPT_METADATA_KEY.to_string(),
            receipt,
        );
    }
    let Ok(mut store) = state.store.lock() else {
        return;
    };
    let _ = append_event(
        &mut store,
        task_id,
        EventKind::TaskStatusChanged,
        "Collaboration quality gate evaluated",
        metadata_with_context(metadata, run_context),
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn quality_gate_adaptive_output(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    user_prompt: &str,
    output: &str,
    verification: PromptVerification,
    evidence_count: usize,
) -> AdaptiveQualityGateResult {
    if verification == PromptVerification::Minimal {
        return AdaptiveQualityGateResult {
            output: output.to_string(),
            score: 0.6,
            safety_violations: 0,
            passed: true,
            issues: Vec::new(),
        };
    }
    let participant_models =
        adaptive_quality_participant_models(state, config, task_id, run_context, collaboration_id);
    let reviewer_models = adaptive_quality_reviewer_models(config, &participant_models);
    let repair_models = adaptive_quality_repair_models(config);
    let repair_budget = adaptive_quality_repair_budget(verification);
    let mut candidate = output.to_string();
    let mut last_gate = None;
    let mut best_evaluated = None::<(String, CollaborationQualityPayload)>;
    let mut review_errors = Vec::new();
    let cancellation =
        active_agent_run_control(state, run_context.get("session_id").map(String::as_str))
            .ok()
            .flatten();

    for review_index in 0..=repair_budget {
        if collaboration_steer_pending(cancellation.as_ref()) {
            break;
        }
        if cancellation
            .as_ref()
            .is_some_and(|control| control.stage_should_stop(RunStageClass::Reviewer))
        {
            review_errors.push("quality review stopped at the run deadline".to_string());
            break;
        }
        let reviewer_model = reviewer_models
            .get(review_index % reviewer_models.len().max(1))
            .cloned()
            .unwrap_or_else(|| config.model_for_role(&ModelRole::Reviewer));
        let stage = if review_index == 0 {
            "quality_gate".to_string()
        } else {
            format!("quality_recheck_{review_index}")
        };
        let evaluated_artifact =
            orchestrator::canonical_auto_teacher_evaluated_artifact(&candidate);
        let raw_gate = match run_collaboration_stage(
            state,
            config,
            task_id,
            run_context,
            collaboration_id,
            &stage,
            ModelRole::Reviewer,
            &reviewer_model,
            format!(
                "Evaluate whether this adaptive team guidance is sufficient for a separate tool-using executor to satisfy the user request. Check branch coverage, evidence-ledger provenance, unsupported claims, contradictions, concrete next actions, and safety. Treat worker prose as proposals unless supported by tool evidence. Return only strict JSON: {{\"pass\":true,\"score\":0.0,\"issues\":[\"...\"],\"safety_violations\":0}}. Use a score from 0 to 1 and count concrete unsafe or scope-violating instructions.\n\nUser request:\n{}\n\nTeam guidance revision {}:\n{}",
                user_prompt,
                review_index,
                evaluated_artifact
            ),
            AgentModelAttribution::actor(
                AgentActor::IndependentVerifier,
                AgentStage::Verify,
                AgentModelProfile::Verifier,
                AgentEffectAuthority::None,
            ),
        ) {
            Ok(raw_gate) => raw_gate,
            Err(error) => {
                if error == COLLABORATION_STEER_INTERRUPTED
                    || collaboration_steer_pending(cancellation.as_ref())
                {
                    break;
                }
                if let Ok(mut store) = state.store.lock() {
                    let _ = append_event(
                        &mut store,
                        task_id,
                        EventKind::TaskStatusChanged,
                        "Collaboration quality gate unavailable",
                        metadata_with_context(
                            [
                                (
                                    "collaboration_id".to_string(),
                                    collaboration_id.to_string(),
                                ),
                                ("quality_review_index".to_string(), review_index.to_string()),
                                (
                                    "quality_error".to_string(),
                                    truncate_for_collaboration(&error, 2_000),
                                ),
                            ]
                            .into_iter()
                            .collect(),
                            run_context,
                        ),
                    );
                }
                review_errors.push(format!("reviewer {reviewer_model} unavailable: {error}"));
                continue;
            }
        };
        let gate = parse_collaboration_quality(&raw_gate).unwrap_or(CollaborationQualityPayload {
            pass: false,
            score: 0.0,
            issues: vec![truncate_for_collaboration(&raw_gate, 2_000)],
            safety_violations: 0,
        });
        record_adaptive_quality_gate_event(
            state,
            config,
            task_id,
            run_context,
            collaboration_id,
            &stage,
            &reviewer_model,
            &evaluated_artifact,
            &gate,
            review_index,
            repair_budget,
        );
        if let Some(control) = cancellation.as_ref() {
            control.record_best_known_result_at(
                run_context_steer_epoch(run_context),
                &format!("quality_gate_revision_{review_index}"),
                &candidate,
                adaptive_quality_result_quality(&gate),
                evidence_count,
                adaptive_quality_gate_passes(&gate),
                false,
            );
        }
        if best_evaluated
            .as_ref()
            .is_none_or(|(_, anchor)| adaptive_quality_candidate_is_better(&gate, anchor))
        {
            best_evaluated = Some((candidate.clone(), gate.clone()));
        }
        if adaptive_quality_gate_passes(&gate) {
            return AdaptiveQualityGateResult {
                output: candidate,
                score: gate.score.clamp(0.0, 1.0) as f64,
                safety_violations: gate.safety_violations,
                passed: true,
                issues: Vec::new(),
            };
        }

        let issues = if gate.issues.is_empty() {
            "Quality score was below threshold.".to_string()
        } else {
            gate.issues.join("\n")
        };
        last_gate = Some(gate);
        if review_index >= repair_budget {
            break;
        }

        let repair_stage = format!("quality_repair_{}", review_index + 1);
        if let Some(control) = cancellation.as_ref() {
            if control.stage_should_stop(RunStageClass::Repair) {
                review_errors.push("quality repair stopped at the run deadline".to_string());
                break;
            }
            match control
                .begin_repair_attempt_at(run_context_steer_epoch(run_context), &repair_stage)
            {
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(reason) => {
                    review_errors.push(format!(
                        "quality repair budget exhausted: {}",
                        reason.code()
                    ));
                    break;
                }
            }
        }
        let synthesizer_model = repair_models
            .get(review_index % repair_models.len().max(1))
            .cloned()
            .unwrap_or_else(|| config.model_for_role(&ModelRole::Summarizer));
        let repaired = match run_collaboration_stage(
            state,
            config,
            task_id,
            run_context,
            collaboration_id,
            &repair_stage,
            ModelRole::Summarizer,
            &synthesizer_model,
            format!(
                "Repair the adaptive team guidance so a separate tool-using executor can fully satisfy the user. Resolve every quality-gate issue, retain provenance-bearing tool evidence and useful disagreements, label unsupported worker claims, and return one concrete execution brief. Do not answer the user directly.\n\nUser request:\n{}\n\nCurrent guidance:\n{}\n\nQuality issues:\n{}",
                user_prompt,
                truncate_for_collaboration(&candidate, 14_000),
                issues
            ),
            AgentModelAttribution::actor(
                AgentActor::Specialist,
                AgentStage::Plan,
                AgentModelProfile::Utility,
                AgentEffectAuthority::None,
            ),
        ) {
            Ok(repaired) if !repaired.trim().is_empty() => repaired,
            Err(error) if error == COLLABORATION_STEER_INTERRUPTED => break,
            _ => break,
        };
        candidate = repaired;
    }

    let (candidate, best_gate) = best_evaluated
        .map(|(candidate, gate)| (candidate, Some(gate)))
        .unwrap_or((candidate, last_gate));
    let (score, safety_violations, mut issues) = best_gate
        .map(|gate| {
            (
                gate.score.clamp(0.0, 1.0) as f64,
                gate.safety_violations,
                gate.issues,
            )
        })
        .unwrap_or((0.5, 0, Vec::new()));
    issues.extend(review_errors);
    AdaptiveQualityGateResult {
        output: candidate,
        score,
        safety_violations,
        passed: false,
        issues,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn compare_team_guidance_with_anchor(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    user_prompt: &str,
    team_output: &str,
    anchor_output: &str,
) -> Result<AdaptivePairwiseComparison, String> {
    if team_output.trim().is_empty() || anchor_output.trim().is_empty() {
        return Err("paired comparison requires both team and anchor guidance".to_string());
    }
    let reviewer_model = config.model_for_role(&ModelRole::Reviewer);
    let forward_prompt = candidate_pair_review_prompt(user_prompt, team_output, anchor_output);
    let reverse_prompt = candidate_pair_review_prompt(user_prompt, anchor_output, team_output);
    let (forward, reverse) = std::thread::scope(|scope| {
        let forward = scope.spawn(|| {
            run_collaboration_stage(
                state,
                config,
                task_id,
                run_context,
                collaboration_id,
                "team_anchor_pairwise_forward",
                ModelRole::Reviewer,
                &reviewer_model,
                forward_prompt,
                AgentModelAttribution::actor(
                    AgentActor::IndependentVerifier,
                    AgentStage::Verify,
                    AgentModelProfile::Verifier,
                    AgentEffectAuthority::None,
                ),
            )
        });
        let reverse = scope.spawn(|| {
            run_collaboration_stage(
                state,
                config,
                task_id,
                run_context,
                collaboration_id,
                "team_anchor_pairwise_reverse",
                ModelRole::Reviewer,
                &reviewer_model,
                reverse_prompt,
                AgentModelAttribution::actor(
                    AgentActor::IndependentVerifier,
                    AgentStage::Verify,
                    AgentModelProfile::Verifier,
                    AgentEffectAuthority::None,
                ),
            )
        });
        (
            forward
                .join()
                .unwrap_or_else(|_| Err("forward team-anchor reviewer panicked".to_string())),
            reverse
                .join()
                .unwrap_or_else(|_| Err("reverse team-anchor reviewer panicked".to_string())),
        )
    });
    let forward = parse_candidate_pair_review(&forward?)?;
    let reverse = parse_candidate_pair_review(&reverse?)?;
    let comparison = compare_team_and_anchor_order_invariant(forward, reverse)?;
    if let Ok(mut store) = state.store.lock() {
        let _ = append_event(
            &mut store,
            task_id,
            EventKind::TaskStatusChanged,
            "Collaboration team compared with direct anchor",
            metadata_with_context(
                [
                    ("collaboration_id".to_string(), collaboration_id.to_string()),
                    ("comparison_blind".to_string(), "true".to_string()),
                    ("comparison_order_invariant".to_string(), "true".to_string()),
                    (
                        "team_score_bps".to_string(),
                        comparison.team_score_bps.to_string(),
                    ),
                    (
                        "anchor_score_bps".to_string(),
                        comparison.anchor_score_bps.to_string(),
                    ),
                    (
                        "team_uplift_bps".to_string(),
                        comparison.team_uplift_bps.to_string(),
                    ),
                    (
                        "team_safety_violations".to_string(),
                        comparison.team_safety_violations.to_string(),
                    ),
                    (
                        "anchor_safety_violations".to_string(),
                        comparison.anchor_safety_violations.to_string(),
                    ),
                    (
                        "team_unsupported_claims".to_string(),
                        comparison.team_unsupported_claims.to_string(),
                    ),
                    (
                        "anchor_unsupported_claims".to_string(),
                        comparison.anchor_unsupported_claims.to_string(),
                    ),
                    (
                        "team_unmet_requirements".to_string(),
                        comparison.team_unmet_requirements.to_string(),
                    ),
                    (
                        "anchor_unmet_requirements".to_string(),
                        comparison.anchor_unmet_requirements.to_string(),
                    ),
                ]
                .into_iter()
                .collect(),
                run_context,
            ),
        );
    }
    Ok(comparison)
}

pub(crate) fn parse_collaboration_quality(
    response: &str,
) -> Result<CollaborationQualityPayload, String> {
    let start = response
        .find('{')
        .ok_or_else(|| "quality gate did not return JSON".to_string())?;
    let end = response
        .rfind('}')
        .filter(|end| *end >= start)
        .ok_or_else(|| "quality gate returned incomplete JSON".to_string())?;
    serde_json::from_str(&response[start..=end])
        .map_err(|error| format!("quality gate JSON is invalid: {error}"))
}
