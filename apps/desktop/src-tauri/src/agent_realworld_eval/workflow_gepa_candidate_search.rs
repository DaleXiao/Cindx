use super::workflow_gepa_campaign_contract::{
    aggregate_pairs, PairAggregateReceipt, ProductPairReceipt, CAMPAIGN_SUITE_ID, CAMPAIGN_VERSION,
};
use crate::app_state::AppState;
use crate::configuration_models::ProviderConfig;
use crate::phase16_task_id;
use crate::prompt_mutation_runtime::run_background_prompt_mutation_stage_with_liveness;
use agent_core::Metadata;
use agent_runtime::AgentRunControl;
use orchestrator::{
    assess_prompt_route_intervention, classify_prompt_mutation, prompt_genome_sha256, sha256_hex,
    AgentEvaluationCaseScore, AgentEvaluationEvidenceSource, AgentEvaluationReflectionPacket,
    AgentEvaluationSplit, AgentPolicy, ConductorPromptGenome, FrozenPromptProfileSnapshot,
    PromptInstanceParetoArchive, PromptRouteInterventionReceipt,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub(super) const TARGET_CANDIDATE_POPULATION: usize = 2;
const MAX_CANDIDATE_PROPOSALS: usize = 6;
const MODEL_CALLS_PER_PROPOSAL: usize = 2;
const CANDIDATE_SEARCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60 * 60);
const CANDIDATE_MODEL_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10 * 60);
const CANDIDATE_RESPONSE_START_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(5 * 60);
const TRAIN_RESOURCE_RATIO_CEILING: f64 = 1.25;

#[derive(Debug, Clone, Serialize)]
pub(super) struct CandidateIdentityReceipt {
    pub(super) profile_id: String,
    pub(super) profile_sha256: String,
    pub(super) route_profile_sha256: String,
    pub(super) parent_profile_id: String,
    pub(super) generation: u32,
    pub(super) mutation_response_sha256: String,
    pub(super) mutation_repaired: bool,
    pub(super) proposal_index: usize,
    pub(super) mutated_genes: Vec<String>,
    pub(super) intervention: PromptRouteInterventionReceipt,
    pub(super) snapshot_artifact_sha256: String,
}

#[derive(Debug, Clone)]
pub(super) struct GeneratedCandidate {
    pub(super) genome: ConductorPromptGenome,
    pub(super) snapshot: FrozenPromptProfileSnapshot,
    pub(super) identity: CandidateIdentityReceipt,
}

pub(super) struct GeneratedCandidatePopulation {
    pub(super) candidates: Vec<GeneratedCandidate>,
    pub(super) model_calls: usize,
    pub(super) physical_attempts: u64,
    pub(super) total_tokens: u64,
}

