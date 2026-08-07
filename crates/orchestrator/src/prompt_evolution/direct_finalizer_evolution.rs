use super::{ConductorPromptGenome, PromptVerification};
use crate::AgentEvaluationReflectionPacket;
use serde::{Deserialize, Serialize};

pub const DIRECT_FINALIZER_GEPA_MAX_TRAJECTORIES: usize = 6;
const DIRECT_FINALIZER_GEPA_MAX_FEEDBACK_BYTES: usize = 192 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectFinalizerGepaDecision {
    Promote,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectFinalizerGepaDecisionResponse {
    decision: DirectFinalizerGepaDecision,
    candidate_value: PromptVerification,
}

impl ConductorPromptGenome {
    pub fn direct_finalizer_candidate_decision_prompt(
        &self,
        candidate: &Self,
        trajectories: &[AgentEvaluationReflectionPacket],
    ) -> Result<String, String> {
        validate_exercised_candidate(self, candidate)?;
        let trajectories = direct_finalizer_trajectories_json(trajectories)?;
        Ok(format!(
            concat!(
                "You are applying GEPA-style reflective selection to one exact Cindx Direct-finalizer candidate. ",
                "The candidate has already been exercised against the parent on the supplied matched, redacted training trajectories. ",
                "Decide whether that exact candidate should be promoted; do not propose or synthesize another candidate. ",
                "Return exactly one strict JSON object with this schema and no commentary: ",
                "{{\"decision\":\"promote|reject\",\"candidate_value\":\"minimal|evidence|adversarial\"}}. ",
                "candidate_value must exactly equal the exercised candidate's direct_finalizer_verification value. ",
                "Use reject when evidence is insufficient, regressive, unsafe, or ambiguous. ",
                "Do not copy user text, case identifiers, benchmark answers, secrets, or model names.\n\n",
                "Parent genome:\n{}\n\nExact exercised candidate:\n{}\n\nMatched training trajectories:\n{}"
            ),
            direct_finalizer_genome_json(self)?,
            direct_finalizer_genome_json(candidate)?,
            trajectories,
        ))
    }

    pub fn direct_finalizer_candidate_decision_repair_prompt(
        &self,
        candidate: &Self,
        invalid_response: &str,
        error: &str,
    ) -> Result<String, String> {
        validate_exercised_candidate(self, candidate)?;
        Ok(format!(
            concat!(
                "Repair one rejected GEPA decision for an exact Cindx Direct-finalizer candidate. ",
                "Return exactly one strict JSON object with no commentary or extra fields: ",
                "{{\"decision\":\"promote|reject\",\"candidate_value\":\"{}\"}}. ",
                "Do not change candidate_value, propose a different candidate, or add case content, secrets, benchmark answers, or model names.\n\n",
                "Parent genome:\n{}\n\nExact exercised candidate:\n{}\n\nValidation error:\n{}\n\nRejected decision:\n{}"
            ),
            prompt_verification_value(candidate.direct_finalizer_verification),
            direct_finalizer_genome_json(self)?,
            direct_finalizer_genome_json(candidate)?,
            error,
            invalid_response,
        ))
    }

    pub fn direct_finalizer_decision_from_response(
        &self,
        candidate: &Self,
        response: &str,
    ) -> Result<DirectFinalizerGepaDecision, String> {
        validate_exercised_candidate(self, candidate)?;
        let decision = serde_json::from_str::<DirectFinalizerGepaDecisionResponse>(response.trim())
            .map_err(|error| format!("direct finalizer GEPA decision JSON is invalid: {error}"))?;
        if decision.candidate_value != candidate.direct_finalizer_verification {
            return Err(
                "direct finalizer GEPA decision candidate_value does not match the exact exercised candidate"
                    .to_string(),
            );
        }
        Ok(decision.decision)
    }

    pub fn direct_finalizer_mutations(&self) -> Result<Vec<Self>, String> {
        self.validate()?;
        let next_generation = self.generation.saturating_add(1);
        let mut variants = Vec::with_capacity(2);
        for verification in [
            PromptVerification::Minimal,
            PromptVerification::Evidence,
            PromptVerification::Adversarial,
        ] {
            if verification == self.direct_finalizer_verification {
                continue;
            }
            let mut variant = self.clone();
            // Candidate identifiers are deliberately opaque: the mutation label
            // must not reveal the hidden treatment to reflection or review.
            variant.id = format!(
                "{}-g{}-direct-finalizer-candidate-{:02}",
                self.id,
                next_generation,
                variants.len() + 1
            );
            variant.generation = next_generation;
            variant.parents = vec![self.id.clone()];
            variant.direct_finalizer_verification = verification;
            variant.validate()?;
            variants.push(variant);
        }
        Ok(variants)
    }
}

fn direct_finalizer_trajectories_json(
    trajectories: &[AgentEvaluationReflectionPacket],
) -> Result<String, String> {
    if trajectories.is_empty() {
        return Err(
            "direct finalizer evolution requires at least one matched trajectory".to_string(),
        );
    }
    if trajectories.len() > DIRECT_FINALIZER_GEPA_MAX_TRAJECTORIES {
        return Err(format!(
            "direct finalizer evolution accepts at most {DIRECT_FINALIZER_GEPA_MAX_TRAJECTORIES} trajectories"
        ));
    }
    let trajectories = serde_json::to_string_pretty(trajectories)
        .map_err(|error| format!("could not serialize direct finalizer trajectories: {error}"))?;
    if trajectories.len() > DIRECT_FINALIZER_GEPA_MAX_FEEDBACK_BYTES {
        return Err("direct finalizer trajectory evidence exceeds 192 KiB".to_string());
    }
    Ok(trajectories)
}

fn validate_exercised_candidate(
    parent: &ConductorPromptGenome,
    candidate: &ConductorPromptGenome,
) -> Result<(), String> {
    parent.validate()?;
    candidate.validate()?;
    if candidate.direct_finalizer_verification == parent.direct_finalizer_verification {
        return Err(
            "exact direct finalizer candidate must change direct_finalizer_verification"
                .to_string(),
        );
    }
    if candidate.id.trim().is_empty()
        || candidate.id == parent.id
        || candidate.generation != parent.generation.saturating_add(1)
        || candidate.parents != [parent.id.clone()]
    {
        return Err("exact direct finalizer candidate lineage is invalid".to_string());
    }
    let mut normalized = candidate.clone();
    normalized.id.clone_from(&parent.id);
    normalized.generation = parent.generation;
    normalized.parents.clone_from(&parent.parents);
    normalized.direct_finalizer_verification = parent.direct_finalizer_verification;
    if normalized != *parent {
        return Err(
            "exact direct finalizer candidate changed a gene outside its one-gene mask".to_string(),
        );
    }
    Ok(())
}

fn prompt_verification_value(value: PromptVerification) -> &'static str {
    match value {
        PromptVerification::Minimal => "minimal",
        PromptVerification::Evidence => "evidence",
        PromptVerification::Adversarial => "adversarial",
    }
}

