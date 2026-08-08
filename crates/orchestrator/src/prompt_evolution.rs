mod auto_teacher_source;
mod direct_finalizer_evolution;
mod distillation;
mod distillation_snapshot;
mod failure_curriculum;
mod fitness;
mod genome;
mod learning_dataset;
mod learning_intent;
mod learning_outbox;
mod matched_evaluation;
mod observation;
mod pareto;
mod phenotype;
mod pro_teacher_source;
mod reflection_selection;
mod search;
mod snapshot;

pub use auto_teacher_source::*;
pub use direct_finalizer_evolution::*;
pub use distillation::*;
pub use distillation_snapshot::*;
pub use failure_curriculum::*;
pub use genome::*;
pub use learning_dataset::*;
pub use learning_intent::*;
pub use learning_outbox::*;
pub use matched_evaluation::*;
pub use observation::*;
pub use pareto::*;
pub use phenotype::{DirectFinalizerPromptPhenotype, DIRECT_FINALIZER_PHENOTYPE_SCHEMA};
pub use pro_teacher_source::*;
pub use reflection_selection::{
    prompt_reflection_success_anchor, prompt_transfer_reflection_pairs,
    PROMPT_REFLECTION_SELECTOR_SCHEMA_V1,
};
pub use snapshot::*;

#[cfg(test)]
mod tests {
    use super::pareto::PROMOTION_MIN_LOWER_BOUND;
    use super::*;
    use crate::{AgentEvaluationCaseScore, AgentEvaluationReflectionPacket, AgentEvaluationSplit};
    use std::collections::{BTreeMap, BTreeSet};

    fn scientific_provenance(candidate_id: &str, opponent_id: &str) -> PromptEvaluationProvenance {
        PromptEvaluationProvenance::blind_pairwise_swap(
            vec!["independent-judge".to_string()],
            vec!["candidate-worker".to_string()],
            "d".repeat(64),
            crate::sha256_hex(candidate_id.as_bytes()),
            crate::sha256_hex(opponent_id.as_bytes()),
        )
    }

    fn observation(
        profile_id: &str,
        split: PromptEvaluationSplit,
        quality: f64,
        latency_ms: u64,
        tokens: u64,
    ) -> PromptEvolutionObservation {
        static OBSERVATION_SEQUENCE: std::sync::atomic::AtomicU64 =
            std::sync::atomic::AtomicU64::new(0);
        let sequence = OBSERVATION_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        PromptEvolutionObservation {
            profile_id: profile_id.to_string(),
            evaluation_id: format!("eval-{profile_id}-{latency_ms}-{tokens}-{sequence}"),
            case_id: format!("case-{latency_ms}-{tokens}"),
            opponent_profile_id: Some("baseline".to_string()),
            task_class: "coding".to_string(),
            split,
            mode: match split {
                PromptEvaluationSplit::Train => PromptEvaluationMode::PairedExecution,
                PromptEvaluationSplit::Holdout => PromptEvaluationMode::ReplayExecution,
            },
            format_valid: true,
            succeeded: true,
            quality_score: quality,
            latency_ms,
            total_tokens: tokens,
            estimated_cost_microusd: tokens * 2,
            safety_violations: 0,
            relative_reward: Some(quality - 0.75),
            step_credits: vec![PromptStepCredit {
                step_id: "final".to_string(),
                role: "synthesizer".to_string(),
                succeeded: true,
                attempts: 1,
                evidence_count: 1,
                latency_ms,
                total_tokens: tokens,
                credit: quality,
            }],
            reflection_packet: None,
            provenance: scientific_provenance(profile_id, "baseline"),
        }
    }

    fn instance_score(
        profile_id: &str,
        case_id: &str,
        run: usize,
        score: f64,
    ) -> AgentEvaluationCaseScore {
        AgentEvaluationCaseScore {
            suite_id: "pareto-suite".to_string(),
            suite_version: 2,
            case_id: case_id.to_string(),
            category: "coding".to_string(),
            split: AgentEvaluationSplit::Pareto,
            run_id: format!("{profile_id}-{case_id}-{run}"),
            seed: run as u64,
            candidate_id: profile_id.to_string(),
            candidate_fingerprint: format!("fingerprint-{profile_id}"),
            evidence_source: crate::AgentEvaluationEvidenceSource::Judge,
            score,
            verified_success: score >= 0.5,
            latency_ms: 100,
            total_tokens: 100,
            safety_violations: 0,
        }
    }

    #[test]
    fn non_independent_evaluation_cannot_train_prompt_evolution() {
        let mut evidence = observation("candidate", PromptEvaluationSplit::Train, 0.9, 100, 100);
        evidence.provenance.evaluator_independent = false;

        assert!(!evidence.is_scientific_evidence());
        assert!(prompt_reflection_packets(&[evidence], "candidate", 1).is_empty());

        let overlapping = PromptEvaluationProvenance::blind_pairwise_swap(
            vec!["shared-model".to_string()],
            vec!["shared-model".to_string()],
            "d".repeat(64),
            crate::sha256_hex(b"candidate"),
            crate::sha256_hex(b"opponent"),
        );
        assert!(!overlapping.evaluator_independent);
        assert!(!overlapping.is_scientific());
    }

    fn paired_minibatch_observations(
        proposal: &ConductorPromptGenome,
        parent: &ConductorPromptGenome,
        rewards: &[f64],
    ) -> Vec<PromptEvolutionObservation> {
        rewards
            .iter()
            .enumerate()
            .flat_map(|(index, reward)| {
                let evaluation_id = format!("minibatch-{index}");
                let case_id = format!("case-{index}");
                let mut proposal_observation = observation(
                    &proposal.id,
                    PromptEvaluationSplit::Train,
                    (0.7 + reward / 2.0).clamp(0.0, 1.0),
                    100,
                    100,
                );
                proposal_observation.evaluation_id = evaluation_id.clone();
                proposal_observation.case_id = case_id.clone();
                proposal_observation.opponent_profile_id = Some(parent.id.clone());
                proposal_observation.relative_reward = Some(*reward);
                let mut parent_observation = observation(
                    &parent.id,
                    PromptEvaluationSplit::Train,
                    (0.7 - reward / 2.0).clamp(0.0, 1.0),
                    100,
                    100,
                );
                parent_observation.evaluation_id = evaluation_id;
                parent_observation.case_id = case_id;
                parent_observation.opponent_profile_id = Some(proposal.id.clone());
                parent_observation.relative_reward = Some(-reward);
                [proposal_observation, parent_observation]
            })
            .collect()
    }

