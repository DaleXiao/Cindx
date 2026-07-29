use agent_core::{Event, EventKind};
use serde::Deserialize;

const LEARNING_EVIDENCE_METADATA_KEY: &str = "learning_evidence_v1";
const LEARNING_EVIDENCE_MAX_BYTES: usize = 1_024;
const LEARNING_EVIDENCE_FIELDS: [&str; 10] = [
    "schema",
    "termination",
    "disposition",
    "verification",
    "attribution",
    "usage_completeness",
    "steer_epoch",
    "budget_fingerprint",
    "independent_quality_source",
    "quality_bps",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrustedOutcomeEvidence {
    IndependentQuality,
    VerifiedPostcondition,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LearningEvidenceV1 {
    schema: LearningEvidenceSchema,
    termination: LearningTermination,
    disposition: LearningDisposition,
    verification: LearningVerification,
    attribution: LearningAttribution,
    usage_completeness: LearningUsageCompleteness,
    steer_epoch: Option<u64>,
    budget_fingerprint: Option<String>,
    independent_quality_source: Option<IndependentQualitySource>,
    quality_bps: Option<u16>,
}

#[derive(Debug, Deserialize)]
enum LearningEvidenceSchema {
    #[serde(rename = "cindx.learning-evidence.v1")]
    V1,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum LearningTermination {
    Completed,
    Failed,
    Cancelled,
    Interrupted,
    Unknown,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum LearningDisposition {
    Positive,
    Negative,
    Censored,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum LearningVerification {
    Passed,
    Failed,
    Unknown,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum LearningAttribution {
    Model,
    Workflow,
    Tool,
    Provider,
    System,
    User,
    Permission,
    Budget,
    Unknown,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum LearningUsageCompleteness {
    Complete,
    Partial,
    Missing,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum IndependentQualitySource {
    CollaborationQualityGate,
    AnytimeSelector,
}

pub(crate) fn trusted_outcome_evidence(event: &Event) -> Option<TrustedOutcomeEvidence> {
    if !matches!(event.kind, EventKind::TaskStatusChanged)
        || event.summary != "Agent task completed"
    {
        return None;
    }
    let encoded = event.metadata.get(LEARNING_EVIDENCE_METADATA_KEY)?;
    if encoded.len() > LEARNING_EVIDENCE_MAX_BYTES {
        return None;
    }

    // Parse the typed contract first so duplicate and unknown fields fail. Then
    // require every v1 field to be present: serde treats absent Option fields as
    // None, but a partial/legacy payload must never become positive evidence.
    let evidence = serde_json::from_str::<LearningEvidenceV1>(encoded).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(encoded).ok()?;
    let object = value.as_object()?;
    if object.len() != LEARNING_EVIDENCE_FIELDS.len()
        || LEARNING_EVIDENCE_FIELDS
            .iter()
            .any(|field| !object.contains_key(*field))
    {
        return None;
    }

    let LearningEvidenceV1 {
        schema: LearningEvidenceSchema::V1,
        termination,
        disposition,
        verification,
        attribution,
        usage_completeness,
        steer_epoch,
        budget_fingerprint,
        independent_quality_source,
        quality_bps,
    } = evidence;
    if termination != LearningTermination::Completed
        || disposition != LearningDisposition::Positive
        || verification != LearningVerification::Passed
        || usage_completeness == LearningUsageCompleteness::Missing
        || steer_epoch.is_none()
        || !budget_fingerprint.as_deref().is_some_and(is_sha256_hex)
        || quality_bps.is_some_and(|quality| quality > 10_000)
    {
        return None;
    }

    match (independent_quality_source, quality_bps, attribution) {
        (Some(_), Some(_), LearningAttribution::Model | LearningAttribution::Workflow) => {
            Some(TrustedOutcomeEvidence::IndependentQuality)
        }
        (None, None, LearningAttribution::Tool) => {
            Some(TrustedOutcomeEvidence::VerifiedPostcondition)
        }
        _ => None,
    }
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{EventId, Metadata, TaskId};

    fn terminal(evidence: &str) -> Event {
        Event {
            id: EventId("terminal".to_string()),
            task_id: TaskId("task".to_string()),
            sequence: 1,
            timestamp_ms: 1,
            kind: EventKind::TaskStatusChanged,
            summary: "Agent task completed".to_string(),
            metadata: [(
                LEARNING_EVIDENCE_METADATA_KEY.to_string(),
                evidence.to_string(),
            )]
            .into_iter()
            .collect::<Metadata>(),
        }
    }

    fn postcondition_evidence() -> String {
        serde_json::json!({
            "schema": "cindx.learning-evidence.v1",
            "termination": "completed",
            "disposition": "positive",
            "verification": "passed",
            "attribution": "tool",
            "usage_completeness": "complete",
            "steer_epoch": 3,
            "budget_fingerprint": "a".repeat(64),
            "independent_quality_source": null,
            "quality_bps": null,
        })
        .to_string()
    }

    #[test]
    fn accepts_only_complete_positive_v1_contracts() {
        assert_eq!(
            trusted_outcome_evidence(&terminal(&postcondition_evidence())),
            Some(TrustedOutcomeEvidence::VerifiedPostcondition)
        );

        let quality = serde_json::json!({
            "schema": "cindx.learning-evidence.v1",
            "termination": "completed",
            "disposition": "positive",
            "verification": "passed",
            "attribution": "workflow",
            "usage_completeness": "partial",
            "steer_epoch": 7,
            "budget_fingerprint": "b".repeat(64),
            "independent_quality_source": "collaboration_quality_gate",
            "quality_bps": 8_250,
        })
        .to_string();
        assert_eq!(
            trusted_outcome_evidence(&terminal(&quality)),
            Some(TrustedOutcomeEvidence::IndependentQuality)
        );
    }

    #[test]
    fn rejects_legacy_partial_and_malformed_positive_evidence() {
        let legacy = terminal(r#"{"termination":"completed","disposition":"positive"}"#);
        assert_eq!(trusted_outcome_evidence(&legacy), None);

        let mut missing_field =
            serde_json::from_str::<serde_json::Value>(&postcondition_evidence())
                .expect("valid fixture");
        missing_field
            .as_object_mut()
            .expect("object fixture")
            .remove("quality_bps");
        assert_eq!(
            trusted_outcome_evidence(&terminal(&missing_field.to_string())),
            None
        );

        let mut bad_fingerprint =
            serde_json::from_str::<serde_json::Value>(&postcondition_evidence())
                .expect("valid fixture");
        bad_fingerprint["budget_fingerprint"] = serde_json::json!("A".repeat(64));
        assert_eq!(
            trusted_outcome_evidence(&terminal(&bad_fingerprint.to_string())),
            None
        );

        let oversized = format!("{}{}", postcondition_evidence(), " ".repeat(1_024));
        assert_eq!(trusted_outcome_evidence(&terminal(&oversized)), None);
    }
}
