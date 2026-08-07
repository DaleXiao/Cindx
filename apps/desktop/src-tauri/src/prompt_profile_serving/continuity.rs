use agent_core::{Event, Metadata, LOGICAL_AGENT_RUN_ID_METADATA_KEY};

const PROMPT_PROFILE_ASSIGNMENT_CONTEXT_KEYS: [&str; 6] = [
    "prompt_profile",
    "prompt_genome",
    "prompt_profile_source",
    "prompt_profile_assignment_receipt",
    "prompt_profile_assignment_sha256",
    "prompt_rollout_status",
];

const REQUIRED_ASSIGNMENT_CONTEXT_KEYS: [&str; 5] = [
    "prompt_profile",
    "prompt_genome",
    "prompt_profile_source",
    "prompt_profile_assignment_receipt",
    "prompt_profile_assignment_sha256",
];

pub(crate) fn copy_prompt_profile_assignment(
    source: &Metadata,
    target: &mut Metadata,
) -> Result<(), String> {
    let has_assignment = PROMPT_PROFILE_ASSIGNMENT_CONTEXT_KEYS
        .iter()
        .any(|key| source.contains_key(*key));
    if has_assignment
        && REQUIRED_ASSIGNMENT_CONTEXT_KEYS
            .iter()
            .any(|key| !source.contains_key(*key))
    {
        return Err("prompt profile assignment context is incomplete".to_string());
    }
    let replacement = PROMPT_PROFILE_ASSIGNMENT_CONTEXT_KEYS
        .iter()
        .filter_map(|key| {
            source
                .get(*key)
                .map(|value| ((*key).to_string(), value.clone()))
        })
        .collect::<Metadata>();
    for key in PROMPT_PROFILE_ASSIGNMENT_CONTEXT_KEYS {
        target.remove(key);
    }
    target.extend(replacement);
    Ok(())
}

pub(crate) fn prompt_profile_assignment_from_events(
    events: &[Event],
    logical_run_id: &str,
) -> Result<Metadata, String> {
    let Some(event) = events.iter().rev().find(|event| {
        event.summary == "Agent run decision selected"
            && event
                .metadata
                .get(LOGICAL_AGENT_RUN_ID_METADATA_KEY)
                .map(String::as_str)
                == Some(logical_run_id)
            && REQUIRED_ASSIGNMENT_CONTEXT_KEYS
                .iter()
                .any(|key| event.metadata.contains_key(*key))
    }) else {
        return Ok(Metadata::new());
    };
    if REQUIRED_ASSIGNMENT_CONTEXT_KEYS
        .iter()
        .any(|key| !event.metadata.contains_key(*key))
    {
        return Err("durable prompt profile assignment is incomplete".to_string());
    }
    let mut assignment = Metadata::new();
    copy_prompt_profile_assignment(&event.metadata, &mut assignment)?;
    Ok(assignment)
}
