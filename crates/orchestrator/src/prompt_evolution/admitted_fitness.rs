//! The admitted direct-judge fitness types live in `agent-core`; this module
//! keeps the re-export so existing `orchestrator::` paths keep compiling.
pub use agent_core::{
    admitted_direct_judge_fitness_into_prompt_fitness, AdmittedDirectJudgeFitness,
    ADMITTED_DIRECT_JUDGE_TASK_CLASS,
};
