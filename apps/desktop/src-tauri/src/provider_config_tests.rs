use super::test_support::provider_input_from_config;
use super::*;

#[test]
fn provider_config_input_preserves_existing_key_when_blank() {
    let mut config = ProviderConfig {
        provider_id: PROVIDER_CUSTOM.to_string(),
        base_url: "https://example.test/v1".to_string(),
        api_key: "existing".to_string(),
        image_endpoint: "https://images.example.test/v1".to_string(),
        ..ProviderConfig::default()
    };

    apply_provider_config_input(
        &mut config,
        ProviderConfigInput {
            provider_id: "custom".to_string(),
            provider_resource: "".to_string(),
            base_url: "https://example.test/v1".to_string(),
            api_key: "".to_string(),
            model: "model-a".to_string(),
            conductor_model: "".to_string(),
            planner_model: "".to_string(),
            executor_model: "".to_string(),
            reviewer_model: "".to_string(),
            summarizer_model: "".to_string(),
            fast_model: "".to_string(),
            auto_model: "".to_string(),
            pro_model: "".to_string(),
            embedding_model: "".to_string(),
            image_model: "image-model-a".to_string(),
            image_endpoint: "https://images.example.test/v1".to_string(),
            voice_model: "gpt-realtime".to_string(),
            collaboration_policy: "auto_router".to_string(),
            direct_judge_fail_closed: false,
            guardian_auto_approval: false,
            plan_first_enabled: false,
            approval_policy: "strict".to_string(),
            context_window_tokens: 128_000,
            agent_system_prompt: "Be concise.\nUse Chinese when asked.".to_string(),
            enabled_models: Vec::new(),
        },
    );

    assert_eq!(config.api_key, "existing");
    assert_eq!(config.model_for_conductor(), "model-a");
    assert_eq!(config.executor_model, "model-a");
    assert_eq!(config.collaboration_policy, "auto_router");
    assert_eq!(config.context_window_tokens, 128_000);
    assert_eq!(
        config.agent_system_prompt,
        "Be concise.\nUse Chinese when asked."
    );
    assert_eq!(config.image_model, "image-model-a");
    assert_eq!(config.image_endpoint, "https://images.example.test/v1");
    assert_eq!(config.voice_model, "gpt-realtime");
}

#[test]
fn direct_judge_fail_closed_config_round_trip_defaults_off() {
    // Legacy configs predate the key and must load fail-open.
    let legacy = provider_config_from_text("base_url=https://example.test/v1\nmodel=base\n");
    assert!(!legacy.direct_judge_fail_closed);
    assert!(provider_config_text(&legacy).contains("direct_judge_fail_closed=false"));

    let enabled = provider_config_from_text(
        "base_url=https://example.test/v1\ndirect_judge_fail_closed=true\n",
    );
    assert!(enabled.direct_judge_fail_closed);
    let reloaded = provider_config_from_text(&provider_config_text(&enabled));
    assert!(reloaded.direct_judge_fail_closed);

    let mut config = ProviderConfig::default();
    assert!(!config.direct_judge_fail_closed);
    let mut input = provider_input_from_config(&config);
    input.direct_judge_fail_closed = true;
    apply_provider_config_input(&mut config, input);
    assert!(config.direct_judge_fail_closed);
}

#[test]
fn guardian_auto_approval_config_round_trip_defaults_off() {
    // Legacy configs predate the key and must load with the guardian off, so
    // every consequential action keeps prompting the user.
    let legacy = provider_config_from_text("base_url=https://example.test/v1\nmodel=base\n");
    assert!(!legacy.guardian_auto_approval);
    assert!(provider_config_text(&legacy).contains("guardian_auto_approval=false"));

    let enabled = provider_config_from_text(
        "base_url=https://example.test/v1\nguardian_auto_approval=true\n",
    );
    assert!(enabled.guardian_auto_approval);
    let reloaded = provider_config_from_text(&provider_config_text(&enabled));
    assert!(reloaded.guardian_auto_approval);

    let mut config = ProviderConfig::default();
    assert!(!config.guardian_auto_approval);
    let mut input = provider_input_from_config(&config);
    input.guardian_auto_approval = true;
    apply_provider_config_input(&mut config, input);
    assert!(config.guardian_auto_approval);
}

