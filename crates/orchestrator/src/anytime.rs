use crate::{ConductorExecutionContract, ConductorStopPolicy};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::BTreeMap;

pub const ANYTIME_BPS_MAX: u16 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnytimeCandidateKind {
    DirectAnchor,
    Workflow,
    Verification,
    Repair,
    Synthesis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnytimeCandidateState {
    Pending,
    Running,
    Usable,
    Verified,
    Failed,
    Cancelled,
}

impl AnytimeCandidateState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Usable | Self::Verified | Self::Failed | Self::Cancelled
        )
    }

    pub fn satisfies_dependency(self) -> bool {
        matches!(self, Self::Usable | Self::Verified)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnytimeCandidate {
    pub id: String,
    pub kind: AnytimeCandidateKind,
    #[serde(default = "default_true")]
    pub commit_eligible: bool,
    #[serde(default)]
    pub dependencies: Vec<String>,
    pub expected_uplift_bps: u16,
    pub uncertainty_bps: u16,
    pub evidence_gap_bps: u16,
    pub latency_risk_bps: u16,
    pub failure_risk_bps: u16,
    pub state: AnytimeCandidateState,
}

impl AnytimeCandidate {
    pub fn direct_anchor(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: AnytimeCandidateKind::DirectAnchor,
            commit_eligible: true,
            dependencies: Vec::new(),
            expected_uplift_bps: 2_500,
            uncertainty_bps: 2_000,
            evidence_gap_bps: 2_000,
            latency_risk_bps: 1_000,
            failure_risk_bps: 1_000,
            state: AnytimeCandidateState::Pending,
        }
    }

    pub fn workflow(
        id: impl Into<String>,
        dependencies: Vec<String>,
        expected_uplift_bps: u16,
    ) -> Self {
        Self {
            id: id.into(),
            kind: AnytimeCandidateKind::Workflow,
            commit_eligible: true,
            dependencies,
            expected_uplift_bps: expected_uplift_bps.min(ANYTIME_BPS_MAX),
            uncertainty_bps: 5_000,
            evidence_gap_bps: 5_000,
            latency_risk_bps: 2_000,
            failure_risk_bps: 2_000,
            state: AnytimeCandidateState::Pending,
        }
    }

    pub fn as_intermediate(mut self) -> Self {
        self.commit_eligible = false;
        self
    }

    pub fn priority_bps(&self, needs_verification: bool) -> i32 {
        let information = u32::from(self.uncertainty_bps)
            .saturating_add(u32::from(self.evidence_gap_bps))
            .min(u32::from(ANYTIME_BPS_MAX) * 2);
        let expected_gain = u32::from(self.expected_uplift_bps).saturating_mul(information)
            / (u32::from(ANYTIME_BPS_MAX) * 2);
        let risk = u32::from(self.latency_risk_bps)
            .saturating_add(u32::from(self.failure_risk_bps))
            .min(u32::from(ANYTIME_BPS_MAX) * 2);
        let reliability = u32::from(ANYTIME_BPS_MAX).saturating_sub(risk / 2).max(1);
        let value = expected_gain.saturating_mul(reliability) / u32::from(ANYTIME_BPS_MAX);
        let kind_bonus = match self.kind {
            AnytimeCandidateKind::DirectAnchor => 5_000,
            AnytimeCandidateKind::Verification if needs_verification => 2_000,
            AnytimeCandidateKind::Synthesis => 1_250,
            AnytimeCandidateKind::Repair => 750,
            AnytimeCandidateKind::Workflow | AnytimeCandidateKind::Verification => 0,
        };
        i32::try_from(value).unwrap_or(i32::MAX) + kind_bonus
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnytimeVerdict {
    pub quality_bps: u16,
    pub confidence_bps: u16,
    pub constraint_coverage_bps: u16,
    pub evidence_count: usize,
    pub safety_violations: u64,
    pub deliverable: bool,
    pub verified: bool,
}

impl AnytimeVerdict {
    pub fn normalized(mut self) -> Self {
        self.quality_bps = self.quality_bps.min(ANYTIME_BPS_MAX);
        self.confidence_bps = self.confidence_bps.min(ANYTIME_BPS_MAX);
        self.constraint_coverage_bps = self.constraint_coverage_bps.min(ANYTIME_BPS_MAX);
        self
    }

    fn rank(&self) -> (bool, u16, u16, u16, usize) {
        (
            self.verified,
            self.quality_bps,
            self.constraint_coverage_bps,
            self.confidence_bps,
            self.evidence_count,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnytimeBestCandidate {
    pub candidate_id: String,
    pub verdict: AnytimeVerdict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnytimeControllerConfig {
    pub max_parallelism: usize,
    pub min_successful_candidates: usize,
    pub max_candidates: usize,
    pub min_usable_quality_bps: u16,
    pub stop_policy: ConductorStopPolicy,
}

impl AnytimeControllerConfig {
    pub fn from_contract(contract: &ConductorExecutionContract) -> Self {
        let max_candidates = match contract.effort.as_str() {
            "fast" => 2,
            "pro" => 12,
            _ => 6,
        };
        Self {
            max_parallelism: contract.max_parallelism.max(1),
            min_successful_candidates: contract.min_successful_branches.max(1),
            max_candidates,
            min_usable_quality_bps: 4_500,
            stop_policy: contract.stop_policy,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnytimeControllerSnapshot {
    pub config: AnytimeControllerConfig,
    pub candidates: Vec<AnytimeCandidate>,
    #[serde(default)]
    pub verdicts: BTreeMap<String, AnytimeVerdict>,
    pub best: Option<AnytimeBestCandidate>,
    pub successful_candidates: usize,
    pub failed_candidates: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnytimeDecision {
    Continue,
    Wait,
    Commit { candidate_id: String },
    Exhausted,
}

#[derive(Debug, Clone)]
pub struct AnytimeController {
    config: AnytimeControllerConfig,
    candidates: BTreeMap<String, AnytimeCandidate>,
    verdicts: BTreeMap<String, AnytimeVerdict>,
    best: Option<AnytimeBestCandidate>,
    successful_candidates: usize,
    failed_candidates: usize,
}

impl AnytimeController {
    pub fn new(config: AnytimeControllerConfig) -> Self {
        Self {
            config,
            candidates: BTreeMap::new(),
            verdicts: BTreeMap::new(),
            best: None,
            successful_candidates: 0,
            failed_candidates: 0,
        }
    }

    pub fn from_snapshot(snapshot: AnytimeControllerSnapshot) -> Result<Self, String> {
        let legacy_best = snapshot.best;
        let mut verdicts = snapshot.verdicts;
        let mut controller = Self::new(snapshot.config);
        for mut candidate in snapshot.candidates {
            // In-flight work is process-local. A restored controller must make it
            // schedulable again instead of waiting forever on a vanished worker.
            if candidate.state == AnytimeCandidateState::Running {
                candidate.state = AnytimeCandidateState::Pending;
            }
            controller.register(candidate)?;
        }
        if verdicts.is_empty() {
            if let Some(best) = legacy_best {
                verdicts.insert(best.candidate_id, best.verdict);
            }
        }
        verdicts.retain(|candidate_id, _| {
            controller
                .candidates
                .get(candidate_id)
                .is_some_and(|candidate| {
                    matches!(
                        candidate.state,
                        AnytimeCandidateState::Usable | AnytimeCandidateState::Verified
                    )
                })
        });
        controller.verdicts = verdicts;
        controller.recount_terminal_candidates();
        controller.recompute_best();
        Ok(controller)
    }

    pub fn snapshot(&self) -> AnytimeControllerSnapshot {
        AnytimeControllerSnapshot {
            config: self.config.clone(),
            candidates: self.candidates.values().cloned().collect(),
            verdicts: self.verdicts.clone(),
            best: self.best.clone(),
            successful_candidates: self.successful_candidates,
            failed_candidates: self.failed_candidates,
        }
    }

    pub fn register(&mut self, mut candidate: AnytimeCandidate) -> Result<(), String> {
        if candidate.id.trim().is_empty() {
            return Err("anytime candidate id is required".to_string());
        }
        if self.candidates.len() >= self.config.max_candidates {
            return Err(format!(
                "anytime candidate limit {} reached",
                self.config.max_candidates
            ));
        }
        if self.candidates.contains_key(&candidate.id) {
            return Err(format!("duplicate anytime candidate id {}", candidate.id));
        }
        candidate.expected_uplift_bps = candidate.expected_uplift_bps.min(ANYTIME_BPS_MAX);
        candidate.uncertainty_bps = candidate.uncertainty_bps.min(ANYTIME_BPS_MAX);
        candidate.latency_risk_bps = candidate.latency_risk_bps.min(ANYTIME_BPS_MAX);
        candidate.failure_risk_bps = candidate.failure_risk_bps.min(ANYTIME_BPS_MAX);
        self.candidates.insert(candidate.id.clone(), candidate);
        Ok(())
    }

    pub fn ready_candidates(&self) -> Vec<&AnytimeCandidate> {
        let needs_verification = self
            .best
            .as_ref()
            .is_some_and(|best| !best.verdict.verified);
        // The direct anchor is a latency hedge, not a workflow branch. It runs
        // beside the contract's branch budget so it cannot starve the actual
        // DAG when max_parallelism is one or two.
        let scheduled_workflow = self
            .candidates
            .values()
            .filter(|candidate| {
                candidate.state == AnytimeCandidateState::Running
                    && candidate.kind != AnytimeCandidateKind::DirectAnchor
            })
            .count();
        let available = self
            .config
            .max_parallelism
            .saturating_sub(scheduled_workflow);
        let mut ready = self
            .candidates
            .values()
            .filter(|candidate| candidate.state == AnytimeCandidateState::Pending)
            .filter(|candidate| {
                candidate.dependencies.iter().all(|dependency| {
                    self.candidates
                        .get(dependency)
                        .is_some_and(|candidate| candidate.state.satisfies_dependency())
                })
            })
            .collect::<Vec<_>>();
        ready.sort_by_key(|candidate| {
            (
                Reverse(candidate.priority_bps(needs_verification)),
                candidate.id.as_str(),
            )
        });
        ready.truncate(available);
        ready
    }

    pub fn mark_running(&mut self, candidate_id: &str) -> Result<(), String> {
        let candidate = self
            .candidates
            .get_mut(candidate_id)
            .ok_or_else(|| format!("unknown anytime candidate {candidate_id}"))?;
        if candidate.state != AnytimeCandidateState::Pending {
            return Err(format!(
                "anytime candidate {candidate_id} cannot start from {:?}",
                candidate.state
            ));
        }
        candidate.state = AnytimeCandidateState::Running;
        Ok(())
    }

    pub fn observe(&mut self, candidate_id: &str, verdict: AnytimeVerdict) -> Result<bool, String> {
        let candidate = self
            .candidates
            .get(candidate_id)
            .ok_or_else(|| format!("unknown anytime candidate {candidate_id}"))?;
        if candidate.state != AnytimeCandidateState::Running {
            return Err(format!(
                "anytime candidate {candidate_id} cannot finish from {:?}",
                candidate.state
            ));
        }
        self.apply_verdict(candidate_id, verdict)
    }

    pub fn revise(&mut self, candidate_id: &str, verdict: AnytimeVerdict) -> Result<bool, String> {
        let candidate = self
            .candidates
            .get(candidate_id)
            .ok_or_else(|| format!("unknown anytime candidate {candidate_id}"))?;
        if !matches!(
            candidate.state,
            AnytimeCandidateState::Usable | AnytimeCandidateState::Verified
        ) {
            return Err(format!(
                "anytime candidate {candidate_id} cannot be revised from {:?}",
                candidate.state
            ));
        }
        self.apply_verdict(candidate_id, verdict)
    }

    pub fn fail(&mut self, candidate_id: &str) -> Result<(), String> {
        self.finish_without_result(candidate_id, AnytimeCandidateState::Failed)
    }

    pub fn cancel(&mut self, candidate_id: &str) -> Result<(), String> {
        self.finish_without_result(candidate_id, AnytimeCandidateState::Cancelled)
    }

    fn finish_without_result(
        &mut self,
        candidate_id: &str,
        terminal_state: AnytimeCandidateState,
    ) -> Result<(), String> {
        let candidate = self
            .candidates
            .get_mut(candidate_id)
            .ok_or_else(|| format!("unknown anytime candidate {candidate_id}"))?;
        if candidate.state.is_terminal() {
            return Ok(());
        }
        candidate.state = terminal_state;
        self.verdicts.remove(candidate_id);
        if terminal_state == AnytimeCandidateState::Failed {
            self.failed_candidates = self.failed_candidates.saturating_add(1);
        }
        self.recompute_best();
        Ok(())
    }

    fn apply_verdict(
        &mut self,
        candidate_id: &str,
        verdict: AnytimeVerdict,
    ) -> Result<bool, String> {
        let verdict = verdict.normalized();
        let prior_best = self.best.clone();
        let rejected = verdict.safety_violations > 0
            || !verdict.deliverable
            || verdict.quality_bps < self.config.min_usable_quality_bps;
        let candidate = self
            .candidates
            .get_mut(candidate_id)
            .ok_or_else(|| format!("unknown anytime candidate {candidate_id}"))?;
        let was_successful = matches!(
            candidate.state,
            AnytimeCandidateState::Usable | AnytimeCandidateState::Verified
        );
        let was_failed = candidate.state == AnytimeCandidateState::Failed;

        if rejected {
            candidate.state = AnytimeCandidateState::Failed;
            self.verdicts.remove(candidate_id);
            if was_successful {
                self.successful_candidates = self.successful_candidates.saturating_sub(1);
            }
            if !was_failed {
                self.failed_candidates = self.failed_candidates.saturating_add(1);
            }
        } else {
            candidate.state = if verdict.verified {
                AnytimeCandidateState::Verified
            } else {
                AnytimeCandidateState::Usable
            };
            self.verdicts.insert(candidate_id.to_string(), verdict);
            if !was_successful {
                self.successful_candidates = self.successful_candidates.saturating_add(1);
            }
            if was_failed {
                self.failed_candidates = self.failed_candidates.saturating_sub(1);
            }
        }
        self.recompute_best();
        Ok(self.best != prior_best)
    }

    fn recount_terminal_candidates(&mut self) {
        self.successful_candidates = self
            .candidates
            .values()
            .filter(|candidate| {
                matches!(
                    candidate.state,
                    AnytimeCandidateState::Usable | AnytimeCandidateState::Verified
                )
            })
            .count();
        self.failed_candidates = self
            .candidates
            .values()
            .filter(|candidate| candidate.state == AnytimeCandidateState::Failed)
            .count();
    }

    fn recompute_best(&mut self) {
        self.best = self
            .verdicts
            .iter()
            .filter(|(candidate_id, _)| {
                self.candidates.get(*candidate_id).is_some_and(|candidate| {
                    candidate.commit_eligible
                        && matches!(
                            candidate.state,
                            AnytimeCandidateState::Usable | AnytimeCandidateState::Verified
                        )
                })
            })
            .max_by(|(left_id, left), (right_id, right)| {
                left.rank()
                    .cmp(&right.rank())
                    .then_with(|| right_id.cmp(left_id))
            })
            .map(|(candidate_id, verdict)| AnytimeBestCandidate {
                candidate_id: candidate_id.clone(),
                verdict: verdict.clone(),
            });
    }

    pub fn decision(&self, remaining_ms: u64, terminal_reserve_ms: u64) -> AnytimeDecision {
        let best = self.best.as_ref();
        if remaining_ms <= terminal_reserve_ms {
            return best.map_or(AnytimeDecision::Exhausted, |best| AnytimeDecision::Commit {
                candidate_id: best.candidate_id.clone(),
            });
        }

        if let Some(best) = best {
            let should_commit = match self.config.stop_policy {
                ConductorStopPolicy::FirstVerified => best.verdict.verified,
                ConductorStopPolicy::Quorum => {
                    best.verdict.verified
                        && self.successful_candidates >= self.config.min_successful_candidates
                }
                ConductorStopPolicy::Exhaustive => false,
            };
            if should_commit {
                return AnytimeDecision::Commit {
                    candidate_id: best.candidate_id.clone(),
                };
            }
        }

        if self.in_flight() > 0 {
            return AnytimeDecision::Wait;
        }
        if !self.ready_candidates().is_empty() {
            return AnytimeDecision::Continue;
        }
        best.map_or(AnytimeDecision::Exhausted, |best| AnytimeDecision::Commit {
            candidate_id: best.candidate_id.clone(),
        })
    }

    pub fn in_flight(&self) -> usize {
        self.candidates
            .values()
            .filter(|candidate| candidate.state == AnytimeCandidateState::Running)
            .count()
    }

    pub fn best(&self) -> Option<&AnytimeBestCandidate> {
        self.best.as_ref()
    }

    pub fn candidate(&self, candidate_id: &str) -> Option<&AnytimeCandidate> {
        self.candidates.get(candidate_id)
    }

    pub fn verdict(&self, candidate_id: &str) -> Option<&AnytimeVerdict> {
        self.verdicts.get(candidate_id)
    }
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConductorFallbackPolicy, OrchestrationPolicy, TaskClass};

    fn config(stop_policy: ConductorStopPolicy) -> AnytimeControllerConfig {
        AnytimeControllerConfig {
            max_parallelism: 3,
            min_successful_candidates: 2,
            max_candidates: 8,
            min_usable_quality_bps: 4_500,
            stop_policy,
        }
    }

    fn verdict(quality_bps: u16, verified: bool) -> AnytimeVerdict {
        AnytimeVerdict {
            quality_bps,
            confidence_bps: 8_000,
            constraint_coverage_bps: 8_000,
            evidence_count: 2,
            safety_violations: 0,
            deliverable: true,
            verified,
        }
    }

    #[test]
    fn direct_anchor_is_scheduled_before_speculative_work() {
        let mut controller = AnytimeController::new(config(ConductorStopPolicy::Quorum));
        controller
            .register(AnytimeCandidate::workflow("worker", Vec::new(), 8_000))
            .unwrap();
        controller
            .register(AnytimeCandidate::direct_anchor("anchor"))
            .unwrap();

        let ready = controller.ready_candidates();
        assert_eq!(ready[0].id, "anchor");
        assert_eq!(ready[1].id, "worker");
    }

    #[test]
    fn running_direct_anchor_does_not_consume_workflow_parallelism() {
        let mut controller = AnytimeController::new(config(ConductorStopPolicy::Quorum));
        controller
            .register(AnytimeCandidate::direct_anchor("anchor"))
            .unwrap();
        controller
            .register(AnytimeCandidate::workflow("worker_a", Vec::new(), 8_000))
            .unwrap();
        controller
            .register(AnytimeCandidate::workflow("worker_b", Vec::new(), 7_000))
            .unwrap();

        controller.mark_running("anchor").unwrap();
        let ready = controller.ready_candidates();

        assert_eq!(ready.len(), 2);
        assert!(ready.iter().any(|candidate| candidate.id == "worker_a"));
        assert!(ready.iter().any(|candidate| candidate.id == "worker_b"));
    }

    #[test]
    fn dependencies_unlock_only_after_a_usable_result() {
        let mut controller = AnytimeController::new(config(ConductorStopPolicy::Exhaustive));
        controller
            .register(AnytimeCandidate::workflow("evidence", Vec::new(), 7_000))
            .unwrap();
        controller
            .register(AnytimeCandidate::workflow(
                "synthesis",
                vec!["evidence".to_string()],
                8_000,
            ))
            .unwrap();
        assert_eq!(controller.ready_candidates()[0].id, "evidence");

        controller.mark_running("evidence").unwrap();
        controller
            .observe("evidence", verdict(7_000, false))
            .unwrap();
        assert_eq!(controller.ready_candidates()[0].id, "synthesis");
    }

    #[test]
    fn intermediate_result_unlocks_dependencies_without_becoming_committable() {
        let mut controller = AnytimeController::new(config(ConductorStopPolicy::FirstVerified));
        controller
            .register(AnytimeCandidate::workflow("evidence", Vec::new(), 7_000).as_intermediate())
            .unwrap();
        controller.mark_running("evidence").unwrap();
        controller
            .observe("evidence", verdict(9_000, true))
            .unwrap();

        assert!(controller.best().is_none());
        assert_eq!(
            controller.decision(60_000, 5_000),
            AnytimeDecision::Exhausted
        );
    }

    #[test]
    fn quorum_commits_the_best_verified_candidate_without_exhausting_frontier() {
        let mut controller = AnytimeController::new(config(ConductorStopPolicy::Quorum));
        for id in ["anchor", "candidate", "straggler"] {
            controller
                .register(AnytimeCandidate::workflow(id, Vec::new(), 6_000))
                .unwrap();
        }
        controller.mark_running("anchor").unwrap();
        controller.observe("anchor", verdict(7_000, true)).unwrap();
        assert_eq!(
            controller.decision(60_000, 5_000),
            AnytimeDecision::Continue
        );

        controller.mark_running("candidate").unwrap();
        controller
            .observe("candidate", verdict(8_000, true))
            .unwrap();
        assert_eq!(
            controller.decision(60_000, 5_000),
            AnytimeDecision::Commit {
                candidate_id: "candidate".to_string()
            }
        );
        assert_eq!(
            controller.candidate("straggler").unwrap().state,
            AnytimeCandidateState::Pending
        );
    }

    #[test]
    fn unsafe_candidate_cannot_replace_a_usable_anchor() {
        let mut controller = AnytimeController::new(config(ConductorStopPolicy::Exhaustive));
        controller
            .register(AnytimeCandidate::direct_anchor("anchor"))
            .unwrap();
        controller
            .register(AnytimeCandidate::workflow("unsafe", Vec::new(), 9_000))
            .unwrap();
        controller.mark_running("anchor").unwrap();
        controller.observe("anchor", verdict(6_000, false)).unwrap();
        controller.mark_running("unsafe").unwrap();
        let mut unsafe_verdict = verdict(10_000, true);
        unsafe_verdict.safety_violations = 1;
        assert!(!controller.observe("unsafe", unsafe_verdict).unwrap());
        assert_eq!(controller.best().unwrap().candidate_id, "anchor");
    }

    #[test]
    fn verifier_can_promote_a_usable_anchor_to_an_early_commit() {
        let mut controller = AnytimeController::new(config(ConductorStopPolicy::FirstVerified));
        controller
            .register(AnytimeCandidate::direct_anchor("anchor"))
            .unwrap();
        controller
            .register(AnytimeCandidate::workflow("workflow", Vec::new(), 8_000))
            .unwrap();
        controller.mark_running("anchor").unwrap();
        controller.observe("anchor", verdict(6_000, false)).unwrap();
        assert_eq!(
            controller.decision(60_000, 5_000),
            AnytimeDecision::Continue
        );

        controller.revise("anchor", verdict(7_500, true)).unwrap();

        assert_eq!(
            controller.candidate("anchor").unwrap().state,
            AnytimeCandidateState::Verified
        );
        assert_eq!(
            controller.decision(60_000, 5_000),
            AnytimeDecision::Commit {
                candidate_id: "anchor".to_string()
            }
        );
    }

    #[test]
    fn verifier_rejection_recomputes_best_instead_of_leaving_a_stale_winner() {
        let mut controller = AnytimeController::new(config(ConductorStopPolicy::Exhaustive));
        for id in ["anchor", "workflow"] {
            controller
                .register(AnytimeCandidate::workflow(id, Vec::new(), 7_000))
                .unwrap();
            controller.mark_running(id).unwrap();
        }
        controller
            .observe("workflow", verdict(6_500, false))
            .unwrap();
        controller.observe("anchor", verdict(8_000, false)).unwrap();
        assert_eq!(controller.best().unwrap().candidate_id, "anchor");

        let mut rejected = verdict(8_500, true);
        rejected.safety_violations = 1;
        controller.revise("anchor", rejected).unwrap();

        assert_eq!(
            controller.candidate("anchor").unwrap().state,
            AnytimeCandidateState::Failed
        );
        assert_eq!(controller.best().unwrap().candidate_id, "workflow");
        assert!(controller.verdict("anchor").is_none());
    }

    #[test]
    fn terminal_reserve_commits_best_known_result_for_exhaustive_policy() {
        let mut controller = AnytimeController::new(config(ConductorStopPolicy::Exhaustive));
        controller
            .register(AnytimeCandidate::direct_anchor("anchor"))
            .unwrap();
        controller.mark_running("anchor").unwrap();
        controller.observe("anchor", verdict(6_000, false)).unwrap();
        assert_eq!(
            controller.decision(4_999, 5_000),
            AnytimeDecision::Commit {
                candidate_id: "anchor".to_string()
            }
        );
    }

    #[test]
    fn snapshot_round_trip_preserves_frontier_and_best_candidate() {
        let contract = ConductorExecutionContract {
            task_class: TaskClass::General,
            effort: "auto".to_string(),
            policy: OrchestrationPolicy::AutoRouter,
            expected_uplift_bps: 6_000,
            confidence_bps: 7_000,
            max_parallelism: 2,
            min_successful_branches: 1,
            verification_required: true,
            terminal_model_call_reserve: 2,
            stop_policy: ConductorStopPolicy::Quorum,
            fallback_policy: ConductorFallbackPolicy::BestKnownResult,
        };
        let mut controller =
            AnytimeController::new(AnytimeControllerConfig::from_contract(&contract));
        controller
            .register(AnytimeCandidate::direct_anchor("anchor"))
            .unwrap();
        controller.mark_running("anchor").unwrap();
        controller.observe("anchor", verdict(7_500, true)).unwrap();

        let snapshot = controller.snapshot();
        let restored = AnytimeController::from_snapshot(snapshot).unwrap();
        assert_eq!(restored.best().unwrap().candidate_id, "anchor");
        assert_eq!(restored.verdict("anchor").unwrap().quality_bps, 7_500);
        assert_eq!(
            restored.candidate("anchor").unwrap().state,
            AnytimeCandidateState::Verified
        );
    }

    #[test]
    fn snapshot_restore_requeues_process_local_running_work() {
        let mut controller = AnytimeController::new(config(ConductorStopPolicy::Quorum));
        controller
            .register(AnytimeCandidate::direct_anchor("anchor"))
            .unwrap();
        controller.mark_running("anchor").unwrap();

        let restored = AnytimeController::from_snapshot(controller.snapshot()).unwrap();
        assert_eq!(
            restored.candidate("anchor").unwrap().state,
            AnytimeCandidateState::Pending
        );
        assert_eq!(restored.ready_candidates()[0].id, "anchor");
    }
}