fn direct_finalizer_genome_json(genome: &ConductorPromptGenome) -> Result<String, String> {
    let mut value = serde_json::to_value(genome)
        .map_err(|error| format!("could not serialize direct finalizer genome: {error}"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| "direct finalizer genome did not serialize as an object".to_string())?;
    object.insert(
        "direct_finalizer_verification".to_string(),
        serde_json::to_value(genome.direct_finalizer_verification)
            .map_err(|error| format!("could not serialize direct finalizer policy: {error}"))?,
    );
    serde_json::to_string_pretty(&value)
        .map_err(|error| format!("could not encode direct finalizer genome: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn trajectory() -> AgentEvaluationReflectionPacket {
        AgentEvaluationReflectionPacket {
            suite_id: "redacted-suite".to_string(),
            suite_version: 1,
            case_id: "redacted-case".to_string(),
            category: "completion".to_string(),
            run_id: "redacted-run".to_string(),
            seed: 7,
            candidate_id: "opaque-candidate".to_string(),
            candidate_fingerprint: "a".repeat(64),
            model_fingerprints: BTreeMap::new(),
            input: "[REDACTED]".to_string(),
            steps: Vec::new(),
            final_output: "[REDACTED]".to_string(),
            verifier: crate::AgentEvaluationVerifierOutcome {
                source: crate::AgentEvaluationEvidenceSource::Judge,
                passed: true,
                score: 1.0,
                checks: Vec::new(),
            },
            actionable_feedback: crate::ActionableSideInformation {
                summary: "The exact candidate improved verified completion.".to_string(),
                passed_constraints: Vec::new(),
                failed_constraints: Vec::new(),
                errors: Vec::new(),
                suggested_changes: Vec::new(),
            },
        }
    }

    fn adversarial_candidate(parent: &ConductorPromptGenome) -> ConductorPromptGenome {
        parent
            .direct_finalizer_mutations()
            .unwrap()
            .into_iter()
            .find(|candidate| {
                candidate.direct_finalizer_verification == PromptVerification::Adversarial
            })
            .unwrap()
    }

    #[test]
    fn exact_candidate_decision_prompt_contains_parent_candidate_and_matched_evidence() {
        let parent = ConductorPromptGenome::seed_for_effort("auto");
        let candidate = adversarial_candidate(&parent);
        let prompt = parent
            .direct_finalizer_candidate_decision_prompt(&candidate, &[trajectory()])
            .unwrap();

        assert!(prompt.contains("Parent genome:"));
        assert!(prompt.contains("Exact exercised candidate:"));
        assert!(prompt.contains("Matched training trajectories:"));
        assert!(prompt.contains("\"candidate_value\":\"minimal|evidence|adversarial\""));
        assert!(prompt.contains("\"direct_finalizer_verification\": \"evidence\""));
        assert!(prompt.contains("\"direct_finalizer_verification\": \"adversarial\""));
    }

    #[test]
    fn exact_candidate_decision_parser_accepts_promote_and_reject() {
        let parent = ConductorPromptGenome::seed_for_effort("auto");
        let candidate = adversarial_candidate(&parent);

        assert_eq!(
            parent
                .direct_finalizer_decision_from_response(
                    &candidate,
                    r#"{"decision":"promote","candidate_value":"adversarial"}"#,
                )
                .unwrap(),
            DirectFinalizerGepaDecision::Promote
        );
        assert_eq!(
            parent
                .direct_finalizer_decision_from_response(
                    &candidate,
                    r#"{"decision":"reject","candidate_value":"adversarial"}"#,
                )
                .unwrap(),
            DirectFinalizerGepaDecision::Reject
        );
    }

    #[test]
    fn exact_candidate_decision_parser_is_strict_and_candidate_bound() {
        let parent = ConductorPromptGenome::seed_for_effort("auto");
        let candidate = adversarial_candidate(&parent);

        for invalid in [
            r#"{"decision":"promote","candidate_value":"minimal"}"#,
            r#"{"decision":"promote","candidate_value":"adversarial","extra":true}"#,
            "```json\n{\"decision\":\"promote\",\"candidate_value\":\"adversarial\"}\n```",
        ] {
            assert!(parent
                .direct_finalizer_decision_from_response(&candidate, invalid)
                .is_err());
        }
    }

    #[test]
    fn exact_candidate_decision_repair_is_bound_to_the_same_value() {
        let parent = ConductorPromptGenome::seed_for_effort("auto");
        let candidate = adversarial_candidate(&parent);
        let repair = parent
            .direct_finalizer_candidate_decision_repair_prompt(
                &candidate,
                "not-json",
                "invalid JSON",
            )
            .unwrap();

        assert!(repair.contains(r#"{"decision":"promote|reject","candidate_value":"adversarial"}"#));
        assert!(repair.contains("Exact exercised candidate:"));
        assert!(repair.contains("Rejected decision:\nnot-json"));
    }

    #[test]
    fn exact_candidate_decision_rejects_an_off_mask_candidate() {
        let parent = ConductorPromptGenome::seed_for_effort("auto");
        let mut candidate = adversarial_candidate(&parent);
        candidate.verification = PromptVerification::Minimal;

        assert!(parent
            .direct_finalizer_candidate_decision_prompt(&candidate, &[trajectory()])
            .unwrap_err()
            .contains("outside its one-gene mask"));
        assert!(parent
            .direct_finalizer_decision_from_response(
                &candidate,
                r#"{"decision":"promote","candidate_value":"adversarial"}"#,
            )
            .unwrap_err()
            .contains("outside its one-gene mask"));
    }

    #[test]
    fn direct_mutation_space_contains_only_the_two_other_policies() {
        let parent = ConductorPromptGenome::seed_for_effort("auto");
        let variants = parent
            .direct_finalizer_mutations()
            .expect("direct variants should be valid");

        assert_eq!(variants.len(), 2);
        assert_eq!(
            variants
                .iter()
                .map(|variant| variant.direct_finalizer_verification)
                .collect::<std::collections::BTreeSet<_>>(),
            [PromptVerification::Minimal, PromptVerification::Adversarial]
                .into_iter()
                .collect()
        );
        for variant in variants {
            let mut normalized = variant.clone();
            normalized.id.clone_from(&parent.id);
            normalized.generation = parent.generation;
            normalized.parents.clone_from(&parent.parents);
            normalized.direct_finalizer_verification = parent.direct_finalizer_verification;
            assert_eq!(normalized, parent);
        }
    }

    #[test]
    fn workflow_parser_rejects_direct_gene_changes() {
        let parent = ConductorPromptGenome::seed_for_effort("auto");
        let mut candidate = parent.clone();
        candidate.direct_finalizer_verification = PromptVerification::Adversarial;
        candidate.context_policy = super::super::PromptContextPolicy::Comprehensive;
        let response = serde_json::to_string(&candidate).expect("candidate should serialize");

        assert!(parent
            .learned_mutation_from_response(&response, "invalid-workflow-child")
            .unwrap_err()
            .contains("must not change the direct finalizer gene"));
    }
}
