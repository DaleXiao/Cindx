use super::{
    execute_http, is_azure_openai_url, ModelError, OpenAiCompatibleProvider,
    MAX_MODEL_RESPONSE_BYTES,
};
use crate::json_wire::{
    extract_json_array_after, extract_json_string_field, split_top_level_objects,
};
use crate::response_parser::parse_provider_error;
use reqwest::StatusCode;

impl OpenAiCompatibleProvider {
    pub fn list_models(&self) -> Result<Vec<String>, ModelError> {
        if self.config.base_url.trim().is_empty() || self.config.api_key.trim().is_empty() {
            return Err(ModelError::new(
                "provider base URL and API key are required",
            ));
        }

        let output = execute_http(
            &self.config.models_url(),
            &self.config.api_key,
            None,
            self.config.timeout_seconds,
            MAX_MODEL_RESPONSE_BYTES,
        )?;

        let stdout = String::from_utf8_lossy(&output.body).to_string();
        if !output.status.is_success() {
            let provider_error = parse_provider_error(&stdout).unwrap_or_default();
            return Err(ModelError::with_status(
                output.status.as_u16(),
                if provider_error.is_empty() {
                    format!("model list request failed with status {}", output.status)
                } else {
                    provider_error
                },
            ));
        }

        parse_model_list_response(&stdout)
    }

    pub fn validate_credentials(&self) -> Result<Vec<String>, ModelError> {
        if self.config.base_url.trim().is_empty() || self.config.api_key.trim().is_empty() {
            return Err(ModelError::new(
                "provider base URL and API key are required",
            ));
        }

        if is_azure_openai_url(&self.config.base_url) {
            self.validate_chat_access()?;
            return Ok(Vec::new());
        }

        let output = execute_http(
            &self.config.models_url(),
            &self.config.api_key,
            None,
            self.config.timeout_seconds,
            MAX_MODEL_RESPONSE_BYTES,
        )?;
        let body = String::from_utf8_lossy(&output.body).to_string();
        if output.status.is_success() {
            return parse_model_list_response(&body);
        }
        if !matches!(output.status.as_u16(), 404 | 405) {
            return Err(provider_credential_error(output.status, &body));
        }

        self.validate_chat_access()?;
        Ok(Vec::new())
    }

    pub fn validate_chat_access(&self) -> Result<(), ModelError> {
        if !self.config.is_ready() {
            return Err(ModelError::new(
                "provider base URL, API key, and Chat model are required",
            ));
        }
        let request_body = chat_verification_request_body(&self.config.model, false);
        let mut probe = execute_http(
            &self.config.chat_completions_url(),
            &self.config.api_key,
            Some(&request_body),
            self.config.timeout_seconds,
            64 * 1024,
        )?;
        let mut probe_body = String::from_utf8_lossy(&probe.body).to_string();
        if probe.status == StatusCode::BAD_REQUEST
            && probe_body
                .to_ascii_lowercase()
                .contains("max_completion_tokens")
        {
            let retry_body = chat_verification_request_body(&self.config.model, true);
            probe = execute_http(
                &self.config.chat_completions_url(),
                &self.config.api_key,
                Some(&retry_body),
                self.config.timeout_seconds,
                64 * 1024,
            )?;
            probe_body = String::from_utf8_lossy(&probe.body).to_string();
        }
        if !probe.status.is_success() {
            return Err(provider_credential_error(probe.status, &probe_body));
        }
        let response: serde_json::Value = serde_json::from_str(&probe_body).map_err(|error| {
            ModelError::new(format!("Chat verification returned invalid JSON: {error}"))
        })?;
        if response["choices"].as_array().is_none_or(Vec::is_empty) {
            return Err(ModelError::new(
                "Chat verification response did not include a completion choice",
            ));
        }
        Ok(())
    }
}

fn chat_verification_request_body(model: &str, use_completion_limit: bool) -> String {
    let mut body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": "Reply OK"}],
        "stream": false
    });
    if use_completion_limit {
        body["max_completion_tokens"] = serde_json::json!(1);
    } else {
        body["max_tokens"] = serde_json::json!(1);
    }
    body.to_string()
}

fn provider_credential_error(status: StatusCode, body: &str) -> ModelError {
    let provider_error = parse_provider_error(body).unwrap_or_default();
    ModelError::with_status(
        status.as_u16(),
        if provider_error.is_empty() {
            format!("provider credential verification failed with status {status}")
        } else {
            provider_error
        },
    )
}

pub fn parse_model_list_response(text: &str) -> Result<Vec<String>, ModelError> {
    if let Some(message) = parse_provider_error(text) {
        return Err(ModelError::new(message));
    }

    let data = extract_json_array_after(text, "\"data\"")
        .ok_or_else(|| ModelError::new("model list response did not include data"))?;
    let mut models = split_top_level_objects(&data)
        .into_iter()
        .filter_map(|object| extract_json_string_field(&object, "id"))
        .filter(|model| !model.trim().is_empty())
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();

    if models.is_empty() {
        return Err(ModelError::new("provider returned an empty model list"));
    }
    Ok(models)
}
