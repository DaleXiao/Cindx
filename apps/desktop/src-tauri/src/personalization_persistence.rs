use crate::configuration_models::PersonalizationConfig;
use crate::persistence_runtime::personalization_config_path;
use crate::runtime_constants::PERSONALIZATION_MAX_NAME_CHARS;
use std::fs;
use tools::write_private_file_atomically;

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
    let payload = serde_json::to_vec_pretty(config)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    write_private_file_atomically(&path, &payload)
}
