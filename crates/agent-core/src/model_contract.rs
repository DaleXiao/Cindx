use crate::{Message, Metadata, ModelRole, ToolSpec};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelCallMode {
    NonStreaming,
    Streaming,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRequest {
    pub role: ModelRole,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub mode: ModelCallMode,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelResponse {
    pub message: Message,
    pub raw_tool_calls_json: Option<String>,
    pub tool_calls: Vec<ModelToolCall>,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelResponseTermination {
    Complete,
    ToolCalls,
    OutputLimit,
    ContentFiltered,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelResponseDisposition {
    Usable,
    ToolCalls,
    Empty,
    IncompleteOutput,
    Filtered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelResponseAssessment {
    pub termination: ModelResponseTermination,
    pub disposition: ModelResponseDisposition,
}

impl ModelResponse {
    pub fn assessment(&self) -> ModelResponseAssessment {
        let finish_reason = self
            .metadata
            .get("finish_reason")
            .map(|value| value.trim().to_ascii_lowercase());
        let termination = if !self.tool_calls.is_empty()
            || matches!(
                finish_reason.as_deref(),
                Some("tool_calls" | "function_call")
            ) {
            ModelResponseTermination::ToolCalls
        } else {
            match finish_reason.as_deref() {
                Some("stop" | "end_turn" | "completed") => ModelResponseTermination::Complete,
                Some("length" | "max_tokens" | "max_output_tokens") => {
                    ModelResponseTermination::OutputLimit
                }
                Some("content_filter" | "safety" | "blocked") => {
                    ModelResponseTermination::ContentFiltered
                }
                _ => ModelResponseTermination::Unknown,
            }
        };
        let disposition = match termination {
            ModelResponseTermination::ToolCalls => ModelResponseDisposition::ToolCalls,
            ModelResponseTermination::OutputLimit => ModelResponseDisposition::IncompleteOutput,
            ModelResponseTermination::ContentFiltered => ModelResponseDisposition::Filtered,
            ModelResponseTermination::Complete | ModelResponseTermination::Unknown => {
                if self.message.content.trim().is_empty() {
                    ModelResponseDisposition::Empty
                } else {
                    ModelResponseDisposition::Usable
                }
            }
        };

        ModelResponseAssessment {
            termination,
            disposition,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelToolCall {
    pub id: String,
    pub name: String,
    pub arguments_json: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderFailureClass {
    Cancelled,
    Timeout,
    RateLimited,
    Unavailable,
    Transport,
    Authentication,
    InvalidRequest,
    ResponseTooLarge,
    MalformedResponse,
    Unknown,
}

impl ProviderFailureClass {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::Timeout => "timeout",
            Self::RateLimited => "rate_limited",
            Self::Unavailable => "unavailable",
            Self::Transport => "transport",
            Self::Authentication => "authentication",
            Self::InvalidRequest => "invalid_request",
            Self::ResponseTooLarge => "response_too_large",
            Self::MalformedResponse => "malformed_response",
            Self::Unknown => "unknown",
        }
    }

    pub fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::Timeout | Self::RateLimited | Self::Unavailable | Self::Transport
        )
    }
}

pub fn classify_provider_failure(message: &str, status_code: Option<u16>) -> ProviderFailureClass {
    if let Some(status) = status_code {
        match status {
            401 | 403 => return ProviderFailureClass::Authentication,
            408 => return ProviderFailureClass::Timeout,
            429 => return ProviderFailureClass::RateLimited,
            500..=599 => return ProviderFailureClass::Unavailable,
            400..=499 => return ProviderFailureClass::InvalidRequest,
            _ => {}
        }
    }

    let message = message.to_ascii_lowercase();
    if message.contains("model request cancelled") || message.contains("request cancelled") {
        ProviderFailureClass::Cancelled
    } else if message.contains("too many requests")
        || message.contains("rate limit")
        || message.contains("status 429")
    {
        ProviderFailureClass::RateLimited
    } else if message.contains("timed out") || message.contains("timeout") {
        ProviderFailureClass::Timeout
    } else if message.contains("service unavailable")
        || message.contains("temporarily unavailable")
        || message.contains("empty reply")
        || ["status 500", "status 502", "status 503", "status 504"]
            .iter()
            .any(|needle| message.contains(needle))
    {
        ProviderFailureClass::Unavailable
    } else if [
        "broken pipe",
        "connection reset",
        "connection refused",
        "failed to start curl",
        "failed to configure curl",
        "failed to wait for curl",
        "error sending request",
        "failed while reading response",
    ]
    .iter()
    .any(|needle| message.contains(needle))
    {
        ProviderFailureClass::Transport
    } else if message.contains("unauthorized")
        || message.contains("forbidden")
        || message.contains("api key")
        || message.contains("authentication")
    {
        ProviderFailureClass::Authentication
    } else if message.contains("exceeded 64 mb")
        || message.contains("response too large")
        || message.contains("response_too_large")
    {
        ProviderFailureClass::ResponseTooLarge
    } else if message.contains("invalid model stream event")
        || message.contains("model response did not include")
        || message.contains("malformed response")
    {
        ProviderFailureClass::MalformedResponse
    } else if message.contains("invalid request")
        || message.contains("invalid_request")
        || message.contains("unexpected item type")
        || message.contains("status 400")
        || message.contains("status 404")
        || message.contains("status 422")
    {
        ProviderFailureClass::InvalidRequest
    } else {
        ProviderFailureClass::Unknown
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelError {
    pub message: String,
    pub class: ProviderFailureClass,
    pub status_code: Option<u16>,
    pub retryable: bool,
}

impl ModelError {
    pub fn new(message: impl Into<String>) -> Self {
        let message = message.into();
        let class = classify_provider_failure(&message, None);
        Self {
            message,
            class,
            status_code: None,
            retryable: class.is_retryable(),
        }
    }

    pub fn with_status(status_code: u16, message: impl Into<String>) -> Self {
        let message = message.into();
        let class = classify_provider_failure(&message, Some(status_code));
        Self {
            message,
            class,
            status_code: Some(status_code),
            retryable: class.is_retryable(),
        }
    }

    pub fn is_retryable(&self) -> bool {
        self.retryable
    }

    pub fn is_cancelled(&self) -> bool {
        self.class == ProviderFailureClass::Cancelled
    }
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for ModelError {}

pub fn tool_function_name(tool_name: &str) -> String {
    let mut name = String::with_capacity(tool_name.len());
    for character in tool_name.chars() {
        if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
            name.push(character);
        } else {
            name.push('_');
        }
    }
    if name.is_empty() {
        "local_tool".to_string()
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_codes_override_ambiguous_provider_messages() {
        assert_eq!(
            classify_provider_failure("request failed", Some(429)),
            ProviderFailureClass::RateLimited
        );
        assert_eq!(
            classify_provider_failure("request failed", Some(400)),
            ProviderFailureClass::InvalidRequest
        );
        assert_eq!(
            classify_provider_failure("request failed", Some(503)),
            ProviderFailureClass::Unavailable
        );
    }

    #[test]
    fn retryability_is_intentionally_narrow() {
        assert!(classify_provider_failure("broken pipe", None).is_retryable());
        assert!(classify_provider_failure("request timed out", None).is_retryable());
        assert!(!classify_provider_failure("invalid request", None).is_retryable());
        assert!(!classify_provider_failure("bad API key", None).is_retryable());
    }

    #[test]
    fn response_assessment_distinguishes_incomplete_and_usable_content() {
        let mut response = ModelResponse {
            message: Message {
                role: crate::MessageRole::Assistant,
                content: "partial".to_string(),
                metadata: Metadata::new(),
            },
            raw_tool_calls_json: None,
            tool_calls: Vec::new(),
            metadata: Metadata::from([("finish_reason".to_string(), "length".to_string())]),
        };
        assert_eq!(
            response.assessment().disposition,
            ModelResponseDisposition::IncompleteOutput
        );
        response
            .metadata
            .insert("finish_reason".to_string(), "stop".to_string());
        assert_eq!(
            response.assessment().disposition,
            ModelResponseDisposition::Usable
        );
    }
}
