use crate::configuration_models::ProviderConfig;
use agent_core::ModelRole;
use agent_runtime::{AgentRunControl, RunStageClass};
use model_provider::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};

pub(super) struct AgentExecutionProviders {
    pub(super) actor: OpenAiCompatibleProvider,
    pub(super) finalizer: OpenAiCompatibleProvider,
}

pub(super) fn build_agent_execution_providers(
    config: &ProviderConfig,
    agent_model: &str,
    cancellation: &AgentRunControl,
) -> AgentExecutionProviders {
    AgentExecutionProviders {
        actor: build_provider(config, agent_model, cancellation, RunStageClass::Actor),
        finalizer: build_provider(config, agent_model, cancellation, RunStageClass::Finalizer),
    }
}

fn build_provider(
    config: &ProviderConfig,
    agent_model: &str,
    cancellation: &AgentRunControl,
    stage: RunStageClass,
) -> OpenAiCompatibleProvider {
    OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: config.base_url.clone(),
        api_key: config.api_key.clone(),
        model: agent_model.to_string(),
        embedding_model: config.model_for_role(&ModelRole::Embedder),
        timeout_seconds: agent_provider_timeout_seconds(cancellation, stage),
    })
}

fn agent_provider_timeout_seconds(cancellation: &AgentRunControl, stage: RunStageClass) -> u64 {
    cancellation.stage_model_call_timeout_seconds(stage)
}

/// One provider serving a delegation's own model choice, with the timeout
/// allowance of the stage a subagent actually charges to (Worker) rather than the
/// parent's Actor stage. The parent's actor provider still serves every delegation
/// that did not ask for a model, so the default path is unchanged.
pub(super) fn build_subagent_provider(
    config: &ProviderConfig,
    model: &str,
    cancellation: &AgentRunControl,
) -> OpenAiCompatibleProvider {
    build_provider(config, model, cancellation, RunStageClass::Worker)
}

#[cfg(test)]
mod tests {
    use super::agent_provider_timeout_seconds;
    use agent_runtime::{AgentRunControl, RunStageClass};

    #[test]
    fn actor_provider_keeps_its_stage_allowance_and_finalizer_reserve() {
        for effort in ["fast", "auto", "pro"] {
            let control = AgentRunControl::new(effort);
            let budget = control.budget();
            let actor_timeout = agent_provider_timeout_seconds(&control, RunStageClass::Actor);
            let finalizer_timeout =
                agent_provider_timeout_seconds(&control, RunStageClass::Finalizer);

            let actor_allowance = budget
                .model_call_timeout
                .min(
                    budget
                        .max_duration
                        .saturating_sub(budget.protected_time_reserve(RunStageClass::Actor)),
                )
                .as_secs();
            assert!(actor_timeout <= actor_allowance);
            assert!(actor_timeout.saturating_add(1) >= actor_allowance);

            assert_eq!(
                finalizer_timeout,
                budget
                    .model_call_timeout
                    .min(budget.finalizer_time_reserve())
                    .as_secs()
            );
            assert_eq!(
                budget.protected_time_reserve(RunStageClass::Actor),
                budget.terminal_time_reserve
            );
        }
    }
}
