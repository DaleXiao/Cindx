//! The effort-tier run planner now lives in the portable `agent-application`
//! crate so the evaluation harness can drive the same deterministic scheduling
//! authority as the shipping product (Phase 4 product-path fidelity; the first
//! bounded step of the audit P2-03 desktop-composition extraction). This module
//! is a thin re-export shim so every desktop call site keeps its
//! `crate::agent_effort_planner::*` path unchanged; the planner logic and its
//! tests moved verbatim, with no behavior change.
pub(crate) use agent_application::{
    apply_effort_plan_keys, plan_effort_run, EffortRunPlan, KnowledgeDecision,
};