#[test]
fn plan_first_enabled_config_round_trip_defaults_off() {
    // Legacy configs predate the key and must load with plan mode off.
    let legacy = provider_config_from_text("base_url=https://example.test/v1\nmodel=base\n");
    assert!(!legacy.plan_first_enabled);
    assert!(provider_config_text(&legacy).contains("plan_first_enabled=false"));

    let enabled = provider_config_from_text(
        "base_url=https://example.test/v1\nplan_first_enabled=true\n",
    );
    assert!(enabled.plan_first_enabled);
    let reloaded = provider_config_from_text(&provider_config_text(&enabled));
    assert!(reloaded.plan_first_enabled);

    let mut config = ProviderConfig::default();
    assert!(!config.plan_first_enabled);
    let mut input = provider_input_from_config(&config);
    input.plan_first_enabled = true;
    apply_provider_config_input(&mut config, input);
    assert!(config.plan_first_enabled);
}

#[test]
fn approval_policy_config_round_trip_defaults_strict() {
    // Legacy configs predate the key and must load strict, so every
    // permission request keeps prompting the user.
    let legacy = provider_config_from_text("base_url=https://example.test/v1\nmodel=base\n");
    assert_eq!(legacy.approval_policy, "strict");
    assert!(provider_config_text(&legacy).contains("approval_policy=strict"));

    for policy in ["session", "all"] {
        let loaded = provider_config_from_text(&format!(
            "base_url=https://example.test/v1\napproval_policy={policy}\n",
        ));
        assert_eq!(loaded.approval_policy, policy);
        let reloaded = provider_config_from_text(&provider_config_text(&loaded));
        assert_eq!(reloaded.approval_policy, policy);
    }

    // Unknown or malformed persisted values fail closed to strict.
    for value in ["always", "SESSION", "", "auto"] {
        let fallback = provider_config_from_text(&format!(
            "base_url=https://example.test/v1\napproval_policy={value}\n",
        ));
        assert_eq!(fallback.approval_policy, "strict", "value {value:?}");
    }

    let mut config = ProviderConfig::default();
    assert_eq!(config.approval_policy, "strict");
    let mut input = provider_input_from_config(&config);
    input.approval_policy = "session".to_string();
    apply_provider_config_input(&mut config, input);
    assert_eq!(config.approval_policy, "session");

    // An unknown input value fails closed instead of widening authority.
    let mut input = provider_input_from_config(&config);
    input.approval_policy = "everything".to_string();
    apply_provider_config_input(&mut config, input);
    assert_eq!(config.approval_policy, "strict");
}

#[test]
fn provider_profiles_resolve_fixed_and_resource_scoped_endpoints() {
    let openai = resolve_provider_profile(
        PROVIDER_OPENAI,
        "ignored",
        "https://ignored.example/v1",
        "https://ignored.example/images",
    );
    assert_eq!(openai.provider_id, PROVIDER_OPENAI);
    assert!(openai.provider_resource.is_empty());
    assert_eq!(openai.base_url, "https://api.openai.com/v1");
    assert!(openai.image_endpoint.is_empty());

    let azure = resolve_provider_profile(
        PROVIDER_AZURE_OPENAI,
        "Team-East",
        "",
        "https://ignored.example/images",
    );
    assert_eq!(azure.provider_resource, "team-east");
    assert_eq!(
        azure.base_url,
        "https://team-east.openai.azure.com/openai/v1"
    );
    assert!(azure.image_endpoint.is_empty());

    let azure_services = resolve_provider_profile(
        PROVIDER_AZURE_OPENAI,
        "Team-East",
        "https://team-east.services.ai.azure.com/openai/v1",
        "",
    );
    assert_eq!(
        azure_services.base_url,
        "https://team-east.services.ai.azure.com/openai/v1"
    );

    let alibaba_shared = resolve_provider_profile(
        PROVIDER_ALIBABA_CN,
        "",
        "https://old-workspace.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
        "",
    );
    assert_eq!(
        alibaba_shared.base_url,
        "https://dashscope.aliyuncs.com/compatible-mode/v1"
    );
    assert_eq!(
        alibaba_shared.image_endpoint,
        "https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation"
    );

    let alibaba_workspace = resolve_provider_profile(PROVIDER_ALIBABA_CN, "WS-123", "", "");
    assert!(alibaba_workspace.provider_resource.is_empty());
    assert_eq!(
        alibaba_workspace.base_url,
        "https://dashscope.aliyuncs.com/compatible-mode/v1"
    );
    assert_eq!(
        alibaba_workspace.image_endpoint,
        "https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation"
    );

    let max_resource = "a".repeat(63);
    let valid_boundary = resolve_provider_profile(PROVIDER_AZURE_OPENAI, &max_resource, "", "");
    assert_eq!(
        valid_boundary.base_url,
        format!("https://{max_resource}.openai.azure.com/openai/v1")
    );
    let invalid_boundary = resolve_provider_profile(PROVIDER_AZURE_OPENAI, &"a".repeat(64), "", "");
    assert!(invalid_boundary.base_url.is_empty());
}

