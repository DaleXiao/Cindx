use super::*;
use crate::agent_strategy_runtime::cumulative_effective_prompt_objective;
use orchestrator::{
    LearningAttribution, LearningDisposition, LearningEvidenceV1, LearningUsageCompleteness,
    PromptLearningPurpose,
};

fn is_agent_run_terminal(event: &Event) -> bool {
    AgentRunEvent::from_event(event).is_some_and(|event| event.status().is_terminal())
}

fn is_agent_run_completed(event: &Event) -> bool {
    AgentRunEvent::from_event(event).map(AgentRunEvent::status) == Some(AgentRunStatus::Completed)
}

fn event_steer_epoch(event: &Event) -> &str {
    event
        .metadata
        .get("steer_epoch")
        .map(String::as_str)
        .unwrap_or("0")
}

fn prompt_auto_teacher_case(
    run_id: &str,
    project_id: &str,
    task_class: &str,
    objective: &crate::prompt_learning_runtime::PromptLearningText,
    run_events: &[&Event],
    terminal: &Event,
    stable_epoch: &str,
) -> Option<PromptAutoTeacherCase> {
    if !is_agent_run_completed(terminal) {
        return None;
    }
    let stable_epoch_number = stable_epoch.parse::<u64>().ok()?;
    let learning_evidence = LearningEvidenceV1::from_metadata(&terminal.metadata)?;
    let learning_receipt = crate::prompt_learning_runtime::prompt_learning_receipt(
        PromptLearningPurpose::AutoTeacher,
        run_id,
        project_id,
        task_class,
        objective,
        run_events,
        terminal,
        stable_epoch_number,
    );
    if !learning_receipt.is_eligible() {
        return None;
    }
    if !learning_evidence.is_learnable()
        || learning_evidence.termination != orchestrator::LearningTermination::Completed
        || learning_evidence.disposition != LearningDisposition::Positive
        || learning_evidence.attribution != LearningAttribution::Workflow
        || learning_evidence.independent_quality_source.is_none()
        || learning_evidence.usage_completeness == LearningUsageCompleteness::Missing
        || learning_evidence.steer_epoch != Some(stable_epoch_number)
    {
        return None;
    }
    let quality_score_bps = learning_evidence.quality_bps?;
    let total_tokens =
        crate::learning_evidence_runtime::learning_lineage_usage_from_metadata(&terminal.metadata)
            .filter(|usage| {
                usage.completeness == learning_evidence.usage_completeness
                    && usage.completeness != LearningUsageCompleteness::Missing
            })?
            .total_tokens;
    if run_events.iter().any(|event| {
        event_steer_epoch(event) == stable_epoch
            && ((event.kind == EventKind::PermissionResolved
                && event.metadata.get("decision").is_some_and(|decision| {
                    !matches!(decision.as_str(), "allow_once" | "allow_for_session")
                }))
                || event
                    .metadata
                    .get("safety_violations")
                    .and_then(|value| value.parse::<u64>().ok())
                    .is_some_and(|count| count > 0))
    }) {
        return None;
    }

    let profile_event = run_events.iter().rev().find(|event| {
        event.summary == "Conductor prompt profile selected"
            && event_steer_epoch(event) == stable_epoch
            && event.metadata.get("prompt_effort").map(String::as_str) == Some("auto")
            && event.sequence < terminal.sequence
    })?;
    let collaboration_id = profile_event.metadata.get("collaboration_id")?;
    let genome = profile_event
        .metadata
        .get("prompt_genome")
        .and_then(|encoded| serde_json::from_str::<ConductorPromptGenome>(encoded).ok())?;
    genome.validate().ok()?;
    if profile_event.metadata.get("prompt_profile") != Some(&genome.id) {
        return None;
    }
    let profile_sha256 = prompt_genome_sha256(&genome).ok()?;

    let plan_event = run_events.iter().rev().find(|event| {
        event.summary == "Collaboration workflow planned"
            && event_steer_epoch(event) == stable_epoch
            && event.metadata.get("collaboration_id") == Some(collaboration_id)
            && event.sequence >= profile_event.sequence
            && event.sequence < terminal.sequence
    })?;
    let encoded_plan = plan_event.metadata.get("workflow_ir")?;
    let unchecked_plan = serde_json::from_str::<WorkflowPlanIr>(encoded_plan).ok()?;
    let mut allowed_models = std::iter::once(unchecked_plan.coordinator_model.clone())
        .chain(unchecked_plan.steps.iter().map(|step| step.model.clone()))
        .chain(
            run_events
                .iter()
                .filter(|event| event.sequence < terminal.sequence)
                .filter(|event| event_steer_epoch(event) == stable_epoch)
                .filter(|event| {
                    matches!(
                        event.kind,
                        EventKind::ModelRequestStarted | EventKind::ModelRequestFinished
                    )
                })
                .filter_map(|event| event.metadata.get("model").cloned()),
        )
        .filter(|model| !model.trim().is_empty())
        .collect::<Vec<_>>();
    allowed_models.sort();
    allowed_models.dedup();
    let mut plan = WorkflowPlanIr::from_json(encoded_plan, &allowed_models).ok()?;
    if plan.effort != "auto"
        || plan.prompt_profile != genome.id
        || plan.workflow_id != collaboration_id.as_str()
    {
        return None;
    }
    let checkpoint_event = run_events.iter().rev().find(|event| {
        event.summary == "Collaboration workflow checkpoint finalized"
            && event_steer_epoch(event) == stable_epoch
            && event.metadata.get("collaboration_id") == Some(collaboration_id)
            && event.sequence >= plan_event.sequence
            && event.sequence < terminal.sequence
    })?;
    let checkpoint = checkpoint_event
        .metadata
        .get("workflow_checkpoint")
        .and_then(|encoded| {
            WorkflowExecutionCheckpoint::from_json(encoded, &allowed_models).ok()
        })?;
    if !checkpoint.finalized || checkpoint.plan.workflow_id != plan.workflow_id {
        return None;
    }
    let canonical_genome = serde_json::to_string(&genome).ok()?;
    if checkpoint.prompt_genome_json != canonical_genome {
        return None;
    }
    let evaluator_receipt =
        crate::prompt_teacher_attestation_runtime::prompt_auto_teacher_evaluator_receipt(
            run_events,
            terminal,
            collaboration_id,
            stable_epoch,
            quality_score_bps,
        )?;
    let steps = plan
        .steps
        .iter()
        .map(|step| {
            let checkpoint_step = checkpoint.steps.get(&step.id)?;
            Some(PromptAutoTeacherStep {
                id: step.id.clone(),
                role: step.role.clone(),
                model: checkpoint_step.model.clone(),
                attempts: checkpoint_step.attempts,
                status: checkpoint_step.status.clone(),
                output: {
                    let output = crate::prompt_learning_runtime::redact_prompt_learning_text(
                        checkpoint_step.output.as_deref().unwrap_or_default(),
                    );
                    if output.residual_sensitive_data {
                        return None;
                    }
                    truncate_for_collaboration(&output.text, 6_000)
                },
                errors: checkpoint_step
                    .error
                    .as_deref()
                    .map(redact_sensitive_text)
                    .into_iter()
                    .collect(),
                latency_ms: checkpoint_step.latency_ms,
                total_tokens: checkpoint_step.total_tokens,
                evidence_count: checkpoint_step.evidence_count,
            })
        })
        .collect::<Option<Vec<_>>>()?;

    let plan_objective =
        crate::prompt_learning_runtime::redact_prompt_learning_text(&plan.objective);
    if plan_objective.residual_sensitive_data {
        return None;
    }
    plan.objective = truncate_for_collaboration(&plan_objective.text, 4_000);
    for step in &mut plan.steps {
        let subtask = crate::prompt_learning_runtime::redact_prompt_learning_text(&step.subtask);
        if subtask.residual_sensitive_data {
            return None;
        }
        step.subtask = truncate_for_collaboration(&subtask.text, 2_000);
    }
    let (participant_models, evaluated_artifact) =
        crate::prompt_teacher_attestation_runtime::prompt_auto_teacher_runtime_attestation(
            run_events,
            terminal,
            collaboration_id,
            stable_epoch,
            &checkpoint,
            &evaluator_receipt,
        )?;
    let redacted_output =
        crate::prompt_learning_runtime::redact_prompt_learning_text(&evaluated_artifact);
    if redacted_output.text.trim().is_empty() || redacted_output.residual_sensitive_data {
        return None;
    }
    let final_output = truncate_for_collaboration(&redacted_output.text, 12_000);
    if orchestrator::auto_teacher_evaluated_artifact_sha256(&final_output).ok()?
        != evaluator_receipt.evaluated_artifact_sha256
    {
        return None;
    }
    let output_sha256 = sha256_hex(final_output.as_bytes());
    let source_context =
        crate::prompt_teacher_attestation_runtime::prompt_auto_teacher_source_context(
            project_id,
            stable_epoch,
            run_events,
            profile_event,
            &plan,
            &checkpoint,
            &learning_evidence,
            &learning_receipt,
            &participant_models,
            &evaluator_receipt,
            &profile_sha256,
        )?;

    Some(PromptAutoTeacherCase {
        source_run_id: run_id.to_string(),
        steer_epoch: stable_epoch_number,
        profile_id: genome.id.clone(),
        profile_sha256,
        output_sha256,
        source_context,
        genome,
        plan,
        steps,
        final_output,
        participant_models,
        quality_score_bps,
        latency_ms: terminal
            .timestamp_ms
            .saturating_sub(profile_event.timestamp_ms),
        total_tokens,
    })
}

