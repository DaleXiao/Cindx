use super::*;
use agent_core::{EventId, EventKind};

fn request(effort: &str, id: &str) -> PromptEvaluationRequest {
    PromptEvaluationRequest {
        schema: PROMPT_EVALUATION_REQUEST_SCHEMA.to_string(),
        request_id: id.to_string(),
        task_id: "task".to_string(),
        run_context: Metadata::new(),
        effort: effort.to_string(),
        policy: "auto".to_string(),
        worker_models: vec!["model".to_string()],
        agent_budget: 1,
        current_profile: ConductorPromptGenome::seed_for_effort(effort),
        configuration_sha256: String::new(),
    }
}

fn request_for_project(effort: &str, id: &str, project_id: &str) -> PromptEvaluationRequest {
    let mut request = request(effort, id);
    request
        .run_context
        .insert("project_id".to_string(), project_id.to_string());
    request
}

fn event(sequence: u64, summary: &str, metadata: Metadata) -> Event {
    Event {
        id: EventId(format!("event-{sequence}")),
        task_id: TaskId("task".to_string()),
        sequence,
        timestamp_ms: sequence,
        kind: EventKind::TaskStatusChanged,
        summary: summary.to_string(),
        metadata,
    }
}

fn request_event(sequence: u64, request: &PromptEvaluationRequest) -> Event {
    let mut request_event = event(
        sequence,
        REQUEST_EVENT,
        [
            (REQUEST_ID_KEY.to_string(), request.request_id.clone()),
            (
                REQUEST_METADATA_KEY.to_string(),
                serde_json::to_string(request).unwrap(),
            ),
        ]
        .into_iter()
        .collect(),
    );
    request_event.timestamp_ms = 1;
    request_event
}

fn checkpoint_event(
    sequence: u64,
    request_id: Option<&str>,
    completed_actions: Option<&str>,
    campaign_usage: Option<&str>,
) -> Event {
    let mut metadata = Metadata::new();
    if let Some(request_id) = request_id {
        metadata.insert(REQUEST_ID_KEY.to_string(), request_id.to_string());
    }
    if let Some(completed_actions) = completed_actions {
        metadata.insert(
            "completed_actions".to_string(),
            completed_actions.to_string(),
        );
    }
    if let Some(campaign_usage) = campaign_usage {
        metadata.insert("campaign_usage".to_string(), campaign_usage.to_string());
    }
    event(sequence, CHECKPOINT_EVENT, metadata)
}

fn action_event(
    sequence: u64,
    summary: &str,
    request_id: &str,
    action_index: usize,
    completed_actions: usize,
    usage: Option<&PromptEvaluationCampaignUsage>,
) -> Event {
    let mut metadata = [
        (REQUEST_ID_KEY.to_string(), request_id.to_string()),
        (
            crate::prompt_evolution_campaign_runtime::ACTION_ID_KEY.to_string(),
            format!("{request_id}:{action_index}"),
        ),
        (
            crate::prompt_evolution_campaign_runtime::ACTION_INDEX_KEY.to_string(),
            action_index.to_string(),
        ),
        (
            "completed_actions".to_string(),
            completed_actions.to_string(),
        ),
    ]
    .into_iter()
    .collect::<Metadata>();
    if let Some(usage) = usage {
        metadata.insert(
            "campaign_usage".to_string(),
            serde_json::to_string(usage).unwrap(),
        );
    }
    event(sequence, summary, metadata)
}

#[test]
fn newest_request_supersedes_an_older_request_for_the_same_effort() {
    let older = request("auto", "older");
    let newer = request("auto", "newer");
    let pending = latest_pending_prompt_evaluations_from_events(&[
        request_event(1, &older),
        request_event(2, &newer),
    ])
    .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].request.request_id, "newer");
}