#[test]
fn effort_default_models_parse_and_anchor_per_tier() {
    let config = provider_config_from_text(
        "base_url=https://example.test/v1\napi_key=secret\nmodel=base-model\nfast_model=fast-model\nauto_model=auto-model\npro_model=pro-model\n",
    );
    assert_eq!(config.fast_model, "fast-model");
    assert_eq!(config.auto_model, "auto-model");
    assert_eq!(config.pro_model, "pro-model");
    assert_eq!(config.effort_default_model("fast"), "fast-model");
    assert_eq!(config.effort_default_model("auto"), "auto-model");
    assert_eq!(config.effort_default_model("pro"), "pro-model");
    assert_eq!(config.effort_default_model("other"), "");

    let empty = provider_config_from_text("base_url=https://example.test/v1\nmodel=base-model\n");
    assert_eq!(empty.effort_default_model("auto"), "");
    assert_eq!(empty.model, "base-model");

    let dashscope = provider_config_from_text(
        "base_url=https://dashscope.aliyuncs.com/compatible-mode/v1\napi_key=secret\nmodel=qwen3.7-plus\n",
    );
    assert_eq!(dashscope.provider_id, PROVIDER_ALIBABA_CN);
    assert_eq!(dashscope.effort_default_model("fast"), "qwen3.7-flash");
    assert_eq!(dashscope.effort_default_model("auto"), "qwen3.7-plus");
    assert_eq!(dashscope.effort_default_model("pro"), "qwen3.7-max");
    assert_eq!(dashscope.effort_default_model("other"), "");

    let openai = provider_config_from_text("base_url=https://api.openai.com/v1\napi_key=secret\n");
    assert_eq!(openai.provider_id, PROVIDER_OPENAI);
    assert_eq!(openai.effort_default_model("fast"), "gpt-4.1-mini");
    assert_eq!(openai.effort_default_model("auto"), "gpt-4.1");
    assert_eq!(openai.effort_default_model("pro"), "gpt-4.1");

    let pinned = effort_model_candidates(&config, "auto");
    assert!(pinned
        .iter()
        .any(|candidate| candidate.name == "auto-model"));

    let candidates = model_candidates_for_config(&config);
    assert!(!candidates
        .iter()
        .any(|candidate| candidate.name == "auto-model"));

    let text = provider_config_text(&config);
    assert!(text.contains("fast_model=fast-model"));
    assert!(text.contains("auto_model=auto-model"));
    assert!(text.contains("pro_model=pro-model"));
}

