//! Execution-constraint plumbing: every shipping run is `Native`. The
//! feature-gated `product-eval` driver (never part of a shipping build) uses
//! `EvalNoDelegation` to build the matched single-agent arm of the frozen
//! Phase 4 protocol: the `task` delegation surface is filtered from the
//! planned tool list and the deferred index, and the delegation router denies
//! instead of dispatching. The marker rides in the run context, so it is
//! recorded on durable events and survives a resume exactly like the effort
//! and identity facts do.
use agent_core::Metadata;

pub(crate) const AGENT_EXECUTION_CONSTRAINT_KEY: &str = "execution_constraint";
/// Run-context marker disabling subagent delegation for a matched eval arm.
pub(crate) const EVAL_DELEGATION_DISABLED_KEY: &str = "cindx.eval.delegation_disabled";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum AgentExecutionConstraint {
    #[default]
    Native,
    /// A matched evaluation arm whose delegation surface must be absent.
    /// Only the feature-gated product-path eval driver may set it; shipping
    /// entries hardcode `Native`.
    // Constructed only by the `product-eval` driver, so it is dead code in
    // every shipping and test build by design.
    #[allow(dead_code)]
    EvalNoDelegation,
}

impl AgentExecutionConstraint {
    pub(crate) fn write_to_context(self, run_context: &mut Metadata) {
        match self {
            Self::Native => {
                run_context.remove(AGENT_EXECUTION_CONSTRAINT_KEY);
                run_context.remove(EVAL_DELEGATION_DISABLED_KEY);
            }
            Self::EvalNoDelegation => {
                run_context.insert(
                    AGENT_EXECUTION_CONSTRAINT_KEY.to_string(),
                    "eval_no_delegation".to_string(),
                );
                run_context.insert(EVAL_DELEGATION_DISABLED_KEY.to_string(), "true".to_string());
            }
        }
    }
}

/// True only when the run explicitly carries the eval no-delegation marker.
pub(crate) fn eval_delegation_disabled(run_context: &Metadata) -> bool {
    run_context
        .get(EVAL_DELEGATION_DISABLED_KEY)
        .map(String::as_str)
        == Some("true")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_runs_carry_no_eval_markers() {
        let mut run_context = Metadata::new();
        run_context.insert(EVAL_DELEGATION_DISABLED_KEY.to_string(), "true".to_string());
        run_context.insert(
            AGENT_EXECUTION_CONSTRAINT_KEY.to_string(),
            "stale".to_string(),
        );

        AgentExecutionConstraint::Native.write_to_context(&mut run_context);

        assert!(!eval_delegation_disabled(&run_context));
        assert!(!run_context.contains_key(AGENT_EXECUTION_CONSTRAINT_KEY));
    }

    #[test]
    fn the_eval_arm_marker_round_trips_through_the_run_context() {
        let mut run_context = Metadata::new();
        assert!(!eval_delegation_disabled(&run_context));

        AgentExecutionConstraint::EvalNoDelegation.write_to_context(&mut run_context);

        assert!(eval_delegation_disabled(&run_context));
        assert_eq!(
            run_context
                .get(AGENT_EXECUTION_CONSTRAINT_KEY)
                .map(String::as_str),
            Some("eval_no_delegation")
        );
    }
}
