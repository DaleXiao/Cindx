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
    fn requested_policy_follows_the_effort_tier() {
        assert_eq!(
            AgentPolicy::Fast.requested_policy(),
            OrchestrationPolicy::Single
        );
        assert_eq!(
            AgentPolicy::Auto.requested_policy(),
            OrchestrationPolicy::AutoRouter
        );
        assert_eq!(
            AgentPolicy::Pro.requested_policy(),
            OrchestrationPolicy::AutoRouter
        );
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