#[test]
fn legacy_provider_inference_uses_exact_hosts_and_preserves_custom_urls() {
    let openai = provider_config_from_text("base_url=https://api.openai.com/v1\n");
    assert_eq!(openai.provider_id, PROVIDER_OPENAI);
    assert_eq!(openai.base_url, "https://api.openai.com/v1");

    let azure = provider_config_from_text(
        "base_url=https://legacy-east.openai.azure.com/openai/v1\napi_key=secret\n",
    );
    assert_eq!(azure.provider_id, PROVIDER_AZURE_OPENAI);
    assert_eq!(azure.provider_resource, "legacy-east");
    assert_eq!(
        azure.base_url,
        "https://legacy-east.openai.azure.com/openai/v1"
    );
    assert!(!azure.supports_webrtc_voice());

    let alibaba = provider_config_from_text(
        "base_url=https://workspace-9.cn-beijing.maas.aliyuncs.com/compatible-mode/v1\n",
    );
    assert_eq!(alibaba.provider_id, PROVIDER_CUSTOM);
    assert!(alibaba.provider_resource.is_empty());
    assert_eq!(
        alibaba.base_url,
        "https://workspace-9.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"
    );
    assert!(!alibaba.supports_webrtc_voice());

    let explicit_legacy_workspace = provider_config_from_text(
        "provider_id=alibaba_cn\n\
         provider_resource=workspace-id\n\
         base_url=https://workspace-9.cn-beijing.maas.aliyuncs.com/compatible-mode/v1\n",
    );
    assert_eq!(explicit_legacy_workspace.provider_id, PROVIDER_CUSTOM);
    assert!(explicit_legacy_workspace.provider_resource.is_empty());
    assert_eq!(
        explicit_legacy_workspace.base_url,
        "https://workspace-9.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"
    );

    let alibaba_shared =
        provider_config_from_text("base_url=https://dashscope.aliyuncs.com/compatible-mode/v1\n");
    assert_eq!(alibaba_shared.provider_id, PROVIDER_ALIBABA_CN);
    assert!(alibaba_shared.provider_resource.is_empty());

    let malicious = "https://api.openai.com.evil.test/custom/path?mode=1";
    let custom_image = "https://images.evil.test/private/generate";
    let custom = provider_config_from_text(&format!(
        "base_url={malicious}\nimage_endpoint={custom_image}\n"
    ));
    assert_eq!(custom.provider_id, PROVIDER_CUSTOM);
    assert_eq!(custom.base_url, malicious);
    assert_eq!(custom.image_endpoint, custom_image);
    assert!(custom.supports_webrtc_voice());
}

#[test]
fn legacy_provider_inference_preserves_independent_image_overrides() {
    let custom_image = "https://images.example.test/private/generate";
    let migrated = provider_config_from_text(&format!(
        "base_url=https://api.openai.com/v1\nimage_endpoint={custom_image}\n"
    ));
    assert_eq!(migrated.provider_id, PROVIDER_CUSTOM);
    assert_eq!(migrated.base_url, "https://api.openai.com/v1");
    assert_eq!(migrated.image_endpoint, custom_image);

    let automatic = provider_config_from_text(
        "base_url=https://dashscope.aliyuncs.com/compatible-mode/v1\n\
         image_endpoint=https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation\n",
    );
    assert_eq!(automatic.provider_id, PROVIDER_ALIBABA_CN);
}

#[test]
fn blank_api_keys_are_reused_only_for_the_same_provider_identity() {
    let mut config = ProviderConfig {
        provider_id: PROVIDER_CUSTOM.to_string(),
        base_url: "https://gateway.example/v1".to_string(),
        api_key: "existing".to_string(),
        ..ProviderConfig::default()
    };
    let mut same_origin = provider_input_from_config(&config);
    same_origin.base_url = "https://gateway.example/openai/v1".to_string();
    apply_provider_config_input(&mut config, same_origin);
    assert_eq!(config.api_key, "existing");

    let mut other_origin = provider_input_from_config(&config);
    other_origin.base_url = "https://other.example/v1".to_string();
    apply_provider_config_input(&mut config, other_origin);
    assert!(config.api_key.is_empty());

    config.provider_id = PROVIDER_AZURE_OPENAI.to_string();
    config.provider_resource = "resource-a".to_string();
    config.base_url = "https://resource-a.openai.azure.com/openai/v1".to_string();
    config.api_key = "azure-key".to_string();
    let mut other_resource = provider_input_from_config(&config);
    other_resource.provider_resource = "resource-b".to_string();
    apply_provider_config_input(&mut config, other_resource);
    assert!(config.api_key.is_empty());

    let mut openai = ProviderConfig {
        api_key: "openai-key".to_string(),
        ..ProviderConfig::default()
    };
    let mut alibaba = provider_input_from_config(&openai);
    alibaba.provider_id = PROVIDER_ALIBABA_CN.to_string();
    alibaba.base_url.clear();
    apply_provider_config_input(&mut openai, alibaba);
    assert_eq!(openai.provider_id, PROVIDER_ALIBABA_CN);
    assert!(openai.api_key.is_empty());
}

