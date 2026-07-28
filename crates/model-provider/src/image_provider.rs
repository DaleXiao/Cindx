use super::{
    execute_http, execute_http_cancellable, GeneratedImage, ImageGenerationRequest,
    ImageGenerationResponse, ModelError, MODEL_REQUEST_CANCELLED,
};
use crate::response_parser::parse_provider_error;
use base64::Engine;
use reqwest::StatusCode;

const MAX_IMAGE_RESPONSE_BYTES: usize = 48 * 1024 * 1024;
const MAX_GENERATED_IMAGE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiCompatibleImageConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImageGenerationProtocol {
    OpenAiImages,
    DashScopeMultimodal,
}

impl OpenAiCompatibleImageConfig {
    pub fn images_url(&self) -> String {
        let endpoint = self.base_url.trim_end_matches('/');
        if endpoint.ends_with("/images/generations")
            || endpoint.ends_with("/api/v1/services/aigc/multimodal-generation/generation")
        {
            endpoint.to_string()
        } else {
            format!("{endpoint}/images/generations")
        }
    }

    pub fn is_ready(&self) -> bool {
        !self.api_key.trim().is_empty()
            && !self.model.trim().is_empty()
            && !self.base_url.trim().is_empty()
    }

    fn protocol(&self) -> ImageGenerationProtocol {
        if self
            .images_url()
            .contains("/api/v1/services/aigc/multimodal-generation/generation")
        {
            ImageGenerationProtocol::DashScopeMultimodal
        } else {
            ImageGenerationProtocol::OpenAiImages
        }
    }
}

pub struct OpenAiCompatibleImageProvider {
    config: OpenAiCompatibleImageConfig,
}

pub(crate) enum ImagePayload {
    Base64(String),
    Url(String),
}

pub(crate) struct ParsedImagePayload {
    pub(crate) payload: ImagePayload,
    pub(crate) revised_prompt: Option<String>,
}

impl OpenAiCompatibleImageProvider {
    pub fn new(config: OpenAiCompatibleImageConfig) -> Self {
        Self { config }
    }

    pub fn validate_endpoint(&self) -> Result<String, ModelError> {
        if self.config.base_url.trim().is_empty() || self.config.model.trim().is_empty() {
            return Err(ModelError::new(
                "image generation endpoint and model are required",
            ));
        }
        let endpoint = self.config.images_url();
        let output = execute_http(
            &endpoint,
            &self.config.api_key,
            Some("{}"),
            self.config.timeout_seconds.clamp(1, 10),
            64 * 1024,
        )?;
        if image_endpoint_probe_succeeded(output.status) {
            Ok(endpoint)
        } else {
            Err(ModelError::with_status(
                output.status.as_u16(),
                format!("image endpoint probe returned status {}", output.status),
            ))
        }
    }

