use super::*;

pub(crate) fn prompt_instance_pareto_scores(
    population: &[ConductorPromptGenome],
    observations: &[PromptEvolutionObservation],
) -> Vec<AgentEvaluationCaseScore> {
    let active_cohort_sha256 =
        orchestrator::latest_scientific_training_dataset_digest(observations);
    let fingerprints = population
        .iter()
        .map(|genome| {
            let fingerprint = serde_json::to_vec(genome)
                .map(|encoded| sha256_hex(&encoded))
                .unwrap_or_else(|_| genome.id.clone());
            (genome.id.as_str(), fingerprint)
        })
        .collect::<BTreeMap<_, _>>();
    observations
        .iter()
        .filter(|observation| observation.split == PromptEvaluationSplit::Train)
        .filter(|observation| observation.mode == PromptEvaluationMode::PairedExecution)
        .filter(|observation| observation.is_scientific_evidence())
        .filter(|observation| {
            active_cohort_sha256.is_some_and(|digest| {
                observation.scientific_cohort_sha256() == Some(digest)
            })
        })
        .filter(|observation| !observation.case_id.trim().is_empty())
        .filter_map(|observation| {
            let fingerprint = fingerprints.get(observation.profile_id.as_str())?;
            let safe = observation.format_valid && observation.safety_violations == 0;
            let identity = observation.evidence_identity();
            let digest = sha256_hex(identity.as_bytes());
            let seed = u64::from_str_radix(&digest[..16], 16).unwrap_or_default();
            Some(AgentEvaluationCaseScore {
                suite_id: "runtime-prompt-evolution".to_string(),
                suite_version: 2,
                case_id: observation.case_id.clone(),
                category: observation.task_class.clone(),
                split: AgentEvaluationSplit::Pareto,
                run_id: observation.evaluation_id.clone(),
                seed,
                candidate_id: observation.profile_id.clone(),
                candidate_fingerprint: fingerprint.clone(),
                evidence_source: AgentEvaluationEvidenceSource::Judge,
                score: if safe {
                    observation.quality_score.clamp(0.0, 1.0)
                } else {
                    0.0
                },
                verified_success: safe && observation.succeeded,
                latency_ms: observation.latency_ms,
                total_tokens: observation.total_tokens,
                safety_violations: observation.safety_violations,
            })
        })
        .collect()
}

pub(crate) fn prompt_instance_merge_candidate(
    archive: &PromptInstanceParetoArchive,
    population: &[ConductorPromptGenome],
) -> Option<ConductorPromptGenome> {
    let by_id = population
        .iter()
        .map(|genome| (genome.id.as_str(), genome))
        .collect::<BTreeMap<_, _>>();
    for (left_index, left_candidate) in archive.candidates.iter().enumerate() {
        let Some(left) = by_id.get(left_candidate.profile_id.as_str()).copied() else {
            continue;
        };
        for right_candidate in archive.candidates.iter().skip(left_index + 1) {
            let Some(right) = by_id.get(right_candidate.profile_id.as_str()).copied() else {
                continue;
            };
            for ancestor_id in left
                .parents
                .iter()
                .filter(|parent| right.parents.iter().any(|candidate| candidate == *parent))
            {
                let Some(ancestor) = by_id.get(ancestor_id.as_str()).copied() else {
                    continue;
                };
                let merge_id = format!(
                    "merge-g{}-{}-{}",
                    left.generation.max(right.generation).saturating_add(1),
                    left.id,
                    right.id
                );
                if by_id.contains_key(merge_id.as_str()) {
                    continue;
                }
                if let Ok(merged) = archive.merge_complementary(merge_id, ancestor, left, right) {
                    return Some(merged);
                }
            }
        }
    }
    None
}