#[test]
fn model_list_reuses_saved_key_only_for_the_same_draft_identity() {
    let saved = ProviderConfig {
        provider_id: PROVIDER_CUSTOM.to_string(),
        base_url: "https://gateway.example/v1".to_string(),
        api_key: "saved-key".to_string(),
        ..ProviderConfig::default()
    };
    let same_origin = provider_config_for_model_list(
        &saved,
        &ProviderModelsInput {
            provider_id: PROVIDER_CUSTOM.to_string(),
            provider_resource: String::new(),
            base_url: "https://gateway.example/openai/v1".to_string(),
            api_key: String::new(),
        },
    );
    assert_eq!(same_origin.api_key, "saved-key");

    let other_origin = provider_config_for_model_list(
        &saved,
        &ProviderModelsInput {
            provider_id: PROVIDER_CUSTOM.to_string(),
            provider_resource: String::new(),
            base_url: "https://other.example/v1".to_string(),
            api_key: String::new(),
        },
    );
    assert!(other_origin.api_key.is_empty());

    let malicious = provider_config_for_model_list(
        &ProviderConfig {
            api_key: "openai-key".to_string(),
            ..ProviderConfig::default()
        },
        &ProviderModelsInput {
            provider_id: String::new(),
            provider_resource: String::new(),
            base_url: "https://api.openai.com.evil.test/v1".to_string(),
            api_key: String::new(),
        },
    );
    assert_eq!(malicious.base_url, "https://api.openai.com.evil.test/v1");
    assert!(malicious.api_key.is_empty());
}

#[test]
fn openai_voice_model_is_built_in_and_can_still_be_overridden() {
    let mut config = provider_config_from_text(
        "base_url=https://api.openai.com/v1\napi_key=secret\nexecutor_model=model-a\n",
    );
    assert!(config.is_ready());
    assert!(config.voice_is_ready());
    assert_eq!(config.voice_model, "gpt-realtime-2.1");

    config = provider_config_from_text(
        "base_url=https://api.openai.com/v1\napi_key=secret\nexecutor_model=model-a\nvoice_model=gpt-realtime\n",
    );
    assert!(config.is_ready());
    assert!(config.voice_is_ready());
    assert_eq!(config.voice_model, "gpt-realtime");

    config.provider_id = PROVIDER_ALIBABA_CN.to_string();
    config.base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string();
    config.voice_model = "qwen3.5-omni-flash-realtime".to_string();
    assert!(config.voice_is_ready());
    assert_eq!(
        config.voice_transport(),
        ProviderVoiceTransport::DashScopeWebSocket
    );
}

#[test]
fn custom_provider_voice_gating_uses_exact_known_hosts() {
    for base_url in [
        "https://team.openai.azure.com/openai/v1",
        "https://team.services.ai.azure.com/openai/v1",
        "https://dashscope.aliyuncs.com/compatible-mode/v1",
        "https://workspace.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
    ] {
        let config = provider_config_from_text(&format!(
            "provider_id=custom\nbase_url={base_url}\nvoice_model=realtime-model\n"
        ));
        assert_eq!(config.provider_id, PROVIDER_CUSTOM);
        assert!(!config.supports_webrtc_voice(), "{base_url}");
    }

    for base_url in [
        "https://gateway.example.test/v1",
        "https://team.openai.azure.com.evil.test/v1",
        "https://team.services.ai.azure.com.evil.test/v1",
        "https://dashscope.aliyuncs.com.evil.test/v1",
        "https://workspace.cn-beijing.maas.aliyuncs.com.evil.test/v1",
    ] {
        let config = provider_config_from_text(&format!(
            "provider_id=custom\nbase_url={base_url}\nvoice_model=realtime-model\n"
        ));
        assert!(config.supports_webrtc_voice(), "{base_url}");
    }

    let legacy_azure_services = provider_config_from_text(
        "base_url=https://legacy.services.ai.azure.com/openai/v1\nvoice_model=realtime-model\n",
    );
    assert_eq!(legacy_azure_services.provider_id, PROVIDER_CUSTOM);
    assert!(!legacy_azure_services.supports_webrtc_voice());
}

