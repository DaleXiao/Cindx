use crate::configuration_models::ProviderConfig;
use crate::provider_profiles::{provider_model_supports_tools, provider_model_supports_vision};
use model_provider::model_supports_vision_content;
use orchestrator::{ModelCandidate, ModelCapabilitySource};
use agent_core::ModelRole;

/// The configured model pool as capability candidates: the compatibility model,
/// the per-role models, and (when pinned) the effort-tier default model.
pub(crate) fn model_candidates_for_config(config: &ProviderConfig) -> Vec<ModelCandidate> {
    effort_model_candidates(config, "")
}

pub(crate) fn effort_model_candidates(
    config: &ProviderConfig,
    effort_label: &str,
) -> Vec<ModelCandidate> {
    let mut candidates = base_model_candidates_for_config(config);
    let pinned = config.effort_default_model(effort_label);
    if !pinned.is_empty() && !candidates.iter().any(|candidate| candidate.name == pinned) {
        let (cost_tier, latency_tier) = match effort_label {
            "fast" => (1, 1),
            "pro" => (3, 1),
            _ => (2, 1),
        };
        let name = pinned;
        let catalog_vision = provider_model_supports_vision(&config.provider_id, &name);
        let catalog_tools = provider_model_supports_tools(&config.provider_id, &name);
        let supports_vision =
            catalog_vision.unwrap_or_else(|| model_supports_vision_content(&name));
        let supports_tools = catalog_tools.unwrap_or(true);
        candidates.push(ModelCandidate {
            name,
            role: ModelRole::Executor,
            supports_tools,
            supports_vision,
            tools_capability_source: if catalog_tools.is_some() {
                ModelCapabilitySource::ProviderCatalog
            } else {
                ModelCapabilitySource::CompatibilityAssumption
            },
            vision_capability_source: if catalog_vision.is_some() {
                ModelCapabilitySource::ProviderCatalog
            } else {
                ModelCapabilitySource::CompatibilityAssumption
            },
            cost_tier,
            latency_tier,
        });
    }
    candidates
}

fn base_model_candidates_for_config(config: &ProviderConfig) -> Vec<ModelCandidate> {
    [
        (ModelRole::Executor, config.model.clone(), 1, 1),
        (
            ModelRole::Planner,
            config.model_for_role(&ModelRole::Planner),
            3,
            2,
        ),
        (
            ModelRole::Executor,
            config.model_for_role(&ModelRole::Executor),
            2,
            1,
        ),
        (
            ModelRole::Reviewer,
            config.model_for_role(&ModelRole::Reviewer),
            2,
            2,
        ),
        (
            ModelRole::Summarizer,
            config.model_for_role(&ModelRole::Summarizer),
            1,
            1,
        ),
    ]
    .into_iter()
    .map(|(role, name, cost_tier, latency_tier)| {
        let catalog_vision = provider_model_supports_vision(&config.provider_id, &name);
        let catalog_tools = provider_model_supports_tools(&config.provider_id, &name);
        let supports_vision =
            catalog_vision.unwrap_or_else(|| model_supports_vision_content(&name));
        let supports_tools = catalog_tools.unwrap_or(true);
        ModelCandidate {
            name,
            role,
            supports_tools,
            supports_vision,
            tools_capability_source: if catalog_tools.is_some() {
                ModelCapabilitySource::ProviderCatalog
            } else {
                ModelCapabilitySource::CompatibilityAssumption
            },
            vision_capability_source: if catalog_vision.is_some() {
                ModelCapabilitySource::ProviderCatalog
            } else {
                ModelCapabilitySource::CompatibilityAssumption
            },
            cost_tier,
            latency_tier,
        }
    })
    .collect()
}
