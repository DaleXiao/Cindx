//! Which model a delegated subagent runs on.
//!
//! A `task` delegation may name a model, but only one the user's own
//! configuration can serve; every other name falls back to the run's model. This
//! module owns that decision and the providers it implies, so the delegation
//! orchestrator neither judges a model name nor builds a provider.

use crate::configuration_models::ProviderConfig;
use agent_runtime::AgentRunControl;
use model_provider::OpenAiCompatibleProvider;
use std::collections::BTreeMap;
use std::sync::Arc;

/// The model one delegation runs on, and the model it asked for.
///
/// Both are kept because an unhonored request must stay visible: the durable
/// record names the model that actually served the child and the name that was
/// asked for, so a silent fallback is impossible to mistake for a choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubagentModelChoice {
    /// The model that will serve the child: its own choice when the run can honor
    /// it, otherwise the run's model.
    pub(crate) effective: String,
    /// The name the delegation asked for, empty when it asked for none.
    pub(crate) requested: String,
}

/// Resolve one delegation's model choice against the user's own configuration.
///
/// Only a model the user configured somewhere — the enabled catalog or a tier or
/// role pin — can serve a delegation, so a name the model invented never reaches
/// the provider. An unavailable name, an unreadable configuration, and no request
/// at all all land on the run's own model, which keeps the delegation running
/// instead of failing it over a model name.
pub(crate) fn subagent_model_choice(
    config: Option<&ProviderConfig>,
    run_model: &str,
    input_json: &str,
) -> SubagentModelChoice {
    let Some(requested) = subagent_requested_model(input_json) else {
        return SubagentModelChoice {
            effective: run_model.to_string(),
            requested: String::new(),
        };
    };
    let honored = config.is_some_and(|config| config.serves_model(&requested));
    SubagentModelChoice {
        effective: if honored {
            requested.clone()
        } else {
            run_model.to_string()
        },
        requested,
    }
}

/// The model name a delegation asked for, if any. A missing, non-string, or blank
/// value means "the run's own model".
fn subagent_requested_model(input_json: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(input_json)
        .ok()
        .and_then(|input| {
            input
                .get("model")
                .and_then(|value| value.as_str())
                .map(|value| value.trim().to_string())
        })
        .filter(|value| !value.is_empty())
}

/// One provider per distinct honored model choice, built before any child spawns
/// so children sharing a choice share a provider. A choice that lands on the run's
/// own model gets no entry: the parent's actor provider already serves it, so the
/// default path builds nothing and dispatches exactly as before.
pub(crate) fn subagent_model_providers(
    config: Option<&ProviderConfig>,
    choices: &[SubagentModelChoice],
    run_model: &str,
    cancellation: &Arc<AgentRunControl>,
) -> BTreeMap<String, OpenAiCompatibleProvider> {
    let mut providers = BTreeMap::new();
    let Some(config) = config else {
        // Nothing can be honored without the configuration, and every choice
        // already fell back to the run's model.
        return providers;
    };
    for choice in choices {
        if choice.effective == run_model || providers.contains_key(&choice.effective) {
            continue;
        }
        providers.insert(
            choice.effective.clone(),
            crate::agent_execution_provider_runtime::build_subagent_provider(
                config,
                &choice.effective,
                cancellation,
            ),
        );
    }
    providers
}