#[test]
fn same_effort_requests_are_isolated_by_project() {
    let project_a_old = request_for_project("pro", "project-a-old", "project-a");
    let project_b = request_for_project("pro", "project-b", "project-b");
    let project_a_new = request_for_project("pro", "project-a-new", "project-a");
    let pending = latest_pending_prompt_evaluations_from_events(&[
        request_event(1, &project_a_old),
        request_event(2, &project_b),
        request_event(3, &project_a_new),
    ])
    .unwrap();
    let ids = pending
        .iter()
        .map(|pending| pending.request.request_id.as_str())
        .collect::<BTreeSet<_>>();

    assert_eq!(ids, BTreeSet::from(["project-a-new", "project-b"]));
}

#[test]
fn terminal_latest_request_does_not_resurrect_an_older_request() {
    let older = request("auto", "older");
    let newer = request("auto", "newer");
    let pending = latest_pending_prompt_evaluations_from_events(&[
        request_event(1, &older),
        request_event(2, &newer),
        event(
            3,
            COMPLETED_EVENT,
            [(REQUEST_ID_KEY.to_string(), "newer".to_string())]
                .into_iter()
                .collect(),
        ),
    ])
    .unwrap();
    assert!(pending.is_empty());
}

#[test]
fn checkpoint_preserves_campaign_progress_for_restart() {
    let request = request("pro", "resume");
    let campaign_usage = PromptEvaluationCampaignUsage {
        started_at_ms: 1,
        total_tokens: 42,
        physical_model_attempts: 3,
    };
    let pending = latest_pending_prompt_evaluations_from_events(&[
        request_event(1, &request),
        event(
            2,
            CHECKPOINT_EVENT,
            [
                (REQUEST_ID_KEY.to_string(), "resume".to_string()),
                ("completed_actions".to_string(), "4".to_string()),
                (
                    "campaign_usage".to_string(),
                    serde_json::to_string(&campaign_usage).unwrap(),
                ),
            ]
            .into_iter()
            .collect(),
        ),
    ])
    .unwrap();
    assert_eq!(pending[0].completed_actions, 4);
    assert_eq!(pending[0].campaign_usage, campaign_usage);
}

#[test]
fn completed_action_recovers_usage_newer_than_the_batch_checkpoint() {
    let request = request("pro", "resume");
    let checkpoint_usage = PromptEvaluationCampaignUsage {
        started_at_ms: 1,
        total_tokens: 42,
        physical_model_attempts: 3,
    };
    let action_usage = PromptEvaluationCampaignUsage {
        started_at_ms: 1,
        total_tokens: 75,
        physical_model_attempts: 5,
    };
    let pending = latest_pending_prompt_evaluations_from_events(&[
        request_event(1, &request),
        checkpoint_event(
            2,
            Some("resume"),
            Some("4"),
            Some(&serde_json::to_string(&checkpoint_usage).unwrap()),
        ),
        action_event(
            3,
            crate::prompt_evolution_campaign_runtime::ACTION_STARTED_EVENT,
            "resume",
            4,
            4,
            None,
        ),
        action_event(
            4,
            crate::prompt_evolution_campaign_runtime::ACTION_COMPLETED_EVENT,
            "resume",
            4,
            5,
            Some(&action_usage),
        ),
    ])
    .unwrap();

    assert_eq!(pending[0].completed_actions, 5);
    assert_eq!(pending[0].campaign_usage, action_usage);
}

#[test]
fn interrupted_action_fails_closed_instead_of_reacquiring_budget() {
    let request = request("pro", "resume");
    let pending = latest_pending_prompt_evaluations_from_events(&[
        request_event(1, &request),
        action_event(
            2,
            crate::prompt_evolution_campaign_runtime::ACTION_STARTED_EVENT,
            "resume",
            0,
            0,
            None,
        ),
    ])
    .unwrap();

    assert!(pending[0].campaign_usage.remaining_budget(3).is_none());
}

