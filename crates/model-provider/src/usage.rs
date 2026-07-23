use crate::{ModelResponse, ModelToolCall};
use agent_core::{Message, Metadata, ToolSpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageSource {
    Provider,
    ProviderPartial,
    Estimated,
}

impl UsageSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::ProviderPartial => "provider_partial",
            Self::Estimated => "estimated",
        }
    }
}

pub fn estimate_text_tokens(text: &str) -> u64 {
    if text.is_empty() {
        return 0;
    }
    let (ascii, non_ascii) = text.chars().fold((0_u64, 0_u64), |counts, ch| {
        if ch.is_ascii() {
            (counts.0 + 1, counts.1)
        } else {
            (counts.0, counts.1 + 1)
        }
    });
    ascii
        .div_ceil(4)
        .saturating_add(non_ascii.saturating_mul(2).div_ceil(3))
}

pub fn estimate_request_tokens(messages: &[Message], tools: &[ToolSpec]) -> u64 {
    let message_tokens = messages.iter().fold(0_u64, |total, message| {
        total
            .saturating_add(4)
            .saturating_add(estimate_text_tokens(&message.content))
    });
    tools.iter().fold(message_tokens, |total, tool| {
        total
            .saturating_add(estimate_text_tokens(&tool.name))
            .saturating_add(estimate_text_tokens(&tool.description))
            .saturating_add(estimate_text_tokens(&tool.input_schema_json))
    })
}

pub fn estimate_completion_tokens(content: &str, tool_calls: &[ModelToolCall]) -> u64 {
    tool_calls
        .iter()
        .fold(estimate_text_tokens(content), |total, call| {
            total
                .saturating_add(estimate_text_tokens(&call.name))
                .saturating_add(estimate_text_tokens(&call.arguments_json))
        })
}

pub fn normalize_model_usage(response: &mut ModelResponse, estimated_prompt_tokens: u64) {
    let estimated_completion_tokens =
        estimate_completion_tokens(&response.message.content, &response.tool_calls);
    normalize_usage_metadata(
        &mut response.metadata,
        estimated_prompt_tokens,
        estimated_completion_tokens,
    );
    for key in [
        "prompt_tokens",
        "completion_tokens",
        "total_tokens",
        "usage_source",
        "usage_estimated",
    ] {
        if let Some(value) = response.metadata.get(key) {
            response
                .message
                .metadata
                .insert(key.to_string(), value.clone());
        }
    }
}

fn normalize_usage_metadata(
    metadata: &mut Metadata,
    estimated_prompt_tokens: u64,
    estimated_completion_tokens: u64,
) {
    let provider_prompt = usage_value(metadata, "prompt_tokens");
    let provider_completion = usage_value(metadata, "completion_tokens");
    let provider_total = usage_value(metadata, "total_tokens");
    let provider_field_count = [provider_prompt, provider_completion, provider_total]
        .into_iter()
        .flatten()
        .count();

    let prompt_tokens = provider_prompt.unwrap_or(estimated_prompt_tokens);
    let completion_tokens = provider_completion.unwrap_or(estimated_completion_tokens);
    let total_tokens =
        provider_total.unwrap_or_else(|| prompt_tokens.saturating_add(completion_tokens));
    let source = match provider_field_count {
        3 => UsageSource::Provider,
        1 | 2 => UsageSource::ProviderPartial,
        _ => UsageSource::Estimated,
    };

    metadata.insert("prompt_tokens".to_string(), prompt_tokens.to_string());
    metadata.insert(
        "completion_tokens".to_string(),
        completion_tokens.to_string(),
    );
    metadata.insert("total_tokens".to_string(), total_tokens.to_string());
    metadata.insert("usage_source".to_string(), source.label().to_string());
    metadata.insert(
        "usage_estimated".to_string(),
        (source != UsageSource::Provider).to_string(),
    );
}

fn usage_value(metadata: &Metadata, key: &str) -> Option<u64> {
    metadata
        .get(key)
        .and_then(|value| value.parse::<u64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{MessageRole, ToolRisk};

    fn response(metadata: Metadata) -> ModelResponse {
        ModelResponse {
            message: Message {
                role: MessageRole::Assistant,
                content: "final answer".to_string(),
                metadata: Metadata::new(),
            },
            raw_tool_calls_json: None,
            tool_calls: Vec::new(),
            metadata,
        }
    }

    #[test]
    fn provider_usage_is_preserved_exactly() {
        let mut response = response(
            [
                ("prompt_tokens".to_string(), "21".to_string()),
                ("completion_tokens".to_string(), "4".to_string()),
                ("total_tokens".to_string(), "25".to_string()),
            ]
            .into_iter()
            .collect(),
        );
        normalize_model_usage(&mut response, 100);
        assert_eq!(
            response.metadata.get("total_tokens").map(String::as_str),
            Some("25")
        );
        assert_eq!(
            response.metadata.get("usage_source").map(String::as_str),
            Some("provider")
        );
    }

    #[test]
    fn missing_usage_is_estimated_and_marked() {
        let request = [Message {
            role: MessageRole::User,
            content: "Explain this implementation".to_string(),
            metadata: Metadata::new(),
        }];
        let tool = ToolSpec::builtin(
            "file.read",
            "file",
            "Read a workspace file",
            ToolRisk::ReadOnly,
            r#"{"type":"object"}"#,
        );
        let prompt_tokens = estimate_request_tokens(&request, &[tool]);
        let mut response = response(Metadata::new());
        normalize_model_usage(&mut response, prompt_tokens);
        assert!(usage_value(&response.metadata, "total_tokens").unwrap_or_default() > 0);
        assert_eq!(
            response.metadata.get("usage_source").map(String::as_str),
            Some("estimated")
        );
        assert_eq!(
            response.metadata.get("usage_estimated").map(String::as_str),
            Some("true")
        );
    }
}