pub(crate) fn reconstruct_prompt_auto_teacher_from_canonical_events(
    events: &[Event],
    project_id: &str,
    source_run_id: &str,
) -> Option<PromptAutoTeacherCase> {
    let mut run_events = events
        .iter()
        .filter(|event| {
            event.metadata.get("agent_run_id").map(String::as_str) == Some(source_run_id)
        })
        .collect::<Vec<_>>();
    run_events.sort_by_key(|event| event.sequence);
    let terminal = run_events
        .iter()
        .rev()
        .find(|event| is_agent_run_terminal(event))?;
    if !run_events
        .iter()
        .any(|event| event.metadata.get("project_id").map(String::as_str) == Some(project_id))
    {
        return None;
    }
    let stable_epoch = terminal
        .metadata
        .get("steer_epoch")
        .map(String::as_str)
        .unwrap_or("0");
    let stable_epoch_number = stable_epoch.parse::<u64>().ok()?;
    let decision = run_events.iter().rev().find(|event| {
        event.summary == "Agent run decision selected" && event_steer_epoch(event) == stable_epoch
    });
    if decision.is_none() && stable_epoch != "0" {
        return None;
    }
    let reconstructed_objective = cumulative_effective_prompt_objective(
        run_events
            .iter()
            .filter(|event| event.kind == EventKind::MessageAdded)
            .filter(|event| {
                event.metadata.get("role").map(String::as_str) == Some("user")
                    && event.metadata.get("internal").map(String::as_str) != Some("true")
            })
            .filter(|event| {
                event
                    .metadata
                    .get("steer_epoch")
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or_default()
                    <= stable_epoch_number
            })
            .filter_map(|event| {
                event
                    .metadata
                    .get("display_content")
                    .or_else(|| event.metadata.get("content"))
                    .or_else(|| event.metadata.get("model_content"))
                    .cloned()
            }),
    );
    let objective = decision
        .and_then(|event| event.metadata.get("effective_prompt_objective"))
        .or(reconstructed_objective.as_ref())
        .or_else(|| decision.and_then(|event| event.metadata.get("prompt_objective")))
        .or_else(|| {
            run_events
                .iter()
                .find(|event| AgentRunEvent::from_event(event).is_some_and(AgentRunEvent::is_start))
                .and_then(|event| event.metadata.get("prompt"))
        })
        .map(|objective| crate::prompt_learning_runtime::redact_prompt_learning_text(objective))
        .filter(|objective| objective.text.chars().count() >= 4)?;
    let task_class = decision
        .and_then(|event| event.metadata.get("task_class"))
        .or_else(|| {
            run_events
                .iter()
                .find_map(|event| event.metadata.get("task_class"))
        })
        .cloned()
        .unwrap_or_else(|| "general".to_string());
    prompt_auto_teacher_case(
        source_run_id,
        project_id,
        &task_class,
        &objective,
        &run_events,
        terminal,
        stable_epoch,
    )
}

