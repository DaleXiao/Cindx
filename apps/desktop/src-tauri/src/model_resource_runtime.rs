use agent_core::LearningUsageCompleteness;
use agent_core::Metadata;
use agent_runtime::{
    estimate_request_tokens, estimate_text_tokens, AgentRunControl, ModelAttemptUsage,
    ModelUsageSource, PhysicalModelAttempt, RunResourceSnapshot, RunStageClass, RunStopReason,
    CONSERVATIVE_TOKENS_PER_PHYSICAL_MODEL_ATTEMPT,
};
use model_provider::{ModelError, ModelRequest, ModelResponse};
use std::sync::Arc;

#[must_use = "a reserved physical model attempt must be settled"]
pub(crate) struct ControlledModelAttempt {
    control: Arc<AgentRunControl>,
    epoch: Option<u64>,
    attempt: Option<PhysicalModelAttempt>,
    estimated_prompt_tokens: u64,
}

impl ControlledModelAttempt {
    pub(crate) fn reserve_at(
        control: &Arc<AgentRunControl>,
        epoch: u64,
        model: &str,
        request: &ModelRequest,
        stage: RunStageClass,
    ) -> Result<Option<Self>, RunStopReason> {
        let estimated_prompt_tokens = estimate_request_tokens(&request.messages, &request.tools);
        let max_completion_tokens = reserved_completion_tokens(request, estimated_prompt_tokens);
        control
            .begin_physical_model_attempt_at(
                epoch,
                model,
                estimated_prompt_tokens,
                max_completion_tokens,
                stage,
            )
            .map(|attempt| {
                attempt.map(|attempt| Self {
                    control: Arc::clone(control),
                    epoch: Some(epoch),
                    attempt: Some(attempt),
                    estimated_prompt_tokens,
                })
            })
    }

    pub(crate) fn reserve_embedding_at(
        control: &Arc<AgentRunControl>,
        epoch: Option<u64>,
        model: &str,
        inputs: &[String],
    ) -> Result<Option<Self>, RunStopReason> {
        let estimated_prompt_tokens = inputs.iter().fold(0_u64, |total, input| {
            total.saturating_add(estimate_text_tokens(input))
        });
        let reserved_completion_tokens =
            CONSERVATIVE_TOKENS_PER_PHYSICAL_MODEL_ATTEMPT.saturating_sub(estimated_prompt_tokens);
        let attempt = if let Some(epoch) = epoch {
            control.begin_physical_model_attempt_at(
                epoch,
                model,
                estimated_prompt_tokens,
                reserved_completion_tokens,
                RunStageClass::Other,
            )?
        } else {
            Some(control.begin_physical_model_attempt(
                model,
                estimated_prompt_tokens,
                reserved_completion_tokens,
                RunStageClass::Other,
            )?)
        };
        Ok(attempt.map(|attempt| Self {
            control: Arc::clone(control),
            epoch,
            attempt: Some(attempt),
            estimated_prompt_tokens,
        }))
    }

    pub(crate) fn settle_response(mut self, response: &ModelResponse) -> bool {
        self.settle(model_attempt_usage(response))
    }

    pub(crate) fn settle_estimated_input(mut self) -> bool {
        self.settle(Some(ModelAttemptUsage::new(
            self.estimated_prompt_tokens,
            0,
            self.estimated_prompt_tokens,
            ModelUsageSource::Estimated,
        )))
    }

    pub(crate) fn settle_unknown(mut self) -> bool {
        self.settle(None)
    }

    fn settle(&mut self, usage: Option<ModelAttemptUsage>) -> bool {
        let Some(attempt) = self.attempt.take() else {
            return false;
        };
        if let Some(epoch) = self.epoch {
            self.control
                .finish_physical_model_attempt_at(epoch, attempt, usage)
        } else {
            self.control.finish_physical_model_attempt(attempt, usage)
        }
    }
}

impl Drop for ControlledModelAttempt {
    fn drop(&mut self) {
        let _ = self.settle(None);
    }
}

/// The outcome of one auxiliary model call routed through the unified physical
/// resource ledger.
pub(crate) enum AuxModelCall {
    /// Dispatched and settled with the provider's usage.
    Response(ModelResponse),
    /// The provider call failed; the reserved attempt was settled as unknown.
    ProviderError,
    /// The run's physical resource budget is exhausted; the caller degrades
    /// gracefully (fallback summary, abandoned plan draft, bounded child answer).
    BudgetExhausted,
    /// The run stopped (cancel/steer); the caller stops.
    Stopped,
}

