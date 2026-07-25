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

fn adaptive_quality_reviewer_models(config: &ProviderConfig) -> Vec<String> {
    distinct_quality_models([
        config.model_for_role(&ModelRole::Reviewer),
        config.model_for_conductor(),
        config.model_for_role(&ModelRole::Planner),
    ])
}

fn adaptive_quality_repair_models(config: &ProviderConfig) -> Vec<String> {
    distinct_quality_models([
        config.model_for_role(&ModelRole::Summarizer),
        config.model_for_conductor(),
        config.model_for_role(&ModelRole::Executor),
    ])
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn record_adaptive_quality_gate_event(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    gate: &CollaborationQualityPayload,
    review_index: usize,
    repair_budget: usize,
) {
    let Ok(mut store) = state.store.lock() else {
        return;
    };
    let _ = append_event(
        &mut store,
        task_id,
        EventKind::TaskStatusChanged,
        "Collaboration quality gate evaluated",
        metadata_with_context(
            [
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
            .collect(),
            run_context,
        ),
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
    let reviewer_models = adaptive_quality_reviewer_models(config);
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
                truncate_for_collaboration(&candidate, 14_000)
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
            task_id,
            run_context,
            collaboration_id,
            &gate,
            review_index,
            repair_budget,
        );
        if let Some(control) = cancellation.as_ref() {
            control.record_best_known_result(
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
            if let Err(reason) = control.begin_repair_attempt(&repair_stage) {
                review_errors.push(format!(
                    "quality repair budget exhausted: {}",
                    reason.code()
                ));
                break;
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
    let team_is_a = sha256_hex(collaboration_id.as_bytes())
        .as_bytes()
        .first()
        .is_none_or(|byte| byte % 2 == 0);
    let (candidate_a, candidate_b) = if team_is_a {
        (team_output, anchor_output)
    } else {
        (anchor_output, team_output)
    };
    let reviewer_model = config.model_for_role(&ModelRole::Reviewer);
    let raw = run_collaboration_stage(
        state,
        config,
        task_id,
        run_context,
        collaboration_id,
        "team_anchor_pairwise",
        ModelRole::Reviewer,
        &reviewer_model,
        format!(
            "Blindly compare two internal execution-guidance candidates for the same user request. Judge objective fidelity, constraint coverage, evidence discipline, concrete executable next actions, robustness, and safety. Do not reward verbosity. Candidate labels are randomized and reveal no source. Return only strict JSON: {{\"score_a\":0.0,\"score_b\":0.0,\"safety_violations_a\":0,\"safety_violations_b\":0}}. Scores must be finite numbers from 0 to 1.\n\nUser request:\n{}\n\nCandidate A:\n{}\n\nCandidate B:\n{}",
            truncate_for_collaboration(user_prompt, 12_000),
            truncate_for_collaboration(candidate_a, 14_000),
            truncate_for_collaboration(candidate_b, 14_000),
        ),
    )?;
    let payload = parse_prompt_pairwise_payload(&raw)?;
    for (label, score) in [("A", payload.score_a), ("B", payload.score_b)] {
        if !score.is_finite() || !(0.0..=1.0).contains(&score) {
            return Err(format!(
                "paired comparison score {label} must be finite and between 0 and 1"
            ));
        }
    }
    let score_a_bps = (payload.score_a * 10_000.0).round() as u16;
    let score_b_bps = (payload.score_b * 10_000.0).round() as u16;
    let (team_score_bps, anchor_score_bps, team_safety_violations, anchor_safety_violations) =
        if team_is_a {
            (
                score_a_bps,
                score_b_bps,
                payload.safety_violations_a,
                payload.safety_violations_b,
            )
        } else {
            (
                score_b_bps,
                score_a_bps,
                payload.safety_violations_b,
                payload.safety_violations_a,
            )
        };
    let team_uplift_bps = i32::from(team_score_bps)
        .saturating_sub(i32::from(anchor_score_bps))
        .clamp(-10_000, 10_000) as i16;
    let comparison = AdaptivePairwiseComparison {
        team_score_bps,
        anchor_score_bps,
        team_uplift_bps,
        team_safety_violations,
        anchor_safety_violations,
    };
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
                    (
                        "comparison_team_label".to_string(),
                        if team_is_a { "A" } else { "B" }.to_string(),
                    ),
                    ("team_score_bps".to_string(), team_score_bps.to_string()),
                    ("anchor_score_bps".to_string(), anchor_score_bps.to_string()),
                    ("team_uplift_bps".to_string(), team_uplift_bps.to_string()),
                    (
                        "team_safety_violations".to_string(),
                        team_safety_violations.to_string(),
                    ),
                    (
                        "anchor_safety_violations".to_string(),
                        anchor_safety_violations.to_string(),
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

fn parse_prompt_pairwise_payload(
    response: &str,
) -> Result<PromptPairwiseEvaluationPayload, String> {
    let start = response
        .find('{')
        .ok_or_else(|| "paired comparison did not return JSON".to_string())?;
    let end = response
        .rfind('}')
        .filter(|end| *end >= start)
        .ok_or_else(|| "paired comparison returned incomplete JSON".to_string())?;
    serde_json::from_str(&response[start..=end])
        .map_err(|error| format!("paired comparison JSON is invalid: {error}"))
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
