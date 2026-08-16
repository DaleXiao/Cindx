use crate::app_state::AppState;
use crate::configuration_models::{embedding_model_for_provider, ProviderConfig};
use crate::persistence_runtime::provider_config_path;
use crate::provider_profiles::{
    provider_model_defaults, resolve_provider_profile, same_provider_credential_identity,
    same_provider_model_identity, ProviderProfile, PROVIDER_CUSTOM,
};
use crate::runtime_values::{
    config_hex_decode, config_hex_encode, normalized_agent_instructions, normalized_config_value,
    sanitize_config_value,
};
use crate::sidecar_runtime::config_bool;
use crate::view_models::ProviderConfigInput;
use std::collections::HashSet;
use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

pub(crate) fn clone_provider_config(
    state: &tauri::State<'_, AppState>,
) -> Result<ProviderConfig, String> {
    state
        .provider_config
        .lock()
        .map(|config| config.clone())
        .map_err(|error| format!("provider config lock poisoned: {error}"))
}

pub(crate) fn apply_provider_config_input(config: &mut ProviderConfig, input: ProviderConfigInput) {
    let previous_profile = config.provider_profile();
    let next_profile = resolve_provider_profile(
        &normalized_config_value(&input.provider_id),
        &normalized_config_value(&input.provider_resource),
        &normalized_config_value(&input.base_url),
        &normalized_config_value(&input.image_endpoint),
    );
    let provider_changed = !same_provider_model_identity(&previous_profile, &next_profile);
    let defaults = provider_model_defaults(&next_profile.provider_id);
    let resolve_model = |value: &str, previous: &str, default: &str| {
        let value = normalized_config_value(value);
        if value.is_empty() || (provider_changed && value == previous) {
            default.to_string()
        } else {
            value
        }
    };
    config.provider_id = next_profile.provider_id.clone();
    config.provider_resource = next_profile.provider_resource.clone();
    config.base_url = next_profile.base_url.clone();
    config.model = resolve_model(
        &input.model,
        &config.model,
        defaults
            .map(|value| value.chat.as_str())
            .unwrap_or_default(),
    );
    config.conductor_model = resolve_model(
        &input.conductor_model,
        &config.conductor_model,
        defaults
            .map(|value| value.conductor.as_str())
            .unwrap_or_default(),
    );
    config.planner_model = resolve_model(
        &input.planner_model,
        &config.planner_model,
        defaults
            .map(|value| value.planner.as_str())
            .unwrap_or_default(),
    );
    config.executor_model = resolve_model(
        &input.executor_model,
        &config.executor_model,
        defaults
            .map(|value| value.executor.as_str())
            .unwrap_or_default(),
    );
    config.reviewer_model = resolve_model(
        &input.reviewer_model,
        &config.reviewer_model,
        defaults
            .map(|value| value.reviewer.as_str())
            .unwrap_or_default(),
    );
    config.summarizer_model = resolve_model(
        &input.summarizer_model,
        &config.summarizer_model,
        defaults
            .map(|value| value.summarizer.as_str())
            .unwrap_or_default(),
    );
    config.embedding_model = resolve_model(
        &input.embedding_model,
        &config.embedding_model,
        defaults
            .map(|value| value.embedding.as_str())
            .unwrap_or_default(),
    );
    config.image_model = resolve_model(
        &input.image_model,
        &config.image_model,
        defaults
            .map(|value| value.image.as_str())
            .unwrap_or_default(),
    );
    config.image_endpoint = next_profile.image_endpoint.clone();
    config.voice_model = resolve_model(
        &input.voice_model,
        &config.voice_model,
        defaults
            .map(|value| value.voice.as_str())
            .unwrap_or_default(),
    );
    config.collaboration_policy = match input.collaboration_policy.as_str() {
        "single" | "plan_execute_review" | "best_of_n" | "auto_router" => {
            input.collaboration_policy
        }
        _ => "auto_router".to_string(),
    };
    config.prompt_evolution_enabled = input.prompt_evolution_enabled;
    config.context_window_tokens = if input.context_window_tokens < 4_096 {
        defaults
            .map(|value| value.context_window_tokens)
            .unwrap_or(128_000)
    } else {
        input.context_window_tokens
    };
    config.agent_system_prompt = normalized_agent_instructions(&input.agent_system_prompt);
    let api_key = normalized_config_value(&input.api_key);
    if !api_key.is_empty() {
        if api_key != config.api_key {
            config.auth_verified_at_ms = None;
        }
        config.api_key = api_key;
    } else if !same_provider_credential_identity(&previous_profile, &next_profile) {
        config.api_key.clear();
    }
    if provider_changed {
        config.auth_verified_at_ms = None;
    }
    if config.planner_model.is_empty() {
        config.planner_model = config.model.clone();
    }
    if config.conductor_model.is_empty() {
        config.conductor_model = config.planner_model.clone();
    }
    if config.executor_model.is_empty() {
        config.executor_model = config.model.clone();
    }
    if config.reviewer_model.is_empty() {
        config.reviewer_model = config.model.clone();
    }
    if config.summarizer_model.is_empty() {
        config.summarizer_model = config.model.clone();
    }
    config.embedding_model =
        embedding_model_for_provider(&config.base_url, &config.embedding_model);
}

