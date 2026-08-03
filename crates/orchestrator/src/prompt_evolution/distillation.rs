use super::{
    prompt_genome_sha256, ConductorPromptGenome, FrozenPromptProfileSnapshot,
    ProTeacherAttestationV1, PromptCommitStrategy, PromptContextPolicy, PromptGraphDepth,
    PromptRetryPolicy, PromptRoleStrategy, PromptToolPolicy, PromptTopologyStrategy,
    PromptVerification,
};
use crate::sha256_hex;

pub const PRO_TO_AUTO_DISTILLATION_CHILD_PROTOCOL_V1: &str =
    "pro-to-auto-bounded-strategy-child-v1";
pub const AUTO_DISTILLATION_MAX_PARALLEL_BRANCHES: usize = 2;
pub const AUTO_DISTILLATION_MAX_STEP_ATTEMPTS: usize = 2;
pub const AUTO_DISTILLATION_MAX_MODEL_TURNS_PER_STEP: usize = 2;
pub const AUTO_DISTILLATION_MAX_TOOL_CALLS_PER_STEP: usize = 4;
const MAX_TRANSFERRED_GENES: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransferableGene {
    RetryPolicy(PromptRetryPolicy),
    TopologyStrategy(PromptTopologyStrategy),
    RoleStrategy(PromptRoleStrategy),
    CommitStrategy(PromptCommitStrategy),
    Verification(PromptVerification),
    GraphDepth(PromptGraphDepth),
    ContextPolicy(PromptContextPolicy),
    MaxParallelBranches(usize),
    MaxStepAttempts(usize),
    MaxModelTurnsPerStep(usize),
    ToolPolicy(PromptToolPolicy),
    MaxToolCallsPerStep(usize),
}

impl TransferableGene {
    fn name(self) -> &'static str {
        match self {
            Self::RetryPolicy(_) => "retry_policy",
            Self::TopologyStrategy(_) => "topology_strategy",
            Self::RoleStrategy(_) => "role_strategy",
            Self::CommitStrategy(_) => "commit_strategy",
            Self::Verification(_) => "verification",
            Self::GraphDepth(_) => "graph_depth",
            Self::ContextPolicy(_) => "context_policy",
            Self::MaxParallelBranches(_) => "max_parallel_branches",
            Self::MaxStepAttempts(_) => "max_step_attempts",
            Self::MaxModelTurnsPerStep(_) => "max_model_turns_per_step",
            Self::ToolPolicy(_) => "tool_policy",
            Self::MaxToolCallsPerStep(_) => "max_tool_calls_per_step",
        }
    }

    fn apply(self, child: &mut ConductorPromptGenome) {
        match self {
            Self::RetryPolicy(value) => child.retry_policy = value,
            Self::TopologyStrategy(value) => child.topology_strategy = value,
            Self::RoleStrategy(value) => child.role_strategy = value,
            Self::CommitStrategy(value) => child.commit_strategy = value,
            Self::Verification(value) => child.verification = value,
            Self::GraphDepth(value) => child.graph_depth = value,
            Self::ContextPolicy(value) => child.context_policy = value,
            Self::MaxParallelBranches(value) => child.max_parallel_branches = value,
            Self::MaxStepAttempts(value) => child.max_step_attempts = value,
            Self::MaxModelTurnsPerStep(value) => child.max_model_turns_per_step = value,
            Self::ToolPolicy(value) => child.tool_policy = value,
            Self::MaxToolCallsPerStep(value) => child.max_tool_calls_per_step = value,
        }
    }
}

pub fn validate_auto_distillation_contract(genome: &ConductorPromptGenome) -> Result<(), String> {
    genome.validate()?;
    if !genome.require_final_synthesis {
        return Err("Auto distillation requires final synthesis".to_string());
    }
    if genome.verification < PromptVerification::Evidence {
        return Err("Auto distillation requires evidence verification".to_string());
    }
    if genome.max_parallel_branches > AUTO_DISTILLATION_MAX_PARALLEL_BRANCHES
        || genome.max_step_attempts > AUTO_DISTILLATION_MAX_STEP_ATTEMPTS
        || genome.effective_max_model_turns_per_step() > AUTO_DISTILLATION_MAX_MODEL_TURNS_PER_STEP
        || genome.effective_max_tool_calls_per_step() > AUTO_DISTILLATION_MAX_TOOL_CALLS_PER_STEP
    {
        return Err("Auto distillation resource contract exceeded".to_string());
    }
    if genome.tool_policy == PromptToolPolicy::ReadOnlyExploration {
        return Err("Auto distillation cannot inherit Pro exploration tools".to_string());
    }
    Ok(())
}

