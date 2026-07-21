use super::*;

pub(crate) fn initial_prompt_population(effort: &str) -> Vec<ConductorPromptGenome> {
    let seed = ConductorPromptGenome::seed_for_effort(effort);
    let mutations = seed.mutations();
    let mut population = vec![seed.clone()];
    for variant in [
        mutations
            .iter()
            .find(|variant| variant.graph_depth != seed.graph_depth),
        mutations
            .iter()
            .find(|variant| variant.verification != seed.verification),
        mutations
            .iter()
            .find(|variant| variant.context_policy != seed.context_policy),
        mutations
            .iter()
            .find(|variant| variant.max_parallel_branches != seed.max_parallel_branches),
        mutations
            .iter()
            .find(|variant| variant.tool_policy != seed.tool_policy),
        mutations
            .iter()
            .find(|variant| variant.retry_policy != seed.retry_policy),
    ]
    .into_iter()
    .flatten()
    {
        population.push(variant.clone());
    }
    population.truncate(PROMPT_EVOLUTION_POPULATION_LIMIT);
    population
}

pub(crate) fn prompt_genomes_from_events(
    events: &[Event],
    effort: &str,
) -> Vec<ConductorPromptGenome> {
    let mut population = initial_prompt_population(effort);
    for event in events {
        for record in prompt_genome_records_from_event(event) {
            if record.effort == effort {
                population.push(record.genome);
            }
        }
    }
    let mut ids = BTreeSet::new();
    population.retain(|genome| ids.insert(genome.id.clone()));
    population
}

pub(crate) fn prompt_evolution_observations_from_events(
    events: &[Event],
) -> Vec<(String, PromptEvolutionObservation)> {
    let mut workflows = BTreeMap::<String, Vec<&Event>>::new();
    let mut agent_runs = BTreeMap::<String, Vec<&Event>>::new();
    for event in events {
        if let Some(workflow_id) = event.metadata.get("collaboration_id") {
            workflows
                .entry(workflow_id.clone())
                .or_default()
                .push(event);
        }
        if let Some(run_id) = event.metadata.get("agent_run_id") {
            agent_runs.entry(run_id.clone()).or_default().push(event);
        }
    }
    let mut runs = workflows
        .into_iter()
        .filter_map(|(workflow_id, mut workflow_events)| {
            workflow_events.sort_by_key(|event| event.sequence);
            let profile_event = workflow_events
                .iter()
                .find(|event| event.summary == "Conductor prompt profile selected")
                .copied()
                .or_else(|| {
                    workflow_events
                        .iter()
                        .find(|event| event.summary == "Collaboration workflow planned")
                        .copied()
                })?;
            let plan = workflow_events
                .iter()
                .find(|event| event.summary == "Collaboration workflow planned")
                .and_then(|event| event.metadata.get("workflow_ir"))
                .and_then(|value| serde_json::from_str::<WorkflowPlanIr>(value).ok());
            let profile_id = profile_event
                .metadata
                .get("prompt_profile")
                .cloned()
                .or_else(|| plan.as_ref().map(|plan| plan.prompt_profile.clone()))?;
            let effort = profile_event
                .metadata
                .get("prompt_effort")
                .cloned()
                .or_else(|| plan.as_ref().map(|plan| plan.effort.clone()))?;
            let bounded_profile = profile_event
                .metadata
                .get("collaboration_profile")
                .map(String::as_str)
                == Some("bounded");
            let run_events = profile_event
                .metadata
                .get("agent_run_id")
                .and_then(|run_id| agent_runs.get(run_id));
            let terminal = if let Some(run_events) = run_events {
                run_events.iter().rev().find(|event| {
                    matches!(
                        event.summary.as_str(),
                        "Agent task completed" | "Agent task cancelled" | "Agent task failed"
                    )
                })
            } else {
                workflow_events.iter().rev().find(|event| {
                    matches!(
                        event.summary.as_str(),
                        "Collaboration workflow completed" | "Collaboration workflow failed"
                    )
                })
            }?;
            if terminal.summary == "Agent task cancelled" {
                return None;
            }
            let succeeded = matches!(
                terminal.summary.as_str(),
                "Agent task completed" | "Collaboration workflow completed"
            );
            let quality_event = workflow_events
                .iter()
                .rev()
                .find(|event| event.summary == "Collaboration quality gate evaluated");
            let measured_quality = quality_event
                .and_then(|event| event.metadata.get("quality_score"))
                .and_then(|score| score.parse::<f64>().ok());
            let measured_safety_violations = quality_event
                .and_then(|event| event.metadata.get("safety_violations"))
                .and_then(|count| count.parse::<u64>().ok())
                .unwrap_or_default();
            let evaluation_events = run_events
                .map(Vec::as_slice)
                .unwrap_or(workflow_events.as_slice());
            let permission_denials = evaluation_events
                .iter()
                .filter(|event| {
                    event.kind == EventKind::PermissionResolved
                        && event.metadata.get("decision").is_some_and(|decision| {
                            !matches!(decision.as_str(), "allow_once" | "allow_for_session")
                        })
                })
                .count() as u64;
            let total_tokens = evaluation_events
                .iter()
                .filter(|event| event.kind == EventKind::ModelRequestFinished)
                .filter_map(|event| event.metadata.get("total_tokens"))
                .filter_map(|tokens| tokens.parse::<u64>().ok())
                .sum();
            let step_credits = workflow_events
                .iter()
                .rev()
                .find_map(|event| event.metadata.get("step_credits"))
                .and_then(|encoded| serde_json::from_str::<Vec<PromptStepCredit>>(encoded).ok())
                .unwrap_or_default();
            Some((
                profile_event.sequence,
                effort,
                PromptEvolutionObservation {
                    profile_id,
                    evaluation_id: workflow_id.clone(),
                    case_id: workflow_id,
                    opponent_profile_id: None,
                    task_class: profile_event
                        .metadata
                        .get("task_class")
                        .cloned()
                        .unwrap_or_else(|| "general".to_string()),
                    split: PromptEvaluationSplit::Train,
                    mode: PromptEvaluationMode::Live,
                    format_valid: plan.is_some() || bounded_profile,
                    succeeded,
                    quality_score: measured_quality.unwrap_or(if succeeded { 0.5 } else { 0.0 }),
                    latency_ms: terminal
                        .timestamp_ms
                        .saturating_sub(profile_event.timestamp_ms),
                    total_tokens,
                    estimated_cost_microusd: 0,
                    safety_violations: measured_safety_violations
                        .saturating_add(permission_denials),
                    relative_reward: None,
                    step_credits,
                    reflection_packet: None,
                },
            ))
        })
        .collect::<Vec<_>>();
    runs.extend(events.iter().flat_map(|event| {
        prompt_observation_records_from_event(event)
            .into_iter()
            .map(|(effort, observation)| (event.sequence, effort, observation))
    }));
    runs.sort_by_key(|(sequence, _, _)| *sequence);
    runs.into_iter()
        .map(|(_, effort, observation)| (effort, observation))
        .collect()
}