#[test]
fn legacy_provider_config_inherits_conductor_from_planner() {
    let migrated = provider_config_from_text(
        "model=default-a\nplanner_model=planner-b\nexecutor_model=executor-c\n",
    );
    assert_eq!(migrated.model_for_conductor(), "planner-b");

    let explicit = provider_config_from_text(
        "model=default-a\nconductor_model=conductor-z\nplanner_model=planner-b\n",
    );
    assert_eq!(explicit.model_for_conductor(), "conductor-z");
}

#[test]
fn dashscope_provider_migrates_the_openai_embedding_default() {
    let migrated = provider_config_from_text(
        "base_url=https://dashscope.aliyuncs.com/compatible-mode/v1\n\
             model=qwen-plus\n\
             embedding_model=text-embedding-3-small\n",
    );
    assert_eq!(
        migrated.model_for_role(&ModelRole::Embedder),
        DASHSCOPE_DEFAULT_EMBEDDING_MODEL
    );

    let explicit = provider_config_from_text(
        "base_url=https://dashscope.aliyuncs.com/compatible-mode/v1\n\
             embedding_model=custom-embedding-model\n",
    );
    assert_eq!(
        explicit.model_for_role(&ModelRole::Embedder),
        "custom-embedding-model"
    );

    let workspace = provider_config_from_text(
        "base_url=https://workspace-9.cn-beijing.maas.aliyuncs.com/compatible-mode/v1\n\
             embedding_model=text-embedding-3-small\n",
    );
    assert_eq!(
        workspace.model_for_role(&ModelRole::Embedder),
        "text-embedding-3-small"
    );

    let openai = ProviderConfig::default();
    assert_eq!(
        openai.model_for_role(&ModelRole::Embedder),
        "text-embedding-3-large"
    );
}

#[test]
fn provider_catalog_supplies_complete_defaults_and_explicit_vision_capabilities() {
    let openai = provider_model_defaults(PROVIDER_OPENAI).expect("OpenAI preset");
    assert_eq!(openai.chat, "gpt-4.1");
    assert_eq!(openai.embedding, "text-embedding-3-large");
    assert_eq!(openai.image, "gpt-image-2");
    assert_eq!(openai.voice, "gpt-realtime-2.1");
    assert_eq!(openai.context_window_tokens, 1_047_576);
    assert!(provider_supports_model_discovery(PROVIDER_OPENAI));
    assert!(provider_model_supports_tools(PROVIDER_OPENAI, "gpt-4.1").unwrap());
    assert_eq!(
        provider_model_supports_vision(PROVIDER_OPENAI, "gpt-4.1"),
        Some(true)
    );

    let alibaba = provider_model_defaults(PROVIDER_ALIBABA_CN).expect("Alibaba preset");
    assert_eq!(alibaba.chat, "qwen3.7-plus");
    assert_eq!(alibaba.embedding, "text-embedding-v4");
    assert_eq!(alibaba.image, "qwen-image-3.0-pro");
    assert_eq!(alibaba.voice, "qwen3-asr-flash-realtime");
    assert_eq!(alibaba.context_window_tokens, 1_000_000);
    assert!(!provider_supports_model_discovery(PROVIDER_AZURE_OPENAI));
    assert!(provider_discovers_modality(
        PROVIDER_ALIBABA_CN,
        "embedding"
    ));
    assert!(!provider_discovers_modality(
        PROVIDER_ALIBABA_CN,
        "imageGeneration"
    ));
    assert_eq!(
        provider_models_for_modality(PROVIDER_ALIBABA_CN, "embedding"),
        vec!["text-embedding-v4"]
    );
    assert_eq!(
        provider_model_supports_vision(PROVIDER_ALIBABA_CN, "qwen3.7-plus"),
        Some(true)
    );
    assert_eq!(
        provider_model_supports_vision(PROVIDER_ALIBABA_CN, "qwen3.7-max"),
        Some(false)
    );
}

