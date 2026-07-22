use super::*;

pub(crate) fn prompt_profile_evidence_counts(
    observations: &[PromptEvolutionObservation],
    profile_id: &str,
) -> (usize, usize) {
    prompt_unique_evidence_counts(
        observations
            .iter()
            .filter(|observation| observation.profile_id == profile_id),
    )
}

pub(crate) fn prompt_direct_profile_evidence_counts(
    observations: &[PromptEvolutionObservation],
    profile_id: &str,
    opponent_profile_id: &str,
) -> (usize, usize) {
    prompt_unique_evidence_counts(observations.iter().filter(|observation| {
        observation.profile_id == profile_id
            && observation.opponent_profile_id.as_deref() == Some(opponent_profile_id)
    }))
}

fn prompt_unique_evidence_counts<'a>(
    observations: impl Iterator<Item = &'a PromptEvolutionObservation>,
) -> (usize, usize) {
    let mut paired = BTreeSet::new();
    let mut replay = BTreeSet::new();
    for observation in observations {
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

pub(crate) fn prompt_offline_dataset(events: &[Event], project_id: &str) -> Vec<PromptOfflineCase> {
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
        if !run_events
            .iter()
            .any(|event| event.summary == "Agent task completed")
            || !run_events.iter().any(|event| {
                event.metadata.get("project_id").map(String::as_str) == Some(project_id)
            })
        {
            continue;
        }
        let objective = run_events
            .iter()
            .find(|event| event.summary == "Agent task started")
            .and_then(|event| event.metadata.get("prompt"))
            .or_else(|| {
                run_events
                    .iter()
                    .find_map(|event| event.metadata.get("prompt_objective"))
            })
            .map(|objective| {
                truncate_for_collaboration(&redact_sensitive_text(objective.trim()), 4_000)
            });
        let Some(objective) = objective.filter(|objective| objective.chars().count() >= 4) else {
            continue;
        };
        let task_class = run_events
            .iter()
            .find_map(|event| event.metadata.get("task_class"))
            .cloned()
            .unwrap_or_else(|| "general".to_string());
        let case_digest = sha256_hex(objective.as_bytes());
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
        cases.entry(id.clone()).or_insert(PromptOfflineCase {
            id,
            objective,
            task_class,
            project_id: project_id.to_string(),
            source_run_id: run_id,
            split,
        });
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
        if !cases
            .iter()
            .any(|case| case.split == PromptEvaluationSplit::Holdout)
        {
            let index = cases
                .iter()
                .enumerate()
                .rev()
                .find(|(_, case)| !previously_assigned.contains(&case.id))
                .map(|(index, _)| index);
            if let Some(case) = index.and_then(|index| cases.get_mut(index)) {
                case.split = PromptEvaluationSplit::Holdout;
            }
        }
    }
    cases
}

pub(crate) fn select_prompt_offline_case(
    dataset: &[PromptOfflineCase],
    observations: &[PromptEvolutionObservation],
    current_profile_id: &str,
    challenger_profile_id: &str,
    split: PromptEvaluationSplit,
) -> Option<PromptOfflineCase> {
    dataset
        .iter()
        .filter(|case| case.split == split)
        .min_by_key(|case| {
            let current_repeats =
                prompt_unique_evidence_counts(observations.iter().filter(|observation| {
                    observation.case_id == case.id
                        && observation.profile_id == current_profile_id
                        && observation.opponent_profile_id.as_deref() == Some(challenger_profile_id)
                }));
            let challenger_repeats =
                prompt_unique_evidence_counts(observations.iter().filter(|observation| {
                    observation.case_id == case.id
                        && observation.profile_id == challenger_profile_id
                        && observation.opponent_profile_id.as_deref() == Some(current_profile_id)
                }));
            let current_repeats = current_repeats.0.saturating_add(current_repeats.1);
            let challenger_repeats = challenger_repeats.0.saturating_add(challenger_repeats.1);
            (
                current_repeats.saturating_add(challenger_repeats),
                current_repeats.max(challenger_repeats),
                case.id.as_str(),
            )
        })
        .cloned()
}

pub(crate) fn prompt_offline_dataset_digest(dataset: &[PromptOfflineCase]) -> String {
    sha256_hex(
        dataset
            .iter()
            .map(|case| format!("{}:{:?}:{}", case.id, case.split, case.source_run_id))
            .collect::<Vec<_>>()
            .join("\n")
            .as_bytes(),
    )
}

pub(crate) fn append_prompt_offline_dataset_snapshot(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    effort: &str,
    dataset: &[PromptOfflineCase],
    selected: Option<&PromptOfflineCase>,
) -> Result<(), String> {
    let split_manifest = dataset
        .iter()
        .map(|case| (case.id.clone(), case.split))
        .collect::<BTreeMap<_, _>>();
    let split_manifest = serde_json::to_string(&split_manifest)
        .map_err(|error| format!("offline split manifest serialization failed: {error}"))?;
    let dataset_digest = prompt_offline_dataset_digest(dataset);
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
        .filter(|event| event.summary == "Agent task completed")
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
            event.metadata.get("prompt_objective").cloned()
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