pub(crate) fn prompt_genome_records_from_event(event: &Event) -> Vec<PromptGenomeRecord> {
    let Some(effort) = event.metadata.get("prompt_effort").cloned() else {
        return Vec::new();
    };
    let genomes = event
        .metadata
        .get("prompt_genomes")
        .and_then(|encoded| serde_json::from_str::<Vec<ConductorPromptGenome>>(encoded).ok())
        .or_else(|| {
            event
                .metadata
                .get("prompt_genome")
                .and_then(|encoded| serde_json::from_str::<ConductorPromptGenome>(encoded).ok())
                .map(|genome| vec![genome])
        })
        .unwrap_or_default();
    genomes
        .into_iter()
        .filter(|genome| genome.validate().is_ok())
        .map(|genome| PromptGenomeRecord {
            effort: effort.clone(),
            genome,
        })
        .collect()
}

pub(crate) fn prompt_observation_records_from_event(
    event: &Event,
) -> Vec<(String, PromptEvolutionObservation)> {
    if event.summary != "Conductor pairwise evaluation" {
        return Vec::new();
    }
    let Some(effort) = event.metadata.get("prompt_effort").cloned() else {
        return Vec::new();
    };
    let observations = event
        .metadata
        .get("prompt_observations")
        .and_then(|encoded| serde_json::from_str::<Vec<PromptEvolutionObservation>>(encoded).ok())
        .or_else(|| {
            event
                .metadata
                .get("prompt_observation")
                .and_then(|encoded| {
                    serde_json::from_str::<PromptEvolutionObservation>(encoded).ok()
                })
                .map(|observation| vec![observation])
        })
        .unwrap_or_default();
    observations
        .into_iter()
        .map(|observation| (effort.clone(), observation))
        .collect()
}

