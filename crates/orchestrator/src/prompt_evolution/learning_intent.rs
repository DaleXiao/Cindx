use super::{FrozenPromptProfileSnapshot, ProTeacherAttestationV1};
use crate::sha256_hex;
use agent_core::{Metadata, TaskId};
use serde::{Deserialize, Serialize};

const AUTO_TRANSFER_INTENT_SCHEMA: &str = "cindx.prompt-auto-transfer-intent.v1";
const AUTO_TRANSFER_INTENT_PREFIX: &str = "prompt-auto-transfer-";
const PRO_DISTILLATION_INTENT_PREFIX: &str = "prompt-pro-distillation-";
const PROMPT_EVALUATION_REQUEST_PREFIX: &str = "prompt-evaluation-";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptAutoTransferIntent {
    schema: String,
    intent_id: String,
    task_id: String,
    run_context: Metadata,
}

impl PromptAutoTransferIntent {
    pub fn from_context(task_id: &TaskId, run_context: &Metadata) -> Option<Self> {
        let project_id = run_context.get("project_id")?.trim();
        let source_id = ["agent_run_id", "collaboration_id"]
            .into_iter()
            .find_map(|key| {
                run_context
                    .get(key)
                    .map(String::as_str)
                    .map(str::trim)
                    .filter(|source_id| !source_id.is_empty())
            })?;
        if project_id.is_empty() {
            return None;
        }
        let steer_epoch = run_context
            .get("steer_epoch")
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("0");
        let digest = sha256_hex(
            format!("{}\n{project_id}\n{source_id}\n{steer_epoch}", task_id.0).as_bytes(),
        );
        Some(Self {
            schema: AUTO_TRANSFER_INTENT_SCHEMA.to_string(),
            intent_id: format!("{AUTO_TRANSFER_INTENT_PREFIX}{digest}"),
            task_id: task_id.0.clone(),
            run_context: persistent_auto_context(run_context),
        })
    }

    pub fn validate(&self) -> bool {
        self.schema == AUTO_TRANSFER_INTENT_SCHEMA
            && Self::from_context(&TaskId(self.task_id.clone()), &self.run_context).as_ref()
                == Some(self)
    }

