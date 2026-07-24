use crate::{RagChunk, RagSearchResult};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct RetrievalChannelOutcome {
    pub name: String,
    pub duration_ms: u64,
    pub results: Vec<RagSearchResult>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FusedRagSource {
    pub path: String,
    pub start_line: u64,
    pub end_line: u64,
    pub file_hash: String,
    pub score: f32,
    pub reason: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RetrievalFusionResult {
    pub results: Vec<RagSearchResult>,
    pub sources: Vec<FusedRagSource>,
}

#[derive(Debug)]
struct FusedRetrievalCandidate {
    result: RagSearchResult,
    best_contribution: f32,
    family_scores: BTreeMap<String, f32>,
    channels: Vec<String>,
}

impl FusedRetrievalCandidate {
    fn score(&self) -> f32 {
        let base = self.family_scores.values().sum::<f32>();
        let family_count = self.family_scores.len();
        if family_count <= 1 {
            return base;
        }
        let average = base / family_count as f32;
        let consensus_depth = family_count.saturating_sub(1).min(3) as f32;
        base + average * 0.12 * consensus_depth
    }
}

pub fn merge_retrieval_channel(
    channel: &mut RetrievalChannelOutcome,
    enrichment: RetrievalChannelOutcome,
    limit: usize,
) {
    channel.duration_ms = channel.duration_ms.saturating_add(enrichment.duration_ms);
    channel.results.extend(enrichment.results);
    channel.results.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.chunk.path.cmp(&right.chunk.path))
            .then_with(|| left.chunk.start_line.cmp(&right.chunk.start_line))
    });
    let mut unique = Vec::new();
    for result in channel.results.drain(..) {
        if unique.iter().any(|candidate: &RagSearchResult| {
            candidate.chunk.id == result.chunk.id
                || (candidate.chunk.path == result.chunk.path
                    && retrieval_ranges_overlap(&candidate.chunk, &result.chunk))
        }) {
            continue;
        }
        unique.push(result);
        if unique.len() >= limit.max(1) {
            break;
        }
    }
    channel.results = unique;
    if !channel.results.is_empty() {
        channel.error = None;
    } else if channel.error.is_none() {
        channel.error = enrichment.error;
    }
}

pub fn fuse_retrieval_channels(
    channels: &[RetrievalChannelOutcome],
    limit: usize,
) -> RetrievalFusionResult {
    let mut fused = Vec::<FusedRetrievalCandidate>::new();
    for channel in channels {
        let weight = retrieval_channel_weight(&channel.name);
        let family = retrieval_channel_family(&channel.name).to_string();
        let calibrated = calibrated_retrieval_scores(&channel.results);
        for (result, calibrated_score) in channel.results.iter().zip(calibrated) {
            let contribution = weight * calibrated_score;
            let existing = fused.iter().position(|candidate| {
                candidate.result.chunk.id == result.chunk.id
                    || (candidate.result.chunk.path == result.chunk.path
                        && retrieval_ranges_overlap(&candidate.result.chunk, &result.chunk))
            });
            let entry = if let Some(index) = existing {
                &mut fused[index]
            } else {
                fused.push(FusedRetrievalCandidate {
                    result: result.clone(),
                    best_contribution: contribution,
                    family_scores: BTreeMap::new(),
                    channels: Vec::new(),
                });
                fused.last_mut().expect("fused result was just inserted")
            };
            entry
                .family_scores
                .entry(family.clone())
                .and_modify(|score| *score = score.max(contribution))
                .or_insert(contribution);
            if !entry.channels.contains(&channel.name) {
                entry.channels.push(channel.name.clone());
            }
            if contribution > entry.best_contribution {
                entry.result = result.clone();
                entry.best_contribution = contribution;
            }
        }
    }
    fused.sort_by(|left, right| {
        right
            .score()
            .partial_cmp(&left.score())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.result.chunk.path.cmp(&right.result.chunk.path))
            .then_with(|| {
                left.result
                    .chunk
                    .start_line
                    .cmp(&right.result.chunk.start_line)
            })
    });
    let mut path_counts = BTreeMap::<String, usize>::new();
    let mut selected = Vec::new();
    for candidate in fused {
        let count = path_counts
            .entry(candidate.result.chunk.path.clone())
            .or_default();
        if *count >= 2 {
            continue;
        }
        *count += 1;
        selected.push(candidate);
        if selected.len() >= limit.max(1) {
            break;
        }
    }
    let max_score = selected
        .first()
        .map(FusedRetrievalCandidate::score)
        .unwrap_or(1.0)
        .max(f32::EPSILON);
    let results = selected
        .iter()
        .map(|candidate| RagSearchResult {
            chunk: candidate.result.chunk.clone(),
            score: candidate.score() / max_score,
        })
        .collect::<Vec<_>>();
    let sources = selected
        .into_iter()
        .map(|candidate| {
            let score = candidate.score() / max_score;
            let family_count = candidate.family_scores.len();
            let mut reasons = candidate.channels;
            if family_count > 1 {
                reasons.push(format!("consensus:{family_count}"));
            }
            FusedRagSource {
                path: candidate.result.chunk.path,
                start_line: candidate.result.chunk.start_line,
                end_line: candidate.result.chunk.end_line,
                file_hash: candidate.result.chunk.file_hash,
                score,
                reason: reasons.join(" + "),
                text: candidate.result.chunk.text,
            }
        })
        .collect();
    RetrievalFusionResult { results, sources }
}