pub(crate) fn prompt_profile_evidence_counts(
    observations: &[PromptEvolutionObservation],
    profile_id: &str,
) -> (usize, usize) {
    let active_cohort_sha256 = orchestrator::latest_scientific_dataset_digest(observations);
    prompt_unique_evidence_counts(
        observations
            .iter()
            .filter(|observation| observation.profile_id == profile_id),
        active_cohort_sha256,
    )
}

pub(crate) fn prompt_profile_training_evidence_count(
    observations: &[PromptEvolutionObservation],
    profile_id: &str,
) -> usize {
    let Some(cohort_sha256) = orchestrator::latest_scientific_training_dataset_digest(observations)
    else {
        return 0;
    };
    observations
        .iter()
        .filter(|observation| observation.profile_id == profile_id)
        .filter(|observation| observation.split == PromptEvaluationSplit::Train)
        .filter(|observation| observation.mode == PromptEvaluationMode::PairedExecution)
        .filter(|observation| observation.is_scientific_evidence())
        .filter(|observation| observation.scientific_cohort_sha256() == Some(cohort_sha256))
        .map(PromptEvolutionObservation::evidence_identity)
        .collect::<BTreeSet<_>>()
        .len()
}

pub(crate) fn prompt_direct_profile_evidence_counts(
    observations: &[PromptEvolutionObservation],
    profile_id: &str,
    opponent_profile_id: &str,
) -> (usize, usize) {
    let active_cohort_sha256 = orchestrator::latest_scientific_dataset_digest(observations);
    prompt_unique_evidence_counts(
        observations.iter().filter(|observation| {
            observation.profile_id == profile_id
                && observation.opponent_profile_id.as_deref() == Some(opponent_profile_id)
        }),
        active_cohort_sha256,
    )
}

fn prompt_unique_evidence_counts<'a>(
    observations: impl Iterator<Item = &'a PromptEvolutionObservation>,
    cohort_sha256: Option<&str>,
) -> (usize, usize) {
    let Some(cohort_sha256) = cohort_sha256 else {
        return (0, 0);
    };
    let mut paired = BTreeSet::new();
    let mut replay = BTreeSet::new();
    for observation in observations
        .filter(|observation| observation.is_scientific_evidence())
        .filter(|observation| observation.scientific_cohort_sha256() == Some(cohort_sha256))
    {
        if observation.mode.is_paired_execution() {
            paired.insert(observation.evidence_identity());
        } else if observation.mode.is_replay_execution() {
            replay.insert(observation.evidence_identity());
        }
    }
    (paired.len(), replay.len())
}

