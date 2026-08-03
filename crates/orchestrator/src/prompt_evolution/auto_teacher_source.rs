use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const AUTO_TEACHER_PROVIDER_IDENTITY_SCHEMA_V1: &str =
    "cindx.auto-teacher-provider-identity.v1";
pub const AUTO_TEACHER_EVALUATOR_RECEIPT_SCHEMA_V1: &str =
    "cindx.auto-teacher-evaluator-receipt.v1";
pub const AUTO_TEACHER_SOURCE_CONTEXT_SCHEMA_V1: &str = "cindx.auto-teacher-source-context.v1";
pub const AUTO_TEACHER_PROVIDER_REVIEW_PROTOCOL_V1: &str =
    "cindx.provider-backed-auto-teacher-review.v1";
pub const AUTO_TEACHER_EVALUATED_ARTIFACT_SCHEMA_V1: &str =
    "cindx.auto-teacher-evaluated-artifact.v1";
pub const AUTO_TEACHER_EVALUATOR_RECEIPT_METADATA_KEY: &str = "auto_teacher_evaluator_receipt_v1";
pub const AUTO_TEACHER_EVALUATED_ARTIFACT_MAX_CHARS: usize = 12_000;

pub fn canonical_auto_teacher_evaluated_artifact(value: &str) -> String {
    let value = value.trim();
    let mut canonical = value
        .chars()
        .take(AUTO_TEACHER_EVALUATED_ARTIFACT_MAX_CHARS)
        .collect::<String>();
    if value.chars().count() > AUTO_TEACHER_EVALUATED_ARTIFACT_MAX_CHARS {
        canonical.push_str("\n[truncated]");
    }
    canonical
}