pub(crate) fn load_provider_config() -> ProviderConfig {
    let Ok(text) = fs::read_to_string(provider_config_path()) else {
        return ProviderConfig::default();
    };
    provider_config_from_text(&text)
}

pub(crate) fn provider_config_from_text(text: &str) -> ProviderConfig {
    let mut config = ProviderConfig::default();
    let mut loaded_model_fields = HashSet::new();
    let mut provider_id_loaded = false;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "provider_id" => {
                config.provider_id = value.to_string();
                provider_id_loaded = true;
            }
            "provider_resource" => config.provider_resource = value.to_string(),
            "base_url" => config.base_url = value.to_string(),
            "api_key" => config.api_key = value.to_string(),
            "model" => {
                config.model = value.to_string();
                loaded_model_fields.insert("model");
            }
            "conductor_model" => {
                config.conductor_model = value.to_string();
                loaded_model_fields.insert("conductor_model");
            }
            "planner_model" => {
                config.planner_model = value.to_string();
                loaded_model_fields.insert("planner_model");
            }
            "executor_model" => {
                config.executor_model = value.to_string();
                loaded_model_fields.insert("executor_model");
            }
            "reviewer_model" => {
                config.reviewer_model = value.to_string();
                loaded_model_fields.insert("reviewer_model");
            }
            "summarizer_model" => {
                config.summarizer_model = value.to_string();
                loaded_model_fields.insert("summarizer_model");
            }
            "embedding_model" => {
                config.embedding_model = value.to_string();
                loaded_model_fields.insert("embedding_model");
            }
            "image_model" => {
                config.image_model = value.to_string();
                loaded_model_fields.insert("image_model");
            }
            "image_endpoint" => config.image_endpoint = value.to_string(),
            "fast_model" => config.fast_model = value.to_string(),
            "auto_model" => config.auto_model = value.to_string(),
            "pro_model" => config.pro_model = value.to_string(),
            "voice_model" => {
                config.voice_model = value.to_string();
                loaded_model_fields.insert("voice_model");
            }
            "auth_verified_at_ms" => {
                config.auth_verified_at_ms = value.parse::<u64>().ok().filter(|value| *value > 0)
            }
            "collaboration_policy" => config.collaboration_policy = value.to_string(),
            "prompt_evolution_enabled" => config.prompt_evolution_enabled = config_bool(value),
            "context_window_tokens" => {
                config.context_window_tokens = value.parse().unwrap_or(128_000)
            }
            "agent_system_prompt_hex" => {
                if let Some(prompt) = config_hex_decode(value) {
                    config.agent_system_prompt = normalized_agent_instructions(&prompt);
                }
            }
            _ => {}
        }
    }
    let mut profile = resolve_provider_profile(
        if provider_id_loaded {
            &config.provider_id
        } else {
            ""
        },
        &config.provider_resource,
        &config.base_url,
        &config.image_endpoint,
    );
    if !provider_id_loaded && legacy_image_endpoint_is_custom(&profile, &config.image_endpoint) {
        profile = resolve_provider_profile(
            PROVIDER_CUSTOM,
            "",
            &config.base_url,
            &config.image_endpoint,
        );
    }
    config.provider_id = profile.provider_id;
    config.provider_resource = profile.provider_resource;
    config.base_url = profile.base_url;
    config.image_endpoint = profile.image_endpoint;
    if let Some(defaults) = provider_model_defaults(&config.provider_id) {
        if !loaded_model_fields.contains("model") || config.model.trim().is_empty() {
            config.model = defaults.chat.clone();
        }
        if !loaded_model_fields.contains("planner_model") || config.planner_model.trim().is_empty()
        {
            config.planner_model = defaults.planner.clone();
        }
        if !loaded_model_fields.contains("executor_model")
            || config.executor_model.trim().is_empty()
        {
            config.executor_model = defaults.executor.clone();
        }
        if !loaded_model_fields.contains("reviewer_model")
            || config.reviewer_model.trim().is_empty()
        {
            config.reviewer_model = defaults.reviewer.clone();
        }
        if !loaded_model_fields.contains("summarizer_model")
            || config.summarizer_model.trim().is_empty()
        {
            config.summarizer_model = defaults.summarizer.clone();
        }
        if !loaded_model_fields.contains("embedding_model") {
            config.embedding_model = defaults.embedding.clone();
        }
        if !loaded_model_fields.contains("image_model") {
            config.image_model = defaults.image.clone();
        }
        if !loaded_model_fields.contains("voice_model") {
            config.voice_model = defaults.voice.clone();
        }
        if !loaded_model_fields.contains("conductor_model")
            || config.conductor_model.trim().is_empty()
        {
            config.conductor_model = if loaded_model_fields.contains("planner_model")
                && !config.planner_model.trim().is_empty()
            {
                config.planner_model.clone()
            } else {
                defaults.conductor.clone()
            };
        }
    }
    config.embedding_model =
        embedding_model_for_provider(&config.base_url, &config.embedding_model);
    config
}

