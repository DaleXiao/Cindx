use agent_core::Metadata;
use agent_runtime::{PreparedTaskState, PromptCompletionIntent};

const EFFECTIVE_OBJECTIVE_KEY: &str = "effective_prompt_objective";
const STEER_EPOCH_KEY: &str = "steer_epoch";
const CONTRACT_EPOCH_KEY: &str = "prompt_contract_epoch";

pub(crate) fn prepared_task_state_from_legacy_metadata(
    run_context: &Metadata,
    latest_prompt: &str,
    completion_intent: PromptCompletionIntent,
) -> PreparedTaskState {
    PreparedTaskState::from_run_context(run_context, latest_prompt, completion_intent)
}

pub(crate) fn project_prepared_task_state_to_legacy_metadata(
    state: &PreparedTaskState,
    metadata: &mut Metadata,
) {
    metadata.insert(
        EFFECTIVE_OBJECTIVE_KEY.to_string(),
        state.effective_objective().to_string(),
    );
    metadata.insert(STEER_EPOCH_KEY.to_string(), state.steer_epoch().to_string());
    metadata.insert(
        CONTRACT_EPOCH_KEY.to_string(),
        state.contract_epoch().to_string(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_runtime::{PromptEvidenceScope, PromptToolRequirement};

    fn intent() -> PromptCompletionIntent {
        PromptCompletionIntent {
            evidence_scopes: [PromptEvidenceScope::Workspace].into_iter().collect(),
            tool_requirement: PromptToolRequirement::ReadOnly,
            target_anchors: Default::default(),
        }
    }

    #[test]
    fn parses_and_projects_the_existing_wire_fields_without_touching_other_metadata() {
        let run_context = [
            (
                EFFECTIVE_OBJECTIVE_KEY.to_string(),
                "inspect the workspace".to_string(),
            ),
            (STEER_EPOCH_KEY.to_string(), "7".to_string()),
            (CONTRACT_EPOCH_KEY.to_string(), "6".to_string()),
        ]
        .into_iter()
        .collect::<Metadata>();
        let completion_intent = intent();
        let state = prepared_task_state_from_legacy_metadata(
            &run_context,
            "latest prompt",
            completion_intent.clone(),
        );

        assert_eq!(state.effective_objective(), "inspect the workspace");
        assert_eq!(state.steer_epoch(), 7);
        assert_eq!(state.contract_epoch(), 6);
        assert_eq!(state.completion_intent(), &completion_intent);

        let mut projected = [("session_id".to_string(), "session-a".to_string())]
            .into_iter()
            .collect::<Metadata>();
        project_prepared_task_state_to_legacy_metadata(&state, &mut projected);

        assert_eq!(
            projected.get("session_id").map(String::as_str),
            Some("session-a")
        );
        assert_eq!(
            projected.get(EFFECTIVE_OBJECTIVE_KEY).map(String::as_str),
            Some("inspect the workspace")
        );
        assert_eq!(
            projected.get(STEER_EPOCH_KEY).map(String::as_str),
            Some("7")
        );
        assert_eq!(
            projected.get(CONTRACT_EPOCH_KEY).map(String::as_str),
            Some("6")
        );
    }

    #[test]
    fn missing_or_invalid_epochs_keep_the_legacy_fallbacks() {
        let missing = Metadata::new();
        let missing_state = prepared_task_state_from_legacy_metadata(
            &missing,
            "latest prompt",
            PromptCompletionIntent::default(),
        );
        assert_eq!(missing_state.effective_objective(), "latest prompt");
        assert_eq!(missing_state.steer_epoch(), 0);
        assert_eq!(missing_state.contract_epoch(), 0);

        let invalid = [
            (EFFECTIVE_OBJECTIVE_KEY.to_string(), "  ".to_string()),
            (STEER_EPOCH_KEY.to_string(), "invalid".to_string()),
            (CONTRACT_EPOCH_KEY.to_string(), "also-invalid".to_string()),
        ]
        .into_iter()
        .collect::<Metadata>();
        let invalid_state = prepared_task_state_from_legacy_metadata(
            &invalid,
            "fallback prompt",
            PromptCompletionIntent::default(),
        );
        assert_eq!(invalid_state.effective_objective(), "fallback prompt");
        assert_eq!(invalid_state.steer_epoch(), 0);
        assert_eq!(invalid_state.contract_epoch(), 0);
    }

    #[test]
    fn noop_steer_keeps_the_contract_epoch_independent() {
        let run_context = [
            (
                EFFECTIVE_OBJECTIVE_KEY.to_string(),
                "generate the requested image".to_string(),
            ),
            (STEER_EPOCH_KEY.to_string(), "4".to_string()),
            (CONTRACT_EPOCH_KEY.to_string(), "0".to_string()),
        ]
        .into_iter()
        .collect::<Metadata>();
        let state = prepared_task_state_from_legacy_metadata(
            &run_context,
            "unused",
            PromptCompletionIntent::default(),
        );

        assert_eq!(state.steer_epoch(), 4);
        assert_eq!(state.contract_epoch(), 0);

        let mut projected = Metadata::new();
        project_prepared_task_state_to_legacy_metadata(&state, &mut projected);
        assert_eq!(
            projected.get(STEER_EPOCH_KEY).map(String::as_str),
            Some("4")
        );
        assert_eq!(
            projected.get(CONTRACT_EPOCH_KEY).map(String::as_str),
            Some("0")
        );
    }
}
