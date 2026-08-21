use agent_core::{Metadata, ModelRole};
use std::collections::BTreeMap;

mod candidate_selection;
mod causal_routing;
mod computation_value;
mod evaluation;
mod execution_constraint;
mod execution_contract;
mod execution_plan;
mod routing;
mod run_decision;

pub use agent_core::run_decision_enums::AgentEffectAuthority;
pub use agent_core::{
    default_plan, direct_judge_eligible, direct_judge_model, direct_judge_prompt,
    direct_judge_repair_directive, parse_policy, role_label, sha256_hex, step_prompt,
    AgentExecutionMode, AgentModelSelectionKind, AgentPolicy, AgentToolRequirement,
    AgentVerificationPolicy, DirectJudgeReceipt, DirectJudgeVerdict, IndependentQualitySource,
    LearningAttribution, LearningDisposition, LearningEvidenceSchema, LearningEvidenceV1,
    LearningTermination, LearningUsageCompleteness, LearningVerification, ModelCandidate,
    ModelCapabilitySource, OrchestrationPlan, OrchestrationPolicy, OrchestrationStep,
    PromptEvolutionStrategy, RoutingOutcome, RoutingTelemetry, TaskClass,
    DIRECT_JUDGE_MAX_REPAIR_ROUNDS, DIRECT_JUDGE_RECEIPT_SCHEMA, LEARNING_EVIDENCE_MAX_BYTES,
    LEARNING_EVIDENCE_METADATA_KEY, LEARNING_EVIDENCE_SCHEMA_V1,
};
pub use candidate_selection::*;
pub use causal_routing::*;
pub use computation_value::*;
pub use evaluation::*;
pub use execution_contract::*;
pub use execution_plan::*;
pub use routing::*;
pub use run_decision::*;

pub const CONDUCTOR_MAX_ATTEMPTS: usize = 2;

#[cfg(test)]
mod tests;
