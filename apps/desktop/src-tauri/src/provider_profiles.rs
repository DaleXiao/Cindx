use tauri::Url;

pub(crate) const PROVIDER_OPENAI: &str = "openai";
pub(crate) const PROVIDER_AZURE_OPENAI: &str = "azure_openai";
pub(crate) const PROVIDER_ALIBABA_CN: &str = "alibaba_cn";
pub(crate) const PROVIDER_CUSTOM: &str = "custom";

const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
const ALIBABA_CN_BASE_URL: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1";
const ALIBABA_CN_IMAGE_ENDPOINT: &str =
    "https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation";
const AZURE_OPENAI_HOST_SUFFIX: &str = ".openai.azure.com";
const AZURE_AI_SERVICES_HOST_SUFFIX: &str = ".services.ai.azure.com";
const ALIBABA_CN_WORKSPACE_HOST_SUFFIX: &str = ".cn-beijing.maas.aliyuncs.com";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderProfile {
    pub(crate) provider_id: String,
    pub(crate) provider_resource: String,
    pub(crate) base_url: String,
    pub(crate) image_endpoint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProviderEndpointIdentity {
    provider_id: String,
    provider_resource: String,
    base_origin: String,
    image_origin: Option<String>,
}

pub(crate) fn resolve_provider_profile(
    provider_id: &str,
    provider_resource: &str,
    base_url: &str,
    image_endpoint: &str,
) -> ProviderProfile {
    let explicit_provider_id = provider_id.trim().to_ascii_lowercase();
    let (provider_id, inferred_resource) = if explicit_provider_id.is_empty() {
        infer_provider(base_url)
    } else {
        match explicit_provider_id.as_str() {
            PROVIDER_OPENAI | PROVIDER_AZURE_OPENAI | PROVIDER_ALIBABA_CN | PROVIDER_CUSTOM => {
                (explicit_provider_id, String::new())
            }
            _ => (PROVIDER_CUSTOM.to_string(), String::new()),
        }
    };

    match provider_id.as_str() {
        PROVIDER_OPENAI => ProviderProfile {
            provider_id,
            provider_resource: String::new(),
            base_url: OPENAI_BASE_URL.to_string(),
            image_endpoint: String::new(),
        },
        PROVIDER_AZURE_OPENAI => {
            let resource = resolved_resource(provider_resource, &inferred_resource);
            let base_url = valid_resource_label(&resource)
                .then(|| format!("https://{resource}.openai.azure.com/openai/v1"))
                .unwrap_or_default();
            ProviderProfile {
                provider_id,
                provider_resource: resource,
                base_url,
                image_endpoint: String::new(),
            }
        }
        PROVIDER_ALIBABA_CN => {
            let resource = resolved_resource(provider_resource, &inferred_resource);
            if resource.is_empty() {
                ProviderProfile {
                    provider_id,
                    provider_resource: resource,
                    base_url: ALIBABA_CN_BASE_URL.to_string(),
                    image_endpoint: ALIBABA_CN_IMAGE_ENDPOINT.to_string(),
                }
            } else if valid_resource_label(&resource) {
                let host = format!("{resource}.cn-beijing.maas.aliyuncs.com");
                ProviderProfile {
                    provider_id,
                    provider_resource: resource,
                    base_url: format!("https://{host}/compatible-mode/v1"),
                    image_endpoint: format!(
                        "https://{host}/api/v1/services/aigc/multimodal-generation/generation"
                    ),
                }
            } else {
                ProviderProfile {
                    provider_id,
                    provider_resource: resource,
                    base_url: String::new(),
                    image_endpoint: String::new(),
                }
            }
        }
        _ => ProviderProfile {
            provider_id: PROVIDER_CUSTOM.to_string(),
            provider_resource: String::new(),
            base_url: base_url.to_string(),
            image_endpoint: image_endpoint.to_string(),
        },
    }
}

pub(crate) fn same_provider_credential_identity(
    left: &ProviderProfile,
    right: &ProviderProfile,
) -> bool {
    provider_endpoint_identity(left, true) == provider_endpoint_identity(right, true)
}

pub(crate) fn same_provider_model_identity(
    left: &ProviderProfile,
    right: &ProviderProfile,
) -> bool {
    provider_endpoint_identity(left, false) == provider_endpoint_identity(right, false)
}

pub(crate) fn provider_supports_webrtc_voice(provider_id: &str, base_url: &str) -> bool {
    match provider_id.trim().to_ascii_lowercase().as_str() {
        PROVIDER_OPENAI => true,
        PROVIDER_CUSTOM => !uses_known_incompatible_voice_host(base_url),
        _ => false,
    }
}

fn infer_provider(base_url: &str) -> (String, String) {
    let Some(url) = parsed_http_url(base_url) else {
        return (PROVIDER_CUSTOM.to_string(), String::new());
    };
    let Some(host) = normalized_host(&url) else {
        return (PROVIDER_CUSTOM.to_string(), String::new());
    };
    if host == "api.openai.com" {
        return (PROVIDER_OPENAI.to_string(), String::new());
    }
    if host == "dashscope.aliyuncs.com" {
        return (PROVIDER_ALIBABA_CN.to_string(), String::new());
    }
    if let Some(resource) = resource_from_normalized_host(&host, AZURE_OPENAI_HOST_SUFFIX) {
        return (PROVIDER_AZURE_OPENAI.to_string(), resource);
    }
    if let Some(resource) = resource_from_normalized_host(&host, ALIBABA_CN_WORKSPACE_HOST_SUFFIX) {
        return (PROVIDER_ALIBABA_CN.to_string(), resource);
    }
    (PROVIDER_CUSTOM.to_string(), String::new())
}

fn resolved_resource(configured: &str, inferred: &str) -> String {
    let configured = configured.trim().to_ascii_lowercase();
    if !configured.is_empty() {
        return configured;
    }
    inferred.to_string()
}

fn parsed_http_url(value: &str) -> Option<Url> {
    let url = Url::parse(value.trim()).ok()?;
    matches!(url.scheme(), "http" | "https").then_some(url)
}

fn normalized_host(url: &Url) -> Option<String> {
    url.host_str()
        .map(|host| host.trim_end_matches('.').to_ascii_lowercase())
}

fn resource_from_normalized_host(host: &str, suffix: &str) -> Option<String> {
    let resource = host.strip_suffix(suffix)?;
    valid_resource_label(resource).then(|| resource.to_string())
}

fn uses_known_incompatible_voice_host(base_url: &str) -> bool {
    let Some(host) = parsed_http_url(base_url).as_ref().and_then(normalized_host) else {
        return false;
    };
    host == "dashscope.aliyuncs.com"
        || resource_from_normalized_host(&host, AZURE_OPENAI_HOST_SUFFIX).is_some()
        || resource_from_normalized_host(&host, AZURE_AI_SERVICES_HOST_SUFFIX).is_some()
        || resource_from_normalized_host(&host, ALIBABA_CN_WORKSPACE_HOST_SUFFIX).is_some()
}

fn valid_resource_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
}

fn provider_endpoint_identity(
    profile: &ProviderProfile,
    include_image_endpoint: bool,
) -> ProviderEndpointIdentity {
    let image_endpoint = if profile.image_endpoint.trim().is_empty() {
        &profile.base_url
    } else {
        &profile.image_endpoint
    };
    ProviderEndpointIdentity {
        provider_id: profile.provider_id.clone(),
        provider_resource: profile.provider_resource.clone(),
        base_origin: endpoint_origin(&profile.base_url),
        image_origin: include_image_endpoint.then(|| endpoint_origin(image_endpoint)),
    }
}

fn endpoint_origin(value: &str) -> String {
    parsed_http_url(value)
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or_else(|| format!("invalid:{value}"))
}