pub(crate) fn prompt_rollout_counterpart(
    rollout: Option<&PromptRolloutState>,
    known_profiles: &[ConductorPromptGenome],
    current_profile: &ConductorPromptGenome,
) -> Option<ConductorPromptGenome> {
    let rollout = rollout?;
    let canary = rollout.canary_profile_id.as_deref()?;
    let counterpart_id = if current_profile.id == rollout.stable_profile_id {
        canary
    } else if current_profile.id == canary {
        rollout.stable_profile_id.as_str()
    } else {
        return None;
    };
    known_profiles
        .iter()
        .find(|profile| profile.id == counterpart_id)
        .cloned()
}

pub(crate) fn prompt_evolution_challenger(
    evaluation: &PromptEvolutionEvaluation,
    current_profile: &ConductorPromptGenome,
    task_class: &str,
) -> Option<ConductorPromptGenome> {
    if let Some(champion_id) = evaluation.champion_id.as_deref() {
        if champion_id != current_profile.id {
            if let Some(champion) = evaluation
                .population
                .iter()
                .find(|profile| profile.id == champion_id)
            {
                return Some(champion.clone());
            }
        }
    }
    evaluation
        .population
        .iter()
        .filter(|profile| profile.id != current_profile.id)
        .min_by_key(|profile| {
            let (paired, replay) =
                prompt_profile_evidence_counts(&evaluation.observations, &profile.id);
            let task_class_evidence = evaluation
                .observations
                .iter()
                .filter(|observation| {
                    observation.profile_id == profile.id && observation.task_class == task_class
                })
                .count();
            (
                task_class_evidence,
                paired + replay,
                Reverse(profile.generation),
                profile.id.clone(),
            )
        })
        .cloned()
        .or_else(|| {
            current_profile
                .mutations()
                .into_iter()
                .find(|profile| profile.id != current_profile.id)
        })
}

pub(crate) fn prompt_offline_split_manifest(
    events: &[Event],
    project_id: &str,
) -> BTreeMap<String, PromptEvaluationSplit> {
    events
        .iter()
        .filter(|event| event.summary == "Conductor offline dataset selected")
        .filter(|event| event.metadata.get("project_id").map(String::as_str) == Some(project_id))
        .filter_map(|event| {
            let manifest = event.metadata.get("dataset_split_manifest")?;
            serde_json::from_str::<BTreeMap<String, PromptEvaluationSplit>>(manifest)
                .ok()
                .map(|manifest| (event.sequence, manifest))
        })
        .max_by_key(|(sequence, _)| *sequence)
        .map(|(_, manifest)| manifest)
        .unwrap_or_default()
}

pub(crate) fn prompt_offline_case_rank(
    case: &PromptOfflineCase,
    preferred_auto_profile: Option<(&str, &str)>,
) -> (bool, bool, u16, Reverse<u64>, Reverse<u64>, String) {
    let Some(teacher) = case.auto_teacher.as_ref() else {
        return (
            false,
            false,
            0,
            Reverse(u64::MAX),
            Reverse(u64::MAX),
            case.source_run_id.clone(),
        );
    };
    let preferred = preferred_auto_profile.is_some_and(|(profile_id, profile_sha256)| {
        teacher.profile_id == profile_id && teacher.profile_sha256 == profile_sha256
    });
    (
        preferred,
        true,
        teacher.quality_score_bps,
        Reverse(teacher.total_tokens),
        Reverse(teacher.latency_ms),
        teacher.source_run_id.clone(),
    )
}