    #[test]
    fn prompt_genomes_mutate_and_cross_without_unbounded_fields() {
        let auto = ConductorPromptGenome::seed_for_effort("auto");
        let pro = ConductorPromptGenome::seed_for_effort("pro");
        let mutations = auto.mutations();
        assert!(mutations.len() >= 8);
        assert!(mutations
            .iter()
            .any(|genome| genome.context_policy != auto.context_policy));
        assert!(mutations
            .iter()
            .any(|genome| genome.tool_policy != auto.tool_policy));
        assert!(mutations
            .iter()
            .any(|genome| genome.retry_policy != auto.retry_policy));
        assert!(mutations
            .iter()
            .any(|genome| genome.topology_strategy != auto.topology_strategy));
        assert!(mutations
            .iter()
            .any(|genome| genome.role_strategy != auto.role_strategy));
        assert!(mutations
            .iter()
            .any(|genome| genome.commit_strategy != auto.commit_strategy));
        assert!(mutations
            .iter()
            .any(|genome| genome.max_step_attempts != auto.max_step_attempts));
        assert!(mutations.iter().any(|genome| {
            genome.effective_max_model_turns_per_step() != auto.effective_max_model_turns_per_step()
        }));
        assert!(mutations.iter().any(|genome| {
            genome.effective_max_tool_calls_per_step() != auto.effective_max_tool_calls_per_step()
        }));
        let child = ConductorPromptGenome::crossover("child", &auto, &pro).unwrap();
        assert_eq!(child.parents, vec![auto.id, pro.id]);
        assert_eq!(child.generation, 1);
        child.validate().unwrap();
    }

    #[test]
    fn prompt_budget_mutations_are_canonical_and_phenotypically_distinct() {
        let mut parent = ConductorPromptGenome::seed_for_effort("auto");
        parent.max_model_turns_per_step = 4;
        parent.max_tool_calls_per_step = 8;
        let mutations = parent.mutations();
        let mut phenotypes = BTreeSet::new();

        for mutation in &mutations {
            assert_ne!(mutation.execution_phenotype(), parent.execution_phenotype());
            assert!(phenotypes.insert(mutation.execution_phenotype()));
            assert_eq!(
                mutation.max_model_turns_per_step,
                mutation.effective_max_model_turns_per_step()
            );
            assert_eq!(
                mutation.max_tool_calls_per_step,
                mutation.effective_max_tool_calls_per_step()
            );
        }

        let ids = mutations
            .iter()
            .map(|mutation| mutation.id.as_str())
            .collect::<BTreeSet<_>>();
        assert!(ids.iter().any(|id| id.ends_with("-t2")));
        assert!(!ids.iter().any(|id| id.ends_with("-t1")));
        assert!(ids.iter().any(|id| id.ends_with("-tc4")));
        assert!(!ids
            .iter()
            .any(|id| id.ends_with("-tc1") || id.ends_with("-tc2")));
    }

    #[test]
    fn reflective_proposal_must_improve_on_a_paired_minibatch() {
        let parent = ConductorPromptGenome::seed_for_effort("auto");
        let mut proposal = parent.mutations().remove(0);
        proposal.id = "learned-proposal".to_string();
        let pending = paired_minibatch_observations(&proposal, &parent, &[0.2, 0.1]);
        assert!(matches!(
            prompt_proposal_minibatch_decision(&proposal, &pending, 3, 0.02).unwrap(),
            PromptProposalMinibatchDecision::Pending {
                comparisons: 2,
                required: 3
            }
        ));

        let accepted = paired_minibatch_observations(&proposal, &parent, &[0.2, 0.1, 0.15]);
        assert!(matches!(
            prompt_proposal_minibatch_decision(&proposal, &accepted, 3, 0.02).unwrap(),
            PromptProposalMinibatchDecision::Accepted { wins: 3, .. }
        ));

        let rejected = paired_minibatch_observations(&proposal, &parent, &[0.1, -0.2, 0.0]);
        assert!(matches!(
            prompt_proposal_minibatch_decision(&proposal, &rejected, 3, 0.02).unwrap(),
            PromptProposalMinibatchDecision::Rejected {
                reason,
                ..
            } if reason == "no_measured_minibatch_improvement"
        ));
    }

    #[test]
    fn reflective_proposal_requires_real_paired_parent_evidence() {
        let parent = ConductorPromptGenome::seed_for_effort("auto");
        let mut proposal = parent.mutations().remove(0);
        proposal.id = "learned-proposal".to_string();
        let mut observations = paired_minibatch_observations(&proposal, &parent, &[0.2, 0.2, 0.2]);
        observations.retain(|observation| observation.profile_id == proposal.id);

        assert!(matches!(
            prompt_proposal_minibatch_decision(&proposal, &observations, 3, 0.02).unwrap(),
            PromptProposalMinibatchDecision::Pending { comparisons: 0, .. }
        ));
    }

    #[test]
    fn effort_delivery_contract_preserves_every_evolved_harness_gene() {
        let mut evolved = ConductorPromptGenome::seed_for_effort("fast");
        evolved.id = "learned-deadline-efficient-profile".to_string();
        evolved.generation = 7;
        evolved.parents = vec!["stable-pro".to_string()];
        evolved.commit_strategy = PromptCommitStrategy::Exhaustive;
        evolved.custom_directive = "Prefer the shortest verified delivery path.".to_string();
        evolved.require_final_synthesis = false;

        let mut expected = evolved.clone();
        expected.require_final_synthesis = true;
        assert_eq!(
            evolved.clone().with_effort_delivery_contract("auto"),
            expected
        );
        assert_eq!(evolved.with_effort_delivery_contract("pro"), expected);

        let mut fast = expected.clone();
        fast.require_final_synthesis = false;
        assert_eq!(fast.clone().with_effort_delivery_contract("fast"), fast);
    }