    pub fn decode_for_project(
        project_id: &str,
        intent_id: &str,
        payload: &str,
    ) -> Result<Self, String> {
        let intent = serde_json::from_str::<Self>(payload)
            .map_err(|error| format!("prompt Auto transfer intent is invalid: {error}"))?;
        if !intent.validate()
            || intent.intent_id != intent_id
            || intent.project_id() != Some(project_id)
        {
            return Err("prompt Auto transfer intent identity is invalid".to_string());
        }
        Ok(intent)
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self)
            .map_err(|error| format!("prompt Auto transfer intent serialization failed: {error}"))
    }

    pub fn intent_id(&self) -> &str {
        &self.intent_id
    }

    pub fn schema(&self) -> &str {
        &self.schema
    }

    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    pub fn run_context(&self) -> &Metadata {
        &self.run_context
    }

    pub fn project_id(&self) -> Option<&str> {
        self.run_context.get("project_id").map(String::as_str)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptProDistillationIntent {
    intent_id: String,
    request_id: String,
    run_context: Metadata,
    snapshot: FrozenPromptProfileSnapshot,
    attestation: ProTeacherAttestationV1,
}

impl PromptProDistillationIntent {
    pub fn new(
        project_id: &str,
        stable_profile_id: &str,
        source_metadata: &Metadata,
        snapshot: FrozenPromptProfileSnapshot,
    ) -> Result<Self, String> {
        let project_id = project_id.trim();
        let stable_profile_id = stable_profile_id.trim();
        if project_id.is_empty() || stable_profile_id.is_empty() {
            return Err(
                "prompt Pro distillation project and stable profile are required".to_string(),
            );
        }
        let attestation =
            ProTeacherAttestationV1::from_stable_snapshot(&snapshot, stable_profile_id)?;
        let attestation_digest = attestation.digest()?;
        let run_context = persistent_pro_context(source_metadata, project_id);
        let intent_id = pro_distillation_intent_id(project_id, &attestation_digest, &run_context)?;
        Ok(Self {
            request_id: format!("{PROMPT_EVALUATION_REQUEST_PREFIX}{intent_id}"),
            intent_id,
            run_context,
            snapshot,
            attestation,
        })
    }

    pub fn validate(&self) -> bool {
        let Some(project_id) = self.project_id() else {
            return false;
        };
        if project_id.trim().is_empty()
            || project_id != project_id.trim()
            || persistent_pro_context(&self.run_context, project_id) != self.run_context
            || self.intent_id.trim().is_empty()
            || self.request_id != format!("{PROMPT_EVALUATION_REQUEST_PREFIX}{}", self.intent_id)
        {
            return false;
        }
        let Ok(expected_attestation) =
            ProTeacherAttestationV1::from_stable_snapshot(&self.snapshot, &self.snapshot.genome.id)
        else {
            return false;
        };
        expected_attestation == self.attestation
            && self.attestation.digest().is_ok_and(|digest| {
                pro_distillation_intent_id(project_id, &digest, &self.run_context)
                    .is_ok_and(|expected_id| self.intent_id == expected_id)
            })
    }

    pub fn decode_for_project(
        project_id: &str,
        intent_id: &str,
        payload: &str,
    ) -> Result<Self, String> {
        let intent = serde_json::from_str::<Self>(payload)
            .map_err(|error| format!("prompt Pro distillation intent is invalid: {error}"))?;
        if !intent.validate()
            || intent.intent_id != intent_id
            || intent.project_id() != Some(project_id)
        {
            return Err("prompt Pro distillation intent identity is invalid".to_string());
        }
        Ok(intent)
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|error| {
            format!("prompt Pro distillation intent serialization failed: {error}")
        })
    }

    pub fn matches_dispatch_marker(
        &self,
        project_id: &str,
        intent_id: &str,
        marker_context: &Metadata,
    ) -> bool {
        if !self.validate() || self.project_id() != Some(project_id) {
            return false;
        }
        if self.intent_id == intent_id {
            return true;
        }
        if persistent_pro_context(marker_context, project_id) != self.run_context {
            return false;
        }
        self.attestation.digest().is_ok_and(|digest| {
            previous_project_scoped_pro_distillation_intent_id(project_id, &digest) == intent_id
                || legacy_pro_distillation_intent_id(&digest) == intent_id
        })
    }

    pub fn intent_id(&self) -> &str {
        &self.intent_id
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn run_context(&self) -> &Metadata {
        &self.run_context
    }

    pub fn project_id(&self) -> Option<&str> {
        self.run_context.get("project_id").map(String::as_str)
    }

    pub fn snapshot(&self) -> &FrozenPromptProfileSnapshot {
        &self.snapshot
    }

    pub fn attestation(&self) -> &ProTeacherAttestationV1 {
        &self.attestation
    }
}

fn persistent_auto_context(context: &Metadata) -> Metadata {
    let mut normalized = [
        "agent_run_id",
        "collaboration_id",
        "project_id",
        "project_root",
        "session_id",
        "steer_epoch",
    ]
    .into_iter()
    .filter_map(|key| {
        context
            .get(key)
            .map(|value| (key.to_string(), value.clone()))
    })
    .collect::<Metadata>();
    for key in [
        "agent_run_id",
        "collaboration_id",
        "project_id",
        "session_id",
        "steer_epoch",
    ] {
        let Some(value) = normalized.get(key).map(|value| value.trim().to_string()) else {
            continue;
        };
        if value.is_empty() {
            normalized.remove(key);
        } else {
            normalized.insert(key.to_string(), value);
        }
    }
    normalized
}

fn persistent_pro_context(metadata: &Metadata, project_id: &str) -> Metadata {
    let mut context = [
        "agent_run_id",
        "collaboration_policy",
        "project_root",
        "session_id",
        "task_class",
    ]
    .into_iter()
    .filter_map(|key| {
        metadata
            .get(key)
            .map(|value| (key.to_string(), value.clone()))
    })
    .collect::<Metadata>();
    context.insert("project_id".to_string(), project_id.to_string());
    context.insert("agent_effort".to_string(), "auto".to_string());
    context
}

fn pro_distillation_intent_id(
    project_id: &str,
    attestation_digest: &str,
    run_context: &Metadata,
) -> Result<String, String> {
    let context = serde_json::to_vec(run_context).map_err(|error| {
        format!("prompt Pro distillation context serialization failed: {error}")
    })?;
    let context_digest = sha256_hex(&context);
    let digest =
        sha256_hex(format!("{project_id}\n{attestation_digest}\n{context_digest}").as_bytes());
    Ok(format!("{PRO_DISTILLATION_INTENT_PREFIX}{digest}"))
}

fn previous_project_scoped_pro_distillation_intent_id(
    project_id: &str,
    attestation_digest: &str,
) -> String {
    let digest = sha256_hex(format!("{project_id}\n{attestation_digest}").as_bytes());
    format!("{PRO_DISTILLATION_INTENT_PREFIX}{digest}")
}

fn legacy_pro_distillation_intent_id(attestation_digest: &str) -> String {
    format!("{PRO_DISTILLATION_INTENT_PREFIX}{attestation_digest}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ConductorPromptGenome, FrozenPromptSourceProfileLineageV1, FrozenPromptTransferEvidence,
        PROMPT_AUTO_TRANSFER_GATE_PROTOCOL,
    };

    fn certified_pro_snapshot(mutation_index: usize) -> FrozenPromptProfileSnapshot {
        let seed = ConductorPromptGenome::seed_for_effort("pro");
        let genome = seed
            .mutations()
            .into_iter()
            .nth(mutation_index)
            .expect("Pro seed should expose deterministic mutations");
        FrozenPromptProfileSnapshot::new_gepa(
            "pro",
            genome,
            seed.id,
            "1".repeat(64),
            format!("{:064x}", mutation_index + 2),
        )
        .unwrap()
        .with_auto_teacher_evidence(FrozenPromptTransferEvidence {
            source_effort: "auto".to_string(),
            source_profile_id: "auto-source".to_string(),
            source_profile_sha256: "3".repeat(64),
            dataset_sha256: "4".repeat(64),
            cohort_sha256: Some("5".repeat(64)),
            paired_evidence_sha256: "6".repeat(64),
            promotion_gate_protocol: PROMPT_AUTO_TRANSFER_GATE_PROTOCOL.to_string(),
            source_profile_lineage: Some(
                FrozenPromptSourceProfileLineageV1::undistilled("3".repeat(64)).unwrap(),
            ),
        })
        .unwrap()
    }

    fn auto_context(project_id: &str) -> Metadata {
        [
            ("project_id".to_string(), project_id.to_string()),
            ("agent_run_id".to_string(), "run".to_string()),
            ("steer_epoch".to_string(), "2".to_string()),
            ("transient".to_string(), "must-not-persist".to_string()),
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn auto_intent_wire_and_identity_are_golden_and_transient_free() {
        let intent = PromptAutoTransferIntent::from_context(
            &TaskId("task".to_string()),
            &auto_context("project"),
        )
        .unwrap();
        let expected_id =
            "prompt-auto-transfer-a1461f8e4750e4bbc36b29bbbb6f47b792223210434d3183e14e6320495e2683";
        assert_eq!(intent.intent_id(), expected_id);
        assert_eq!(
            intent.to_json().unwrap(),
            format!(
                "{{\"schema\":\"cindx.prompt-auto-transfer-intent.v1\",\"intent_id\":\"{expected_id}\",\"task_id\":\"task\",\"run_context\":{{\"agent_run_id\":\"run\",\"project_id\":\"project\",\"steer_epoch\":\"2\"}}}}"
            )
        );
        assert!(intent.validate());
    }

    #[test]
    fn auto_decode_rejects_payload_or_scope_tampering() {
        let intent = PromptAutoTransferIntent::from_context(
            &TaskId("task".to_string()),
            &auto_context("project-a"),
        )
        .unwrap();
        let payload = intent.to_json().unwrap();
        assert_eq!(
            PromptAutoTransferIntent::decode_for_project(
                "project-a",
                intent.intent_id(),
                &payload,
            )
            .unwrap(),
            intent
        );
        assert!(PromptAutoTransferIntent::decode_for_project(
            "project-b",
            intent.intent_id(),
            &payload,
        )
        .is_err());

        let mut tampered = intent.clone();
        tampered.task_id = "other-task".to_string();
        assert!(!tampered.validate());
    }

    #[test]
    fn auto_identity_normalizes_scope_and_falls_back_from_an_empty_run_id() {
        let noisy = [
            ("project_id".to_string(), " project ".to_string()),
            ("agent_run_id".to_string(), "   ".to_string()),
            (
                "collaboration_id".to_string(),
                " collaboration ".to_string(),
            ),
            ("steer_epoch".to_string(), " 2 ".to_string()),
        ]
        .into_iter()
        .collect();
        let canonical = [
            ("project_id".to_string(), "project".to_string()),
            ("collaboration_id".to_string(), "collaboration".to_string()),
            ("steer_epoch".to_string(), "2".to_string()),
        ]
        .into_iter()
        .collect();
        let task_id = TaskId("task".to_string());
        let normalized = PromptAutoTransferIntent::from_context(&task_id, &noisy).unwrap();
        let expected = PromptAutoTransferIntent::from_context(&task_id, &canonical).unwrap();

        assert_eq!(normalized, expected);
        assert!(normalized.validate());
    }

    #[test]
    fn pro_intent_is_project_and_normalized_context_scoped() {
        let snapshot = certified_pro_snapshot(0);
        let stable_profile_id = snapshot.genome.id.clone();
        let first_source = [
            ("agent_run_id".to_string(), "run-a".to_string()),
            ("transient".to_string(), "ignored-a".to_string()),
        ]
        .into_iter()
        .collect::<Metadata>();
        let mut same_source = first_source.clone();
        same_source.insert("transient".to_string(), "ignored-b".to_string());
        let first = PromptProDistillationIntent::new(
            "project-a",
            &stable_profile_id,
            &first_source,
            snapshot.clone(),
        )
        .unwrap();
        let same = PromptProDistillationIntent::new(
            "project-a",
            &stable_profile_id,
            &same_source,
            snapshot.clone(),
        )
        .unwrap();
        let different_project = PromptProDistillationIntent::new(
            "project-b",
            &stable_profile_id,
            &first_source,
            snapshot,
        )
        .unwrap();

        assert_eq!(first.intent_id(), same.intent_id());
        assert_ne!(first.intent_id(), different_project.intent_id());
        assert_eq!(first.run_context().get("transient"), None);
        assert_eq!(
            first.run_context().get("agent_effort").map(String::as_str),
            Some("auto")
        );
        assert!(first.validate());
    }

    #[test]
    fn pro_wire_order_decode_and_all_dispatch_marker_generations_are_stable() {
        let snapshot = certified_pro_snapshot(0);
        let stable_profile_id = snapshot.genome.id.clone();
        let source = [("agent_run_id".to_string(), "run".to_string())]
            .into_iter()
            .collect();
        let intent =
            PromptProDistillationIntent::new("project", &stable_profile_id, &source, snapshot)
                .unwrap();
        let payload = intent.to_json().unwrap();
        let intent_offset = payload.find("\"intent_id\"").unwrap();
        let request_offset = payload.find("\"request_id\"").unwrap();
        let context_offset = payload.find("\"run_context\"").unwrap();
        let snapshot_offset = payload.find("\"snapshot\"").unwrap();
        let attestation_offset = payload.find("\"attestation\"").unwrap();
        assert!(intent_offset < request_offset);
        assert!(request_offset < context_offset);
        assert!(context_offset < snapshot_offset);
        assert!(snapshot_offset < attestation_offset);
        assert_eq!(
            PromptProDistillationIntent::decode_for_project(
                "project",
                intent.intent_id(),
                &payload,
            )
            .unwrap(),
            intent
        );

        let digest = intent.attestation().digest().unwrap();
        let previous = previous_project_scoped_pro_distillation_intent_id("project", &digest);
        let legacy = legacy_pro_distillation_intent_id(&digest);
        assert!(intent.matches_dispatch_marker("project", intent.intent_id(), &Metadata::new()));
        assert!(intent.matches_dispatch_marker("project", &previous, &source));
        assert!(intent.matches_dispatch_marker("project", &legacy, &source));
        let stale_source = [("agent_run_id".to_string(), "stale-run".to_string())]
            .into_iter()
            .collect();
        assert!(!intent.matches_dispatch_marker("project", &legacy, &stale_source));
        assert!(!intent.matches_dispatch_marker("other-project", &previous, &source));
        assert!(!intent.matches_dispatch_marker("project", "unknown", &source));
    }

    #[test]
    fn pro_validation_rejects_context_and_request_tampering() {
        let snapshot = certified_pro_snapshot(0);
        let stable_profile_id = snapshot.genome.id.clone();
        let source = Metadata::new();
        let intent =
            PromptProDistillationIntent::new("project", &stable_profile_id, &source, snapshot)
                .unwrap();

        let mut wrong_request = intent.clone();
        wrong_request.request_id.push_str("-tampered");
        assert!(!wrong_request.validate());

        let mut wrong_context = intent;
        wrong_context
            .run_context
            .insert("unexpected".to_string(), "value".to_string());
        assert!(!wrong_context.validate());
    }
}
