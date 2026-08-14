use agent_core::{Metadata, ModelRole};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

mod agent_engine;
mod agent_policy;
mod anytime;
mod candidate_selection;
mod causal_routing;
mod computation_value;
mod conductor_runtime;
mod direct_judge;
mod evaluation;
mod evolution_campaign;
mod execution_constraint;
mod execution_contract;
mod execution_plan;
mod owner_execution_graph;
mod policy;
mod prompt_distillation_gate;
mod prompt_evolution;
mod prompt_promotion_gate;
mod routing;
mod run_decision;
mod task_graph;
mod uplift_policy;
mod workflow_handoff;
mod workflow_revision;
mod workflow_runtime;
mod workflow_validation;

pub use agent_engine::*;
pub use agent_policy::*;
pub use anytime::*;
pub use candidate_selection::*;
pub use causal_routing::*;
pub use computation_value::*;
pub use conductor_runtime::*;
pub use direct_judge::*;
pub use evaluation::*;
pub use evolution_campaign::*;
pub use execution_contract::*;
pub use execution_plan::*;
pub use policy::*;
pub use prompt_distillation_gate::*;
pub use prompt_evolution::*;
pub use prompt_promotion_gate::*;
pub use routing::*;
pub use run_decision::*;
pub use task_graph::*;
pub use uplift_policy::*;
pub use workflow_handoff::*;
pub use workflow_revision::*;
pub use workflow_runtime::*;
pub use workflow_validation::*;

#[cfg(test)]
use routing::LEARNED_ROUTER_MIN_SUCCESS_CONFIDENCE;

pub const MAX_ADAPTIVE_WORKFLOW_STEPS: usize = 5;
pub const MAX_ADAPTIVE_WORKFLOW_AGENTS: usize = 3;
pub const WORKFLOW_IR_SCHEMA: &str = "cindx.workflow.v1";
pub const WORKFLOW_CHECKPOINT_SCHEMA: &str = "cindx.workflow.checkpoint.v1";
pub const WORKFLOW_VERIFICATION_RECEIPT_SCHEMA: &str = "cindx.workflow-verification.v1";
pub const CONDUCTOR_MAX_ATTEMPTS: usize = 2;
const ADAPTIVE_WORKER_SHARED_MEMORY_MAX_CHARS: usize = 24_000;
const ADAPTIVE_WORKER_DEPENDENCY_MAX_CHARS: usize = 16_000;
const ADAPTIVE_WORKER_AUTHORIZED_OUTPUTS_MAX_CHARS: usize = 32_000;

#[cfg(test)]
mod tests;