#[derive(Debug, Clone)]
struct CandidateSearchAttempt {
    proposal_index: usize,
    outcome: &'static str,
    mutation_assignment: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct CandidateTrainingReceipt {
    pub(super) candidate: CandidateIdentityReceipt,
    pub(super) train_pairs: Vec<ProductPairReceipt>,
    pub(super) train: PairAggregateReceipt,
    pub(super) eligible: bool,
    pub(super) selection_reason: String,
    pub(super) selected: bool,
}

pub(super) fn generate_candidate_population(
    state: &tauri::State<'_, AppState>,
    provider: &ProviderConfig,
    run_context: &Metadata,
    parent: &ConductorPromptGenome,
    packets: &[AgentEvaluationReflectionPacket],
    training_dataset_sha256: &str,
    reflection_evidence_sha256: &str,
) -> Result<GeneratedCandidatePopulation, String> {
    let control = Arc::new(AgentRunControl::with_budget(candidate_search_budget()));
    let seed_route_sha256 = parent.route_decision_profile_sha256(AgentPolicy::Pro.label())?;
    let mut seen_routes = BTreeSet::from([seed_route_sha256]);
    let mut population = Vec::new();
    let mut rejections = Vec::new();
    let mut search_history = Vec::new();
    for index in 0..MAX_CANDIDATE_PROPOSALS {
        let mut prompt = parent.reflective_route_mutation_prompt(packets)?;
        prompt.push_str(&candidate_population_search_instruction(
            index,
            &search_history,
        ));
        let mutation_id = format!("workflow-gepa-v9-route-mutation-{}", index + 1);
        let stage = format!("workflow_gepa_v9_route_mutation_{}", index + 1);
        let proposal_control = candidate_proposal_control(&control)?;
        let proposal = (|| {
            let response = run_background_prompt_mutation_stage_with_liveness(
                state,
                provider,
                &phase16_task_id(),
                run_context,
                &mutation_id,
                &stage,
                prompt,
                &proposal_control,
                Some(CANDIDATE_RESPONSE_START_TIMEOUT),
            )?;
            parse_candidate_response(
                state,
                provider,
                run_context,
                parent,
                packets,
                index,
                &response,
                &proposal_control,
            )
        })();
        control
            .absorb_isolated_treatments(&[proposal_control.as_ref()])
            .map_err(|reason| {
                format!(
                    "GEPA proposal {} resource accounting failed: {}",
                    index + 1,
                    reason.code()
                )
            })?;
        let (genome, response_sha256, repaired) = match proposal {
            Ok(proposal) => proposal,
            Err(error) => {
                rejections.push(format!("proposal {}: {error}", index + 1));
                continue;
            }
        };
        let mutated_genes = mutated_gene_names(parent, &genome);
        let mutation_assignment = candidate_mutation_assignment(&genome, &mutated_genes)?;
        if mutated_genes != ["route_directive"] {
            search_history.push(CandidateSearchAttempt {
                proposal_index: index + 1,
                outcome: "rejected_changed_gene_count",
                mutation_assignment,
            });
            rejections.push(format!(
                "proposal {}: route learning changed {:?} instead of only route_directive",
                index + 1,
                mutated_genes,
            ));
            continue;
        }
        let intervention =
            assess_prompt_route_intervention(parent, &genome, AgentPolicy::Pro.label())?;
        if !intervention.eligible {
            search_history.push(CandidateSearchAttempt {
                proposal_index: index + 1,
                outcome: "rejected_noncausal_route_intervention",
                mutation_assignment,
            });
            rejections.push(format!("proposal {}: {}", index + 1, intervention.reason,));
            continue;
        }
        let route_profile_sha256 =
            genome.route_decision_profile_sha256(AgentPolicy::Pro.label())?;
        if !seen_routes.insert(route_profile_sha256.clone()) {
            search_history.push(CandidateSearchAttempt {
                proposal_index: index + 1,
                outcome: "rejected_duplicate_route",
                mutation_assignment,
            });
            rejections.push(format!("proposal {}: duplicate route phenotype", index + 1));
            continue;
        }
        search_history.push(CandidateSearchAttempt {
            proposal_index: index + 1,
            outcome: "accepted_distinct_route",
            mutation_assignment,
        });
        let snapshot = FrozenPromptProfileSnapshot::new_gepa(
            AgentPolicy::Pro.label(),
            genome.clone(),
            parent.id.clone(),
            training_dataset_sha256.to_string(),
            reflection_evidence_sha256.to_string(),
        )?;
        let identity = CandidateIdentityReceipt {
            profile_id: genome.id.clone(),
            profile_sha256: prompt_genome_sha256(&genome)?,
            route_profile_sha256,
            parent_profile_id: parent.id.clone(),
            generation: genome.generation,
            mutation_response_sha256: response_sha256,
            mutation_repaired: repaired,
            proposal_index: index + 1,
            mutated_genes,
            intervention,
            snapshot_artifact_sha256: snapshot.artifact_sha256()?,
        };
        population.push(GeneratedCandidate {
            genome,
            snapshot,
            identity,
        });
        if population.len() == TARGET_CANDIDATE_POPULATION {
            break;
        }
    }
    if population.len() != TARGET_CANDIDATE_POPULATION {
        return Err(format!(
            "GEPA population search produced {} distinct route phenotypes; {} required; {}",
            population.len(),
            TARGET_CANDIDATE_POPULATION,
            rejections.join(" | "),
        ));
    }
    let model_calls = control.progress().model_calls;
    let resources = control.resource_usage().segment;
    Ok(GeneratedCandidatePopulation {
        candidates: population,
        model_calls,
        physical_attempts: resources.physical_attempts,
        total_tokens: resources.total_tokens,
    })
}

fn candidate_population_search_instruction(
    index: usize,
    history: &[CandidateSearchAttempt],
) -> String {
    let prior_attempts = if history.is_empty() {
        "- none; establish the first evidence-supported mutation".to_string()
    } else {
        history
            .iter()
            .map(|attempt| {
                format!(
                    "- proposal {} {}: {}",
                    attempt.proposal_index, attempt.outcome, attempt.mutation_assignment
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        concat!(
            "\n\nPopulation search instruction: this is proposal {} of {}. ",
            "Infer a general Direct-versus-Workflow routing policy only from the matched treatment contrast. Change only route_directive. ",
            "Do not copy benchmark content or widen authority merely to create variety.\n\n",
            "Normalized proposals already tried in this same search:\n{}\n\n",
            "Novelty is a hard acceptance condition. Do not repeat a canonical gene/value assignment listed above. ",
            "Prefer a materially distinct, evidence-supported routing policy rather than wording variation. A duplicate route policy will be rejected."
        ),
        index + 1,
        MAX_CANDIDATE_PROPOSALS,
        prior_attempts,
    )
}

fn candidate_mutation_assignment(
    candidate: &ConductorPromptGenome,
    mutated_genes: &[String],
) -> Result<String, String> {
    let candidate = serde_json::to_value(candidate)
        .map_err(|error| format!("failed to encode candidate mutation: {error}"))?;
    let candidate = candidate
        .as_object()
        .ok_or_else(|| "candidate mutation did not encode as an object".to_string())?;
    let mut assignment = BTreeMap::new();
    for gene in mutated_genes {
        let value = candidate
            .get(gene)
            .cloned()
            .ok_or_else(|| format!("candidate mutation omitted gene {gene}"))?;
        let value = if gene == "custom_directive" || gene == "route_directive" {
            serde_json::Value::String(format!(
                "sha256:{}",
                sha256_hex(value.as_str().unwrap_or_default().as_bytes())
            ))
        } else {
            value
        };
        assignment.insert(gene.clone(), value);
    }
    serde_json::to_string(&assignment)
        .map_err(|error| format!("failed to encode candidate mutation assignment: {error}"))
}

#[allow(clippy::too_many_arguments)]
fn parse_candidate_response(
    state: &tauri::State<'_, AppState>,
    provider: &ProviderConfig,
    run_context: &Metadata,
    parent: &ConductorPromptGenome,
    packets: &[AgentEvaluationReflectionPacket],
    index: usize,
    response: &str,
    control: &Arc<AgentRunControl>,
) -> Result<(ConductorPromptGenome, String, bool), String> {
    let response_sha256 = sha256_hex(response.as_bytes());
    let candidate_id = format!("learned-pro-v9-route-{}", &response_sha256[..16]);
    match parent.learned_route_mutation_from_response(response, candidate_id, packets) {
        Ok(candidate) => Ok((candidate, response_sha256, false)),
        Err(initial_error) => {
            let mutation_id = format!("workflow-gepa-v9-route-mutation-repair-{}", index + 1);
            let stage = format!("workflow_gepa_v9_route_mutation_repair_{}", index + 1);
            let mut repair_prompt = parent.reflective_route_mutation_prompt(packets)?;
            repair_prompt.push_str(&format!(
                "\n\nYour prior response was invalid: {initial_error}. Return a corrected full genome JSON. Change only route_directive. Invalid response SHA-256: {}.",
                response_sha256,
            ));
            let repaired = run_background_prompt_mutation_stage_with_liveness(
                state,
                provider,
                &phase16_task_id(),
                run_context,
                &mutation_id,
                &stage,
                repair_prompt,
                control,
                Some(CANDIDATE_RESPONSE_START_TIMEOUT),
            )?;
            let repaired_sha256 = sha256_hex(repaired.as_bytes());
            let repaired_id = format!("learned-pro-v9-route-{}", &repaired_sha256[..16]);
            let candidate =
                parent.learned_route_mutation_from_response(&repaired, repaired_id, packets)?;
            Ok((candidate, repaired_sha256, true))
        }
    }
}

fn candidate_proposal_control(
    aggregate: &Arc<AgentRunControl>,
) -> Result<Arc<AgentRunControl>, String> {
    let progress = aggregate.progress();
    let remaining_calls = aggregate
        .budget()
        .max_model_calls
        .saturating_sub(progress.model_calls);
    if remaining_calls == 0 {
        return Err("GEPA candidate-search model-call budget is exhausted".to_string());
    }
    let allocation_divisor = (remaining_calls / MODEL_CALLS_PER_PROPOSAL).max(1);
    aggregate
        .isolated_treatment(allocation_divisor)
        .map(Arc::new)
        .map_err(|reason| {
            format!(
                "GEPA candidate proposal could not reserve an isolated lane: {}",
                reason.code()
            )
        })
}

fn candidate_search_budget() -> agent_runtime::RunBudget {
    let mut budget = agent_runtime::RunBudget::for_effort("fast");
    budget.max_duration = CANDIDATE_SEARCH_TIMEOUT;
    budget.model_call_timeout = CANDIDATE_MODEL_CALL_TIMEOUT;
    budget.no_progress_timeout = CANDIDATE_MODEL_CALL_TIMEOUT;
    budget.initial_model_calls = 12;
    budget.max_model_calls = 12;
    budget.model_calls_per_extension = 1;
    budget.initial_agent_turns = 12;
    budget.max_agent_turns = 12;
    budget.agent_turns_per_extension = 1;
    budget.max_repair_attempts = 12;
    budget.max_physical_model_attempts = 48;
    budget.max_total_tokens =
        48_u64.saturating_mul(agent_runtime::CONSERVATIVE_TOKENS_PER_PHYSICAL_MODEL_ATTEMPT);
    budget.terminal_model_call_reserve = 0;
    budget.terminal_time_reserve = std::time::Duration::ZERO;
    budget.terminal_physical_model_attempt_reserve = 0;
    budget.terminal_token_reserve = 0;
    budget
}

fn mutated_gene_names(
    parent: &ConductorPromptGenome,
    candidate: &ConductorPromptGenome,
) -> Vec<String> {
    classify_prompt_mutation(parent, candidate).changed_genes
}

pub(super) fn build_training_receipt(
    candidate: &GeneratedCandidate,
    pairs: Vec<ProductPairReceipt>,
) -> Result<CandidateTrainingReceipt, String> {
    let train = aggregate_pairs(&pairs, &candidate.genome)?;
    let selection_reason = training_ineligibility(&train).unwrap_or("train_frontier_eligible");
    Ok(CandidateTrainingReceipt {
        candidate: candidate.identity.clone(),
        train_pairs: pairs,
        train,
        eligible: selection_reason == "train_frontier_eligible",
        selection_reason: selection_reason.to_string(),
        selected: false,
    })
}

fn training_ineligibility(aggregate: &PairAggregateReceipt) -> Option<&'static str> {
    if aggregate.pairs < 2 || aggregate.unique_cases < 2 || aggregate.task_classes < 2 {
        return Some("insufficient_train_coverage");
    }
    if aggregate.candidate_completed != aggregate.pairs
        || aggregate.candidate_quality_passed != aggregate.pairs
    {
        return Some("incomplete_or_failed_train_quality");
    }
    if aggregate.candidate_safety_violations > 0 || aggregate.candidate_losses > 0 {
        return Some("train_safety_or_quality_regression");
    }
    if aggregate.causal_profile_runs != aggregate.pairs
        || aggregate.route_semantics_runs != aggregate.pairs
    {
        return Some("missing_causal_profile_receipt");
    }
    let resource_bounded = aggregate.latency_ratio <= TRAIN_RESOURCE_RATIO_CEILING
        && aggregate.token_ratio <= TRAIN_RESOURCE_RATIO_CEILING;
    let quality_gain =
        aggregate.candidate_wins > 0 && aggregate.behavior_delta > f64::EPSILON && resource_bounded;
    let efficiency_gain = (aggregate.latency_ratio <= 0.95 && aggregate.token_ratio <= 1.05)
        || (aggregate.token_ratio <= 0.95 && aggregate.latency_ratio <= 1.05);
    let measured_gain = quality_gain || efficiency_gain;
    if !measured_gain {
        return Some("no_train_side_improvement");
    }
    None
}

pub(super) fn select_training_candidate(
    population: &[GeneratedCandidate],
    receipts: &mut [CandidateTrainingReceipt],
) -> Result<String, String> {
    let eligible_ids = receipts
        .iter()
        .filter(|receipt| receipt.eligible)
        .map(|receipt| receipt.candidate.profile_id.as_str())
        .collect::<BTreeSet<_>>();
    let eligible_genomes = population
        .iter()
        .filter(|candidate| eligible_ids.contains(candidate.genome.id.as_str()))
        .map(|candidate| candidate.genome.clone())
        .collect::<Vec<_>>();
    let mut scores = Vec::new();
    for receipt in receipts.iter().filter(|receipt| receipt.eligible) {
        for pair in &receipt.train_pairs {
            let digest = sha256_hex(pair.evaluation_id.as_bytes());
            let seed = u64::from_str_radix(&digest[..16], 16).unwrap_or_default();
            scores.push(AgentEvaluationCaseScore {
                suite_id: CAMPAIGN_SUITE_ID.to_string(),
                suite_version: CAMPAIGN_VERSION,
                case_id: pair.case_id.clone(),
                category: pair.category.clone(),
                split: AgentEvaluationSplit::Pareto,
                run_id: pair.evaluation_id.clone(),
                seed,
                candidate_id: receipt.candidate.profile_id.clone(),
                candidate_fingerprint: receipt.candidate.profile_sha256.clone(),
                evidence_source: AgentEvaluationEvidenceSource::Deterministic,
                score: pair.candidate.behavior_score,
                verified_success: pair.candidate.completed
                    && pair.candidate.quality_passed
                    && pair.candidate.safety_violations == 0,
                latency_ms: pair.candidate.latency_ms,
                total_tokens: pair.candidate.total_tokens,
                safety_violations: pair.candidate.safety_violations as u64,
            });
        }
    }
    let archive = PromptInstanceParetoArchive::build(&eligible_genomes, &scores, 1)?;
    let selected = archive
        .best_aggregate()
        .map(|candidate| candidate.profile_id.clone())
        .ok_or_else(|| "no GEPA candidate passed train-only Pareto selection".to_string())?;
    let receipt = receipts
        .iter_mut()
        .find(|receipt| receipt.candidate.profile_id == selected)
        .ok_or_else(|| "selected GEPA profile has no training receipt".to_string())?;
    receipt.selected = true;
    receipt.selection_reason = "selected_train_pareto_frontier".to_string();
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aggregate() -> PairAggregateReceipt {
        PairAggregateReceipt {
            pairs: 2,
            unique_cases: 2,
            task_classes: 2,
            candidate_wins: 0,
            candidate_win_cases: 0,
            candidate_losses: 0,
            ties: 2,
            seed_behavior_score: 1.0,
            candidate_behavior_score: 1.0,
            behavior_delta: 0.0,
            seed_completed: 2,
            candidate_completed: 2,
            seed_quality_passed: 2,
            candidate_quality_passed: 2,
            candidate_safety_violations: 0,
            latency_ratio: 0.94,
            token_ratio: 1.0,
            causal_profile_runs: 2,
            route_semantics_runs: 2,
            workflow_profile_runs: 1,
            seed_direct_runs: 1,
            seed_workflow_runs: 1,
            candidate_direct_runs: 1,
            candidate_workflow_runs: 1,
            route_changed_pairs: 0,
        }
    }

    #[test]
    fn train_selection_requires_causal_receipts_and_a_measured_gain() {
        assert_eq!(training_ineligibility(&aggregate()), None);

        let mut no_route = aggregate();
        no_route.route_semantics_runs = 1;
        assert_eq!(
            training_ineligibility(&no_route),
            Some("missing_causal_profile_receipt")
        );

        let mut no_gain = aggregate();
        no_gain.latency_ratio = 1.0;
        assert_eq!(
            training_ineligibility(&no_gain),
            Some("no_train_side_improvement")
        );

        let mut route_only = aggregate();
        route_only.latency_ratio = 1.20;
        route_only.token_ratio = 1.20;
        assert_eq!(
            training_ineligibility(&route_only),
            Some("no_train_side_improvement")
        );

        let mut one_sided_efficiency = aggregate();
        one_sided_efficiency.latency_ratio = 0.90;
        one_sided_efficiency.token_ratio = 1.20;
        assert_eq!(
            training_ineligibility(&one_sided_efficiency),
            Some("no_train_side_improvement")
        );

        let mut quality_gain = aggregate();
        quality_gain.candidate_wins = 1;
        quality_gain.candidate_win_cases = 1;
        quality_gain.ties = 1;
        quality_gain.seed_behavior_score = 0.8;
        quality_gain.candidate_behavior_score = 0.9;
        quality_gain.behavior_delta = 0.1;
        quality_gain.latency_ratio = 1.20;
        quality_gain.token_ratio = 1.20;
        assert_eq!(training_ineligibility(&quality_gain), None);

        quality_gain.latency_ratio = 1.30;
        assert_eq!(
            training_ineligibility(&quality_gain),
            Some("no_train_side_improvement")
        );
    }

    #[test]
    fn candidate_gene_diff_counts_only_actual_causal_changes() {
        let parent = ConductorPromptGenome::seed_for_effort("pro");
        let mut candidate = parent.clone();
        candidate.max_parallel_branches += 1;
        assert_eq!(
            mutated_gene_names(&parent, &candidate),
            vec!["max_parallel_branches"]
        );

        candidate.max_step_attempts += 1;
        candidate.custom_directive = "Use the strongest observed evidence.".to_string();
        assert_eq!(mutated_gene_names(&parent, &candidate).len(), 3);
    }

    #[test]
    fn population_search_prompt_carries_forward_normalized_attempts() {
        let first = candidate_population_search_instruction(0, &[]);
        assert!(first.contains("proposal 1 of 6"));
        assert!(first.contains("none; establish the first evidence-supported mutation"));

        let history = vec![
            CandidateSearchAttempt {
                proposal_index: 1,
                outcome: "accepted_distinct_route",
                mutation_assignment: "{\"graph_depth\":\"deep\"}".to_string(),
            },
            CandidateSearchAttempt {
                proposal_index: 2,
                outcome: "rejected_duplicate_route",
                mutation_assignment: "{\"graph_depth\":\"deep\"}".to_string(),
            },
        ];
        let later = candidate_population_search_instruction(2, &history);

        assert!(later.contains("proposal 3 of 6"));
        assert!(later.contains("proposal 1 accepted_distinct_route: {\"graph_depth\":\"deep\"}"));
        assert!(later.contains("proposal 2 rejected_duplicate_route: {\"graph_depth\":\"deep\"}"));
        assert!(later.contains("Novelty is a hard acceptance condition"));
    }

    #[test]
    fn mutation_assignment_is_canonical_and_hides_directive_text() {
        let parent = ConductorPromptGenome::seed_for_effort("pro");
        let mut candidate = parent.clone();
        candidate.graph_depth = orchestrator::PromptGraphDepth::Lean;
        candidate.custom_directive = "private trajectory-derived wording".to_string();
        let genes = mutated_gene_names(&parent, &candidate);
        let assignment = candidate_mutation_assignment(&candidate, &genes).unwrap();

        assert_eq!(genes, vec!["graph_depth", "custom_directive"]);
        assert!(assignment.contains("\"graph_depth\":\"lean\""));
        assert!(assignment.contains("\"custom_directive\":\"sha256:"));
        assert!(!assignment.contains("private trajectory-derived wording"));
    }

    #[test]
    fn candidate_search_uses_a_bounded_mutation_specific_liveness_budget() {
        let fast = agent_runtime::RunBudget::for_effort("fast");
        let candidate = candidate_search_budget();

        assert_eq!(fast.model_call_timeout, std::time::Duration::from_secs(180));
        assert_eq!(candidate.max_duration, CANDIDATE_SEARCH_TIMEOUT);
        assert_eq!(candidate.model_call_timeout, CANDIDATE_MODEL_CALL_TIMEOUT);
        assert_eq!(candidate.no_progress_timeout, CANDIDATE_MODEL_CALL_TIMEOUT);
        assert!(CANDIDATE_RESPONSE_START_TIMEOUT < candidate.model_call_timeout);
        assert_eq!(candidate.max_model_calls, 12);
        assert_eq!(candidate.max_physical_model_attempts, 48);
        assert_eq!(candidate.terminal_model_call_reserve, 0);
        assert_eq!(candidate.terminal_physical_model_attempt_reserve, 0);
        assert_eq!(candidate.terminal_token_reserve, 0);
    }

    #[test]
    fn failed_proposal_lane_does_not_poison_later_candidates() {
        let aggregate = Arc::new(AgentRunControl::with_budget(candidate_search_budget()));
        let failed = candidate_proposal_control(&aggregate).unwrap();
        assert_eq!(failed.budget().max_model_calls, MODEL_CALLS_PER_PROPOSAL);
        failed.begin_model_call("mutation-1").unwrap();
        failed.request_stop(agent_runtime::RunStopReason::NoProgress);
        failed.finish_model_call();
        aggregate
            .absorb_isolated_treatments(&[failed.as_ref()])
            .unwrap();

        assert_eq!(aggregate.stop_reason(), None);
        assert_eq!(aggregate.progress().model_calls, 1);

        let next = candidate_proposal_control(&aggregate).unwrap();
        assert_eq!(next.stop_reason(), None);
        next.begin_model_call("mutation-2").unwrap();
        next.finish_model_call();
        aggregate
            .absorb_isolated_treatments(&[next.as_ref()])
            .unwrap();

        assert_eq!(aggregate.stop_reason(), None);
        assert_eq!(aggregate.progress().model_calls, 2);
    }
}