pub(crate) fn prompt_offline_dataset(
    events: &[Event],
    project_id: &str,
    preferred_auto_profile: Option<(&str, &str)>,
) -> Vec<PromptOfflineCase> {
    let prior_splits = prompt_offline_split_manifest(events, project_id);
    let previously_assigned = prior_splits.keys().cloned().collect::<BTreeSet<_>>();
    let mut runs = BTreeMap::<String, Vec<&Event>>::new();
    for event in events {
        if let Some(run_id) = event.metadata.get("agent_run_id") {
            runs.entry(run_id.clone()).or_default().push(event);
        }
    }
    let mut cases = BTreeMap::<String, PromptOfflineCase>::new();
    for (run_id, mut run_events) in runs {
        run_events.sort_by_key(|event| event.sequence);
        let Some(terminal) = run_events
            .iter()
            .rev()
            .find(|event| is_agent_run_terminal(event))
        else {
            continue;
        };
        if !run_events
            .iter()
            .any(|event| event.metadata.get("project_id").map(String::as_str) == Some(project_id))
        {
            continue;
        }
        let stable_epoch = terminal
            .metadata
            .get("steer_epoch")
            .map(String::as_str)
            .unwrap_or("0");
        let decision = run_events.iter().rev().find(|event| {
            event.summary == "Agent run decision selected"
                && event
                    .metadata
                    .get("steer_epoch")
                    .map(String::as_str)
                    .unwrap_or("0")
                    == stable_epoch
        });
        if decision.is_none() && stable_epoch != "0" {
            continue;
        }
        let stable_epoch_number = stable_epoch.parse::<u64>().unwrap_or_default();
        let reconstructed_objective = cumulative_effective_prompt_objective(
            run_events
                .iter()
                .filter(|event| event.kind == EventKind::MessageAdded)
                .filter(|event| {
                    event.metadata.get("role").map(String::as_str) == Some("user")
                        && event.metadata.get("internal").map(String::as_str) != Some("true")
                })
                .filter(|event| {
                    event
                        .metadata
                        .get("steer_epoch")
                        .and_then(|value| value.parse::<u64>().ok())
                        .unwrap_or_default()
                        <= stable_epoch_number
                })
                .filter_map(|event| {
                    event
                        .metadata
                        .get("display_content")
                        .or_else(|| event.metadata.get("content"))
                        .or_else(|| event.metadata.get("model_content"))
                        .cloned()
                }),
        );
        let objective = decision
            .and_then(|event| event.metadata.get("effective_prompt_objective"))
            .or(reconstructed_objective.as_ref())
            .or_else(|| decision.and_then(|event| event.metadata.get("prompt_objective")))
            .or_else(|| {
                run_events
                    .iter()
                    .find(|event| {
                        AgentRunEvent::from_event(event).is_some_and(AgentRunEvent::is_start)
                    })
                    .and_then(|event| event.metadata.get("prompt"))
            })
            .map(|objective| {
                crate::prompt_learning_runtime::redact_prompt_learning_text(objective)
            });
        let Some(objective) = objective.filter(|objective| objective.text.chars().count() >= 4)
        else {
            continue;
        };
        let task_class = decision
            .and_then(|event| event.metadata.get("task_class"))
            .or_else(|| {
                run_events
                    .iter()
                    .find_map(|event| event.metadata.get("task_class"))
            })
            .cloned()
            .unwrap_or_else(|| "general".to_string());
        let learning_receipt = crate::prompt_learning_runtime::prompt_learning_receipt(
            PromptLearningPurpose::ObjectiveReplay,
            &run_id,
            project_id,
            &task_class,
            &objective,
            &run_events,
            terminal,
            stable_epoch_number,
        );
        let case_digest = sha256_hex(objective.text.as_bytes());
        let id = format!("runtime-{task_class}-{}", &case_digest[..16]);
        let split_digest = sha256_hex(id.as_bytes());
        let split_bucket = u8::from_str_radix(&split_digest[..2], 16).unwrap_or_default();
        let split = prior_splits.get(&id).copied().unwrap_or_else(|| {
            if split_bucket.is_multiple_of(5) {
                PromptEvaluationSplit::Holdout
            } else {
                PromptEvaluationSplit::Train
            }
        });
        let auto_teacher = prompt_auto_teacher_case(
            &run_id,
            project_id,
            &task_class,
            &objective,
            &run_events,
            terminal,
            stable_epoch,
        );
        let candidate = PromptOfflineCase {
            id,
            objective: objective.text,
            task_class,
            project_id: project_id.to_string(),
            source_run_id: run_id,
            split,
            learning_receipt: Some(learning_receipt),
            auto_teacher,
        };
        match cases.entry(candidate.id.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(candidate);
            }
            std::collections::btree_map::Entry::Occupied(mut entry)
                if prompt_offline_case_rank(&candidate, preferred_auto_profile)
                    > prompt_offline_case_rank(entry.get(), preferred_auto_profile) =>
            {
                entry.insert(candidate);
            }
            std::collections::btree_map::Entry::Occupied(_) => {}
        }
    }
    let mut cases = cases.into_values().collect::<Vec<_>>();
    cases.sort_by(|left, right| left.id.cmp(&right.id));
    cases.truncate(PROMPT_EVOLUTION_OFFLINE_MAX_CASES);
    if cases.len() >= PROMPT_EVOLUTION_OFFLINE_MIN_CASES {
        while cases
            .iter()
            .filter(|case| case.split == PromptEvaluationSplit::Train)
            .count()
            < 2
        {
            let Some(index) = cases.iter().position(|case| {
                case.split == PromptEvaluationSplit::Holdout
                    && !previously_assigned.contains(&case.id)
            }) else {
                break;
            };
            let Some(case) = cases.get_mut(index) else {
                break;
            };
            case.split = PromptEvaluationSplit::Train;
        }
        while cases
            .iter()
            .filter(|case| case.split == PromptEvaluationSplit::Holdout)
            .count()
            < 2
        {
            let index = cases
                .iter()
                .enumerate()
                .rev()
                .find(|(_, case)| {
                    case.split == PromptEvaluationSplit::Train
                        && !previously_assigned.contains(&case.id)
                })
                .map(|(index, _)| index);
            if let Some(case) = index.and_then(|index| cases.get_mut(index)) {
                case.split = PromptEvaluationSplit::Holdout;
            } else {
                break;
            }
        }

        // Keep every task class with enough evidence represented on both
        // sides of the immutable split. Existing assignments never move;
        // only newly discovered cases can repair missing class coverage.
        let task_classes = cases
            .iter()
            .map(|case| case.task_class.clone())
            .collect::<BTreeSet<_>>();
        for task_class in task_classes {
            let class_size = cases
                .iter()
                .filter(|case| case.task_class == task_class)
                .count();
            if class_size < 2 {
                continue;
            }
            for required_split in [PromptEvaluationSplit::Train, PromptEvaluationSplit::Holdout] {
                if cases
                    .iter()
                    .any(|case| case.task_class == task_class && case.split == required_split)
                {
                    continue;
                }
                let Some(index) = cases.iter().position(|case| {
                    case.task_class == task_class
                        && !previously_assigned.contains(&case.id)
                        && case.split != required_split
                }) else {
                    continue;
                };
                cases[index].split = required_split;
            }
        }
    }
    cases
}