/// Run one auxiliary model call (Plan, Subagent, Summary, Judge, Guardian) on the
/// same physical resource ledger as the Owner: reserve a physical attempt against
/// the run budget, dispatch through the caller's closure, and settle with the
/// provider usage. The reservation is RAII-settled on every path, so a reserved
/// attempt is never leaked.
///
/// This closes audit P1-01 for model calls: aux calls previously counted only a
/// logical stage call (Plan/Subagent) or nothing at all (Summary), so
/// `max_total_tokens` and physical-attempt telemetry undercounted real cost and
/// parallel children could amplify provider calls for free under one parent
/// budget. The caller retains its logical `begin_stage_model_call` /
/// `finish_model_call` pair; this helper owns only the physical reservation and
/// settlement, keeping every aux lane on one accounting spine.
pub(crate) fn controlled_aux_model_call<F>(
    control: &Arc<AgentRunControl>,
    model: &str,
    request: &ModelRequest,
    stage: RunStageClass,
    dispatch: F,
) -> AuxModelCall
where
    F: FnOnce() -> Result<ModelResponse, ModelError>,
{
    let attempt = match ControlledModelAttempt::reserve_at(
        control,
        control.steer_epoch(),
        model,
        request,
        stage,
    ) {
        Ok(Some(attempt)) => attempt,
        Ok(None) => return AuxModelCall::BudgetExhausted,
        Err(_) => return AuxModelCall::Stopped,
    };
    match dispatch() {
        Ok(response) => {
            attempt.settle_response(&response);
            AuxModelCall::Response(response)
        }
        Err(_) => {
            attempt.settle_unknown();
            AuxModelCall::ProviderError
        }
    }
}

pub(crate) fn learning_usage_completeness(
    resources: &RunResourceSnapshot,
) -> LearningUsageCompleteness {
    let usage = &resources.lineage;
    if usage.physical_attempts == 0
        || usage.usage_sources.total() != usage.physical_attempts
        || usage.usage_sources.unknown > 0
    {
        LearningUsageCompleteness::Missing
    } else if usage.usage_sources.estimated > 0 || usage.usage_sources.provider_partial > 0 {
        LearningUsageCompleteness::Partial
    } else {
        LearningUsageCompleteness::Complete
    }
}

pub(crate) fn add_model_resource_metadata(metadata: &mut Metadata, control: &AgentRunControl) {
    let resources = control.resource_usage();
    add_model_resource_snapshot_metadata(metadata, &resources);
}

pub(crate) fn add_model_resource_snapshot_metadata(
    metadata: &mut Metadata,
    resources: &RunResourceSnapshot,
) {
    for (prefix, usage) in [
        ("run_segment", &resources.segment),
        ("run_lineage", &resources.lineage),
    ] {
        metadata.insert(
            format!("{prefix}_physical_model_attempts"),
            usage.physical_attempts.to_string(),
        );
        metadata.insert(
            format!("{prefix}_prompt_tokens"),
            usage.prompt_tokens.to_string(),
        );
        metadata.insert(
            format!("{prefix}_completion_tokens"),
            usage.completion_tokens.to_string(),
        );
        metadata.insert(
            format!("{prefix}_total_tokens"),
            usage.total_tokens.to_string(),
        );
        metadata.insert(
            format!("{prefix}_reserved_tokens"),
            usage.reserved_tokens.to_string(),
        );
        metadata.insert(
            format!("{prefix}_provider_usage_attempts"),
            usage.usage_sources.provider.to_string(),
        );
        metadata.insert(
            format!("{prefix}_partial_usage_attempts"),
            usage.usage_sources.provider_partial.to_string(),
        );
        metadata.insert(
            format!("{prefix}_estimated_usage_attempts"),
            usage.usage_sources.estimated.to_string(),
        );
        metadata.insert(
            format!("{prefix}_unknown_usage_attempts"),
            usage.usage_sources.unknown.to_string(),
        );
    }
}

fn reserved_completion_tokens(request: &ModelRequest, estimated_prompt_tokens: u64) -> u64 {
    let requested = request
        .metadata
        .get("max_output_tokens")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default();
    requested
        .max(CONSERVATIVE_TOKENS_PER_PHYSICAL_MODEL_ATTEMPT.saturating_sub(estimated_prompt_tokens))
}