    pub fn generate(
        &self,
        request: ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse, ModelError> {
        self.generate_cancellable(request, || false)
    }

    pub fn generate_cancellable(
        &self,
        request: ImageGenerationRequest,
        mut should_cancel: impl FnMut() -> bool,
    ) -> Result<ImageGenerationResponse, ModelError> {
        if !self.config.is_ready() {
            return Err(ModelError::new(
                "image generation provider config is incomplete",
            ));
        }
        if request.prompt.trim().is_empty() {
            return Err(ModelError::new("image generation prompt is empty"));
        }

        let protocol = self.config.protocol();
        let request_body = match protocol {
            ImageGenerationProtocol::OpenAiImages => build_image_generation_request_json(
                &self.config.model,
                &request.prompt,
                request.size.as_deref(),
            )?,
            ImageGenerationProtocol::DashScopeMultimodal => {
                build_dashscope_image_generation_request_json(
                    &self.config.model,
                    &request.prompt,
                    request.size.as_deref(),
                )?
            }
        };
        let output = execute_http_cancellable(
            &self.config.images_url(),
            &self.config.api_key,
            Some(&request_body),
            self.config.timeout_seconds,
            MAX_IMAGE_RESPONSE_BYTES,
            &mut should_cancel,
        )?;
        let stdout = String::from_utf8_lossy(&output.body).to_string();
        if !output.status.is_success() {
            let provider_error = parse_provider_error(&stdout).unwrap_or_default();
            return Err(ModelError::with_status(
                output.status.as_u16(),
                if provider_error.is_empty() {
                    format!(
                        "image generation request failed with status {}",
                        output.status
                    )
                } else {
                    provider_error
                },
            ));
        }
        if output.body.len() > MAX_IMAGE_RESPONSE_BYTES {
            return Err(ModelError::new("image generation response exceeded 48 MB"));
        }

        let (response_model, payloads) =
            parse_image_generation_payloads(&stdout, &self.config.model)?;
        let mut images = Vec::with_capacity(payloads.len());
        for parsed in payloads {
            if should_cancel() {
                return Err(ModelError::new(MODEL_REQUEST_CANCELLED));
            }
            let bytes = match parsed.payload {
                ImagePayload::Base64(value) => decode_generated_image(&value)?,
                ImagePayload::Url(url) => {
                    if !url.starts_with("https://") && !url.starts_with("http://") {
                        return Err(ModelError::new(
                            "image generation response included an unsupported URL",
                        ));
                    }
                    let output = execute_http_cancellable(
                        &url,
                        "",
                        None,
                        self.config.timeout_seconds,
                        MAX_GENERATED_IMAGE_BYTES,
                        &mut should_cancel,
                    )?;
                    if !output.status.is_success() {
                        return Err(ModelError::with_status(
                            output.status.as_u16(),
                            format!(
                                "generated image download failed with status {}",
                                output.status
                            ),
                        ));
                    }
                    output.body
                }
            };
            if bytes.is_empty() {
                return Err(ModelError::new("image generation returned an empty image"));
            }
            if bytes.len() > MAX_GENERATED_IMAGE_BYTES {
                return Err(ModelError::new("generated image exceeded 32 MB"));
            }
            let mime_type = generated_image_mime_type(&bytes)
                .ok_or_else(|| ModelError::new("image generation returned unsupported data"))?
                .to_string();
            images.push(GeneratedImage {
                bytes,
                mime_type,
                revised_prompt: parsed.revised_prompt,
            });
        }

        let mut metadata = request.metadata;
        metadata.insert("model".to_string(), response_model.clone());
        metadata.insert("images".to_string(), images.len().to_string());
        metadata.insert(
            "protocol".to_string(),
            match protocol {
                ImageGenerationProtocol::OpenAiImages => "openai-images",
                ImageGenerationProtocol::DashScopeMultimodal => "dashscope-multimodal",
            }
            .to_string(),
        );
        Ok(ImageGenerationResponse {
            model: response_model,
            images,
            metadata,
        })
    }
}

pub(crate) fn image_endpoint_probe_succeeded(status: StatusCode) -> bool {
    status.is_success() || matches!(status.as_u16(), 400 | 401 | 403 | 422 | 429)
}

pub fn build_image_generation_request_json(
    model: &str,
    prompt: &str,
    size: Option<&str>,
) -> Result<String, ModelError> {
    if model.trim().is_empty() {
        return Err(ModelError::new("image generation model is empty"));
    }
    if prompt.trim().is_empty() {
        return Err(ModelError::new("image generation prompt is empty"));
    }

    let mut body = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "n": 1
    });
    if let Some(size) = size.filter(|value| !value.trim().is_empty()) {
        body["size"] = serde_json::Value::String(size.to_string());
    }
    serde_json::to_string(&body)
        .map_err(|error| ModelError::new(format!("failed to encode image request: {error}")))
}