pub(crate) fn prompt_learning_dataset(
    events: &[Event],
    project_id: &str,
    preferred_auto_profile: Option<(&str, &str)>,
) -> Vec<PromptOfflineCase> {
    prompt_offline_dataset(events, project_id, preferred_auto_profile)
        .into_iter()
        .filter(|case| {
            case.learning_receipt
                .as_ref()
                .is_some_and(PromptLearningEligibilityReceiptV1::is_eligible)
        })
        .collect()
}

pub(crate) fn select_prompt_offline_case(
    dataset: &[PromptOfflineCase],
    observations: &[PromptEvolutionObservation],
    current_profile_id: &str,
    challenger_profile_id: &str,
    split: PromptEvaluationSplit,
) -> Option<PromptOfflineCase> {
    let dataset_sha256 = prompt_offline_dataset_digest(dataset);
    dataset
        .iter()
        .filter(|case| case.split == split)
        .min_by_key(|case| {
            let evidence_for = |profile_id: &str, opponent_id: &str, case_id: Option<&str>| {
                observations
                    .iter()
                    .filter(|observation| {
                        observation.profile_id == profile_id
                            && observation.opponent_profile_id.as_deref() == Some(opponent_id)
                            && observation.split == split
                            && observation.task_class == case.task_class
                            && observation.is_scientific_evidence()
                            && observation.provenance.dataset_sha256 == dataset_sha256
                            && case_id.is_none_or(|case_id| observation.case_id == case_id)
                    })
                    .map(PromptEvolutionObservation::evidence_identity)
                    .collect::<BTreeSet<_>>()
                    .len()
            };
            let current_class_repeats =
                evidence_for(current_profile_id, challenger_profile_id, None);
            let challenger_class_repeats =
                evidence_for(challenger_profile_id, current_profile_id, None);
            let current_repeats = evidence_for(
                current_profile_id,
                challenger_profile_id,
                Some(case.id.as_str()),
            );
            let challenger_repeats = evidence_for(
                challenger_profile_id,
                current_profile_id,
                Some(case.id.as_str()),
            );
            (
                current_class_repeats.saturating_add(challenger_class_repeats),
                current_class_repeats.max(challenger_class_repeats),
                current_repeats.saturating_add(challenger_repeats),
                current_repeats.max(challenger_repeats),
                case.id.as_str(),
            )
        })
        .cloned()
}

pub(crate) fn select_prompt_auto_transfer_case(
    dataset: &[PromptOfflineCase],
    observations: &[PromptEvolutionObservation],
    profile_ids: [&str; 2],
    active_cohorts: [Option<&str>; 2],
    auto_profile_id: &str,
    auto_profile_sha256: &str,
    split: PromptEvaluationSplit,
) -> Option<PromptOfflineCase> {
    let dataset_sha256 =
        prompt_auto_transfer_dataset_digest(dataset, auto_profile_id, auto_profile_sha256)?;
    dataset
        .iter()
        .filter(|case| case.split == split)
        .filter(|case| {
            case.auto_teacher.as_ref().is_some_and(|teacher| {
                teacher.profile_id == auto_profile_id
                    && teacher.profile_sha256 == auto_profile_sha256
            })
        })
        .min_by_key(|case| {
            let evidence_for = |profile_id: &str, case_id: Option<&str>| {
                let active_cohort = profile_ids
                    .iter()
                    .position(|candidate| *candidate == profile_id)
                    .and_then(|index| active_cohorts[index]);
                observations
                    .iter()
                    .filter(|observation| {
                        observation.profile_id == profile_id
                            && observation.opponent_profile_id.as_deref() == Some(auto_profile_id)
                            && observation.split == split
                            && observation.is_strict_source_attested_transfer_evidence()
                            && observation.provenance.dataset_sha256 == dataset_sha256
                            && active_cohort.is_some_and(|cohort| {
                                observation.scientific_cohort_sha256() == Some(cohort)
                            })
                            && observation
                                .provenance
                                .transfer
                                .as_ref()
                                .is_some_and(|transfer| {
                                    transfer.source_profile_id == auto_profile_id
                                        && transfer.source_profile_sha256 == auto_profile_sha256
                                })
                            && case_id.is_none_or(|case_id| observation.case_id == case_id)
                    })
                    .map(PromptEvolutionObservation::evidence_identity)
                    .collect::<BTreeSet<_>>()
                    .len()
            };
            let first_class = evidence_for(profile_ids[0], None);
            let second_class = evidence_for(profile_ids[1], None);
            let first_case = evidence_for(profile_ids[0], Some(case.id.as_str()));
            let second_case = evidence_for(profile_ids[1], Some(case.id.as_str()));
            (
                first_class.saturating_add(second_class),
                first_class.max(second_class),
                first_case.saturating_add(second_case),
                first_case.max(second_case),
                case.id.as_str(),
            )
        })
        .cloned()
}