pub fn derive_pro_to_auto_distillation_child(
    auto_parent: &ConductorPromptGenome,
    pro_teacher_snapshot: &FrozenPromptProfileSnapshot,
    active_stable_pro_profile_id: &str,
) -> Result<ConductorPromptGenome, String> {
    validate_auto_distillation_contract(auto_parent)?;
    let attestation = ProTeacherAttestationV1::from_stable_snapshot(
        pro_teacher_snapshot,
        active_stable_pro_profile_id,
    )?;
    let teacher = &pro_teacher_snapshot.genome;
    if prompt_genome_sha256(teacher)? != attestation.teacher_profile_sha256 {
        return Err("certified Pro teacher genome fingerprint does not match".to_string());
    }
    let auto_parent_sha256 = prompt_genome_sha256(auto_parent)?;
    if auto_parent.id == teacher.id || auto_parent_sha256 == attestation.teacher_profile_sha256 {
        return Err("Pro-to-Auto distillation requires distinct parent and teacher".to_string());
    }

    let pro_seed = ConductorPromptGenome::seed_for_effort("pro");
    let transferable = transferable_teacher_changes(auto_parent, teacher, &pro_seed);
    if transferable.is_empty() {
        return Err(
            "certified Pro teacher has no learned Auto-compatible strategy change".to_string(),
        );
    }

    let selected = transferable
        .into_iter()
        .take(MAX_TRANSFERRED_GENES)
        .collect::<Vec<_>>();
    let mut child = auto_parent.clone();
    for gene in &selected {
        gene.apply(&mut child);
    }
    child.generation = auto_parent
        .generation
        .max(teacher.generation)
        .saturating_add(1);
    child.parents = vec![auto_parent.id.clone(), teacher.id.clone()];
    child.require_final_synthesis = true;

    let teacher_attestation_sha256 = attestation.digest()?;
    let transferred_genes = selected.iter().map(|gene| gene.name()).collect::<Vec<_>>();
    let lineage = serde_json::to_vec(&(
        PRO_TO_AUTO_DISTILLATION_CHILD_PROTOCOL_V1,
        auto_parent_sha256,
        teacher_attestation_sha256,
        &transferred_genes,
    ))
    .map_err(|error| format!("distillation lineage serialization failed: {error}"))?;
    let lineage_sha256 = sha256_hex(&lineage);
    child.id = format!(
        "auto-distilled-g{}-{}",
        child.generation,
        &lineage_sha256[..16]
    );

    validate_auto_distillation_contract(&child)?;
    let changed_genes = child.changed_genes_from_auto_parent(auto_parent);
    if !(1..=MAX_TRANSFERRED_GENES).contains(&changed_genes) {
        return Err(format!(
            "Auto distillation must change one or two genes, changed {changed_genes}"
        ));
    }
    if child.parents != [auto_parent.id.clone(), attestation.teacher_profile_id] {
        return Err("Auto distillation lineage does not match parent and teacher".to_string());
    }
    Ok(child)
}