fn legacy_image_endpoint_is_custom(profile: &ProviderProfile, image_endpoint: &str) -> bool {
    let endpoint = image_endpoint.trim().trim_end_matches('/');
    if endpoint.is_empty() || profile.provider_id == PROVIDER_CUSTOM {
        return false;
    }
    let base_url = profile.base_url.trim_end_matches('/');
    let automatic_image_endpoint = profile.image_endpoint.trim().trim_end_matches('/');
    endpoint != base_url
        && endpoint != format!("{base_url}/images/generations")
        && (automatic_image_endpoint.is_empty() || endpoint != automatic_image_endpoint)
}

pub(crate) fn save_provider_config_to_disk(config: &ProviderConfig) -> Result<(), std::io::Error> {
    let path = provider_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary_path = path.with_extension("conf.tmp");
    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temporary_path)?;
    if let Err(error) = file.write_all(provider_config_text(config).as_bytes()) {
        let _ = fs::remove_file(&temporary_path);
        return Err(error);
    }
    if let Err(error) = file.sync_all() {
        let _ = fs::remove_file(&temporary_path);
        return Err(error);
    }
    #[cfg(unix)]
    if let Err(error) = fs::set_permissions(&temporary_path, fs::Permissions::from_mode(0o600)) {
        let _ = fs::remove_file(&temporary_path);
        return Err(error);
    }
    drop(file);
    match fs::rename(&temporary_path, &path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temporary_path);
            Err(error)
        }
    }
}

pub(crate) fn provider_config_text(config: &ProviderConfig) -> String {
    format!(
        "provider_id={}\nprovider_resource={}\nbase_url={}\napi_key={}\nmodel={}\nconductor_model={}\nplanner_model={}\nexecutor_model={}\nreviewer_model={}\nsummarizer_model={}\nfast_model={}\nauto_model={}\npro_model={}\nembedding_model={}\nimage_model={}\nimage_endpoint={}\nvoice_model={}\nauth_verified_at_ms={}\ncollaboration_policy={}\nprompt_evolution_enabled={}\ncontext_window_tokens={}\nagent_system_prompt_hex={}\n",
        sanitize_config_value(&config.provider_id),
        sanitize_config_value(&config.provider_resource),
        sanitize_config_value(&config.base_url),
        sanitize_config_value(&config.api_key),
        sanitize_config_value(&config.model),
        sanitize_config_value(&config.model_for_conductor()),
        sanitize_config_value(&config.planner_model),
        sanitize_config_value(&config.executor_model),
        sanitize_config_value(&config.reviewer_model),
        sanitize_config_value(&config.summarizer_model),
        sanitize_config_value(&config.fast_model),
        sanitize_config_value(&config.auto_model),
        sanitize_config_value(&config.pro_model),
        sanitize_config_value(&config.embedding_model),
        sanitize_config_value(&config.image_model),
        sanitize_config_value(&config.image_endpoint),
        sanitize_config_value(&config.voice_model),
        config.auth_verified_at_ms.unwrap_or_default(),
        sanitize_config_value(&config.collaboration_policy),
        config.prompt_evolution_enabled,
        config.context_window_tokens,
        config_hex_encode(&config.agent_system_prompt)
    )
}