pub fn auto_teacher_evaluated_artifact_sha256(value: &str) -> Result<String, String> {
    let value = canonical_auto_teacher_evaluated_artifact(value);
    if value.is_empty() {
        return Err("Auto teacher evaluated artifact is empty".to_string());
    }
    serde_json::to_vec(&(AUTO_TEACHER_EVALUATED_ARTIFACT_SCHEMA_V1, &value))
        .map(|encoded| sha256_hex(&encoded))
        .map_err(|error| format!("Auto teacher artifact serialization failed: {error}"))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutoTeacherProviderIdentityV1 {
    pub schema: String,
    pub provider_id: String,
    pub provider_resource_sha256: String,
    pub endpoint_sha256: String,
    pub model: String,
    pub identity_sha256: String,
}

impl AutoTeacherProviderIdentityV1 {
    pub fn new(
        provider_id: &str,
        provider_resource: &str,
        endpoint: &str,
        model: &str,
    ) -> Result<Self, String> {
        let provider_id = provider_id.trim().to_ascii_lowercase();
        let provider_resource = provider_resource.trim().to_ascii_lowercase();
        let endpoint = endpoint.trim().trim_end_matches('/');
        let model = model.trim();
        if provider_id.is_empty() || endpoint.is_empty() || model.is_empty() {
            return Err("Auto teacher provider identity is incomplete".to_string());
        }
        let provider_resource_sha256 = sha256_hex(provider_resource.as_bytes());
        let endpoint_sha256 = sha256_hex(endpoint.as_bytes());
        let identity_sha256 = provider_model_identity_digest(
            &provider_id,
            &provider_resource_sha256,
            &endpoint_sha256,
            model,
        )?;
        let identity = Self {
            schema: AUTO_TEACHER_PROVIDER_IDENTITY_SCHEMA_V1.to_string(),
            provider_id,
            provider_resource_sha256,
            endpoint_sha256,
            model: model.to_string(),
            identity_sha256,
        };
        identity.validate()?;
        Ok(identity)
    }

    pub fn with_model(&self, model: &str) -> Result<Self, String> {
        self.validate()?;
        let model = model.trim();
        if model.is_empty() {
            return Err("Auto teacher participant model is empty".to_string());
        }
        Ok(Self {
            schema: AUTO_TEACHER_PROVIDER_IDENTITY_SCHEMA_V1.to_string(),
            provider_id: self.provider_id.clone(),
            provider_resource_sha256: self.provider_resource_sha256.clone(),
            endpoint_sha256: self.endpoint_sha256.clone(),
            model: model.to_string(),
            identity_sha256: provider_model_identity_digest(
                &self.provider_id,
                &self.provider_resource_sha256,
                &self.endpoint_sha256,
                model,
            )?,
        })
    }

    pub fn provider_digest(&self) -> Result<String, String> {
        self.validate()?;
        serde_json::to_vec(&(
            &self.provider_id,
            &self.provider_resource_sha256,
            &self.endpoint_sha256,
        ))
        .map(|encoded| sha256_hex(&encoded))
        .map_err(|error| format!("Auto teacher provider serialization failed: {error}"))
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema != AUTO_TEACHER_PROVIDER_IDENTITY_SCHEMA_V1
            || self.provider_id.trim().is_empty()
            || self.provider_id != self.provider_id.trim().to_ascii_lowercase()
            || self.model.trim().is_empty()
            || self.model != self.model.trim()
            || !is_sha256(&self.provider_resource_sha256)
            || !is_sha256(&self.endpoint_sha256)
            || !is_sha256(&self.identity_sha256)
            || self.identity_sha256
                != provider_model_identity_digest(
                    &self.provider_id,
                    &self.provider_resource_sha256,
                    &self.endpoint_sha256,
                    &self.model,
                )?
        {
            return Err("Auto teacher provider identity is malformed".to_string());
        }
        Ok(())
    }
}

fn provider_model_identity_digest(
    provider_id: &str,
    provider_resource_sha256: &str,
    endpoint_sha256: &str,
    model: &str,
) -> Result<String, String> {
    serde_json::to_vec(&(
        provider_id,
        provider_resource_sha256,
        endpoint_sha256,
        model,
    ))
    .map(|encoded| sha256_hex(&encoded))
    .map_err(|error| format!("Auto teacher model identity serialization failed: {error}"))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutoTeacherEvaluatorReceiptV1 {
    pub schema: String,
    pub protocol: String,
    pub evaluator: AutoTeacherProviderIdentityV1,
    pub stage: String,
    pub request_id_sha256: String,
    pub stage_event_sha256: String,
    pub evaluated_artifact_sha256: String,
    pub system_prompt_sha256: String,
    pub tool_contract_sha256: String,
    pub source_revision_sha256: String,
    pub quality_score_bps: u16,
    pub passed: bool,
    pub safety_violations: u64,
}

impl AutoTeacherEvaluatorReceiptV1 {
    pub fn validate(&self) -> Result<(), String> {
        self.evaluator.validate()?;
        if self.schema != AUTO_TEACHER_EVALUATOR_RECEIPT_SCHEMA_V1
            || self.protocol != AUTO_TEACHER_PROVIDER_REVIEW_PROTOCOL_V1
            || !(self.stage == "quality_gate" || self.stage.starts_with("quality_recheck_"))
            || self.quality_score_bps > 10_000
            || [
                &self.request_id_sha256,
                &self.stage_event_sha256,
                &self.evaluated_artifact_sha256,
                &self.system_prompt_sha256,
                &self.tool_contract_sha256,
                &self.source_revision_sha256,
            ]
            .into_iter()
            .any(|digest| !is_sha256(digest))
        {
            return Err("Auto teacher evaluator receipt is malformed".to_string());
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String, String> {
        self.validate()?;
        serde_json::to_vec(self)
            .map(|encoded| sha256_hex(&encoded))
            .map_err(|error| {
                format!("Auto teacher evaluator receipt serialization failed: {error}")
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutoTeacherSourceContextV1 {
    pub schema: String,
    pub provider_sha256: String,
    pub model_pool_sha256: String,
    pub system_prompt_sha256: String,
    pub policy_sha256: String,
    pub budget_sha256: String,
    pub tool_contract_sha256: String,
    pub source_revision_sha256: String,
    pub workspace_revision_sha256: String,
    pub evaluator_identity_sha256: String,
    pub evaluator_receipt_sha256: String,
    pub checkpoint_sha256: String,
    pub learning_receipt_sha256: String,
}

impl AutoTeacherSourceContextV1 {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != AUTO_TEACHER_SOURCE_CONTEXT_SCHEMA_V1
            || [
                &self.provider_sha256,
                &self.model_pool_sha256,
                &self.system_prompt_sha256,
                &self.policy_sha256,
                &self.budget_sha256,
                &self.tool_contract_sha256,
                &self.source_revision_sha256,
                &self.workspace_revision_sha256,
                &self.evaluator_identity_sha256,
                &self.evaluator_receipt_sha256,
                &self.checkpoint_sha256,
                &self.learning_receipt_sha256,
            ]
            .into_iter()
            .any(|digest| !is_sha256(digest))
        {
            return Err("Auto teacher source context is malformed".to_string());
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String, String> {
        self.validate()?;
        serde_json::to_vec(self)
            .map(|encoded| sha256_hex(&encoded))
            .map_err(|error| format!("Auto teacher source context serialization failed: {error}"))
    }
}

fn sha256_hex(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt() -> AutoTeacherEvaluatorReceiptV1 {
        AutoTeacherEvaluatorReceiptV1 {
            schema: AUTO_TEACHER_EVALUATOR_RECEIPT_SCHEMA_V1.to_string(),
            protocol: AUTO_TEACHER_PROVIDER_REVIEW_PROTOCOL_V1.to_string(),
            evaluator: AutoTeacherProviderIdentityV1::new(
                "OpenAI",
                "",
                "https://api.openai.com/v1/",
                "judge-model",
            )
            .unwrap(),
            stage: "quality_gate".to_string(),
            request_id_sha256: "1".repeat(64),
            stage_event_sha256: "2".repeat(64),
            evaluated_artifact_sha256: auto_teacher_evaluated_artifact_sha256(
                "verified final output",
            )
            .unwrap(),
            system_prompt_sha256: "3".repeat(64),
            tool_contract_sha256: "4".repeat(64),
            source_revision_sha256: "5".repeat(64),
            quality_score_bps: 9_000,
            passed: true,
            safety_violations: 0,
        }
    }

    #[test]
    fn provider_receipt_and_source_context_replay_exactly() {
        let receipt = receipt();
        receipt.validate().unwrap();
        assert_ne!(
            receipt.evaluator.identity_sha256,
            receipt
                .evaluator
                .with_model("worker-model")
                .unwrap()
                .identity_sha256
        );
        let context = AutoTeacherSourceContextV1 {
            schema: AUTO_TEACHER_SOURCE_CONTEXT_SCHEMA_V1.to_string(),
            provider_sha256: receipt.evaluator.provider_digest().unwrap(),
            model_pool_sha256: "6".repeat(64),
            system_prompt_sha256: receipt.system_prompt_sha256.clone(),
            policy_sha256: "7".repeat(64),
            budget_sha256: "8".repeat(64),
            tool_contract_sha256: receipt.tool_contract_sha256.clone(),
            source_revision_sha256: receipt.source_revision_sha256.clone(),
            workspace_revision_sha256: "9".repeat(64),
            evaluator_identity_sha256: receipt.evaluator.identity_sha256.clone(),
            evaluator_receipt_sha256: receipt.digest().unwrap(),
            checkpoint_sha256: "a".repeat(64),
            learning_receipt_sha256: "b".repeat(64),
        };
        let encoded = serde_json::to_string(&context).unwrap();
        let replayed: AutoTeacherSourceContextV1 = serde_json::from_str(&encoded).unwrap();
        assert_eq!(replayed, context);
        assert_eq!(replayed.digest().unwrap(), context.digest().unwrap());
    }

    #[test]
    fn malformed_or_legacy_attestation_fails_closed() {
        assert!(
            AutoTeacherProviderIdentityV1::new("", "", "https://example.test", "judge").is_err()
        );
        let mut malformed_request = receipt();
        malformed_request.request_id_sha256 = "legacy-missing-request".to_string();
        assert!(malformed_request.validate().is_err());

        let mut missing_artifact = receipt();
        missing_artifact.evaluated_artifact_sha256.clear();
        assert!(missing_artifact.validate().is_err());
    }

    #[test]
    fn long_evaluated_artifact_canonicalization_is_bounded_and_idempotent() {
        let output = format!(
            "  {}  ",
            "x".repeat(AUTO_TEACHER_EVALUATED_ARTIFACT_MAX_CHARS + 1)
        );
        let canonical = canonical_auto_teacher_evaluated_artifact(&output);

        assert_eq!(
            canonical,
            format!(
                "{}\n[truncated]",
                "x".repeat(AUTO_TEACHER_EVALUATED_ARTIFACT_MAX_CHARS)
            )
        );
        assert_eq!(
            canonical_auto_teacher_evaluated_artifact(&canonical),
            canonical
        );
        assert_eq!(
            auto_teacher_evaluated_artifact_sha256(&output).unwrap(),
            auto_teacher_evaluated_artifact_sha256(&canonical).unwrap()
        );
    }
}
