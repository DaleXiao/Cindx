use super::*;

pub(crate) fn test_message(role: MessageRole, content: impl Into<String>) -> Message {
    Message {
        role,
        content: content.into(),
        metadata: Metadata::new(),
    }
}

pub(crate) fn provider_input_from_config(config: &ProviderConfig) -> ProviderConfigInput {
    ProviderConfigInput {
        provider_id: config.provider_id.clone(),
        provider_resource: config.provider_resource.clone(),
        base_url: config.base_url.clone(),
        api_key: String::new(),
        model: config.model.clone(),
        conductor_model: config.conductor_model.clone(),
        planner_model: config.planner_model.clone(),
        executor_model: config.executor_model.clone(),
        reviewer_model: config.reviewer_model.clone(),
        summarizer_model: config.summarizer_model.clone(),
        fast_model: config.fast_model.clone(),
        default_model: config.default_model.clone(),
        high_model: config.high_model.clone(),
        xhigh_model: config.xhigh_model.clone(),
        embedding_model: config.embedding_model.clone(),
        image_model: config.image_model.clone(),
        image_endpoint: config.image_endpoint.clone(),
        voice_model: config.voice_model.clone(),
        collaboration_policy: config.collaboration_policy.clone(),
        direct_judge_fail_closed: config.direct_judge_fail_closed,
        guardian_auto_approval: config.guardian_auto_approval,
        plan_first_enabled: config.plan_first_enabled,
        approval_policy: config.approval_policy.clone(),
        context_window_tokens: config.context_window_tokens,
        agent_system_prompt: config.agent_system_prompt.clone(),
        enabled_models: config.enabled_models.clone(),
    }
}

pub(crate) fn temp_test_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{name}-{}", unique_id("test")))
}