pub(crate) fn build_dashscope_image_generation_request_json(
    model: &str,
    prompt: &str,
    size: Option<&str>,
) -> Result<String, ModelError> {
    if model.trim().is_empty() {
        return Err(ModelError::new("image generation model is empty"));
    }
    if prompt.trim().is_empty() {
        return Err(ModelError::new("image generation prompt is empty"));
    }

    let mut parameters = serde_json::json!({
        "n": 1,
        "watermark": false
    });
    if let Some(size) = size.filter(|value| !value.trim().is_empty()) {
        parameters["size"] = serde_json::Value::String(size.replace('x', "*"));
    }
    let body = serde_json::json!({
        "model": model,
        "input": {
            "messages": [{
                "role": "user",
                "content": [{ "text": prompt }]
            }]
        },
        "parameters": parameters
    });
    serde_json::to_string(&body)
        .map_err(|error| ModelError::new(format!("failed to encode image request: {error}")))
}

pub(crate) fn parse_image_generation_payloads(
    text: &str,
    fallback_model: &str,
) -> Result<(String, Vec<ParsedImagePayload>), ModelError> {
    if let Some(message) = parse_provider_error(text) {
        return Err(ModelError::new(message));
    }
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|error| ModelError::new(format!("invalid image generation response: {error}")))?;
    if let Some(message) = value
        .get("message")
        .and_then(serde_json::Value::as_str)
        .filter(|message| !message.trim().is_empty())
        .filter(|_| value.get("code").is_some())
    {
        return Err(ModelError::new(message));
    }
    let model = value
        .get("model")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(fallback_model)
        .to_string();
    let mut payloads = Vec::new();
    if let Some(data) = value.get("data").and_then(serde_json::Value::as_array) {
        for item in data {
            let revised_prompt = item
                .get("revised_prompt")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            let payload = if let Some(value) = item
                .get("b64_json")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
            {
                ImagePayload::Base64(value.to_string())
            } else if let Some(value) = item
                .get("url")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
            {
                ImagePayload::Url(value.to_string())
            } else {
                return Err(ModelError::new(
                    "image generation item did not include image data",
                ));
            };
            payloads.push(ParsedImagePayload {
                payload,
                revised_prompt,
            });
        }
    } else if let Some(choices) = value
        .pointer("/output/choices")
        .and_then(serde_json::Value::as_array)
    {
        for content in choices.iter().filter_map(|choice| {
            choice
                .pointer("/message/content")
                .and_then(serde_json::Value::as_array)
        }) {
            for item in content {
                if let Some(url) = item
                    .get("image")
                    .and_then(serde_json::Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                {
                    payloads.push(ParsedImagePayload {
                        payload: ImagePayload::Url(url.to_string()),
                        revised_prompt: None,
                    });
                }
            }
        }
    } else {
        return Err(ModelError::new(
            "image generation response did not include image data",
        ));
    }
    if payloads.is_empty() {
        return Err(ModelError::new("image generation returned no images"));
    }
    Ok((model, payloads))
}

pub(crate) fn decode_generated_image(value: &str) -> Result<Vec<u8>, ModelError> {
    let encoded = value
        .strip_prefix("data:")
        .and_then(|data| data.split_once(',').map(|(_, encoded)| encoded))
        .unwrap_or(value);
    let compact = encoded
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    base64::engine::general_purpose::STANDARD
        .decode(compact)
        .map_err(|error| ModelError::new(format!("invalid generated image data: {error}")))
}

pub(crate) fn generated_image_mime_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.len() >= 12
        && &bytes[4..8] == b"ftyp"
        && (&bytes[8..12] == b"avif" || &bytes[8..12] == b"avis")
    {
        Some("image/avif")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_url_accepts_a_base_url_or_explicit_endpoint() {
        let base = OpenAiCompatibleImageConfig {
            base_url: "https://example.test/v1/".to_string(),
            api_key: "key".to_string(),
            model: "image-a".to_string(),
            timeout_seconds: 10,
        };
        let dashscope = OpenAiCompatibleImageConfig {
            base_url: "https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation".to_string(),
            ..base.clone()
        };

        assert_eq!(
            base.images_url(),
            "https://example.test/v1/images/generations"
        );
        assert_eq!(
            dashscope.images_url(),
            "https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation"
        );
        assert_eq!(
            dashscope.protocol(),
            ImageGenerationProtocol::DashScopeMultimodal
        );
    }
}