pub(crate) fn prompt_rollout_record_from_event(
    event: &Event,
) -> Option<(String, PromptRolloutState)> {
    if event.summary != "Conductor prompt rollout updated" {
        return None;
    }
    let effort = event.metadata.get("prompt_effort")?.clone();
    let stable_profile_id = event.metadata.get("stable_profile")?.clone();
    if effort.trim().is_empty() || stable_profile_id.trim().is_empty() {
        return None;
    }
    let optional_text = |key: &str| {
        event
            .metadata
            .get(key)
            .filter(|value| !value.trim().is_empty())
            .cloned()
    };
    let parse_usize = |key: &str| {
        event
            .metadata
            .get(key)
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or_default()
    };
    Some((
        effort,
        PromptRolloutState {
            stable_profile_id,
            canary_profile_id: optional_text("canary_profile"),
            canary_percent: event
                .metadata
                .get("canary_percent")
                .and_then(|value| value.parse::<u8>().ok())
                .unwrap_or_default()
                .min(100),
            evidence_checkpoint: parse_usize("evidence_checkpoint"),
            live_checkpoint: parse_usize("live_checkpoint"),
            rollback_count: parse_usize("rollback_count"),
            status: event
                .metadata
                .get("rollout_status")
                .cloned()
                .unwrap_or_else(|| "stable".to_string()),
            last_reason: optional_text("rollout_reason"),
            promotion_confidence: event
                .metadata
                .get("promotion_confidence")
                .and_then(|value| value.parse::<f64>().ok())
                .filter(|value| value.is_finite()),
        },
    ))
}

pub(crate) fn prompt_dataset_key(effort: &str, project_id: &str) -> String {
    format!("{effort}:{project_id}")
}

pub(crate) fn prompt_dataset_record_from_event(
    event: &Event,
) -> Option<(String, PromptOfflineDatasetState)> {
    if event.summary != "Conductor offline dataset selected" {
        return None;
    }
    let effort = event.metadata.get("prompt_effort")?.clone();
    let project_id = event.metadata.get("project_id")?.clone();
    let digest = event.metadata.get("dataset_sha256")?.clone();
    if effort.trim().is_empty() || project_id.trim().is_empty() || digest.trim().is_empty() {
        return None;
    }
    let parse_usize = |key: &str| {
        event
            .metadata
            .get(key)
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or_default()
    };
    let selected_case_id = event
        .metadata
        .get("selected_case_id")
        .filter(|value| !value.trim().is_empty())
        .cloned();
    let key = prompt_dataset_key(&effort, &project_id);
    Some((
        key,
        PromptOfflineDatasetState {
            effort,
            project_id,
            digest,
            case_count: parse_usize("dataset_case_count"),
            train_count: parse_usize("dataset_train_count"),
            holdout_count: parse_usize("dataset_holdout_count"),
            selected_case_id,
            status: event
                .metadata
                .get("dataset_status")
                .cloned()
                .unwrap_or_else(|| "ready".to_string()),
            updated_at_ms: event.timestamp_ms,
        },
    ))
}

pub(crate) fn upsert_prompt_genome(
    records: &mut Vec<PromptGenomeRecord>,
    record: PromptGenomeRecord,
) {
    if let Some(existing) = records
        .iter_mut()
        .find(|existing| existing.effort == record.effort && existing.genome.id == record.genome.id)
    {
        *existing = record;
    } else {
        records.push(record);
    }
}

pub(crate) fn upsert_prompt_observation(
    observations: &mut Vec<(String, PromptEvolutionObservation)>,
    effort: String,
    observation: PromptEvolutionObservation,
) {
    if let Some(existing) = observations.iter_mut().find(|(existing_effort, existing)| {
        existing_effort == &effort
            && existing.evaluation_id == observation.evaluation_id
            && existing.profile_id == observation.profile_id
    }) {
        *existing = (effort, observation);
    } else {
        observations.push((effort, observation));
    }
}

pub(crate) fn build_prompt_evolution_read_model(
    events: &[Event],
    revision: u64,
    event_count: u64,
) -> PromptEvolutionReadModel {
    let mut genomes = Vec::new();
    let mut rollouts = BTreeMap::new();
    let mut datasets = BTreeMap::new();
    for event in events {
        for record in prompt_genome_records_from_event(event) {
            upsert_prompt_genome(&mut genomes, record);
        }
        if let Some((effort, rollout)) = prompt_rollout_record_from_event(event) {
            rollouts.insert(effort, rollout);
        }
        if let Some((key, dataset)) = prompt_dataset_record_from_event(event) {
            datasets.insert(key, dataset);
        }
    }
    PromptEvolutionReadModel {
        schema: PROMPT_EVOLUTION_READ_MODEL_NAMESPACE.to_string(),
        revision,
        event_count,
        genomes,
        observations: prompt_evolution_observations_from_events(events),
        rollouts,
        datasets,
    }
}

