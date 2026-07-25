use crate::RunStopReason;
use model_provider::{ModelError, ProviderFailureClass};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentFailureClass {
    Cancelled,
    Budget,
    Contract,
    ModelOutput,
    ProviderTransient,
    ProviderPermanent,
    Tool,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRecoveryAction {
    Stop,
    RetrySame,
    RetryAlternate,
}

impl AgentFailureClass {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::Budget => "budget",
            Self::Contract => "contract",
            Self::ModelOutput => "model_output",
            Self::ProviderTransient => "provider_transient",
            Self::ProviderPermanent => "provider_permanent",
            Self::Tool => "tool",
            Self::Internal => "internal",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFailure {
    pub code: String,
    pub message: String,
    pub class: AgentFailureClass,
    pub retryable: bool,
}

impl AgentFailure {
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        class: AgentFailureClass,
        retryable: bool,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            class,
            retryable,
        }
    }

    pub fn model_output(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(code, message, AgentFailureClass::ModelOutput, false)
    }

    pub fn contract(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(code, message, AgentFailureClass::Contract, false)
    }

    pub fn budget(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(code, message, AgentFailureClass::Budget, false)
    }

    pub fn cancelled(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(code, message, AgentFailureClass::Cancelled, false)
    }

    pub fn tool(code: impl Into<String>, message: impl Into<String>, retryable: bool) -> Self {
        Self::new(code, message, AgentFailureClass::Tool, retryable)
    }

    pub fn internal(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(code, message, AgentFailureClass::Internal, false)
    }

    pub fn from_stop_reason(reason: RunStopReason, message: impl Into<String>) -> Self {
        let message = message.into();
        match reason {
            RunStopReason::UserCancelled => Self::cancelled(reason.code(), message),
            RunStopReason::ProviderUnavailable => Self::new(
                reason.code(),
                message,
                AgentFailureClass::ProviderTransient,
                true,
            ),
            RunStopReason::RepeatedAction | RunStopReason::NoProgress => {
                Self::contract(reason.code(), message)
            }
            RunStopReason::DeadlineExceeded
            | RunStopReason::ModelCallBudgetExceeded
            | RunStopReason::ToolCallBudgetExceeded
            | RunStopReason::TurnBudgetExhausted
            | RunStopReason::StageBudgetExhausted
            | RunStopReason::RepairBudgetExhausted => Self::budget(reason.code(), message),
        }
    }

    pub fn from_model_error(error: &ModelError) -> Self {
        let class = match error.class {
            ProviderFailureClass::Cancelled => AgentFailureClass::Cancelled,
            ProviderFailureClass::Timeout
            | ProviderFailureClass::RateLimited
            | ProviderFailureClass::Unavailable
            | ProviderFailureClass::Transport => AgentFailureClass::ProviderTransient,
            ProviderFailureClass::Authentication
            | ProviderFailureClass::InvalidRequest
            | ProviderFailureClass::ResponseTooLarge
            | ProviderFailureClass::MalformedResponse => AgentFailureClass::ProviderPermanent,
            ProviderFailureClass::Unknown if error.retryable => {
                AgentFailureClass::ProviderTransient
            }
            ProviderFailureClass::Unknown => AgentFailureClass::ProviderPermanent,
        };
        Self::new(
            format!("provider_{}", error.class.label()),
            error.message.clone(),
            class,
            error.retryable,
        )
    }

    pub fn should_retry(&self) -> bool {
        self.retryable
            && matches!(
                self.class,
                AgentFailureClass::ProviderTransient | AgentFailureClass::Tool
            )
    }

    pub fn recovery_action(&self, alternate_available: bool) -> AgentRecoveryAction {
        match self.class {
            AgentFailureClass::ProviderTransient if alternate_available => {
                AgentRecoveryAction::RetryAlternate
            }
            AgentFailureClass::ProviderTransient => AgentRecoveryAction::RetrySame,
            AgentFailureClass::ModelOutput if alternate_available => {
                AgentRecoveryAction::RetryAlternate
            }
            AgentFailureClass::Tool if self.retryable => AgentRecoveryAction::RetrySame,
            AgentFailureClass::Cancelled
            | AgentFailureClass::Budget
            | AgentFailureClass::Contract
            | AgentFailureClass::ModelOutput
            | AgentFailureClass::ProviderPermanent
            | AgentFailureClass::Tool
            | AgentFailureClass::Internal => AgentRecoveryAction::Stop,
        }
    }
}

impl fmt::Display for AgentFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AgentFailure {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_retry_policy_is_typed_and_narrow() {
        let timeout = AgentFailure::from_model_error(&ModelError::new("request timed out"));
        assert_eq!(timeout.class, AgentFailureClass::ProviderTransient);
        assert!(timeout.should_retry());

        let invalid = AgentFailure::from_model_error(&ModelError::with_status(
            400,
            "unexpected item type in content",
        ));
        assert_eq!(invalid.class, AgentFailureClass::ProviderPermanent);
        assert!(!invalid.should_retry());
    }

    #[test]
    fn deterministic_runtime_failures_never_retry() {
        assert!(!AgentFailure::budget("turn_budget", "exhausted").should_retry());
        assert!(!AgentFailure::contract("missing_evidence", "verify").should_retry());
        assert!(!AgentFailure::model_output("empty", "empty response").should_retry());
    }

    #[test]
    fn recovery_never_spends_more_budget_on_deterministic_failures() {
        assert_eq!(
            AgentFailure::budget("turn_budget", "exhausted").recovery_action(true),
            AgentRecoveryAction::Stop
        );
        assert_eq!(
            AgentFailure::contract("missing_evidence", "verify").recovery_action(true),
            AgentRecoveryAction::Stop
        );
        assert_eq!(
            AgentFailure::model_output("empty", "empty response").recovery_action(true),
            AgentRecoveryAction::RetryAlternate
        );
        assert_eq!(
            AgentFailure::model_output("empty", "empty response").recovery_action(false),
            AgentRecoveryAction::Stop
        );
    }

    #[test]
    fn run_stop_reasons_preserve_cancellation_and_budget_semantics() {
        assert_eq!(
            AgentFailure::from_stop_reason(RunStopReason::UserCancelled, "cancelled").class,
            AgentFailureClass::Cancelled
        );
        assert_eq!(
            AgentFailure::from_stop_reason(RunStopReason::StageBudgetExhausted, "spent").class,
            AgentFailureClass::Budget
        );
        assert_eq!(
            AgentFailure::from_stop_reason(RunStopReason::RepeatedAction, "looping").class,
            AgentFailureClass::Contract
        );
    }
}
