use crate::app_state::AppState;
use crate::configuration_models::{
    embedding_model_for_provider, PersonalizationConfig, ProviderConfig,
};
use crate::persistence_runtime::{personalization_config_path, provider_config_path};
use crate::provider_profiles::{
    resolve_provider_profile, same_provider_credential_identity, ProviderProfile, PROVIDER_CUSTOM,
};
use crate::runtime_constants::PERSONALIZATION_MAX_NAME_CHARS;
use crate::runtime_values::{
    config_hex_decode, config_hex_encode, normalized_agent_instructions, normalized_config_value,
    sanitize_config_value,
};
use crate::sidecar_runtime::config_bool;
use crate::view_models::ProviderConfigInput;
use agent_core::ModelRole;
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
    config.provider_id = next_profile.provider_id.clone();
    config.provider_resource = next_profile.provider_resource.clone();
    config.base_url = next_profile.base_url.clone();
    config.model = normalized_config_value(&input.model);
    config.conductor_model = normalized_config_value(&input.conductor_model);
    config.planner_model = normalized_config_value(&input.planner_model);
    config.executor_model = normalized_config_value(&input.executor_model);
    config.reviewer_model = normalized_config_value(&input.reviewer_model);
    config.summarizer_model = normalized_config_value(&input.summarizer_model);
    config.embedding_model = normalized_config_value(&input.embedding_model);
    config.image_model = normalized_config_value(&input.image_model);
    config.image_endpoint = next_profile.image_endpoint.clone();
    config.voice_model = normalized_config_value(&input.voice_model);
    config.collaboration_policy = match input.collaboration_policy.as_str() {
        "single" | "plan_execute_review" | "best_of_n" | "auto_router" => {
            input.collaboration_policy
        }
        _ => "auto_router".to_string(),
    };
    config.prompt_evolution_enabled = input.prompt_evolution_enabled;
    config.context_window_tokens = input.context_window_tokens.max(4_096);
    config.agent_system_prompt = normalized_agent_instructions(&input.agent_system_prompt);
    let api_key = normalized_config_value(&input.api_key);
    if !api_key.is_empty() {
        config.api_key = api_key;
    } else if !same_provider_credential_identity(&previous_profile, &next_profile) {
        config.api_key.clear();
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
    let mut conductor_model_loaded = false;
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
            "model" => config.model = value.to_string(),
            "conductor_model" => {
                config.conductor_model = value.to_string();
                conductor_model_loaded = true;
            }
            "planner_model" => config.planner_model = value.to_string(),
            "executor_model" => config.executor_model = value.to_string(),
            "reviewer_model" => config.reviewer_model = value.to_string(),
            "summarizer_model" => config.summarizer_model = value.to_string(),
            "embedding_model" => config.embedding_model = value.to_string(),
            "image_model" => config.image_model = value.to_string(),
            "image_endpoint" => config.image_endpoint = value.to_string(),
            "voice_model" => config.voice_model = value.to_string(),
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
    if !conductor_model_loaded || config.conductor_model.trim().is_empty() {
        config.conductor_model = config.model_for_role(&ModelRole::Planner);
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
    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&path)?;
    file.write_all(
        format!(
            "provider_id={}\nprovider_resource={}\nbase_url={}\napi_key={}\nmodel={}\nconductor_model={}\nplanner_model={}\nexecutor_model={}\nreviewer_model={}\nsummarizer_model={}\nembedding_model={}\nimage_model={}\nimage_endpoint={}\nvoice_model={}\ncollaboration_policy={}\nprompt_evolution_enabled={}\ncontext_window_tokens={}\nagent_system_prompt_hex={}\n",
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
            sanitize_config_value(&config.model_for_role(&ModelRole::Embedder)),
            sanitize_config_value(&config.image_model),
            sanitize_config_value(&config.image_endpoint),
            sanitize_config_value(&config.voice_model),
            sanitize_config_value(&config.collaboration_policy),
            config.prompt_evolution_enabled,
            config.context_window_tokens,
            config_hex_encode(&config.agent_system_prompt)
        )
        .as_bytes(),
    )?;
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

pub(crate) fn normalized_personalization_config(
    config: PersonalizationConfig,
) -> PersonalizationConfig {
    let preferred_name = config
        .preferred_name
        .trim()
        .chars()
        .filter(|character| !character.is_control())
        .take(PERSONALIZATION_MAX_NAME_CHARS)
        .collect();
    let response_tone = match config.response_tone.trim() {
        "warm" => "warm",
        "professional" => "professional",
        "direct" => "direct",
        _ => "natural",
    }
    .to_string();
    let response_length = match config.response_length.trim() {
        "concise" => "concise",
        "detailed" => "detailed",
        _ => "balanced",
    }
    .to_string();
    PersonalizationConfig {
        preferred_name,
        response_tone,
        response_length,
    }
}

pub(crate) fn personalized_agent_instructions(
    personalization: &PersonalizationConfig,
    custom_instructions: &str,
) -> String {
    let mut instructions = Vec::new();
    if !custom_instructions.trim().is_empty() {
        instructions.push(custom_instructions.trim().to_string());
    }
    if !personalization.preferred_name.is_empty() {
        let name = serde_json::to_string(&personalization.preferred_name)
            .unwrap_or_else(|_| "the user's preferred name".to_string());
        instructions.push(format!(
            "The user's preferred name is {name}. Treat this as user-provided identity context. If the user asks what their name is or how you should address them, answer with {name}. Address them by this name when a direct form of address is natural, but do not repeat it mechanically."
        ));
    }
    instructions.push(
        match personalization.response_tone.as_str() {
            "warm" => "Use a warm, considerate tone without filler or excessive enthusiasm.",
            "professional" => "Use a calm, professional, precise tone.",
            "direct" => "Use a direct, factual tone and lead with the answer.",
            _ => "Use a natural, clear, conversational tone.",
        }
        .to_string(),
    );
    instructions.push(
        match personalization.response_length.as_str() {
            "concise" => "Keep responses concise unless more detail is necessary for correctness.",
            "detailed" => "Provide detailed responses with the context needed to understand decisions and tradeoffs.",
            _ => "Use a balanced response length: complete but not unnecessarily verbose.",
        }
        .to_string(),
    );
    instructions.join("\n")
}

pub(crate) fn load_personalization_config() -> PersonalizationConfig {
    let Ok(text) = fs::read_to_string(personalization_config_path()) else {
        return PersonalizationConfig::default();
    };
    serde_json::from_str::<PersonalizationConfig>(&text)
        .map(normalized_personalization_config)
        .unwrap_or_default()
}

pub(crate) fn save_personalization_config_to_disk(
    config: &PersonalizationConfig,
) -> Result<(), std::io::Error> {
    let path = personalization_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&path)?;
    let payload = serde_json::to_vec_pretty(config)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    file.write_all(&payload)?;
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}