pub(crate) fn load_prompt_evolution_read_model(
    store: &mut SqliteStore,
) -> Result<PromptEvolutionReadModel, StorageError> {
    let task_id = phase16_task_id();
    let revision = store.event_revision(&task_id)?;
    let stored = store
        .load_read_model(
            PROMPT_EVOLUTION_READ_MODEL_NAMESPACE,
            PROMPT_EVOLUTION_READ_MODEL_KEY,
        )?
        .and_then(|stored| {
            serde_json::from_str::<PromptEvolutionReadModel>(&stored.payload)
                .ok()
                .filter(|model| {
                    model.schema == PROMPT_EVOLUTION_READ_MODEL_NAMESPACE
                        && model.revision == stored.revision
                        && model.revision <= revision.latest_sequence
                        && model.event_count <= revision.event_count
                })
        });
    let mut model = stored.unwrap_or_else(|| PromptEvolutionReadModel {
        schema: PROMPT_EVOLUTION_READ_MODEL_NAMESPACE.to_string(),
        revision: 0,
        event_count: 0,
        genomes: Vec::new(),
        observations: Vec::new(),
        rollouts: BTreeMap::new(),
        datasets: BTreeMap::new(),
    });
    let mut delta = store.list_by_task_after(&task_id, model.revision)?;
    if model.event_count.saturating_add(delta.len() as u64) != revision.event_count {
        delta = store.list_by_task_after(&task_id, 0)?;
        model = build_prompt_evolution_read_model(
            &delta,
            revision.latest_sequence,
            revision.event_count,
        );
    } else if model.revision == 0 {
        model = build_prompt_evolution_read_model(
            &delta,
            revision.latest_sequence,
            revision.event_count,
        );
    } else {
        for event in &delta {
            for record in prompt_genome_records_from_event(event) {
                upsert_prompt_genome(&mut model.genomes, record);
            }
            if let Some((effort, rollout)) = prompt_rollout_record_from_event(event) {
                model.rollouts.insert(effort, rollout);
            }
            if let Some((key, dataset)) = prompt_dataset_record_from_event(event) {
                model.datasets.insert(key, dataset);
            }
            for (effort, observation) in prompt_observation_records_from_event(event) {
                upsert_prompt_observation(&mut model.observations, effort, observation);
            }
        }
        let terminal_scopes = delta
            .iter()
            .filter_map(|event| {
                if matches!(
                    event.summary.as_str(),
                    "Agent task completed" | "Agent task cancelled" | "Agent task failed"
                ) {
                    event
                        .metadata
                        .get("agent_run_id")
                        .map(|id| ("agent_run_id", id.clone()))
                } else if matches!(
                    event.summary.as_str(),
                    "Collaboration workflow completed" | "Collaboration workflow failed"
                ) {
                    event
                        .metadata
                        .get("collaboration_id")
                        .map(|id| ("collaboration_id", id.clone()))
                } else {
                    None
                }
            })
            .collect::<BTreeSet<_>>();
        for (scope, value) in terminal_scopes {
            let scoped_events = store.list_by_task_and_metadata(&task_id, scope, &value)?;
            for (effort, observation) in prompt_evolution_observations_from_events(&scoped_events) {
                upsert_prompt_observation(&mut model.observations, effort, observation);
            }
        }
        model.revision = revision.latest_sequence;
        model.event_count = revision.event_count;
    }

    save_prompt_evolution_read_model(store, &model)?;
    Ok(model)
}

pub(crate) fn save_prompt_evolution_read_model(
    store: &mut SqliteStore,
    model: &PromptEvolutionReadModel,
) -> Result<(), StorageError> {
    let payload = serde_json::to_string(model).map_err(|error| {
        StorageError::new(format!("prompt evolution serialization failed: {error}"))
    })?;
    store.save_read_model(
        PROMPT_EVOLUTION_READ_MODEL_NAMESPACE,
        PROMPT_EVOLUTION_READ_MODEL_KEY,
        model.revision,
        &payload,
    )
}

pub(crate) fn prompt_evolution_profile_events(model: &PromptEvolutionReadModel) -> Vec<Event> {
    model
        .genomes
        .iter()
        .enumerate()
        .filter_map(|(index, record)| {
            Some(Event {
                id: EventId(format!("prompt-read-model-{index}")),
                task_id: phase16_task_id(),
                sequence: index as u64 + 1,
                timestamp_ms: 0,
                kind: EventKind::TaskStatusChanged,
                summary: "Conductor prompt profile indexed".to_string(),
                metadata: [
                    ("prompt_effort".to_string(), record.effort.clone()),
                    (
                        "prompt_genome".to_string(),
                        serde_json::to_string(&record.genome).ok()?,
                    ),
                ]
                .into_iter()
                .collect(),
            })
        })
        .collect()
}
