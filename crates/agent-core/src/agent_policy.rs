use crate::OrchestrationPolicy;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPolicy {
    Fast,
    #[default]
    Auto,
    Pro,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentModelSelectionKind {
    DefaultConfigured,
    RoutedExecutor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptEvolutionStrategy {
    Disabled,
    NativePromotion,
    AutoTransferPromotion,
}

impl AgentPolicy {
    pub fn generation_temperature(self) -> Option<&'static str> {
        match self {
            Self::Pro => None,
            Self::Fast | Self::Auto => Some("0"),
        }
    }

    pub fn parse_ingress(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "fast" => Self::Fast,
            "pro" => Self::Pro,
            _ => Self::Auto,
        }
    }

    pub fn parse_persisted(value: &str) -> Option<Self> {
        match value {
            "fast" => Some(Self::Fast),
            "auto" => Some(Self::Auto),
            "pro" => Some(Self::Pro),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Auto => "auto",
            Self::Pro => "pro",
        }
    }

    pub fn requested_policy(self) -> OrchestrationPolicy {
        match self {
            Self::Fast => OrchestrationPolicy::Single,
            Self::Auto | Self::Pro => OrchestrationPolicy::AutoRouter,
        }
    }

    pub const fn uses_conductor(self) -> bool {
        !matches!(self, Self::Fast)
    }

    pub const fn max_parallelism(self) -> usize {
        match self {
            Self::Fast => 1,
            Self::Auto => 2,
            Self::Pro => 3,
        }
    }

    pub const fn model_selection(self) -> AgentModelSelectionKind {
        match self {
            Self::Fast => AgentModelSelectionKind::DefaultConfigured,
            Self::Auto | Self::Pro => AgentModelSelectionKind::RoutedExecutor,
        }
    }

    pub const fn uses_default_model(self) -> bool {
        matches!(
            self.model_selection(),
            AgentModelSelectionKind::DefaultConfigured
        )
    }

    pub const fn prompt_evolution(self) -> PromptEvolutionStrategy {
        match self {
            Self::Fast => PromptEvolutionStrategy::Disabled,
            Self::Auto => PromptEvolutionStrategy::NativePromotion,
            Self::Pro => PromptEvolutionStrategy::AutoTransferPromotion,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingress_is_forgiving_and_defaults_to_auto() {
        assert_eq!(AgentPolicy::parse_ingress(" FAST "), AgentPolicy::Fast);
        assert_eq!(AgentPolicy::parse_ingress("Pro"), AgentPolicy::Pro);
        assert_eq!(AgentPolicy::parse_ingress(""), AgentPolicy::Auto);
        assert_eq!(AgentPolicy::parse_ingress("future"), AgentPolicy::Auto);
        assert_eq!(AgentPolicy::default(), AgentPolicy::Auto);
    }

    #[test]
    fn persisted_policy_is_strict() {
        assert_eq!(
            AgentPolicy::parse_persisted("fast"),
            Some(AgentPolicy::Fast)
        );
        assert_eq!(
            AgentPolicy::parse_persisted("auto"),
            Some(AgentPolicy::Auto)
        );
        assert_eq!(AgentPolicy::parse_persisted("pro"), Some(AgentPolicy::Pro));
        assert_eq!(AgentPolicy::parse_persisted("AUTO"), None);
        assert_eq!(AgentPolicy::parse_persisted(" auto "), None);
        assert_eq!(AgentPolicy::parse_persisted("future"), None);
    }

    #[test]
    fn policy_matrix_is_coherent() {
        let matrix = [
            (
                AgentPolicy::Fast,
                OrchestrationPolicy::Single,
                false,
                1,
                AgentModelSelectionKind::DefaultConfigured,
                PromptEvolutionStrategy::Disabled,
            ),
            (
                AgentPolicy::Auto,
                OrchestrationPolicy::AutoRouter,
                true,
                2,
                AgentModelSelectionKind::RoutedExecutor,
                PromptEvolutionStrategy::NativePromotion,
            ),
            (
                AgentPolicy::Pro,
                OrchestrationPolicy::AutoRouter,
                true,
                3,
                AgentModelSelectionKind::RoutedExecutor,
                PromptEvolutionStrategy::AutoTransferPromotion,
            ),
        ];

        for (policy, requested, conductor, parallelism, model, evolution) in matrix {
            assert_eq!(policy.requested_policy(), requested);
            assert_eq!(policy.uses_conductor(), conductor);
            assert_eq!(policy.max_parallelism(), parallelism);
            assert_eq!(policy.model_selection(), model);
            assert_eq!(policy.uses_default_model(), policy == AgentPolicy::Fast);
            assert_eq!(policy.prompt_evolution(), evolution);
            assert_eq!(AgentPolicy::parse_persisted(policy.label()), Some(policy));
            assert_eq!(policy.uses_conductor(), parallelism > 1);
        }
    }

    #[test]
    fn serde_uses_stable_strict_wire_labels() {
        for (policy, label) in [
            (AgentPolicy::Fast, "fast"),
            (AgentPolicy::Auto, "auto"),
            (AgentPolicy::Pro, "pro"),
        ] {
            assert_eq!(
                serde_json::to_string(&policy).unwrap(),
                format!("\"{label}\"")
            );
            assert_eq!(
                serde_json::from_str::<AgentPolicy>(&format!("\"{label}\"")).unwrap(),
                policy
            );
        }
        assert!(serde_json::from_str::<AgentPolicy>("\"future\"").is_err());
    }

    #[test]
    fn generation_temperature_pins_deterministic_sampling_below_pro() {
        assert_eq!(AgentPolicy::Fast.generation_temperature(), Some("0"));
        assert_eq!(AgentPolicy::Auto.generation_temperature(), Some("0"));
        assert_eq!(AgentPolicy::Pro.generation_temperature(), None);
    }
}
