//! Portable deterministic evaluation skeleton for general agent runs.
//!
//! This crate never contacts a provider: the model side is injected through
//! [`EvalModelProvider`], and the bundled [`ScriptedProvider`] replays a fixed
//! queue of assistant messages and tool calls so suites are replayable
//! byte-for-byte. Cases run in an isolated per-case workspace, execute only
//! the case's `allowed_tools` through the portable `tools` registry, and are
//! judged by pure postcondition checks. The crate has no production consumer.

mod case;
mod matched;
mod postcondition;
mod report;
mod runner;

pub use case::{parse_suite, CaseBudget, CaseCategory, EvalCase, FixtureFile, Postcondition};
pub use matched::{run_matched_arms, ArmRun, MatchedArmReport, MatchedCaseReport};
pub use postcondition::{check_postconditions, CheckResult, CHECK_OUTPUT_LIMIT};
pub use report::SuiteReport;
pub use runner::{
    run_case, CaseReceipts, CaseReport, EvalError, EvalModelProvider, ScriptedProvider,
    ScriptedStep, ScriptedToolCall,
};

// Phase 4 product-path fidelity: the harness shares the product's deterministic
// effort-tier scheduling authority (extracted from the desktop crate into
// agent-application) so an effort-tier arm plans identically to the shipping
// product instead of reinventing budget/knowledge defaults.
pub use agent_application::{
    knowledge_decision_for_effort, plan_effort_run, EffortRunPlan, KnowledgeDecision,
};
