use crate::provider_receipt::request_payload_sha256;
use crate::ModelRequest;
use bytes::Bytes;
use std::fmt;

pub struct PreparedStreamingModelRequest(PreparedStreamingPayload);

enum PreparedStreamingPayload {
    Deferred(ModelRequest),
    Encoded {
        request_body: Bytes,
        request_payload_sha256: String,
        estimated_prompt_tokens: u64,
    },
}

impl PreparedStreamingModelRequest {
    pub fn deferred(request: ModelRequest) -> Self {
        Self(PreparedStreamingPayload::Deferred(request))
    }

    pub(super) fn deferred_request(&self) -> Option<&ModelRequest> {
        match &self.0 {
            PreparedStreamingPayload::Deferred(request) => Some(request),
            PreparedStreamingPayload::Encoded { .. } => None,
        }
    }

    pub(super) fn encoded(request_body: String, estimated_prompt_tokens: u64) -> Self {
        let request_body = Bytes::from(request_body);
        let request_payload_sha256 = request_payload_sha256(request_body.as_ref());
        Self(PreparedStreamingPayload::Encoded {
            request_body,
            request_payload_sha256,
            estimated_prompt_tokens,
        })
    }

    pub(super) fn encoded_parts(&self) -> Option<(&Bytes, u64, &str)> {
        match &self.0 {
            PreparedStreamingPayload::Encoded {
                request_body,
                request_payload_sha256,
                estimated_prompt_tokens,
            } => Some((
                request_body,
                *estimated_prompt_tokens,
                request_payload_sha256,
            )),
            PreparedStreamingPayload::Deferred(_) => None,
        }
    }
}

impl fmt::Debug for PreparedStreamingModelRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            PreparedStreamingPayload::Deferred(_) => {
                formatter.write_str("PreparedStreamingModelRequest { payload: deferred }")
            }
            PreparedStreamingPayload::Encoded { request_body, .. } => write!(
                formatter,
                "PreparedStreamingModelRequest {{ payload: encoded, request_body_bytes: {} }}",
                request_body.len()
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{Metadata, ModelRole};

    fn request() -> ModelRequest {
        ModelRequest {
            role: ModelRole::Executor,
            messages: Vec::new(),
            tools: Vec::new(),
            mode: crate::ModelCallMode::Streaming,
            metadata: Metadata::new(),
        }
    }

    #[test]
    fn agent_collaboration_learning_offline_adapter_contract_encoded_payload_receipt_exposes_only_digest_and_size(
    ) {
        let prepared = PreparedStreamingModelRequest::encoded("{\"safe\":true}".to_string(), 3);

        let (digest, bytes) = prepared.payload_receipt().expect("encoded receipt");

        assert_eq!(bytes, 13);
        assert_eq!(digest, request_payload_sha256(b"{\"safe\":true}"));
    }

    #[test]
    fn agent_collaboration_learning_offline_adapter_contract_deferred_payload_has_no_encoded_receipt(
    ) {
        let prepared = PreparedStreamingModelRequest::deferred(request());

        assert_eq!(prepared.payload_receipt(), None);
    }
}
