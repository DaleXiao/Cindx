use crate::{ModelToolCall, PreparedStreamingModelRequest};
use agent_core::Metadata;
use sha2::{Digest, Sha256};
use std::fmt::Write;

pub(crate) const REQUEST_PAYLOAD_SHA256_KEY: &str = "request_payload_sha256";
pub(crate) const RESPONSE_SEMANTIC_SHA256_KEY: &str = "response_semantic_sha256";
pub(crate) const PROVIDER_RECEIPT_STATUS_KEY: &str = "provider_receipt_status";
pub(crate) const PROVIDER_RESPONSE_ID_KEY: &str = "provider_response_id";
pub(crate) const PROVIDER_RESPONSE_MODEL_KEY: &str = "provider_response_model";
pub(crate) const PROVIDER_SYSTEM_FINGERPRINT_KEY: &str = "provider_system_fingerprint";

const REQUEST_PAYLOAD_DOMAIN: &[u8] = b"cindx.model-provider.request-payload.v1\0";
const RESPONSE_SEMANTIC_DOMAIN: &[u8] = b"cindx.model-provider.response-semantic.v1\0";
const STATUS_OBSERVED: &str = "observed";
const STATUS_PROVIDER_ID_MISSING: &str = "provider_id_missing";
const STATUS_IDENTITY_CONFLICT: &str = "identity_conflict";

#[cfg(test)]
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    digest_hex(hasher.finalize())
}

pub(crate) fn request_payload_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(REQUEST_PAYLOAD_DOMAIN);
    hasher.update(bytes);
    digest_hex(hasher.finalize())
}

impl PreparedStreamingModelRequest {
    /// Returns the digest and size of the exact encoded payload, without the body.
    pub fn payload_receipt(&self) -> Option<(&str, usize)> {
        self.encoded_parts()
            .map(|(body, _, digest)| (digest, body.len()))
    }
}

pub(crate) fn provider_identity_metadata(value: &serde_json::Value) -> Metadata {
    [
        ("id", PROVIDER_RESPONSE_ID_KEY),
        ("model", PROVIDER_RESPONSE_MODEL_KEY),
        ("system_fingerprint", PROVIDER_SYSTEM_FINGERPRINT_KEY),
    ]
    .into_iter()
    .filter_map(|(field, metadata_key)| {
        value
            .get(field)
            .and_then(serde_json::Value::as_str)
            .filter(|field_value| !field_value.trim().is_empty())
            .map(|field_value| (metadata_key.to_string(), field_value.to_string()))
    })
    .collect()
}

pub(crate) fn merge_provider_identity(target: &mut Metadata, incoming: &Metadata) {
    for key in [
        PROVIDER_RESPONSE_ID_KEY,
        PROVIDER_RESPONSE_MODEL_KEY,
        PROVIDER_SYSTEM_FINGERPRINT_KEY,
    ] {
        let Some(incoming_value) = incoming.get(key) else {
            continue;
        };
        match target.get(key) {
            Some(existing_value) if existing_value != incoming_value => {
                target.insert(
                    PROVIDER_RECEIPT_STATUS_KEY.to_string(),
                    STATUS_IDENTITY_CONFLICT.to_string(),
                );
            }
            Some(_) => {}
            None => {
                target.insert(key.to_string(), incoming_value.clone());
            }
        }
    }
}

pub(crate) fn finalize_provider_receipt_status(metadata: &mut Metadata) {
    if metadata
        .get(PROVIDER_RECEIPT_STATUS_KEY)
        .is_some_and(|status| status == STATUS_IDENTITY_CONFLICT)
    {
        return;
    }
    let status = if metadata
        .get(PROVIDER_RESPONSE_ID_KEY)
        .is_some_and(|value| !value.trim().is_empty())
    {
        STATUS_OBSERVED
    } else {
        STATUS_PROVIDER_ID_MISSING
    };
    metadata.insert(PROVIDER_RECEIPT_STATUS_KEY.to_string(), status.to_string());
}

pub(crate) fn attach_request_payload_sha256(metadata: &mut Metadata, digest: &str) {
    metadata.insert(REQUEST_PAYLOAD_SHA256_KEY.to_string(), digest.to_string());
}