    #[test]
    fn legacy_genomes_receive_safe_harness_gene_defaults() {
        let legacy = serde_json::json!({
            "schema": PROMPT_GENOME_SCHEMA,
            "id": "legacy-auto",
            "generation": 0,
            "parents": [],
            "graph_depth": "balanced",
            "verification": "evidence",
            "context_policy": "relevant",
            "max_parallel_branches": 2,
            "require_final_synthesis": true,
            "custom_directive": ""
        });
        let genome: ConductorPromptGenome = serde_json::from_value(legacy).unwrap();

        assert_eq!(genome.tool_policy, PromptToolPolicy::EvidenceOnly);
        assert_eq!(genome.retry_policy, PromptRetryPolicy::AlternateModel);
        assert_eq!(
            genome.topology_strategy,
            PromptTopologyStrategy::AdaptiveDag
        );
        assert_eq!(genome.role_strategy, PromptRoleStrategy::Specialists);
        assert_eq!(genome.commit_strategy, PromptCommitStrategy::Adaptive);
        assert_eq!(genome.max_step_attempts, 2);
        assert_eq!(genome.effective_max_model_turns_per_step(), 2);
        assert_eq!(genome.effective_max_tool_calls_per_step(), 4);
        genome.validate().unwrap();
    }

    #[test]
    fn learned_mutation_changes_only_bounded_genes() {
        let parent = ConductorPromptGenome::seed_for_effort("auto");
        let response = serde_json::json!({
            "schema": PROMPT_GENOME_SCHEMA,
            "id": "ignored",
            "generation": 99,
            "parents": [],
            "graph_depth": "deep",
            "verification": "evidence",
            "context_policy": "relevant",
            "max_parallel_branches": 2,
            "require_final_synthesis": false,
            "custom_directive": "Prefer independent hypotheses when the task is ambiguous."
        })
        .to_string();
        let mutation = parent
            .learned_mutation_from_response(&response, "learned-auto-g1")
            .unwrap();

        assert_eq!(mutation.id, "learned-auto-g1");
        assert_eq!(mutation.generation, 1);
        assert_eq!(mutation.parents, vec![parent.id]);
        assert!(mutation.require_final_synthesis);
        assert_eq!(mutation.graph_depth, PromptGraphDepth::Deep);
        assert!(!mutation.custom_directive.is_empty());
    }

    #[test]
    fn learned_tool_policy_upgrade_counts_as_one_gene_before_budget_normalization() {
        let auto = ConductorPromptGenome::seed_for_effort("auto");
        let mut auto_response = auto.clone();
        auto_response.tool_policy = PromptToolPolicy::ReadOnlyExploration;
        let auto_mutation = auto
            .learned_mutation_from_response(
                &serde_json::to_string(&auto_response).unwrap(),
                "learned-auto-tools-explore",
            )
            .unwrap();
        assert_eq!(
            auto_mutation.tool_policy,
            PromptToolPolicy::ReadOnlyExploration
        );
        assert_eq!(auto_mutation.max_model_turns_per_step, 3);
        assert_eq!(auto_mutation.max_tool_calls_per_step, 6);

        let fast = ConductorPromptGenome::seed_for_effort("fast");
        let mut fast_response = fast.clone();
        fast_response.tool_policy = PromptToolPolicy::EvidenceOnly;
        let fast_mutation = fast
            .learned_mutation_from_response(
                &serde_json::to_string(&fast_response).unwrap(),
                "learned-fast-tools-evidence",
            )
            .unwrap();
        assert_eq!(fast_mutation.tool_policy, PromptToolPolicy::EvidenceOnly);
        assert_eq!(fast_mutation.max_model_turns_per_step, 2);
        assert_eq!(fast_mutation.max_tool_calls_per_step, 4);
    }

    #[test]
    fn reflective_mutation_reads_full_feedback_trajectories() {
        let parent = ConductorPromptGenome::seed_for_effort("auto");
        let packet = AgentEvaluationReflectionPacket {
            suite_id: "feedback-suite".to_string(),
            suite_version: 2,
            case_id: "feedback-case".to_string(),
            category: "coding".to_string(),
            run_id: "run-1".to_string(),
            seed: 1,
            candidate_id: parent.id.clone(),
            candidate_fingerprint: "fingerprint".to_string(),
            model_fingerprints: BTreeMap::new(),
            input: "Fix the parser".to_string(),
            steps: vec![crate::AgentEvaluationTraceStep {
                step_id: "worker".to_string(),
                role: "worker".to_string(),
                model: "configured-worker".to_string(),
                prompt: "Run the parser tests".to_string(),
                output: "tests failed".to_string(),
                tool_calls: vec![crate::AgentEvaluationToolTrace {
                    tool: "shell.run".to_string(),
                    request: "cargo test".to_string(),
                    response: "compiler error".to_string(),
                    error: Some("exit 101".to_string()),
                }],
                errors: vec!["compiler error".to_string()],
                latency_ms: 10,
                total_tokens: 20,
            }],
            final_output: "The parser is fixed".to_string(),
            verifier: crate::AgentEvaluationVerifierOutcome {
                source: crate::AgentEvaluationEvidenceSource::Judge,
                passed: false,
                score: 0.0,
                checks: vec![crate::AgentEvaluationCheck {
                    id: "tests".to_string(),
                    passed: false,
                    detail: "cargo test failed".to_string(),
                }],
            },
            actionable_feedback: crate::ActionableSideInformation {
                summary: "tests failed".to_string(),
                passed_constraints: Vec::new(),
                failed_constraints: vec!["cargo test must pass".to_string()],
                errors: vec!["compiler error".to_string()],
                suggested_changes: vec!["inspect the parser error".to_string()],
            },
        };

        let prompt = parent.reflective_mutation_prompt(&[packet]).unwrap();
        assert!(prompt.contains("compiler error"));
        assert!(prompt.contains("shell.run"));
        assert!(prompt.contains("cargo test must pass"));
        assert!(prompt.contains("Fix the parser"));
    }

