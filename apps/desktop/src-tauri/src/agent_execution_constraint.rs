//! Execution-constraint plumbing: every shipping run is `Native`. The matched
//! and grounded-direct evaluation arms were retired with the realworld-eval
//! harness; only the metadata round-trip survives.
use agent_core::Metadata;

pub(crate) const AGENT_EXECUTION_CONSTRAINT_KEY: &str = "execution_constraint";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum AgentExecutionConstraint {
    #[default]
    Native,
}

impl AgentExecutionConstraint {
    pub(crate) fn write_to_context(self, run_context: &mut Metadata) {
        run_context.remove(AGENT_EXECUTION_CONSTRAINT_KEY);
    }
}
