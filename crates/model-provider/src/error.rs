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
}