pub(crate) fn response_semantic_sha256(
    content: &str,
    tool_calls: &[ModelToolCall],
    finish_reason: Option<&str>,
) -> String {
    let payload = serde_json::json!({
        "content": content,
        "tool_calls": tool_calls
            .iter()
            .map(|call| serde_json::json!({
                "id": call.id,
                "name": call.name,
                "arguments_json": call.arguments_json,
            }))
            .collect::<Vec<_>>(),
        "finish_reason": finish_reason,
    });
    let encoded = serde_json::to_vec(&payload)
        .expect("semantic model response contains only serializable strings");
    let mut hasher = Sha256::new();
    hasher.update(RESPONSE_SEMANTIC_DOMAIN);
    hasher.update(encoded);
    digest_hex(hasher.finalize())
}

pub(crate) fn attach_response_semantic_sha256(
    metadata: &mut Metadata,
    content: &str,
    tool_calls: &[ModelToolCall],
    finish_reason: Option<&str>,
) {
    metadata.insert(
        RESPONSE_SEMANTIC_SHA256_KEY.to_string(),
        response_semantic_sha256(content, tool_calls, finish_reason),
    );
}

fn digest_hex(digest: impl AsRef<[u8]>) -> String {
    let bytes = digest.as_ref();
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_digest_is_stable_domain_separated_and_content_sensitive() {
        let first = request_payload_sha256(br#"{"model":"a","messages":[]}"#);
        let repeated = request_payload_sha256(br#"{"model":"a","messages":[]}"#);
        let changed = request_payload_sha256(br#"{"model":"b","messages":[]}"#);

        assert_eq!(first, repeated);
        assert_ne!(first, changed);
        assert_ne!(first, sha256_hex(br#"{"model":"a","messages":[]}"#));
        assert_eq!(first.len(), 64);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn semantic_response_digest_binds_text_ordered_tools_and_finish_reason() {
        let first = response_semantic_sha256(
            "",
            &[ModelToolCall {
                id: "call-1".to_string(),
                name: "file.read".to_string(),
                arguments_json: r#"{"path":"a"}"#.to_string(),
            }],
            Some("tool_calls"),
        );
        let changed_tool = response_semantic_sha256(
            "",
            &[ModelToolCall {
                id: "call-1".to_string(),
                name: "file.write".to_string(),
                arguments_json: r#"{"path":"a"}"#.to_string(),
            }],
            Some("tool_calls"),
        );
        let changed_finish = response_semantic_sha256(
            "",
            &[ModelToolCall {
                id: "call-1".to_string(),
                name: "file.read".to_string(),
                arguments_json: r#"{"path":"a"}"#.to_string(),
            }],
            Some("stop"),
        );

        assert_ne!(first, changed_tool);
        assert_ne!(first, changed_finish);
        assert_ne!(first, sha256_hex(b""));
    }

    #[test]
    fn conflicting_identity_preserves_first_observation_and_marks_receipt() {
        let mut metadata = provider_identity_metadata(&serde_json::json!({
            "id": "response-1",
            "model": "served-a",
        }));
        let conflict = provider_identity_metadata(&serde_json::json!({
            "id": "response-2",
            "model": "served-b",
        }));

        merge_provider_identity(&mut metadata, &conflict);
        finalize_provider_receipt_status(&mut metadata);

        assert_eq!(metadata[PROVIDER_RESPONSE_ID_KEY], "response-1");
        assert_eq!(metadata[PROVIDER_RESPONSE_MODEL_KEY], "served-a");
        assert_eq!(
            metadata[PROVIDER_RECEIPT_STATUS_KEY],
            STATUS_IDENTITY_CONFLICT
        );
    }

    #[test]
    fn model_conflict_is_not_hidden_when_provider_id_is_missing() {
        let mut metadata = provider_identity_metadata(&serde_json::json!({
            "model": "served-a",
        }));
        let conflict = provider_identity_metadata(&serde_json::json!({
            "model": "served-b",
        }));

        merge_provider_identity(&mut metadata, &conflict);
        finalize_provider_receipt_status(&mut metadata);

        assert!(!metadata.contains_key(PROVIDER_RESPONSE_ID_KEY));
        assert_eq!(metadata[PROVIDER_RESPONSE_MODEL_KEY], "served-a");
        assert_eq!(
            metadata[PROVIDER_RECEIPT_STATUS_KEY],
            STATUS_IDENTITY_CONFLICT
        );
    }
}
