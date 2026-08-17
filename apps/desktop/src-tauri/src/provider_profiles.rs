use serde::Deserialize;
use std::sync::OnceLock;
use tauri::Url;

pub(crate) const PROVIDER_OPENAI: &str = "openai";
pub(crate) const PROVIDER_AZURE_OPENAI: &str = "azure_openai";
pub(crate) const PROVIDER_ALIBABA_CN: &str = "alibaba_cn";
pub(crate) const PROVIDER_CUSTOM: &str = "custom";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderVoiceTransport {
    OpenAiWebRtc,
    DashScopeWebSocket,
    Unavailable,
}

const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
const ALIBABA_CN_BASE_URL: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1";
const ALIBABA_CN_IMAGE_ENDPOINT: &str =
    "https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation";
const AZURE_OPENAI_HOST_SUFFIX: &str = ".openai.azure.com";
const AZURE_AI_SERVICES_HOST_SUFFIX: &str = ".services.ai.azure.com";
const ALIBABA_CN_WORKSPACE_HOST_SUFFIX: &str = ".cn-beijing.maas.aliyuncs.com";
const PROVIDER_CATALOG_JSON: &str =
    include_str!("../../../../crates/model-provider/providerCatalog.json");

#[derive(Debug, Deserialize)]
struct ProviderCatalogDocument {
    version: u32,
    providers: Vec<ProviderPreset>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderPreset {
    id: String,
    base_url: String,
    image_endpoint: String,
    model_discovery: bool,
    model_discovery_modalities: Vec<String>,
    web_rtc_voice: bool,
    models: Vec<ProviderCatalogModel>,
    defaults: ProviderModelDefaults,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderCatalogModel {
    pub(crate) id: String,
    pub(crate) protocol: String,
    pub(crate) modalities: Vec<String>,
    pub(crate) tool_calling: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderModelDefaults {
    pub(crate) chat: String,
    pub(crate) conductor: String,
    pub(crate) planner: String,
    pub(crate) executor: String,
    pub(crate) reviewer: String,
    pub(crate) summarizer: String,
    pub(crate) fast: String,
    pub(crate) auto: String,
    pub(crate) pro: String,
    pub(crate) embedding: String,
    pub(crate) image: String,
    pub(crate) voice: String,
    pub(crate) context_window_tokens: u64,
}

static PROVIDER_CATALOG: OnceLock<ProviderCatalogDocument> = OnceLock::new();

fn provider_catalog() -> &'static ProviderCatalogDocument {
    PROVIDER_CATALOG.get_or_init(|| {
        let catalog: ProviderCatalogDocument = serde_json::from_str(PROVIDER_CATALOG_JSON)
            .expect("embedded provider catalog must be valid JSON");
        assert_eq!(
            catalog.version, 2,
            "unsupported embedded provider catalog version"
        );
        catalog
    })
}

fn provider_preset(provider_id: &str) -> Option<&'static ProviderPreset> {
    provider_catalog()
        .providers
        .iter()
        .find(|provider| provider.id == provider_id)
}

pub(crate) fn provider_model_catalog(provider_id: &str) -> Option<&'static [ProviderCatalogModel]> {
    provider_preset(provider_id).map(|provider| provider.models.as_slice())
}

pub(crate) fn provider_supports_model_discovery(provider_id: &str) -> bool {
    provider_preset(provider_id).is_some_and(|provider| provider.model_discovery)
}

pub(crate) fn provider_discovers_modality(provider_id: &str, modality: &str) -> bool {
    provider_preset(provider_id).is_some_and(|provider| {
        provider
            .model_discovery_modalities
            .iter()
            .any(|value| value == modality)
    })
}

pub(crate) fn provider_models_for_modality(provider_id: &str, modality: &str) -> Vec<&'static str> {
    provider_model_catalog(provider_id)
        .into_iter()
        .flatten()
        .filter(|model| {
            model.modalities.iter().any(|value| value == modality)
                && (modality != "chat" || model.protocol == "openai-chat-completions")
        })
        .map(|model| model.id.as_str())
        .collect()
}

pub(crate) fn provider_model_defaults(provider_id: &str) -> Option<&'static ProviderModelDefaults> {
    provider_preset(provider_id).map(|provider| &provider.defaults)
}

pub(crate) fn provider_effort_default_model(provider_id: &str, effort_label: &str) -> &'static str {
    let Some(defaults) = provider_model_defaults(provider_id) else {
        return "";
    };
    match effort_label {
        "fast" => defaults.fast.as_str(),
        "auto" => defaults.auto.as_str(),
        "pro" => defaults.pro.as_str(),
        _ => "",
    }
}