pub fn retrieval_ranges_overlap(left: &RagChunk, right: &RagChunk) -> bool {
    left.start_line <= right.end_line && right.start_line <= left.end_line
}

fn retrieval_channel_weight(name: &str) -> f32 {
    match name {
        "semantic_rag" => 1.0,
        "graph_recall" => 0.9,
        "graph_walk" => 0.8,
        "file_search" => 0.85,
        _ => 0.5,
    }
}

fn retrieval_channel_family(name: &str) -> &'static str {
    match name {
        "semantic_rag" => "semantic",
        "graph_recall" | "graph_walk" => "graph",
        "file_search" => "file",
        _ => "other",
    }
}

fn calibrated_retrieval_scores(results: &[RagSearchResult]) -> Vec<f32> {
    let raw_ceiling = results
        .iter()
        .filter_map(|result| result.score.is_finite().then_some(result.score.max(0.0)))
        .fold(0.0f32, f32::max)
        .max(f32::EPSILON);
    results
        .iter()
        .enumerate()
        .map(|(rank, result)| {
            let rank_score = 1.0 / (1.0 + rank as f32 * 0.75);
            let evidence_score = if result.score.is_finite() {
                result.score.max(0.0) / raw_ceiling
            } else {
                0.0
            };
            rank_score * 0.65 + evidence_score * 0.35
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(id: &str, path: &str, line: u64, score: f32) -> RagSearchResult {
        RagSearchResult {
            chunk: RagChunk {
                id: id.to_string(),
                path: path.to_string(),
                file_hash: format!("hash-{id}"),
                modified_time_ms: 0,
                start_line: line,
                end_line: line + 4,
                indexed_at_ms: 0,
                text: format!("evidence {id}"),
                embedding: Vec::new(),
                embedding_provider: "test".to_string(),
                embedding_model: "test".to_string(),
                embedding_dimensions: 0,
            },
            score,
        }
    }

    fn channel(name: &str, results: Vec<RagSearchResult>) -> RetrievalChannelOutcome {
        RetrievalChannelOutcome {
            name: name.to_string(),
            duration_ms: 1,
            results,
            error: None,
        }
    }

    #[test]
    fn independent_consensus_beats_a_single_channel_outlier() {
        let shared = result("shared", "shared.rs", 1, 0.7);
        let outlier = result("outlier", "outlier.rs", 1, 1.0);
        let fusion = fuse_retrieval_channels(
            &[
                channel("semantic_rag", vec![outlier, shared.clone()]),
                channel("graph_recall", vec![shared.clone()]),
                channel("file_search", vec![shared]),
            ],
            4,
        );

        assert_eq!(fusion.results[0].chunk.id, "shared");
        assert!(fusion.sources[0].reason.contains("consensus:3"));
    }

    #[test]
    fn correlated_graph_channels_count_as_one_family() {
        let shared = result("shared", "shared.rs", 1, 0.8);
        let fusion = fuse_retrieval_channels(
            &[
                channel("graph_recall", vec![shared.clone()]),
                channel("graph_walk", vec![shared]),
            ],
            4,
        );

        assert!(!fusion.sources[0].reason.contains("consensus:"));
    }

    #[test]
    fn channel_merge_deduplicates_overlapping_ranges_and_clears_recovered_error() {
        let mut original = RetrievalChannelOutcome {
            name: "graph_walk".to_string(),
            duration_ms: 2,
            results: Vec::new(),
            error: Some("initial failure".to_string()),
        };
        merge_retrieval_channel(
            &mut original,
            channel(
                "graph_walk",
                vec![
                    result("a", "src/a.rs", 1, 0.9),
                    result("b", "src/a.rs", 3, 0.8),
                ],
            ),
            4,
        );

        assert_eq!(original.results.len(), 1);
        assert_eq!(original.duration_ms, 3);
        assert!(original.error.is_none());
    }
}