#[test]
fn switching_provider_replaces_stale_models_with_builtin_modality_defaults() {
    let mut config = ProviderConfig {
        api_key: "openai-key".to_string(),
        ..ProviderConfig::default()
    };
    let mut input = provider_input_from_config(&config);
    input.provider_id = PROVIDER_ALIBABA_CN.to_string();
    input.provider_resource = "obsolete-workspace".to_string();
    input.base_url.clear();
    apply_provider_config_input(&mut config, input);

    assert_eq!(config.provider_id, PROVIDER_ALIBABA_CN);
    assert!(config.provider_resource.is_empty());
    assert_eq!(config.model, "qwen3.7-plus");
    assert_eq!(config.summarizer_model, "qwen3.7-flash");
    assert_eq!(config.embedding_model, "text-embedding-v4");
    assert_eq!(config.image_model, "qwen-image-3.0-pro");
    assert_eq!(config.voice_model, "qwen3-asr-flash-realtime");
    assert!(config.api_key.is_empty());
    assert!(config.auth_verified_at_ms.is_none());
}

#[test]
fn authenticated_catalog_reconciles_each_modality_without_inventing_access() {
    let mut config = ProviderConfig::default();
    reconcile_provider_models(
        &mut config,
        &[
            "gpt-4.1-mini".to_string(),
            "text-embedding-3-small".to_string(),
        ],
    )
    .expect("an available Chat preset should reconcile");

    assert_eq!(config.model, "gpt-4.1-mini");
    assert_eq!(config.executor_model, "gpt-4.1-mini");
    assert_eq!(config.embedding_model, "text-embedding-3-small");
    assert!(config.image_model.is_empty());
    assert!(config.voice_model.is_empty());
}

#[test]
fn unavailable_modalities_remain_disabled_after_config_round_trip() {
    let mut config = ProviderConfig::default();
    reconcile_provider_models(&mut config, &["gpt-4.1-mini".to_string()])
        .expect("the available Chat model should reconcile");

    let reloaded = provider_config_from_text(&provider_config_text(&config));
    assert_eq!(reloaded.model, "gpt-4.1-mini");
    assert!(reloaded.embedding_model.is_empty());
    assert!(reloaded.model_for_role(&ModelRole::Embedder).is_empty());
    assert!(reloaded.image_model.is_empty());
    assert!(reloaded.voice_model.is_empty());
}

#[test]
fn discovery_does_not_disable_modalities_served_by_a_separate_api() {
    let mut config = provider_config_from_text(
        "provider_id=alibaba_cn\n\
         base_url=https://dashscope.aliyuncs.com/compatible-mode/v1\n",
    );
    reconcile_provider_models(
        &mut config,
        &["qwen3.7-plus".to_string(), "text-embedding-v4".to_string()],
    )
    .expect("the standard Alibaba catalog should reconcile");

    assert_eq!(config.image_model, "qwen-image-3.0-pro");
    assert_eq!(config.voice_model, "qwen3-asr-flash-realtime");
}

#[test]
fn custom_catalog_must_include_the_configured_chat_model() {
    let mut config = ProviderConfig {
        provider_id: PROVIDER_CUSTOM.to_string(),
        base_url: "https://gateway.example/v1".to_string(),
        model: "private-chat".to_string(),
        executor_model: "private-chat".to_string(),
        ..ProviderConfig::default()
    };

    let error = reconcile_provider_models(&mut config, &["other-chat".to_string()])
        .expect_err("a different model must not validate the configured custom model");
    assert!(error.contains("configured Chat model is unavailable"));
}

#[test]
fn provider_base_url_credential_rules_restrict_plaintext_http() {
    use crate::provider_secret_store::validate_provider_base_url_for_credentials;
    for ok in [
        "https://api.example.com/v1",
        "http://localhost:11434",
        "http://127.0.0.1:8080",
        "http://[::1]:8080",
        "",
    ] {
        assert!(
            validate_provider_base_url_for_credentials(ok).is_ok(),
            "{ok} must be allowed"
        );
    }
    for bad in [
        "http://example.com",
        "http://10.0.0.5:8080",
        "ftp://example.com",
        "http://evil.localhost.example.com",
    ] {
        assert!(
            validate_provider_base_url_for_credentials(bad).is_err(),
            "{bad} must be rejected"
        );
    }
}