fn model_attempt_usage(response: &ModelResponse) -> Option<ModelAttemptUsage> {
    let prompt_tokens = response.metadata.get("prompt_tokens")?.parse().ok()?;
    let completion_tokens = response.metadata.get("completion_tokens")?.parse().ok()?;
    let total_tokens = response.metadata.get("total_tokens")?.parse().ok()?;
    let source = match response.metadata.get("usage_source")?.as_str() {
        "provider" => ModelUsageSource::Provider,
        "provider_partial" => ModelUsageSource::ProviderPartial,
        "estimated" => ModelUsageSource::Estimated,
        _ => return None,
    };
    Some(ModelAttemptUsage::new(
        prompt_tokens,
        completion_tokens,
        total_tokens,
        source,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{Message, MessageRole, Metadata, ModelRole};
    use model_provider::ModelCallMode;

    fn model_request(max_output_tokens: Option<&str>) -> ModelRequest {
        let mut metadata = Metadata::new();
        if let Some(max_output_tokens) = max_output_tokens {
            metadata.insert(
                "max_output_tokens".to_string(),
                max_output_tokens.to_string(),
            );
        }
        ModelRequest {
            role: ModelRole::Executor,
            messages: vec![Message {
                role: MessageRole::User,
                content: "hello".to_string(),
                metadata: Metadata::new(),
            }],
            tools: Vec::new(),
            mode: ModelCallMode::Streaming,
            metadata,
        }
    }

    #[test]
    fn reservation_preserves_a_conservative_attempt_window() {
        let request = model_request(None);
        let prompt = estimate_request_tokens(&request.messages, &request.tools);
        assert_eq!(
            prompt.saturating_add(reserved_completion_tokens(&request, prompt)),
            CONSERVATIVE_TOKENS_PER_PHYSICAL_MODEL_ATTEMPT
        );

        let request = model_request(Some("2097152"));
        assert_eq!(reserved_completion_tokens(&request, prompt), 2_097_152);
    }

    #[test]
    fn response_usage_requires_complete_normalized_metadata() {
        let mut response = ModelResponse {
            message: Message {
                role: MessageRole::Assistant,
                content: "answer".to_string(),
                metadata: Metadata::new(),
            },
            raw_tool_calls_json: None,
            tool_calls: Vec::new(),
            metadata: Metadata::new(),
        };
        assert!(model_attempt_usage(&response).is_none());
        response
            .metadata
            .insert("prompt_tokens".to_string(), "10".to_string());
        response
            .metadata
            .insert("completion_tokens".to_string(), "3".to_string());
        response
            .metadata
            .insert("total_tokens".to_string(), "13".to_string());
        response
            .metadata
            .insert("usage_source".to_string(), "provider".to_string());
        assert_eq!(
            model_attempt_usage(&response),
            Some(ModelAttemptUsage::new(
                10,
                3,
                13,
                ModelUsageSource::Provider
            ))
        );
    }

    #[test]
    fn usage_completeness_does_not_treat_missing_telemetry_as_zero_cost() {
        let control = AgentRunControl::new("fast");
        assert_eq!(
            learning_usage_completeness(&control.resource_usage()),
            LearningUsageCompleteness::Missing
        );

        let request = model_request(Some("32"));
        let attempt = ControlledModelAttempt::reserve_at(
            &Arc::new(control),
            0,
            "model",
            &request,
            RunStageClass::Finalizer,
        )
        .unwrap()
        .unwrap();
        let control = Arc::clone(&attempt.control);
        let _ = attempt.settle_unknown();
        assert_eq!(
            learning_usage_completeness(&control.resource_usage()),
            LearningUsageCompleteness::Missing
        );
    }

    #[test]
    fn continuation_learning_uses_lineage_completeness() {
        let initial = AgentRunControl::new("fast");
        let unknown = initial
            .begin_physical_model_attempt("model-a", 12, 8, RunStageClass::Finalizer)
            .expect("initial attempt should reserve");
        assert!(initial.finish_physical_model_attempt(unknown, None));

        let continued = AgentRunControl::new_for_continuation_at_steer_epoch_with_resource_snapshot(
            "fast",
            0,
            initial.resource_usage(),
        );
        let complete = continued
            .begin_physical_model_attempt("model-b", 10, 5, RunStageClass::Finalizer)
            .expect("continued attempt should reserve");
        assert!(continued.finish_physical_model_attempt(
            complete,
            Some(ModelAttemptUsage::new(
                10,
                5,
                15,
                ModelUsageSource::Provider,
            )),
        ));

        assert_eq!(continued.resource_usage().segment.usage_sources.provider, 1);
        assert_eq!(continued.resource_usage().lineage.usage_sources.unknown, 1);
        assert_eq!(
            learning_usage_completeness(&continued.resource_usage()),
            LearningUsageCompleteness::Missing
        );
    }

    #[test]
    fn retrieval_embedding_and_final_answer_share_one_resource_ledger() {
        let control = Arc::new(AgentRunControl::new("auto"));
        control.begin_model_call("embedding").unwrap();
        let embedding = ControlledModelAttempt::reserve_embedding_at(
            &control,
            None,
            "embedding-model",
            &["retrieved context".to_string()],
        )
        .unwrap()
        .unwrap();
        assert!(embedding.settle_estimated_input());
        control.finish_model_call();

        control
            .begin_stage_model_call("rag_answer", RunStageClass::Finalizer)
            .unwrap();
        let request = model_request(Some("32"));
        let answer = ControlledModelAttempt::reserve_at(
            &control,
            control.steer_epoch(),
            "answer-model",
            &request,
            RunStageClass::Finalizer,
        )
        .unwrap()
        .unwrap();
        let mut response = ModelResponse {
            message: Message {
                role: MessageRole::Assistant,
                content: "answer".to_string(),
                metadata: Metadata::new(),
            },
            raw_tool_calls_json: None,
            tool_calls: Vec::new(),
            metadata: Metadata::new(),
        };
        response
            .metadata
            .insert("prompt_tokens".to_string(), "11".to_string());
        response
            .metadata
            .insert("completion_tokens".to_string(), "2".to_string());
        response
            .metadata
            .insert("total_tokens".to_string(), "13".to_string());
        response
            .metadata
            .insert("usage_source".to_string(), "provider".to_string());
        assert!(answer.settle_response(&response));
        control.finish_model_call();

        let usage = control.resource_usage().segment;
        assert_eq!(usage.physical_attempts, 2);
        assert_eq!(usage.usage_sources.estimated, 1);
        assert_eq!(usage.usage_sources.provider, 1);
        assert_eq!(usage.usage_sources.total(), 2);
    }

    #[test]
    fn aux_model_call_counts_on_the_shared_physical_ledger() {
        let control = Arc::new(AgentRunControl::new("default"));
        control
            .begin_stage_model_call("summary", RunStageClass::Other)
            .unwrap();
        let request = model_request(Some("32"));
        let outcome = controlled_aux_model_call(
            &control,
            "summarizer",
            &request,
            RunStageClass::Other,
            || {
                let mut response = ModelResponse {
                    message: Message {
                        role: MessageRole::Assistant,
                        content: "summary".to_string(),
                        metadata: Metadata::new(),
                    },
                    raw_tool_calls_json: None,
                    tool_calls: Vec::new(),
                    metadata: Metadata::new(),
                };
                response
                    .metadata
                    .insert("prompt_tokens".to_string(), "11".to_string());
                response
                    .metadata
                    .insert("completion_tokens".to_string(), "2".to_string());
                response
                    .metadata
                    .insert("total_tokens".to_string(), "13".to_string());
                response
                    .metadata
                    .insert("usage_source".to_string(), "provider".to_string());
                Ok(response)
            },
        );
        control.finish_model_call();
        assert!(matches!(outcome, AuxModelCall::Response(_)));
        let usage = control.resource_usage().segment;
        assert_eq!(usage.physical_attempts, 1, "aux call must count physically");
        assert_eq!(usage.total_tokens, 13);
        assert_eq!(usage.usage_sources.provider, 1);
    }

    #[test]
    fn aux_model_call_settles_unknown_on_provider_error_and_never_leaks() {
        let control = Arc::new(AgentRunControl::new("default"));
        control
            .begin_stage_model_call("subagent", RunStageClass::Worker)
            .unwrap();
        let request = model_request(Some("32"));
        let outcome = controlled_aux_model_call(
            &control,
            "subagent",
            &request,
            RunStageClass::Worker,
            || Err(ModelError::new("provider unavailable")),
        );
        control.finish_model_call();
        assert!(matches!(outcome, AuxModelCall::ProviderError));
        // The reserved attempt still settled (as unknown) rather than leaking.
        let usage = control.resource_usage().segment;
        assert_eq!(usage.physical_attempts, 1);
        assert_eq!(usage.usage_sources.unknown, 1);
    }
}