pub(crate) fn provider_model_supports_vision(provider_id: &str, model: &str) -> Option<bool> {
    let model = provider_preset(provider_id)?
        .models
        .iter()
        .find(|candidate| candidate.id.eq_ignore_ascii_case(model.trim()))?;
    model
        .modalities
        .iter()
        .any(|modality| modality == "chat")
        .then(|| {
            model
                .modalities
                .iter()
                .any(|modality| modality == "imageInput")
        })
}

pub(crate) fn provider_model_supports_tools(provider_id: &str, model: &str) -> Option<bool> {
    provider_preset(provider_id)?
        .models
        .iter()
        .find(|candidate| candidate.id.eq_ignore_ascii_case(model.trim()))
        .map(|candidate| candidate.tool_calling)
}

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
    if explicit_provider_id == PROVIDER_ALIBABA_CN
        && !provider_resource.trim().is_empty()
        && is_alibaba_workspace_url(base_url)
    {
        return ProviderProfile {
            provider_id: PROVIDER_CUSTOM.to_string(),
            provider_resource: String::new(),
            base_url: base_url.to_string(),
            image_endpoint: image_endpoint.to_string(),
        };
    }
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
            base_url: provider_preset(PROVIDER_OPENAI)
                .map(|provider| provider.base_url.clone())
                .unwrap_or_else(|| OPENAI_BASE_URL.to_string()),
            image_endpoint: provider_preset(PROVIDER_OPENAI)
                .map(|provider| provider.image_endpoint.clone())
                .unwrap_or_default(),
        },
        PROVIDER_AZURE_OPENAI => {
            let resource = resolved_resource(provider_resource, &inferred_resource);
            let base_url = if valid_resource_label(&resource) {
                configured_azure_v1_base_url(&resource, base_url)
                    .unwrap_or_else(|| format!("https://{resource}.openai.azure.com/openai/v1"))
            } else {
                String::new()
            };
            ProviderProfile {
                provider_id,
                provider_resource: resource,
                base_url,
                image_endpoint: String::new(),
            }
        }
        PROVIDER_ALIBABA_CN => {
            let preset = provider_preset(PROVIDER_ALIBABA_CN);
            ProviderProfile {
                provider_id,
                provider_resource: String::new(),
                base_url: preset
                    .map(|provider| provider.base_url.clone())
                    .unwrap_or_else(|| ALIBABA_CN_BASE_URL.to_string()),
                image_endpoint: preset
                    .map(|provider| provider.image_endpoint.clone())
                    .unwrap_or_else(|| ALIBABA_CN_IMAGE_ENDPOINT.to_string()),
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
        known @ (PROVIDER_OPENAI | PROVIDER_AZURE_OPENAI | PROVIDER_ALIBABA_CN) => {
            provider_preset(known).is_some_and(|provider| provider.web_rtc_voice)
        }
        PROVIDER_CUSTOM => !uses_known_incompatible_voice_host(base_url),
        _ => false,
    }
}

pub(crate) fn provider_voice_transport(
    provider_id: &str,
    base_url: &str,
) -> ProviderVoiceTransport {
    if provider_id.trim().eq_ignore_ascii_case(PROVIDER_ALIBABA_CN) {
        return ProviderVoiceTransport::DashScopeWebSocket;
    }
    if provider_supports_webrtc_voice(provider_id, base_url) {
        ProviderVoiceTransport::OpenAiWebRtc
    } else {
        ProviderVoiceTransport::Unavailable
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
    if resource_from_normalized_host(&host, ALIBABA_CN_WORKSPACE_HOST_SUFFIX).is_some() {
        return (PROVIDER_CUSTOM.to_string(), String::new());
    }
    (PROVIDER_CUSTOM.to_string(), String::new())
}

fn configured_azure_v1_base_url(resource: &str, value: &str) -> Option<String> {
    let url = parsed_http_url(value)?;
    if url.scheme() != "https" {
        return None;
    }
    let host = normalized_host(&url)?;
    let expected_openai = format!("{resource}{AZURE_OPENAI_HOST_SUFFIX}");
    let expected_services = format!("{resource}{AZURE_AI_SERVICES_HOST_SUFFIX}");
    if host != expected_openai && host != expected_services {
        return None;
    }
    let path = url.path().trim_end_matches('/');
    (path == "/openai/v1").then(|| value.trim().trim_end_matches('/').to_string())
}

fn is_alibaba_workspace_url(value: &str) -> bool {
    parsed_http_url(value)
        .as_ref()
        .and_then(normalized_host)
        .and_then(|host| resource_from_normalized_host(&host, ALIBABA_CN_WORKSPACE_HOST_SUFFIX))
        .is_some()
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
