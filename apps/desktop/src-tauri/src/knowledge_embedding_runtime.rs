use crate::desktop_prelude::*;
use crate::{agent_query_commands::agent_run_should_stop, configuration_models::ProviderConfig};

pub(crate) struct CloudRagEmbedder<'a> {
    pub(crate) config: ProviderConfig,
    pub(crate) cancellation: Option<Arc<AgentRunControl>>,
    pub(crate) expected_steer_epoch: Option<u64>,
    pub(crate) resource_checkpoint:
        Option<&'a (dyn Fn(&AgentRunControl) -> Result<(), String> + Sync)>,
}

impl RagEmbedder for CloudRagEmbedder<'_> {
    fn embed_texts(&mut self, texts: &[String]) -> Result<EmbeddingBatch, agent_rag::RagError> {
        if self.cancellation.as_ref().is_some_and(|control| {
            self.expected_steer_epoch
                .is_some_and(|expected| !control.preparation_epoch_is_current(expected))
        }) {
            return Err(agent_rag::RagError::new(MODEL_REQUEST_CANCELLED));
        }
        if let Some(control) = self.cancellation.as_ref() {
            let model_call = match self.expected_steer_epoch {
                Some(expected_epoch) => control.begin_model_call_at(expected_epoch, "embedding"),
                None => control.begin_model_call("embedding").map(Some),
            };
            match model_call {
                Ok(Some(_)) => {}
                Ok(None) => return Err(agent_rag::RagError::new(MODEL_REQUEST_CANCELLED)),
                Err(reason) => {
                    return Err(agent_rag::RagError::new(format!(
                        "Run stopped before embedding call: {}",
                        reason.code()
                    )))
                }
            }
        }
        let model = self.config.model_for_role(&ModelRole::Embedder);
        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: self.config.base_url.clone(),
            api_key: self.config.api_key.clone(),
            model: self.config.model_for_role(&ModelRole::Executor),
            embedding_model: model.clone(),
            timeout_seconds: self
                .cancellation
                .as_ref()
                .map(|control| control.model_call_timeout_seconds())
                .unwrap_or(180),
        });
        let model_attempt = if let Some(control) = self.cancellation.as_ref() {
            match crate::model_resource_runtime::ControlledModelAttempt::reserve_embedding_at(
                control,
                self.expected_steer_epoch,
                &model,
                texts,
            ) {
                Ok(Some(attempt)) => Some(attempt),
                Ok(None) => {
                    if let Some(expected_epoch) = self.expected_steer_epoch {
                        control.finish_model_call_at(expected_epoch);
                    } else {
                        control.finish_model_call();
                    }
                    return Err(agent_rag::RagError::new(MODEL_REQUEST_CANCELLED));
                }
                Err(reason) => {
                    if let Some(expected_epoch) = self.expected_steer_epoch {
                        control.finish_model_call_at(expected_epoch);
                    } else {
                        control.finish_model_call();
                    }
                    return Err(agent_rag::RagError::new(format!(
                        "Run stopped before embedding dispatch: {}",
                        reason.code()
                    )));
                }
            }
        } else {
            None
        };
        if let (Some(control), Some(checkpoint)) =
            (self.cancellation.as_ref(), self.resource_checkpoint)
        {
            if let Err(error) = checkpoint(control) {
                if let Some(attempt) = model_attempt {
                    let _ = attempt.settle_unknown();
                }
                if let Some(expected_epoch) = self.expected_steer_epoch {
                    control.finish_model_call_at(expected_epoch);
                } else {
                    control.finish_model_call();
                }
                return Err(agent_rag::RagError::new(format!(
                    "embedding resource checkpoint failed before provider dispatch: {error}"
                )));
            }
        }
        let response = provider.embed_cancellable(
            EmbeddingRequest {
                input: texts.to_vec(),
                dimensions: None,
                metadata: Metadata::new(),
            },
            || {
                self.cancellation.as_ref().is_some_and(|control| {
                    agent_run_should_stop(control)
                        || self
                            .expected_steer_epoch
                            .is_some_and(|expected| !control.preparation_epoch_is_current(expected))
                })
            },
        );
        if let Some(attempt) = model_attempt {
            if response.is_ok() {
                let _ = attempt.settle_estimated_input();
            } else {
                let _ = attempt.settle_unknown();
            }
        }
        if let Some(control) = self.cancellation.as_ref() {
            if let Some(checkpoint) = self.resource_checkpoint {
                if let Err(error) = checkpoint(control) {
                    eprintln!("embedding resource settlement checkpoint unavailable: {error}");
                }
            }
            if let Some(expected_epoch) = self.expected_steer_epoch {
                control.finish_model_call_at(expected_epoch);
            } else {
                control.finish_model_call();
            }
        }
        let response = response.map_err(|error| agent_rag::RagError::new(error.to_string()))?;
        if let Some(control) = self.cancellation.as_ref() {
            let detail = format!("Embedded {} items", texts.len());
            if let Some(expected_epoch) = self.expected_steer_epoch {
                control.mark_progress_at(expected_epoch, "embedding", &detail);
            } else {
                control.mark_progress("embedding", &detail);
            }
        }

        Ok(EmbeddingBatch {
            provider: response
                .metadata
                .get("provider")
                .cloned()
                .unwrap_or_else(|| "openai-compatible".to_string()),
            model: response.model,
            vectors: response
                .vectors
                .into_iter()
                .map(|vector| vector.embedding)
                .collect(),
        })
    }
}
