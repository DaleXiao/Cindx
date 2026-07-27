use super::*;

pub(crate) fn prompt_profile_evidence_counts(
    observations: &[PromptEvolutionObservation],
    profile_id: &str,
) -> (usize, usize) {
    let active_dataset_sha256 = orchestrator::latest_scientific_dataset_digest(observations);
    prompt_unique_evidence_counts(
        observations
            .iter()
            .filter(|observation| observation.profile_id == profile_id),
        active_dataset_sha256,
    )
}

pub(crate) fn prompt_direct_profile_evidence_counts(
    observations: &[PromptEvolutionObservation],
    profile_id: &str,
    opponent_profile_id: &str,
) -> (usize, usize) {
    let active_dataset_sha256 = orchestrator::latest_scientific_dataset_digest(observations);
    prompt_unique_evidence_counts(observations.iter().filter(|observation| {
        observation.profile_id == profile_id
            && observation.opponent_profile_id.as_deref() == Some(opponent_profile_id)
    }), active_dataset_sha256)
}

fn prompt_unique_evidence_counts<'a>(
    observations: impl Iterator<Item = &'a PromptEvolutionObservation>,
    dataset_sha256: Option<&str>,
) -> (usize, usize) {
    let Some(dataset_sha256) = dataset_sha256 else {
        return (0, 0);
    };
    let mut paired = BTreeSet::new();
    let mut replay = BTreeSet::new();
    for observation in observations
        .filter(|observation| observation.is_scientific_evidence())
        .filter(|observation| observation.provenance.dataset_sha256 == dataset_sha256)
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
        if !run_events.iter().any(|event| {
            matches!(
                event.summary.as_str(),
                "Agent task completed" | "Agent task failed" | "Agent task cancelled"
            )
        }) || !run_events
            .iter()
            .any(|event| event.metadata.get("project_id").map(String::as_str) == Some(project_id))
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

pub(crate) fn prompt_offline_dataset_digest(dataset: &[PromptOfflineCase]) -> String {
    sha256_hex(
        dataset
            .iter()
            .map(|case| format!("{}:{:?}", case.id, case.split))
            .collect::<Vec<_>>()
            .join("\n")
            .as_bytes(),
    )
}

pub(crate) fn prompt_offline_dataset_for_generation(
    discovered: Vec<PromptOfflineCase>,
    previous: Option<&PromptOfflineDatasetState>,
    generation: u32,
) -> Vec<PromptOfflineCase> {
    let Some(previous) = previous.filter(|snapshot| {
        snapshot.status == "ready"
            && snapshot.generation == generation
            && snapshot.case_ids.len() >= PROMPT_EVOLUTION_OFFLINE_MIN_CASES
    }) else {
        return discovered;
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
    if frozen.len() == previous.case_ids.len() {
        frozen
    } else {
        discovered
    }
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
    let dataset_digest = prompt_offline_dataset_digest(dataset);
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
