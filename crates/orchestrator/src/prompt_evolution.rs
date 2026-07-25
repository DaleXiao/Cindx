mod genome;
mod observation;
mod pareto;

pub use genome::*;
pub use observation::*;
pub use pareto::*;

#[cfg(test)]
mod tests {
    use super::pareto::PROMOTION_MIN_LOWER_BOUND;
    use super::*;
    use crate::{AgentEvaluationCaseScore, AgentEvaluationReflectionPacket, AgentEvaluationSplit};
    use std::collections::{BTreeMap, BTreeSet};

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
    fn effort_capability_floor_prevents_learned_profiles_from_weakening_auto_or_pro() {
        let mut weak_auto = ConductorPromptGenome::seed_for_effort("fast");
        weak_auto.require_final_synthesis = false;
        let effective_auto = weak_auto.with_effort_capability_floor("auto");
        assert_eq!(effective_auto.graph_depth, PromptGraphDepth::Balanced);
        assert_eq!(effective_auto.verification, PromptVerification::Evidence);
        assert_eq!(
            effective_auto.topology_strategy,
            PromptTopologyStrategy::AdaptiveDag
        );
        assert_eq!(
            effective_auto.role_strategy,
            PromptRoleStrategy::Specialists
        );
        assert_eq!(effective_auto.max_parallel_branches, 2);
        assert!(effective_auto.require_final_synthesis);

        let mut weak_pro = ConductorPromptGenome::seed_for_effort("fast");
        weak_pro.commit_strategy = PromptCommitStrategy::Quorum;
        weak_pro.require_final_synthesis = false;
        let effective_pro = weak_pro.with_effort_capability_floor("pro");
        assert_eq!(effective_pro.graph_depth, PromptGraphDepth::Deep);
        assert_eq!(effective_pro.verification, PromptVerification::Adversarial);
        assert_eq!(
            effective_pro.commit_strategy,
            PromptCommitStrategy::Exhaustive
        );
        assert_eq!(
            effective_pro.topology_strategy,
            PromptTopologyStrategy::ParallelDeliberation
        );
        assert_eq!(
            effective_pro.role_strategy,
            PromptRoleStrategy::DiverseSpecialists
        );
        assert_eq!(effective_pro.max_parallel_branches, 2);
        assert!(effective_pro.require_final_synthesis);
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

    #[test]
    fn reflection_selection_uses_only_unique_paired_feedback_executions() {
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
                }
            };
        let observations = vec![
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

        let selected = prompt_reflection_packets(&observations, profile_id, 6);

        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].run_id, "feedback-2");
        assert_eq!(selected[1].run_id, "feedback-1");
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

        let repeated = (0..4)
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
        assert_eq!(repeated_confidence.comparisons, 4);
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
                4
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
                4
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
        assert!(!archive.next_generation(8).is_empty());
    }

    #[test]
    fn pareto_selection_does_not_treat_token_cost_as_intelligence() {
        let compact = ConductorPromptGenome::seed_for_effort("fast");
        let expansive = ConductorPromptGenome::seed_for_effort("pro");
        let genomes = vec![compact.clone(), expansive.clone()];
        let mut observations = Vec::new();
        for split in [PromptEvaluationSplit::Train, PromptEvaluationSplit::Holdout] {
            let repeats = if split == PromptEvaluationSplit::Holdout {
                4
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
            for _ in 0..4 {
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