pub(crate) fn prompt_offline_dataset_digest(dataset: &[PromptOfflineCase]) -> String {
    crate::prompt_learning_runtime::prompt_dataset_identity(dataset, 0)
        .map(|identity| identity.dataset_sha256)
        .unwrap_or_else(|_| sha256_hex(b"invalid-prompt-dataset"))
}

pub(crate) fn prompt_auto_transfer_dataset_digest(
    dataset: &[PromptOfflineCase],
    auto_profile_id: &str,
    auto_profile_sha256: &str,
) -> Option<String> {
    prompt_auto_transfer_dataset_identity(dataset, auto_profile_id, auto_profile_sha256)
        .map(|identity| identity.dataset_sha256)
}

pub(crate) fn prompt_auto_transfer_dataset_identity(
    dataset: &[PromptOfflineCase],
    auto_profile_id: &str,
    auto_profile_sha256: &str,
) -> Option<PromptDatasetIdentityV1> {
    let project_id = dataset.first()?.project_id.as_str();
    if dataset.iter().any(|case| case.project_id != project_id) {
        return None;
    }
    let cases = dataset
        .iter()
        .filter_map(|case| {
            let teacher = case.auto_teacher.as_ref()?;
            let source_context_sha256 = teacher.source_context.digest().ok()?;
            (teacher.profile_id == auto_profile_id && teacher.profile_sha256 == auto_profile_sha256)
                .then(|| orchestrator::PromptDatasetCaseIdentityV1 {
                    case_id: case.id.clone(),
                    objective_sha256: sha256_hex(
                        format!(
                            "auto_to_pro_transfer_v2\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
                            sha256_hex(case.objective.trim().as_bytes()),
                            teacher.source_run_id,
                            teacher.steer_epoch,
                            teacher.profile_sha256,
                            teacher.output_sha256,
                            source_context_sha256,
                            teacher.source_context.evaluator_receipt_sha256,
                            teacher.source_context.checkpoint_sha256,
                            teacher.source_context.learning_receipt_sha256,
                        )
                        .as_bytes(),
                    ),
                    task_family_sha256: sha256_hex(case.task_class.trim().as_bytes()),
                    split: case.split,
                })
        })
        .collect::<Vec<_>>();
    (!cases.is_empty())
        .then(|| PromptDatasetIdentityV1::new(project_id, 0, cases).ok())
        .flatten()
}

pub(crate) fn prompt_offline_dataset_for_generation(
    discovered: Vec<PromptOfflineCase>,
    previous: Option<&PromptOfflineDatasetState>,
    generation: u32,
) -> Result<Vec<PromptOfflineCase>, String> {
    let Some(previous) = previous.filter(|snapshot| {
        snapshot.status == "ready"
            && snapshot.generation == generation
            && snapshot.case_ids.len() >= PROMPT_EVOLUTION_OFFLINE_MIN_CASES
            && snapshot.identity.as_ref().is_some_and(|identity| {
                identity.validate().is_ok()
                    && identity.generation == generation
                    && identity.dataset_sha256 == snapshot.digest
            })
    }) else {
        return Ok(discovered);
    };
    let by_id = discovered
        .iter()
        .map(|case| (case.id.as_str(), case))
        .collect::<BTreeMap<_, _>>();
    let frozen = previous
        .case_ids
        .iter()
        .filter_map(|case_id| by_id.get(case_id.as_str()).map(|case| (*case).clone()))
        .collect::<Vec<_>>();
    if frozen.len() != previous.case_ids.len() {
        return Err(
            "frozen prompt dataset is incomplete; refusing cohort substitution".to_string(),
        );
    }
    let frozen_identity =
        crate::prompt_learning_runtime::prompt_dataset_identity(&frozen, generation)?;
    if previous.identity.as_ref() != Some(&frozen_identity) {
        return Err(
            "frozen prompt dataset identity changed; refusing cohort substitution".to_string(),
        );
    }
    Ok(frozen)
}

