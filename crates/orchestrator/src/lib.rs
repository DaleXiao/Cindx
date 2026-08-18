use agent_core::{Metadata, ModelRole};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

mod agent_policy;
mod candidate_selection;
mod causal_routing;
mod computation_value;
mod direct_judge;
mod evaluation;
mod evolution_campaign;
mod execution_constraint;
mod execution_contract;
mod execution_plan;
mod policy;
mod prompt_distillation_gate;
mod prompt_evolution;
mod prompt_promotion_gate;
mod routing;
mod run_decision;

pub use agent_policy::*;
pub use candidate_selection::*;
pub use causal_routing::*;
pub use computation_value::*;
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

pub const CONDUCTOR_MAX_ATTEMPTS: usize = 2;

#[cfg(test)]
mod tests;