    fn reflection_packet(input: &str) -> AgentEvaluationReflectionPacket {
        AgentEvaluationReflectionPacket {
            suite_id: "feedback-suite".to_string(),
            suite_version: 2,
            case_id: "private-case-4815".to_string(),
            category: "coding".to_string(),
            run_id: "run-1".to_string(),
            seed: 1,
            candidate_id: "seed-auto-v1".to_string(),
            candidate_fingerprint: "candidate-fingerprint".to_string(),
            model_fingerprints: BTreeMap::from([(
                "worker".to_string(),
                "configured-worker-v7".to_string(),
            )]),
            input: input.to_string(),
            steps: Vec::new(),
            final_output: "A verified result".to_string(),
            verifier: crate::AgentEvaluationVerifierOutcome {
                source: crate::AgentEvaluationEvidenceSource::Judge,
                passed: true,
                score: 1.0,
                checks: Vec::new(),
            },
            actionable_feedback: crate::ActionableSideInformation {
                summary: "Prefer evidence-backed completion.".to_string(),
                passed_constraints: Vec::new(),
                failed_constraints: Vec::new(),
                errors: Vec::new(),
                suggested_changes: Vec::new(),
            },
        }
    }

    #[test]
    fn reflective_mutation_rejects_case_content_and_accepts_general_strategy() {
        let parent = ConductorPromptGenome::seed_for_effort("auto");
        let case_text = "Rebuild the lunar invoice parser using hidden fixture cobalt 4815";
        let leaked = serde_json::json!({
            "schema": PROMPT_GENOME_SCHEMA,
            "id": "ignored",
            "generation": 99,
            "parents": [],
            "graph_depth": "balanced",
            "verification": "evidence",
            "context_policy": "relevant",
            "max_parallel_branches": 2,
            "require_final_synthesis": true,
            "custom_directive": format!("Always solve this exact case: {case_text}")
        })
        .to_string();
        let packets = [reflection_packet(case_text)];
        let error = parent
            .learned_reflective_mutation_from_response(&leaked, "leaked", &packets)
            .unwrap_err();
        assert!(error.contains("copying case content"));

        let leaked_identity = leaked.replace(
            &format!("Always solve this exact case: {case_text}"),
            "Prefer configured-worker-v7 for this case",
        );
        let error = parent
            .learned_reflective_mutation_from_response(
                &leaked_identity,
                "leaked-identity",
                &packets,
            )
            .unwrap_err();
        assert!(error.contains("participant model names"));

        let generalized = serde_json::json!({
            "schema": PROMPT_GENOME_SCHEMA,
            "id": "ignored",
            "generation": 99,
            "parents": [],
            "graph_depth": "balanced",
            "verification": "evidence",
            "context_policy": "relevant",
            "max_parallel_branches": 2,
            "require_final_synthesis": true,
            "custom_directive": "Before synthesis, verify the weakest evidence-bearing claim with an independent check."
        })
        .to_string();
        let mutation = parent
            .learned_reflective_mutation_from_response(&generalized, "generalized", &packets)
            .unwrap();
        assert_eq!(
            mutation.custom_directive,
            "Before synthesis, verify the weakest evidence-bearing claim with an independent check."
        );
    }