fn transferable_teacher_changes(
    auto_parent: &ConductorPromptGenome,
    teacher: &ConductorPromptGenome,
    pro_seed: &ConductorPromptGenome,
) -> Vec<TransferableGene> {
    let mut changes = Vec::new();
    if teacher.retry_policy != pro_seed.retry_policy
        && teacher.retry_policy != auto_parent.retry_policy
    {
        changes.push(TransferableGene::RetryPolicy(teacher.retry_policy));
    }
    let topology = teacher
        .topology_strategy
        .min(PromptTopologyStrategy::AdaptiveDag);
    if teacher.topology_strategy != pro_seed.topology_strategy
        && topology != auto_parent.topology_strategy
    {
        changes.push(TransferableGene::TopologyStrategy(topology));
    }
    if teacher.role_strategy != pro_seed.role_strategy
        && teacher.role_strategy != auto_parent.role_strategy
    {
        changes.push(TransferableGene::RoleStrategy(teacher.role_strategy));
    }
    if teacher.commit_strategy != pro_seed.commit_strategy
        && teacher.commit_strategy != auto_parent.commit_strategy
    {
        changes.push(TransferableGene::CommitStrategy(teacher.commit_strategy));
    }
    if teacher.verification != pro_seed.verification
        && teacher.verification >= PromptVerification::Evidence
        && teacher.verification != auto_parent.verification
    {
        changes.push(TransferableGene::Verification(teacher.verification));
    }
    if teacher.graph_depth != pro_seed.graph_depth && teacher.graph_depth != auto_parent.graph_depth
    {
        changes.push(TransferableGene::GraphDepth(teacher.graph_depth));
    }
    if teacher.context_policy != pro_seed.context_policy
        && teacher.context_policy != auto_parent.context_policy
    {
        changes.push(TransferableGene::ContextPolicy(teacher.context_policy));
    }

    let branches = teacher
        .max_parallel_branches
        .min(AUTO_DISTILLATION_MAX_PARALLEL_BRANCHES)
        .min(auto_parent.max_parallel_branches);
    if teacher.max_parallel_branches != pro_seed.max_parallel_branches
        && branches != auto_parent.max_parallel_branches
    {
        changes.push(TransferableGene::MaxParallelBranches(branches));
    }
    let attempts = teacher
        .max_step_attempts
        .min(AUTO_DISTILLATION_MAX_STEP_ATTEMPTS)
        .min(auto_parent.max_step_attempts);
    if teacher.max_step_attempts != pro_seed.max_step_attempts
        && attempts != auto_parent.max_step_attempts
    {
        changes.push(TransferableGene::MaxStepAttempts(attempts));
    }
    let turns = teacher
        .effective_max_model_turns_per_step()
        .min(AUTO_DISTILLATION_MAX_MODEL_TURNS_PER_STEP)
        .min(auto_parent.effective_max_model_turns_per_step());
    if teacher.effective_max_model_turns_per_step() != pro_seed.effective_max_model_turns_per_step()
        && turns != auto_parent.effective_max_model_turns_per_step()
    {
        changes.push(TransferableGene::MaxModelTurnsPerStep(turns));
    }
    let tool_policy = teacher.tool_policy.min(PromptToolPolicy::EvidenceOnly);
    if teacher.tool_policy != pro_seed.tool_policy && tool_policy != auto_parent.tool_policy {
        changes.push(TransferableGene::ToolPolicy(tool_policy));
    }
    let tool_calls = teacher
        .effective_max_tool_calls_per_step()
        .min(AUTO_DISTILLATION_MAX_TOOL_CALLS_PER_STEP)
        .min(auto_parent.effective_max_tool_calls_per_step());
    if teacher.effective_max_tool_calls_per_step() != pro_seed.effective_max_tool_calls_per_step()
        && tool_calls != auto_parent.effective_max_tool_calls_per_step()
    {
        changes.push(TransferableGene::MaxToolCallsPerStep(tool_calls));
    }
    changes
}

trait AutoParentDifference {
    fn changed_genes_from_auto_parent(&self, parent: &Self) -> usize;
}

