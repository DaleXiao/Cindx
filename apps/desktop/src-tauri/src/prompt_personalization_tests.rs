use super::*;

#[test]
fn agent_system_prompt_config_encoding_preserves_multiline_unicode() {
    let prompt = "你是 Cindx。\n先检查事实，再执行。";
    let encoded = config_hex_encode(prompt);

    assert_eq!(config_hex_decode(&encoded).as_deref(), Some(prompt));
    assert!(config_hex_decode("not-hex").is_none());
}

#[test]
fn personalization_is_normalized_and_applied_to_agent_instructions() {
    let config = normalized_personalization_config(PersonalizationConfig {
        preferred_name: "  Dale\nAdmin  ".to_string(),
        response_tone: "direct".to_string(),
        response_length: "concise".to_string(),
    });
    let instructions = personalized_agent_instructions(&config, "Use Chinese when asked.");

    assert_eq!(config.preferred_name, "DaleAdmin");
    assert!(instructions.contains("The user's preferred name is \"DaleAdmin\""));
    assert!(instructions.contains("answer with \"DaleAdmin\""));
    assert!(instructions.contains("direct, factual tone"));
    assert!(instructions.contains("Keep responses concise"));
    assert!(instructions.starts_with("Use Chinese when asked."));
    assert!(instructions
        .ends_with("Keep responses concise unless more detail is necessary for correctness."));
}

#[test]
fn invalid_personalization_options_fall_back_to_safe_defaults() {
    let config = normalized_personalization_config(PersonalizationConfig {
        preferred_name: String::new(),
        response_tone: "unknown".to_string(),
        response_length: "unbounded".to_string(),
    });

    assert_eq!(config.response_tone, "natural");
    assert_eq!(config.response_length, "balanced");
}

#[test]
fn agent_runtime_context_includes_the_time_computed_for_the_user_turn() {
    let context = [(
        "current_time".to_string(),
        "2026-07-11 10:30 CST".to_string(),
    )]
    .into_iter()
    .collect();
    let runtime_context =
        agent_runtime_context_for_run(&context).expect("time context should exist");

    assert!(runtime_context.contains("Current date and time: 2026-07-11 10:30 CST"));
    assert!(runtime_context.contains("authoritative for this turn"));
}

#[test]
fn legacy_default_prompt_migrates_to_empty_custom_instructions() {
    assert!(ProviderConfig::default().agent_system_prompt.is_empty());
    assert!(normalized_agent_instructions("").is_empty());
    assert!(normalized_agent_instructions(LEGACY_AGENT_SYSTEM_PROMPT).is_empty());
    assert_eq!(
        normalized_agent_instructions("  Prefer concise answers.  "),
        "Prefer concise answers."
    );
}