pub(crate) fn append_prompt_offline_dataset_snapshot(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    effort: &str,
    dataset: &[PromptOfflineCase],
    generation: u32,
    selected: Option<&PromptOfflineCase>,
) -> Result<(), String> {
    let split_manifest = dataset
        .iter()
        .map(|case| (case.id.clone(), case.split))
        .collect::<BTreeMap<_, _>>();
    let split_manifest = serde_json::to_string(&split_manifest)
        .map_err(|error| format!("offline split manifest serialization failed: {error}"))?;
    let dataset_identity =
        crate::prompt_learning_runtime::prompt_dataset_identity(dataset, generation)?;
    let dataset_digest = dataset_identity.dataset_sha256.clone();
    let dataset_identity = serde_json::to_string(&dataset_identity)
        .map_err(|error| format!("offline dataset identity serialization failed: {error}"))?;
    let case_ids = serde_json::to_string(
        &dataset
            .iter()
            .map(|case| case.id.as_str())
            .collect::<Vec<_>>(),
    )
    .map_err(|error| format!("offline case id serialization failed: {error}"))?;
    let train_cases = dataset
        .iter()
        .filter(|case| case.split == PromptEvaluationSplit::Train)
        .count();
    let holdout_cases = dataset.len().saturating_sub(train_cases);
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        task_id,
        EventKind::TaskStatusChanged,
        "Conductor offline dataset selected",
        metadata_with_context(
            [
                ("background_evaluation".to_string(), "true".to_string()),
                ("prompt_effort".to_string(), effort.to_string()),
                ("dataset_sha256".to_string(), dataset_digest),
                ("dataset_identity_v1".to_string(), dataset_identity),
                ("dataset_generation".to_string(), generation.to_string()),
                ("dataset_case_ids".to_string(), case_ids),
                ("dataset_split_manifest".to_string(), split_manifest),
                ("dataset_case_count".to_string(), dataset.len().to_string()),
                ("dataset_train_count".to_string(), train_cases.to_string()),
                (
                    "dataset_holdout_count".to_string(),
                    holdout_cases.to_string(),
                ),
                (
                    "dataset_status".to_string(),
                    if dataset.len() < PROMPT_EVOLUTION_OFFLINE_MIN_CASES {
                        "insufficient_cases"
                    } else {
                        "ready"
                    }
                    .to_string(),
                ),
                (
                    "selected_case_id".to_string(),
                    selected.map(|case| case.id.clone()).unwrap_or_default(),
                ),
                (
                    "selected_case_split".to_string(),
                    selected
                        .map(|case| match case.split {
                            PromptEvaluationSplit::Train => "train",
                            PromptEvaluationSplit::Holdout => "holdout",
                        })
                        .unwrap_or_default()
                        .to_string(),
                ),
                (
                    "selected_source_run_id".to_string(),
                    selected
                        .map(|case| case.source_run_id.clone())
                        .unwrap_or_default(),
                ),
            ]
            .into_iter()
            .collect(),
            run_context,
        ),
    )
    .map_err(|error| error.to_string())
}

#[cfg(test)]
pub(crate) fn prompt_replay_case(
    events: &[Event],
    current_objective: &str,
    replay_index: usize,
) -> Option<PromptReplayCase> {
    let completed_workflows = events
        .iter()
        .filter(|event| event.summary == "Collaboration workflow completed")
        .filter_map(|event| event.metadata.get("collaboration_id"))
        .cloned()
        .collect::<BTreeSet<_>>();
    let completed_agent_runs = events
        .iter()
        .filter(|event| is_agent_run_completed(event))
        .filter_map(|event| event.metadata.get("agent_run_id"))
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    let mut cases = Vec::new();
    for event in events {
        let objective = if event.summary == "Collaboration workflow planned"
            && event
                .metadata
                .get("collaboration_id")
                .is_some_and(|workflow_id| completed_workflows.contains(workflow_id))
        {
            event
                .metadata
                .get("workflow_ir")
                .and_then(|encoded| serde_json::from_str::<WorkflowPlanIr>(encoded).ok())
                .map(|plan| plan.objective)
        } else if event.summary == "Conductor prompt profile selected"
            && event
                .metadata
                .get("collaboration_profile")
                .map(String::as_str)
                == Some("bounded")
            && event
                .metadata
                .get("agent_run_id")
                .is_some_and(|run_id| completed_agent_runs.contains(run_id))
        {
            event
                .metadata
                .get("effective_prompt_objective")
                .or_else(|| event.metadata.get("prompt_objective"))
                .cloned()
        } else {
            None
        };
        let Some(objective) = objective.map(|objective| objective.trim().to_string()) else {
            continue;
        };
        if objective.is_empty()
            || objective == current_objective.trim()
            || !seen.insert(objective.clone())
        {
            continue;
        }
        cases.push(PromptReplayCase {
            objective,
            task_class: event
                .metadata
                .get("task_class")
                .cloned()
                .unwrap_or_else(|| "general".to_string()),
        });
    }
    (!cases.is_empty()).then(|| cases[replay_index % cases.len()].clone())
}