impl AutoParentDifference for ConductorPromptGenome {
    fn changed_genes_from_auto_parent(&self, parent: &Self) -> usize {
        usize::from(self.graph_depth != parent.graph_depth)
            + usize::from(self.verification != parent.verification)
            + usize::from(self.context_policy != parent.context_policy)
            + usize::from(self.max_parallel_branches != parent.max_parallel_branches)
            + usize::from(self.tool_policy != parent.tool_policy)
            + usize::from(self.retry_policy != parent.retry_policy)
            + usize::from(self.topology_strategy != parent.topology_strategy)
            + usize::from(self.role_strategy != parent.role_strategy)
            + usize::from(self.commit_strategy != parent.commit_strategy)
            + usize::from(self.max_step_attempts != parent.max_step_attempts)
            + usize::from(
                self.effective_max_model_turns_per_step()
                    != parent.effective_max_model_turns_per_step(),
            )
            + usize::from(
                self.effective_max_tool_calls_per_step()
                    != parent.effective_max_tool_calls_per_step(),
            )
            + usize::from(self.custom_directive.trim() != parent.custom_directive.trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FrozenPromptTransferEvidence;

    fn certified_pro_snapshot(mut teacher: ConductorPromptGenome) -> FrozenPromptProfileSnapshot {
        teacher.generation = 2;
        teacher.parents = vec!["pro-parent".to_string()];
        let snapshot = FrozenPromptProfileSnapshot::new_gepa(
            "pro",
            teacher,
            "pro-parent",
            "a".repeat(64),
            "b".repeat(64),
        )
        .expect("Pro snapshot should validate");
        snapshot
            .with_auto_teacher_evidence(FrozenPromptTransferEvidence {
                source_effort: "auto".to_string(),
                source_profile_id: "auto-source".to_string(),
                source_profile_sha256: "c".repeat(64),
                dataset_sha256: "d".repeat(64),
                cohort_sha256: Some("e".repeat(64)),
                paired_evidence_sha256: "f".repeat(64),
                promotion_gate_protocol: super::super::PROMPT_AUTO_TRANSFER_GATE_PROTOCOL
                    .to_string(),
                source_profile_lineage: Some(
                    crate::FrozenPromptSourceProfileLineageV1::undistilled("c".repeat(64)).unwrap(),
                ),
            })
            .expect("dual-gate Pro snapshot should validate")
    }

    #[test]
    fn distillation_child_is_deterministic_bounded_and_not_a_pro_clone() {
        let auto_parent = ConductorPromptGenome::seed_for_effort("auto");
        let mut teacher = ConductorPromptGenome::seed_for_effort("pro");
        teacher.retry_policy = PromptRetryPolicy::SameModel;
        teacher.commit_strategy = PromptCommitStrategy::Quorum;
        teacher.max_parallel_branches = 3;
        teacher.max_step_attempts = 4;
        teacher.max_model_turns_per_step = 4;
        teacher.max_tool_calls_per_step = 8;
        teacher.custom_directive = "teacher-only text must not cross modes".to_string();
        let snapshot = certified_pro_snapshot(teacher.clone());

        let first =
            derive_pro_to_auto_distillation_child(&auto_parent, &snapshot, &snapshot.genome.id)
                .expect("certified strategy should distill");
        let second =
            derive_pro_to_auto_distillation_child(&auto_parent, &snapshot, &snapshot.genome.id)
                .expect("same lineage should be deterministic");

        assert_eq!(first, second);
        assert_eq!(first.retry_policy, PromptRetryPolicy::SameModel);
        assert_eq!(first.commit_strategy, PromptCommitStrategy::Quorum);
        assert_eq!(first.max_parallel_branches, 2);
        assert_eq!(first.max_step_attempts, 2);
        assert_eq!(first.effective_max_model_turns_per_step(), 2);
        assert_eq!(first.effective_max_tool_calls_per_step(), 4);
        assert_eq!(first.custom_directive, auto_parent.custom_directive);
        assert_eq!(
            first.parents,
            vec![auto_parent.id.clone(), snapshot.genome.id.clone()]
        );
        assert_ne!(first, teacher);
        validate_auto_distillation_contract(&first).unwrap();
    }

    #[test]
    fn teacher_defaults_and_raw_directive_are_not_presented_as_learned_strategy() {
        let auto_parent = ConductorPromptGenome::seed_for_effort("auto");
        let mut teacher = ConductorPromptGenome::seed_for_effort("pro");
        teacher.custom_directive = "do not copy this".to_string();
        let snapshot = certified_pro_snapshot(teacher);

        let error =
            derive_pro_to_auto_distillation_child(&auto_parent, &snapshot, &snapshot.genome.id)
                .expect_err("raw Pro defaults are not distilled learning");

        assert!(error.contains("no learned Auto-compatible strategy"));
    }

    #[test]
    fn pro_parallel_deliberation_is_projected_to_the_auto_topology_ceiling() {
        let auto_parent = ConductorPromptGenome::seed_for_effort("auto");
        let mut teacher = ConductorPromptGenome::seed_for_effort("pro");
        teacher.topology_strategy = PromptTopologyStrategy::ParallelDeliberation;
        let snapshot = certified_pro_snapshot(teacher);

        let error =
            derive_pro_to_auto_distillation_child(&auto_parent, &snapshot, &snapshot.genome.id)
                .expect_err("Pro parallel topology must not cross into Auto");

        assert!(error.contains("no learned Auto-compatible strategy"));
    }

    #[test]
    fn uncertified_or_non_active_pro_teacher_is_rejected() {
        let auto_parent = ConductorPromptGenome::seed_for_effort("auto");
        let mut teacher = ConductorPromptGenome::seed_for_effort("pro");
        teacher.retry_policy = PromptRetryPolicy::SameModel;
        let snapshot = certified_pro_snapshot(teacher);

        let error =
            derive_pro_to_auto_distillation_child(&auto_parent, &snapshot, "different-active-pro")
                .expect_err("stale Pro profile must be rejected");

        assert!(error.contains("active stable frozen Pro profile"));
    }

    #[test]
    fn invalid_auto_parent_contract_is_rejected_before_distillation() {
        let mut auto_parent = ConductorPromptGenome::seed_for_effort("auto");
        auto_parent.max_parallel_branches = 3;
        let mut teacher = ConductorPromptGenome::seed_for_effort("pro");
        teacher.retry_policy = PromptRetryPolicy::SameModel;
        let snapshot = certified_pro_snapshot(teacher);

        let error =
            derive_pro_to_auto_distillation_child(&auto_parent, &snapshot, &snapshot.genome.id)
                .expect_err("out-of-contract Auto parent must not bootstrap a child");

        assert!(error.contains("resource contract exceeded"));
    }
}