#[test]
fn checkpoint_missing_or_invalid_fields_fail_closed() {
    let request = request("pro", "resume");
    let valid_usage = serde_json::to_string(&PromptEvaluationCampaignUsage {
        started_at_ms: 1,
        total_tokens: 42,
        physical_model_attempts: 3,
    })
    .unwrap();
    let reset_start = serde_json::to_string(&PromptEvaluationCampaignUsage {
        started_at_ms: 2,
        total_tokens: 42,
        physical_model_attempts: 3,
    })
    .unwrap();

    for checkpoint in [
        checkpoint_event(2, Some("resume"), Some("4"), None),
        checkpoint_event(2, Some("resume"), Some("bad"), Some(&valid_usage)),
        checkpoint_event(2, Some("resume"), Some("4"), Some("bad-json")),
        checkpoint_event(2, Some("resume"), Some("4"), Some(&reset_start)),
    ] {
        let pending = latest_pending_prompt_evaluations_from_events(&[
            request_event(1, &request),
            checkpoint,
        ])
        .unwrap();

        assert!(pending[0].campaign_usage.remaining_budget(2).is_none());
    }
}

#[test]
fn checkpoint_missing_or_unknown_request_id_stops_the_scan() {
    let request = request("pro", "resume");
    let usage = serde_json::to_string(&PromptEvaluationCampaignUsage {
        started_at_ms: 1,
        total_tokens: 42,
        physical_model_attempts: 3,
    })
    .unwrap();

    assert!(latest_pending_prompt_evaluations_from_events(&[
        request_event(1, &request),
        checkpoint_event(2, None, Some("4"), Some(&usage)),
    ])
    .is_err());
    assert!(latest_pending_prompt_evaluations_from_events(&[
        request_event(1, &request),
        checkpoint_event(2, Some("unknown"), Some("4"), Some(&usage)),
    ])
    .is_err());
}

#[test]
fn terminal_missing_or_unknown_request_id_stops_the_scan() {
    let request = request("pro", "resume");

    assert!(latest_pending_prompt_evaluations_from_events(&[
        request_event(1, &request),
        event(2, COMPLETED_EVENT, Metadata::new()),
    ])
    .is_err());
    assert!(latest_pending_prompt_evaluations_from_events(&[
        request_event(1, &request),
        event(
            2,
            FAILED_EVENT,
            [(REQUEST_ID_KEY.to_string(), "unknown".to_string())]
                .into_iter()
                .collect(),
        ),
    ])
    .is_err());
}

#[test]
fn checkpoint_usage_or_completed_actions_cannot_move_backwards() {
    let request = request("pro", "resume");
    let first_usage = serde_json::to_string(&PromptEvaluationCampaignUsage {
        started_at_ms: 1,
        total_tokens: 42,
        physical_model_attempts: 3,
    })
    .unwrap();
    let lower_tokens = serde_json::to_string(&PromptEvaluationCampaignUsage {
        started_at_ms: 1,
        total_tokens: 41,
        physical_model_attempts: 4,
    })
    .unwrap();
    let later_usage = serde_json::to_string(&PromptEvaluationCampaignUsage {
        started_at_ms: 1,
        total_tokens: 50,
        physical_model_attempts: 5,
    })
    .unwrap();

    for (second, expected_actions) in [
        (
            checkpoint_event(3, Some("resume"), Some("5"), Some(&lower_tokens)),
            5,
        ),
        (
            checkpoint_event(3, Some("resume"), Some("3"), Some(&first_usage)),
            4,
        ),
    ] {
        let pending = latest_pending_prompt_evaluations_from_events(&[
            request_event(1, &request),
            checkpoint_event(2, Some("resume"), Some("4"), Some(&first_usage)),
            second,
        ])
        .unwrap();

        assert!(pending[0].campaign_usage.remaining_budget(3).is_none());
        assert_eq!(pending[0].completed_actions, expected_actions);
    }

    let pending = latest_pending_prompt_evaluations_from_events(&[
        request_event(1, &request),
        checkpoint_event(2, Some("resume"), Some("4"), Some(&first_usage)),
        checkpoint_event(3, Some("resume"), Some("5"), Some(&lower_tokens)),
        checkpoint_event(4, Some("resume"), Some("6"), Some(&later_usage)),
    ])
    .unwrap();
    assert!(pending[0].campaign_usage.remaining_budget(4).is_none());
}
