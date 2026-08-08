use agent_core::{Metadata, EVENT_TYPE_METADATA_KEY};

const EVENT_LOCAL_RUN_CONTEXT_KEYS: &[&str] = &[
    "initial_prompt_objective",
    "effective_prompt_objective",
    "prompt_objective",
];

/// Merge durable run identity and policy context into an event without copying
/// objective text into every event in the run. Events that own an objective
/// snapshot must insert it explicitly before calling this function.
pub fn merge_persistable_run_context(mut metadata: Metadata, context: &Metadata) -> Metadata {
    for (key, value) in context {
        if key == EVENT_TYPE_METADATA_KEY || EVENT_LOCAL_RUN_CONTEXT_KEYS.contains(&key.as_str()) {
            continue;
        }
        metadata.entry(key.clone()).or_insert_with(|| value.clone());
    }
    metadata
}

/// Persist one canonical display prompt and only the model/recovery variants
/// that differ from it. Legacy readers already fall back in this order.
pub fn insert_run_start_prompts(
    metadata: &mut Metadata,
    display_prompt: &str,
    model_prompt: &str,
    recovery_prompt: &str,
) {
    metadata.insert("prompt".to_string(), display_prompt.to_string());
    insert_distinct(metadata, "model_prompt", model_prompt, display_prompt);
    let recovery_fallback = if model_prompt == display_prompt {
        display_prompt
    } else {
        model_prompt
    };
    insert_distinct(
        metadata,
        "recovery_prompt",
        recovery_prompt,
        recovery_fallback,
    );
}

/// Message content is already the display prompt for persisted user events.
/// Keep a model-only variant only when attachments or other runtime context
/// make the model input differ from what the user sees.
pub fn insert_user_message_model_prompt(
    metadata: &mut Metadata,
    display_prompt: &str,
    model_prompt: &str,
) {
    insert_distinct(metadata, "model_content", model_prompt, display_prompt);
}

/// Runtime messages carry the model prompt as their content. Persist a display
/// override only when the user-facing text is different.
pub fn insert_runtime_message_display_prompt(
    metadata: &mut Metadata,
    display_prompt: &str,
    model_prompt: &str,
) {
    insert_distinct(metadata, "display_content", display_prompt, model_prompt);
    metadata.remove("model_content");
}

/// Copy the objective snapshot only onto events that own that contract, such
/// as run starts, route decisions, permission checkpoints, and recovery data.
pub fn insert_run_objectives(metadata: &mut Metadata, context: &Metadata) {
    for key in EVENT_LOCAL_RUN_CONTEXT_KEYS {
        if let Some(value) = context.get(*key) {
            metadata.insert((*key).to_string(), value.clone());
        }
    }
}

fn insert_distinct(metadata: &mut Metadata, key: &str, value: &str, fallback: &str) {
    if value == fallback {
        metadata.remove(key);
    } else {
        metadata.insert(key.to_string(), value.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_event_context_does_not_duplicate_objective_text() {
        let context = [
            ("agent_run_id".to_string(), "run-1".to_string()),
            (
                "initial_prompt_objective".to_string(),
                "inspect the repository".to_string(),
            ),
            (
                "effective_prompt_objective".to_string(),
                "inspect the repository and tests".to_string(),
            ),
        ]
        .into_iter()
        .collect();

        let merged = merge_persistable_run_context(Metadata::new(), &context);

        assert_eq!(
            merged.get("agent_run_id").map(String::as_str),
            Some("run-1")
        );
        assert!(!merged.contains_key("initial_prompt_objective"));
        assert!(!merged.contains_key("effective_prompt_objective"));
    }

    #[test]
    fn explicit_objective_snapshot_survives_context_merge() {
        let context = [(
            "effective_prompt_objective".to_string(),
            "new objective".to_string(),
        )]
        .into_iter()
        .collect();
        let mut metadata = Metadata::new();
        insert_run_objectives(&mut metadata, &context);

        let merged = merge_persistable_run_context(metadata, &context);

        assert_eq!(
            merged.get("effective_prompt_objective").map(String::as_str),
            Some("new objective")
        );
    }

    #[test]
    fn run_start_stores_only_distinct_prompt_variants() {
        let mut plain = Metadata::new();
        insert_run_start_prompts(&mut plain, "hello", "hello", "hello");
        assert_eq!(plain.len(), 1);
        assert_eq!(plain.get("prompt").map(String::as_str), Some("hello"));

        let mut attached = Metadata::new();
        insert_run_start_prompts(
            &mut attached,
            "review image",
            "review image\n[attachment: image.png]",
            "review image\n[attachment: image.png]",
        );
        assert_eq!(attached.len(), 2);
        assert!(attached.contains_key("model_prompt"));
        assert!(!attached.contains_key("recovery_prompt"));
    }

    #[test]
    fn user_message_stores_model_content_only_when_distinct() {
        let mut plain = Metadata::new();
        insert_user_message_model_prompt(&mut plain, "hello", "hello");
        assert!(!plain.contains_key("model_content"));

        let mut attached = Metadata::new();
        insert_user_message_model_prompt(
            &mut attached,
            "review image",
            "review image\n[attachment: image.png]",
        );
        assert!(attached.contains_key("model_content"));
    }

    #[test]
    fn runtime_message_stores_only_a_distinct_display_override() {
        let mut plain = Metadata::new();
        insert_runtime_message_display_prompt(&mut plain, "hello", "hello");
        assert!(plain.is_empty());

        let mut attached = Metadata::new();
        insert_runtime_message_display_prompt(
            &mut attached,
            "review image",
            "review image\n[attachment: image.png]",
        );
        assert_eq!(attached.len(), 1);
        assert!(attached.contains_key("display_content"));
    }
}
