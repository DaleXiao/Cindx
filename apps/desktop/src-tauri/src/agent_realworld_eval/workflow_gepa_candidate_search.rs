use super::workflow_gepa_campaign_contract::{
    aggregate_pairs, PairAggregateReceipt, ProductPairReceipt, CAMPAIGN_SUITE_ID,
    CAMPAIGN_VERSION,
};
use crate::app_state::AppState;
use crate::configuration_models::ProviderConfig;
use crate::phase16_task_id;
use crate::prompt_mutation_runtime::run_background_prompt_mutation_stage;
use agent_core::Metadata;
use agent_runtime::AgentRunControl;
use orchestrator::{
    prompt_genome_sha256, sha256_hex, AgentEvaluationCaseScore,
    AgentEvaluationEvidenceSource, AgentEvaluationReflectionPacket, AgentEvaluationSplit,
    AgentPolicy, ConductorPromptGenome, FrozenPromptProfileSnapshot,
    PromptInstanceParetoArchive,
};
use serde::Serialize;
use std::collections::BTreeSet;
use std::sync::Arc;

pub(super) const TARGET_CANDIDATE_POPULATION: usize = 3;
const MAX_CANDIDATE_PROPOSALS: usize = 6;
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
    let mut model_calls = 0usize;
    for index in 0..MAX_CANDIDATE_PROPOSALS {
        let mut prompt = parent.reflective_mutation_prompt(packets)?;
        prompt.push_str(&format!(
            "\n\nPopulation search instruction: this is proposal {} of {}. Rank the bottlenecks that are directly supported by the supplied trajectories, choose the single highest-leverage generalizable bottleneck that prior proposals may have missed, and change only the one or two genes causally responsible. Do not follow a preselected optimization direction and do not copy benchmark content.",
            index + 1,
            MAX_CANDIDATE_PROPOSALS,
        ));
        let mutation_id = format!("workflow-gepa-v7-mutation-{}", index + 1);
        let stage = format!("workflow_gepa_v7_mutation_{}", index + 1);
        model_calls = model_calls.saturating_add(1);
        let response = run_background_prompt_mutation_stage(
            state,
            provider,
            &phase16_task_id(),
            run_context,
            &mutation_id,
            &stage,
            prompt,
            &control,
        )?;
        let (genome, response_sha256, repaired, repair_calls) = parse_candidate_response(
            state,
            provider,
            run_context,
            parent,
            packets,
            index,
            &response,
            &control,
        )?;
        model_calls = model_calls.saturating_add(repair_calls);
        let mutated_genes = mutated_gene_names(parent, &genome);
        if !(1..=2).contains(&mutated_genes.len()) {
            return Err(format!(
                "GEPA proposal {} changed {} genes after normalization",
                index + 1,
                mutated_genes.len()
            ));
        }
        let route_profile_sha256 =
            genome.route_decision_profile_sha256(AgentPolicy::Pro.label())?;
        if !seen_routes.insert(route_profile_sha256.clone()) {
            continue;
        }
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
            "GEPA population search produced {} distinct route phenotypes; {} required",
            population.len(),
            TARGET_CANDIDATE_POPULATION,
        ));
    }
    let resources = control.resource_usage().segment;
    Ok(GeneratedCandidatePopulation {
        candidates: population,
        model_calls,
        physical_attempts: resources.physical_attempts,
        total_tokens: resources.total_tokens,
    })
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
) -> Result<(ConductorPromptGenome, String, bool, usize), String> {
    let response_sha256 = sha256_hex(response.as_bytes());
    let candidate_id = format!("learned-pro-v7-{}", &response_sha256[..16]);
    match parent.learned_reflective_mutation_from_response(response, candidate_id, packets) {
        Ok(candidate) => Ok((candidate, response_sha256, false, 0)),
        Err(initial_error) => {
            let mutation_id = format!("workflow-gepa-v7-mutation-repair-{}", index + 1);
            let stage = format!("workflow_gepa_v7_mutation_repair_{}", index + 1);
            let repaired = run_background_prompt_mutation_stage(
                state,
                provider,
                &phase16_task_id(),
                run_context,
                &mutation_id,
                &stage,
                parent.mutation_repair_prompt(response, &initial_error),
                control,
            )?;
            let repaired_sha256 = sha256_hex(repaired.as_bytes());
            let repaired_id = format!("learned-pro-v7-{}", &repaired_sha256[..16]);
            let candidate = parent.learned_reflective_mutation_from_response(
                &repaired,
                repaired_id,
                packets,
            )?;
            Ok((candidate, repaired_sha256, true, 1))
        }
    }
}

fn candidate_search_budget() -> agent_runtime::RunBudget {
    let mut budget = agent_runtime::RunBudget::for_effort("fast");
    budget.max_duration = std::time::Duration::from_secs(20 * 60);
    budget.initial_model_calls = 12;
    budget.max_model_calls = 12;
    budget.model_calls_per_extension = 1;
    budget.initial_agent_turns = 12;
    budget.max_agent_turns = 12;
    budget.agent_turns_per_extension = 1;
    budget.max_physical_model_attempts = 48;
    budget.max_total_tokens = 48_u64.saturating_mul(
        agent_runtime::CONSERVATIVE_TOKENS_PER_PHYSICAL_MODEL_ATTEMPT,
    );
    budget.terminal_model_call_reserve = 1;
    budget.terminal_physical_model_attempt_reserve = 4;
    budget.terminal_token_reserve = 4_u64.saturating_mul(
        agent_runtime::CONSERVATIVE_TOKENS_PER_PHYSICAL_MODEL_ATTEMPT,
    );
    budget
}

fn mutated_gene_names(
    parent: &ConductorPromptGenome,
    candidate: &ConductorPromptGenome,
) -> Vec<String> {
    let mut genes = Vec::new();
    macro_rules! changed {
        ($field:ident) => {
            if candidate.$field != parent.$field {
                genes.push(stringify!($field).to_string());
            }
        };
    }
    changed!(graph_depth);
    changed!(verification);
    changed!(context_policy);
    changed!(max_parallel_branches);
    changed!(tool_policy);
    changed!(retry_policy);
    changed!(topology_strategy);
    changed!(role_strategy);
    changed!(commit_strategy);
    changed!(max_step_attempts);
    let mut budget_comparison = candidate.clone();
    budget_comparison.tool_policy = parent.tool_policy;
    if budget_comparison.effective_max_model_turns_per_step()
        != parent.effective_max_model_turns_per_step()
    {
        genes.push("max_model_turns_per_step".to_string());
    }
    if budget_comparison.effective_max_tool_calls_per_step()
        != parent.effective_max_tool_calls_per_step()
    {
        genes.push("max_tool_calls_per_step".to_string());
    }
    if candidate.custom_directive.trim() != parent.custom_directive.trim() {
        genes.push("custom_directive".to_string());
    }
    genes
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
    let quality_gain = aggregate.candidate_wins > 0
        && aggregate.behavior_delta > f64::EPSILON
        && resource_bounded;
    let efficiency_gain = (aggregate.latency_ratio <= 0.95
        && aggregate.token_ratio <= 1.05)
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
}
