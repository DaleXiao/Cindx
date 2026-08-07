use crate::prompt_profile_serving::{
    restore_prompt_profile_selection, PromptProfileAssignmentSource,
};
use agent_core::Metadata;
use orchestrator::{
    sha256_hex, PromptEvaluationProvenance, PromptLiveAssignmentProvenanceV1,
    PROMPT_LIVE_ASSIGNMENT_PROVENANCE_SCHEMA_V1,
};

pub(super) struct ValidatedLivePromptAssignment {
    pub(super) receipt_sha256: String,
    pub(super) provenance: PromptEvaluationProvenance,
}

pub(super) fn validated_live_prompt_assignment(
    metadata: &Metadata,
    scope: &str,
    effort: &str,
    profile_id: &str,
) -> Option<ValidatedLivePromptAssignment> {
    let selection = restore_prompt_profile_selection(effort, scope, metadata)
        .ok()
        .flatten()?;
    let receipt = &selection.receipt;
    if !matches!(
        receipt.source,
        PromptProfileAssignmentSource::Stable | PromptProfileAssignmentSource::Canary
    ) || selection.genome.id != profile_id
        || receipt.profile_id != profile_id
    {
        return None;
    }
    let source_revision = receipt.source_revision.filter(|revision| *revision > 0)?;
    let deployment_generation = receipt
        .deployment_generation
        .filter(|generation| *generation > 0)?;
    let scope_sha256 = receipt
        .scope_sha256
        .as_ref()
        .filter(|digest| is_sha256(digest))?
        .clone();
    let receipt_sha256 = metadata
        .get("prompt_profile_assignment_sha256")
        .filter(|digest| is_sha256(digest))?
        .clone();
    let distillation_lease_sha256 = match receipt.distillation_lease.as_ref() {
        Some(lease) => Some(sha256_hex(&serde_json::to_vec(lease).ok()?)),
        None => None,
    };
    let live_assignment = PromptLiveAssignmentProvenanceV1 {
        schema: PROMPT_LIVE_ASSIGNMENT_PROVENANCE_SCHEMA_V1.to_string(),
        assignment_receipt_sha256: receipt_sha256.clone(),
        assignment_source: receipt.source.label().to_string(),
        profile_id: receipt.profile_id.clone(),
        profile_sha256: receipt.profile_sha256.clone(),
        scope_sha256,
        source_revision,
        deployment_generation,
        distillation_lease_sha256,
    };
    live_assignment.validate().ok()?;
    let provenance = PromptEvaluationProvenance {
        protocol: PROMPT_LIVE_ASSIGNMENT_PROVENANCE_SCHEMA_V1.to_string(),
        candidate_prompt_sha256: receipt.profile_sha256.clone(),
        live_assignment: Some(live_assignment),
        ..PromptEvaluationProvenance::default()
    };
    Some(ValidatedLivePromptAssignment {
        receipt_sha256,
        provenance,
    })
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt_profile_serving::{
        seed_prompt_profile_selection, PromptProfileAssignmentSource,
        PromptProfileDistillationLease, PromptProfileFallback,
    };
    use agent_core::{AGENT_RUN_ID_METADATA_KEY, LOGICAL_AGENT_RUN_ID_METADATA_KEY};

    fn deployed_context(
        configure: impl FnOnce(&mut crate::prompt_profile_serving::PromptProfileSelection),
    ) -> Metadata {
        let mut context = [
            (
                AGENT_RUN_ID_METADATA_KEY.to_string(),
                "run-live-1".to_string(),
            ),
            (
                LOGICAL_AGENT_RUN_ID_METADATA_KEY.to_string(),
                "logical-live-1".to_string(),
            ),
        ]
        .into_iter()
        .collect::<Metadata>();
        let mut selection =
            seed_prompt_profile_selection("auto", PromptProfileFallback::NoDeployment, &context);
        selection.receipt.source = PromptProfileAssignmentSource::Stable;
        selection.receipt.fallback = None;
        selection.receipt.scope_sha256 =
            Some(crate::prompt_profile_serving::scope_sha256("project-live"));
        selection.receipt.source_revision = Some(7);
        selection.receipt.deployment_generation = Some(3);
        selection.receipt.rollout_status = Some("canary".to_string());
        configure(&mut selection);
        let (receipt, receipt_sha256) = selection.receipt_json_and_sha256().unwrap();
        context.insert("prompt_profile".to_string(), selection.genome.id.clone());
        context.insert(
            "prompt_genome".to_string(),
            serde_json::to_string(&selection.genome).unwrap(),
        );
        context.insert("prompt_profile_source".to_string(), "stable".to_string());
        context.insert("prompt_profile_assignment_receipt".to_string(), receipt);
        context.insert(
            "prompt_profile_assignment_sha256".to_string(),
            receipt_sha256,
        );
        context.insert("prompt_rollout_status".to_string(), "canary".to_string());
        context
    }

    #[test]
    fn live_assignment_requires_the_exact_receipt_genome_and_deployment_lineage() {
        let context = deployed_context(|_| {});
        let profile_id = context["prompt_profile"].clone();
        let validated =
            validated_live_prompt_assignment(&context, "project-live", "auto", &profile_id)
                .expect("exact deployed assignment should be attributable");
        assert_eq!(
            validated
                .provenance
                .live_assignment
                .as_ref()
                .map(|assignment| assignment.scope_sha256.as_str()),
            Some(crate::prompt_profile_serving::scope_sha256("project-live").as_str())
        );

        for key in ["prompt_profile_assignment_sha256", "prompt_genome"] {
            let mut tampered = context.clone();
            tampered.insert(key.to_string(), "0".repeat(64));
            assert!(validated_live_prompt_assignment(
                &tampered,
                "project-live",
                "auto",
                &profile_id,
            )
            .is_none());
        }
        assert!(
            validated_live_prompt_assignment(&context, "project-other", "auto", &profile_id,)
                .is_none()
        );

        let distilled = deployed_context(|selection| {
            let candidate_sha256 = "c".repeat(64);
            selection.receipt.canary_profile_sha256 = Some(candidate_sha256.clone());
            selection.receipt.distillation_lease = Some(PromptProfileDistillationLease {
                schema: crate::runtime_constants::PROMPT_DISTILLATION_CANARY_LEASE_SCHEMA_V1
                    .to_string(),
                candidate_profile_id: "distilled-candidate".to_string(),
                candidate_profile_sha256: candidate_sha256,
                stable_profile_id: selection.receipt.profile_id.clone(),
                stable_profile_sha256: selection.receipt.profile_sha256.clone(),
                cohort_sha256: "a".repeat(64),
                paired_evidence_sha256: "b".repeat(64),
            });
        });
        let distilled_assignment =
            validated_live_prompt_assignment(&distilled, "project-live", "auto", &profile_id)
                .expect("matched distillation lease should remain attributable");
        assert!(distilled_assignment
            .provenance
            .live_assignment
            .unwrap()
            .distillation_lease_sha256
            .is_some());

        let tampered_lease = deployed_context(|selection| {
            selection.receipt.canary_profile_sha256 = Some("c".repeat(64));
            selection.receipt.distillation_lease = Some(PromptProfileDistillationLease {
                schema: crate::runtime_constants::PROMPT_DISTILLATION_CANARY_LEASE_SCHEMA_V1
                    .to_string(),
                candidate_profile_id: "distilled-candidate".to_string(),
                candidate_profile_sha256: "d".repeat(64),
                stable_profile_id: selection.receipt.profile_id.clone(),
                stable_profile_sha256: selection.receipt.profile_sha256.clone(),
                cohort_sha256: "a".repeat(64),
                paired_evidence_sha256: "b".repeat(64),
            });
        });
        assert!(validated_live_prompt_assignment(
            &tampered_lease,
            "project-live",
            "auto",
            &profile_id,
        )
        .is_none());

        let zero_revision = deployed_context(|selection| {
            selection.receipt.source_revision = Some(0);
        });
        assert!(validated_live_prompt_assignment(
            &zero_revision,
            "project-live",
            "auto",
            &profile_id,
        )
        .is_none());
        println!("cindx.prompt-live-assignment-attribution.v1");
    }
}