    #[test]
    fn reflection_selection_is_deterministic_and_uses_unique_paired_feedback() {
        let profile_id = "seed-auto-v1";
        let packet = |run_id: &str, candidate_id: &str| AgentEvaluationReflectionPacket {
            suite_id: "feedback-suite".to_string(),
            suite_version: 2,
            case_id: "shared-case".to_string(),
            category: "coding".to_string(),
            run_id: run_id.to_string(),
            seed: 0,
            candidate_id: candidate_id.to_string(),
            candidate_fingerprint: "fingerprint".to_string(),
            model_fingerprints: BTreeMap::new(),
            input: "Fix the parser".to_string(),
            steps: Vec::new(),
            final_output: "done".to_string(),
            verifier: crate::AgentEvaluationVerifierOutcome {
                source: crate::AgentEvaluationEvidenceSource::Judge,
                passed: true,
                score: 1.0,
                checks: Vec::new(),
            },
            actionable_feedback: crate::ActionableSideInformation::default(),
        };
        let observation =
            |evaluation_id: &str,
             split: PromptEvaluationSplit,
             mode: PromptEvaluationMode,
             reflection_packet: Option<AgentEvaluationReflectionPacket>| {
                PromptEvolutionObservation {
                    profile_id: profile_id.to_string(),
                    evaluation_id: evaluation_id.to_string(),
                    case_id: "shared-case".to_string(),
                    opponent_profile_id: Some("challenger".to_string()),
                    task_class: "coding".to_string(),
                    split,
                    mode,
                    format_valid: true,
                    succeeded: true,
                    quality_score: 1.0,
                    latency_ms: 10,
                    total_tokens: 20,
                    estimated_cost_microusd: 0,
                    safety_violations: 0,
                    relative_reward: Some(0.5),
                    step_credits: Vec::new(),
                    reflection_packet,
                    provenance: scientific_provenance(profile_id, "challenger"),
                }
            };
        let mut observations = vec![
            observation(
                "feedback-1",
                PromptEvaluationSplit::Train,
                PromptEvaluationMode::PairedExecution,
                Some(packet("feedback-1", profile_id)),
            ),
            observation(
                "feedback-1-duplicate",
                PromptEvaluationSplit::Train,
                PromptEvaluationMode::PairedExecution,
                Some(packet("feedback-1", profile_id)),
            ),
            observation(
                "holdout",
                PromptEvaluationSplit::Holdout,
                PromptEvaluationMode::ReplayExecution,
                Some(packet("holdout", profile_id)),
            ),
            observation(
                "legacy-shadow",
                PromptEvaluationSplit::Train,
                PromptEvaluationMode::PairedShadow,
                Some(packet("legacy-shadow", profile_id)),
            ),
            observation(
                "wrong-candidate",
                PromptEvaluationSplit::Train,
                PromptEvaluationMode::PairedExecution,
                Some(packet("wrong-candidate", "other-profile")),
            ),
            observation(
                "feedback-2",
                PromptEvaluationSplit::Train,
                PromptEvaluationMode::PairedExecution,
                Some(packet("feedback-2", profile_id)),
            ),
        ];
        let mut newer_holdout = observation(
            "newer-holdout-dataset",
            PromptEvaluationSplit::Holdout,
            PromptEvaluationMode::ReplayExecution,
            Some(packet("newer-holdout-dataset", profile_id)),
        );
        newer_holdout.provenance.dataset_sha256 = "holdout-only-dataset".to_string();
        observations.push(newer_holdout);

        let selected = prompt_reflection_packets(&observations, profile_id, 6);

        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].run_id, "feedback-1");
        assert_eq!(selected[1].run_id, "feedback-2");
    }

    #[test]
    fn instance_wise_pareto_preserves_and_merges_complementary_candidates() {
        let ancestor = ConductorPromptGenome::seed_for_effort("auto");
        let mut left = ancestor.clone();
        left.id = "left".to_string();
        left.generation = 1;
        left.parents = vec![ancestor.id.clone()];
        left.graph_depth = PromptGraphDepth::Deep;
        let mut right = ancestor.clone();
        right.id = "right".to_string();
        right.generation = 1;
        right.parents = vec![ancestor.id.clone()];
        right.verification = PromptVerification::Adversarial;
        let genomes = vec![ancestor.clone(), left.clone(), right.clone()];
        let mut scores = Vec::new();
        for run in 0..2 {
            scores.extend([
                instance_score(&ancestor.id, "case-a", run, 0.4),
                instance_score(&ancestor.id, "case-b", run, 0.4),
                instance_score(&left.id, "case-a", run, 0.9),
                instance_score(&left.id, "case-b", run, 0.6),
                instance_score(&right.id, "case-a", run, 0.6),
                instance_score(&right.id, "case-b", run, 0.9),
            ]);
        }

        let archive = PromptInstanceParetoArchive::build(&genomes, &scores, 2).unwrap();
        assert_eq!(archive.candidates.len(), 2);
        assert_eq!(archive.candidates[0].profile_id, "left");
        assert_eq!(archive.candidates[0].leading_cases, vec!["case-a"]);
        assert_eq!(archive.candidates[1].profile_id, "right");
        assert_eq!(archive.candidates[1].leading_cases, vec!["case-b"]);
        assert_eq!(archive.select_for_mutation(0).unwrap().profile_id, "left");
        assert_eq!(archive.select_for_mutation(1).unwrap().profile_id, "right");

        let merged = archive
            .merge_complementary("merged", &ancestor, &left, &right)
            .unwrap();
        assert_eq!(merged.graph_depth, PromptGraphDepth::Deep);
        assert_eq!(merged.verification, PromptVerification::Adversarial);
        assert_eq!(merged.context_policy, ancestor.context_policy);
        assert_eq!(merged.parents, vec!["left", "right"]);
    }

    #[test]
    fn instance_wise_pareto_counts_only_independent_repeat_identities() {
        let genome = ConductorPromptGenome::seed_for_effort("auto");
        let score = instance_score(&genome.id, "case-a", 0, 0.9);
        let duplicate = score.clone();

        let archive = PromptInstanceParetoArchive::build(
            std::slice::from_ref(&genome),
            &[score, duplicate],
            2,
        )
        .unwrap();

        assert!(archive.candidates.is_empty());
        assert!(archive.case_best_scores.is_empty());
    }

    #[test]
    fn instance_wise_pareto_rejects_conflicting_duplicate_repeats() {
        let genome = ConductorPromptGenome::seed_for_effort("auto");
        let score = instance_score(&genome.id, "case-a", 0, 0.9);
        let mut conflicting = score.clone();
        conflicting.score = 0.1;

        let error = PromptInstanceParetoArchive::build(
            std::slice::from_ref(&genome),
            &[score, conflicting],
            1,
        )
        .unwrap_err();

        assert!(error.contains("conflicting instance-wise Pareto repeat"));
    }

    #[test]
    fn instance_wise_pareto_uses_performance_only_after_quality_and_success_tie() {
        let ancestor = ConductorPromptGenome::seed_for_effort("pro");
        let mut lower_latency = ancestor.clone();
        lower_latency.id = "lower-latency".to_string();
        lower_latency.generation = 1;
        lower_latency.parents = vec![ancestor.id.clone()];
        let mut lower_tokens = ancestor.clone();
        lower_tokens.id = "lower-tokens".to_string();
        lower_tokens.generation = 1;
        lower_tokens.parents = vec![ancestor.id.clone()];

        let mut latency_score = instance_score(&lower_latency.id, "case-a", 0, 1.0);
        latency_score.latency_ms = 80;
        latency_score.total_tokens = 120;
        let mut token_score = instance_score(&lower_tokens.id, "case-a", 0, 1.0);
        token_score.latency_ms = 100;
        token_score.total_tokens = 60;
        let archive = PromptInstanceParetoArchive::build(
            &[lower_latency.clone(), lower_tokens.clone()],
            &[latency_score, token_score],
            1,
        )
        .unwrap();
        assert_eq!(
            archive.best_aggregate().unwrap().profile_id,
            lower_latency.id
        );

        let mut latency_score = instance_score(&lower_latency.id, "case-a", 0, 1.0);
        latency_score.latency_ms = 100;
        latency_score.total_tokens = 120;
        let mut token_score = instance_score(&lower_tokens.id, "case-a", 0, 1.0);
        token_score.latency_ms = 100;
        token_score.total_tokens = 60;
        let archive = PromptInstanceParetoArchive::build(
            &[lower_latency, lower_tokens.clone()],
            &[latency_score, token_score],
            1,
        )
        .unwrap();
        assert_eq!(
            archive.best_aggregate().unwrap().profile_id,
            lower_tokens.id
        );
    }

    #[test]
    fn promotion_confidence_deduplicates_replayed_observations() {
        let entry = observation("candidate", PromptEvaluationSplit::Holdout, 0.9, 100, 100);
        let observations = [entry.clone(), entry.clone(), entry];

        let confidence = prompt_promotion_confidence(observations.iter());

        assert_eq!(confidence.comparisons, 1);
        assert_eq!(confidence.wins, 1);
    }

    #[test]
    fn system_aware_merge_rejects_conflicting_gene_changes() {
        let ancestor = ConductorPromptGenome::seed_for_effort("auto");
        let mut left = ancestor.clone();
        left.id = "left".to_string();
        left.generation = 1;
        left.parents = vec![ancestor.id.clone()];
        left.graph_depth = PromptGraphDepth::Deep;
        let mut right = left.clone();
        right.id = "right".to_string();

        let error = ConductorPromptGenome::system_aware_merge("merged", &ancestor, &left, &right)
            .unwrap_err();
        assert!(error.contains("conflicting genes"));
    }

    #[test]
    fn malformed_or_unsafe_workflows_receive_zero_reward() {
        let mut entry = observation("profile", PromptEvaluationSplit::Holdout, 1.0, 1_000, 1_000);
        assert!(entry.reward() > 0.8);
        assert!(entry.reward() < 1.0);
        entry.format_valid = false;
        assert_eq!(entry.reward(), 0.0);
        entry.format_valid = true;
        entry.safety_violations = 1;
        assert_eq!(entry.reward(), 0.0);
    }

    #[test]
    fn live_only_runs_cannot_promote_without_paired_and_replay_evidence() {
        let genome = ConductorPromptGenome::seed_for_effort("auto");
        let mut observations = vec![
            observation(&genome.id, PromptEvaluationSplit::Train, 1.0, 100, 100),
            observation(&genome.id, PromptEvaluationSplit::Holdout, 1.0, 100, 100),
        ];
        for observation in &mut observations {
            observation.mode = PromptEvaluationMode::Live;
            observation.opponent_profile_id = None;
            observation.relative_reward = None;
        }

        let archive =
            PromptParetoArchive::build(std::slice::from_ref(&genome), &observations, 1, 1).unwrap();

        assert!(archive.candidates.is_empty());
        assert_eq!(archive.rejected_profiles, vec![genome.id]);
    }

    #[test]
    fn plan_only_comparisons_cannot_promote_without_execution_evidence() {
        let genome = ConductorPromptGenome::seed_for_effort("auto");
        let mut observations = vec![
            observation(&genome.id, PromptEvaluationSplit::Train, 1.0, 100, 100),
            observation(&genome.id, PromptEvaluationSplit::Train, 1.0, 100, 100),
            observation(&genome.id, PromptEvaluationSplit::Holdout, 1.0, 100, 100),
            observation(&genome.id, PromptEvaluationSplit::Holdout, 1.0, 100, 100),
        ];
        for observation in &mut observations {
            observation.mode = match observation.split {
                PromptEvaluationSplit::Train => PromptEvaluationMode::PairedShadow,
                PromptEvaluationSplit::Holdout => PromptEvaluationMode::ReplayHoldout,
            };
        }

        let archive =
            PromptParetoArchive::build(std::slice::from_ref(&genome), &observations, 2, 2).unwrap();

        assert!(archive.candidates.is_empty());
        assert_eq!(archive.rejected_profiles, vec![genome.id]);
    }

    #[test]
    fn promotion_confidence_requires_more_than_two_lucky_wins() {
        let genome = ConductorPromptGenome::seed_for_effort("auto");
        let sparse = [
            observation(&genome.id, PromptEvaluationSplit::Train, 1.0, 100, 100),
            observation(&genome.id, PromptEvaluationSplit::Holdout, 1.0, 100, 100),
        ];
        let sparse_confidence = prompt_promotion_confidence(sparse.iter());
        assert_eq!(sparse_confidence.comparisons, 2);
        assert!(sparse_confidence.wilson_lower_bound < PROMOTION_MIN_LOWER_BOUND);

        let repeated = (0..6)
            .map(|round| {
                let split = if round % 2 == 0 {
                    PromptEvaluationSplit::Train
                } else {
                    PromptEvaluationSplit::Holdout
                };
                observation(&genome.id, split, 1.0, 100 + round, 100)
            })
            .collect::<Vec<_>>();
        let repeated_confidence = prompt_promotion_confidence(repeated.iter());
        assert_eq!(repeated_confidence.comparisons, 6);
        assert!(repeated_confidence.wilson_lower_bound >= PROMOTION_MIN_LOWER_BOUND);
    }

    #[test]
    fn promotion_confidence_is_earned_on_holdout_not_training() {
        let genome = ConductorPromptGenome::seed_for_effort("auto");
        let mut observations = Vec::new();
        for round in 0..12 {
            let mut entry = observation(
                &genome.id,
                PromptEvaluationSplit::Train,
                1.0,
                100 + round,
                100,
            );
            entry.relative_reward = Some(0.5);
            observations.push(entry);
        }
        for round in 0..4 {
            let mut entry = observation(
                &genome.id,
                PromptEvaluationSplit::Holdout,
                0.9,
                200 + round,
                100,
            );
            entry.relative_reward = Some(-0.5);
            observations.push(entry);
        }

        let archive =
            PromptParetoArchive::build(std::slice::from_ref(&genome), &observations, 3, 4).unwrap();

        assert!(archive.candidates.is_empty());
        assert_eq!(archive.rejected_profiles, vec![genome.id]);
    }

    #[test]
    fn same_task_relative_winner_improves_robust_score() {
        let winner = ConductorPromptGenome::seed_for_effort("auto");
        let loser = ConductorPromptGenome {
            id: "relative-loser".to_string(),
            ..ConductorPromptGenome::seed_for_effort("auto")
        };
        let mut observations = Vec::new();
        for split in [PromptEvaluationSplit::Train, PromptEvaluationSplit::Holdout] {
            let repeats = if split == PromptEvaluationSplit::Holdout {
                6
            } else {
                2
            };
            for round in 0..repeats {
                let mut winning = observation(&winner.id, split, 0.9, 1_000, 1_000);
                winning.evaluation_id = format!("pair-{split:?}-{round}");
                winning.opponent_profile_id = Some(loser.id.clone());
                winning.relative_reward = Some(0.3);
                let mut losing = observation(&loser.id, split, 0.9, 1_000, 1_000);
                losing.evaluation_id = winning.evaluation_id.clone();
                losing.opponent_profile_id = Some(winner.id.clone());
                losing.relative_reward = Some(-0.3);
                observations.extend([winning, losing]);
            }
        }

        let archive =
            PromptParetoArchive::build(&[winner.clone(), loser], &observations, 1, 1).unwrap();

        assert_eq!(archive.champion().unwrap().genome.id, winner.id);
        assert!(archive.champion().unwrap().holdout.average_relative_reward > 0.0);
    }

    #[test]
    fn pareto_archive_keeps_quality_and_efficiency_tradeoffs() {
        let fast = ConductorPromptGenome::seed_for_effort("fast");
        let pro = ConductorPromptGenome::seed_for_effort("pro");
        let dominated = ConductorPromptGenome {
            id: "dominated".to_string(),
            ..ConductorPromptGenome::seed_for_effort("auto")
        };
        let genomes = vec![fast.clone(), pro.clone(), dominated.clone()];
        let mut observations = Vec::new();
        for split in [PromptEvaluationSplit::Train, PromptEvaluationSplit::Holdout] {
            let repeats = if split == PromptEvaluationSplit::Holdout {
                6
            } else {
                2
            };
            for _ in 0..repeats {
                observations.push(observation(&fast.id, split, 0.82, 1_000, 1_000));
                observations.push(observation(&pro.id, split, 0.96, 4_000, 3_000));
                observations.push(observation(&dominated.id, split, 0.75, 5_000, 4_000));
            }
        }
        let archive = PromptParetoArchive::build(&genomes, &observations, 2, 2).unwrap();
        let ids = archive
            .candidates
            .iter()
            .map(|candidate| candidate.genome.id.as_str())
            .collect::<BTreeSet<_>>();
        assert!(ids.contains(fast.id.as_str()));
        assert!(ids.contains(pro.id.as_str()));
        assert!(!ids.contains(dominated.id.as_str()));
        let search_archive = PromptSearchArchive::build(&genomes, &observations, 2).unwrap();
        assert!(!search_archive.next_generation(8).is_empty());
    }

    #[test]
    fn pareto_selection_does_not_treat_token_cost_as_intelligence() {
        let compact = ConductorPromptGenome::seed_for_effort("fast");
        let expansive = ConductorPromptGenome::seed_for_effort("pro");
        let genomes = vec![compact.clone(), expansive.clone()];
        let mut observations = Vec::new();
        for split in [PromptEvaluationSplit::Train, PromptEvaluationSplit::Holdout] {
            let repeats = if split == PromptEvaluationSplit::Holdout {
                6
            } else {
                2
            };
            for _ in 0..repeats {
                observations.push(observation(&compact.id, split, 0.9, 2_000, 500));
                observations.push(observation(&expansive.id, split, 0.9, 2_000, 8_000));
            }
        }

        let archive = PromptParetoArchive::build(&genomes, &observations, 2, 2).unwrap();
        let ids = archive
            .candidates
            .iter()
            .map(|candidate| candidate.genome.id.as_str())
            .collect::<BTreeSet<_>>();

        assert!(ids.contains(compact.id.as_str()));
        assert!(ids.contains(expansive.id.as_str()));
    }

    #[test]
    fn holdout_changes_promotion_but_never_search_or_offspring() {
        let stronger_train = ConductorPromptGenome::seed_for_effort("auto");
        let weaker_train = ConductorPromptGenome {
            id: "weaker-train".to_string(),
            ..ConductorPromptGenome::seed_for_effort("auto")
        };
        let genomes = vec![stronger_train.clone(), weaker_train.clone()];
        let training = (0..2)
            .flat_map(|_| {
                [
                    observation(
                        &stronger_train.id,
                        PromptEvaluationSplit::Train,
                        0.90,
                        1_000,
                        1_000,
                    ),
                    observation(
                        &weaker_train.id,
                        PromptEvaluationSplit::Train,
                        0.86,
                        1_000,
                        1_000,
                    ),
                ]
            })
            .collect::<Vec<_>>();
        let with_holdout = |stronger_quality: f64, weaker_quality: f64| {
            let mut observations = training.clone();
            for _ in 0..6 {
                observations.push(observation(
                    &stronger_train.id,
                    PromptEvaluationSplit::Holdout,
                    stronger_quality,
                    1_000,
                    1_000,
                ));
                observations.push(observation(
                    &weaker_train.id,
                    PromptEvaluationSplit::Holdout,
                    weaker_quality,
                    1_000,
                    1_000,
                ));
            }
            observations
        };
        let first = with_holdout(0.95, 0.86);
        let reversed = with_holdout(0.86, 0.95);

        let first_search = PromptSearchArchive::build(&genomes, &first, 2).unwrap();
        let reversed_search = PromptSearchArchive::build(&genomes, &reversed, 2).unwrap();
        assert_eq!(
            first_search
                .champion()
                .map(|candidate| &candidate.genome.id),
            Some(&stronger_train.id)
        );
        assert_eq!(first_search, reversed_search);
        assert_eq!(
            first_search.next_generation(16),
            reversed_search.next_generation(16)
        );
        let mut newer_holdout_dataset = first.clone();
        let mut holdout_only = observation(
            &weaker_train.id,
            PromptEvaluationSplit::Holdout,
            1.0,
            1_000,
            1_000,
        );
        holdout_only.provenance.dataset_sha256 = "holdout-only-dataset".to_string();
        newer_holdout_dataset.push(holdout_only);
        assert_eq!(
            first_search,
            PromptSearchArchive::build(&genomes, &newer_holdout_dataset, 2).unwrap()
        );
        assert_eq!(
            first_search.next_generation(16),
            PromptSearchArchive::build(&genomes, &newer_holdout_dataset, 2)
                .unwrap()
                .next_generation(16)
        );

        let first_promotion = PromptParetoArchive::build(&genomes, &first, 2, 6).unwrap();
        let reversed_promotion = PromptParetoArchive::build(&genomes, &reversed, 2, 6).unwrap();
        assert_eq!(
            first_promotion
                .champion()
                .map(|candidate| &candidate.genome.id),
            Some(&stronger_train.id)
        );
        assert_eq!(
            reversed_promotion
                .champion()
                .map(|candidate| &candidate.genome.id),
            Some(&weaker_train.id)
        );

        let first_convergence =
            evaluate_prompt_convergence(&genomes, &first, 2, 6, 3, 0.02, 12).unwrap();
        let reversed_convergence =
            evaluate_prompt_convergence(&genomes, &reversed, 2, 6, 3, 0.02, 12).unwrap();
        assert_eq!(
            first_convergence
                .champion
                .as_ref()
                .map(|candidate| &candidate.genome.id),
            Some(&stronger_train.id)
        );
        assert_eq!(
            reversed_convergence
                .champion
                .as_ref()
                .map(|candidate| &candidate.genome.id),
            Some(&stronger_train.id)
        );
        let rejected_nominee = with_holdout(0.70, 0.95);
        let rejected_convergence =
            evaluate_prompt_convergence(&genomes, &rejected_nominee, 2, 6, 3, 0.02, 12).unwrap();
        assert!(rejected_convergence.champion.is_none());
    }

    #[test]
    fn holdout_gap_and_safety_block_promotion() {
        let genome = ConductorPromptGenome::seed_for_effort("auto");
        let mut observations = vec![
            observation(&genome.id, PromptEvaluationSplit::Train, 1.0, 1_000, 1_000),
            observation(&genome.id, PromptEvaluationSplit::Train, 1.0, 1_000, 1_000),
            observation(
                &genome.id,
                PromptEvaluationSplit::Holdout,
                0.5,
                1_000,
                1_000,
            ),
            observation(
                &genome.id,
                PromptEvaluationSplit::Holdout,
                0.5,
                1_000,
                1_000,
            ),
        ];
        observations[3].safety_violations = 1;
        let archive =
            PromptParetoArchive::build(std::slice::from_ref(&genome), &observations, 2, 2).unwrap();
        assert!(archive.candidates.is_empty());
        assert_eq!(archive.rejected_profiles, vec![genome.id]);
    }

    #[test]
    fn invalid_holdout_workflow_cannot_become_champion() {
        let genome = ConductorPromptGenome::seed_for_effort("auto");
        let mut observations = vec![
            observation(&genome.id, PromptEvaluationSplit::Train, 0.9, 1_000, 1_000),
            observation(&genome.id, PromptEvaluationSplit::Train, 0.9, 1_000, 1_000),
            observation(
                &genome.id,
                PromptEvaluationSplit::Holdout,
                0.9,
                1_000,
                1_000,
            ),
            observation(
                &genome.id,
                PromptEvaluationSplit::Holdout,
                0.9,
                1_000,
                1_000,
            ),
        ];
        observations[3].format_valid = false;

        let archive =
            PromptParetoArchive::build(std::slice::from_ref(&genome), &observations, 2, 2).unwrap();

        assert!(archive.candidates.is_empty());
        assert_eq!(archive.rejected_profiles, vec![genome.id]);
    }

    #[test]
    fn recent_outcomes_automatically_roll_back_a_degraded_champion() {
        let previous = ConductorPromptGenome::seed_for_effort("auto");
        let fallback = ConductorPromptGenome {
            id: "stable-fallback".to_string(),
            ..ConductorPromptGenome::seed_for_effort("auto")
        };
        let mut observations = vec![
            observation(
                &previous.id,
                PromptEvaluationSplit::Train,
                0.4,
                1_000,
                1_000,
            ),
            observation(
                &previous.id,
                PromptEvaluationSplit::Train,
                0.4,
                1_000,
                1_000,
            ),
            observation(
                &fallback.id,
                PromptEvaluationSplit::Train,
                0.8,
                1_000,
                1_000,
            ),
            observation(
                &fallback.id,
                PromptEvaluationSplit::Train,
                0.8,
                1_000,
                1_000,
            ),
        ];
        for _ in 0..12 {
            observations.push(observation(
                &previous.id,
                PromptEvaluationSplit::Holdout,
                1.0,
                1_000,
                1_000,
            ));
        }
        for _ in 0..12 {
            observations.push(observation(
                &previous.id,
                PromptEvaluationSplit::Holdout,
                0.4,
                1_000,
                1_000,
            ));
            observations.push(observation(
                &fallback.id,
                PromptEvaluationSplit::Holdout,
                0.8,
                1_000,
                1_000,
            ));
        }

        let archive =
            PromptParetoArchive::build(&[previous, fallback.clone()], &observations, 2, 2).unwrap();

        assert_eq!(archive.champion().unwrap().genome.id, fallback.id);
    }

    #[test]
    fn evolution_freezes_after_stagnant_generations() {
        let seed = ConductorPromptGenome::seed_for_effort("auto");
        let mut genomes = vec![seed.clone()];
        let mut parent = seed;
        for generation in 1..=3 {
            let mut child = parent.clone();
            child.id = format!("generation-{generation}");
            child.generation = generation;
            child.parents = vec![parent.id.clone()];
            genomes.push(child.clone());
            parent = child;
        }
        let mut observations = Vec::new();
        for genome in &genomes {
            for _ in 0..6 {
                observations.push(observation(
                    &genome.id,
                    PromptEvaluationSplit::Train,
                    0.9,
                    1_000,
                    1_000,
                ));
                observations.push(observation(
                    &genome.id,
                    PromptEvaluationSplit::Holdout,
                    0.9,
                    1_000,
                    1_000,
                ));
            }
        }
        let convergence =
            evaluate_prompt_convergence(&genomes, &observations, 1, 1, 3, 0.02, 12).unwrap();

        assert!(convergence.frozen);
        assert_eq!(convergence.reason.as_deref(), Some("converged"));
        assert_eq!(convergence.stagnant_generations, 3);
    }
}
