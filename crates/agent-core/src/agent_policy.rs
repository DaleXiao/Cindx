use crate::OrchestrationPolicy;
use serde::{Deserialize, Serialize};

/// The run's reasoning level: how much budget and verification a run gets.
/// The wire labels are `fast`, `default`, `high`, and `xhigh`; the legacy
/// `auto` and `pro` labels migrate to `default` and `high`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPolicy {
    Fast,
    #[default]
    Default,
    High,
    Xhigh,
}

impl AgentPolicy {
    pub fn generation_temperature(self) -> Option<&'static str> {
        match self {
            Self::Xhigh | Self::High => None,
            Self::Fast | Self::Default => Some("0"),
        }
    }

    /// Forgiving parse for ingress (user input, CLI). Legacy labels migrate.
    pub fn parse_ingress(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "fast" => Self::Fast,
            "high" => Self::High,
            "xhigh" | "x-high" | "x_high" => Self::Xhigh,
            "pro" => Self::High,
            "auto" => Self::Default,
            _ => Self::Default,
        }
    }

    /// Strict parse for persisted values; legacy labels migrate to the new
    /// reasoning levels.
    pub fn parse_persisted(value: &str) -> Option<Self> {
        match value {
            "fast" => Some(Self::Fast),
            "default" | "auto" => Some(Self::Default),
            "high" | "pro" => Some(Self::High),
            "xhigh" => Some(Self::Xhigh),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Default => "default",
            Self::High => "high",
            Self::Xhigh => "xhigh",
        }
    }

    pub fn requested_policy(self) -> OrchestrationPolicy {
        match self {
            Self::Fast => OrchestrationPolicy::Single,
            Self::Default | Self::High | Self::Xhigh => OrchestrationPolicy::AutoRouter,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingress_is_forgiving_and_defaults_to_default() {
        assert_eq!(AgentPolicy::parse_ingress(" FAST "), AgentPolicy::Fast);
        assert_eq!(AgentPolicy::parse_ingress("Xhigh"), AgentPolicy::Xhigh);
        assert_eq!(AgentPolicy::parse_ingress("pro"), AgentPolicy::High);
        assert_eq!(AgentPolicy::parse_ingress("auto"), AgentPolicy::Default);
        assert_eq!(AgentPolicy::parse_ingress(""), AgentPolicy::Default);
        assert_eq!(AgentPolicy::parse_ingress("future"), AgentPolicy::Default);
        assert_eq!(AgentPolicy::default(), AgentPolicy::Default);
    }

    #[test]
    fn persisted_policy_migrates_legacy_labels() {
        assert_eq!(
            AgentPolicy::parse_persisted("fast"),
            Some(AgentPolicy::Fast)
        );
        assert_eq!(
            AgentPolicy::parse_persisted("default"),
            Some(AgentPolicy::Default)
        );
        assert_eq!(
            AgentPolicy::parse_persisted("auto"),
            Some(AgentPolicy::Default)
        );
        assert_eq!(
            AgentPolicy::parse_persisted("high"),
            Some(AgentPolicy::High)
        );
        assert_eq!(AgentPolicy::parse_persisted("pro"), Some(AgentPolicy::High));
        assert_eq!(
            AgentPolicy::parse_persisted("xhigh"),
            Some(AgentPolicy::Xhigh)
        );
        assert_eq!(AgentPolicy::parse_persisted("DEFAULT"), None);
        assert_eq!(AgentPolicy::parse_persisted(" high "), None);
        assert_eq!(AgentPolicy::parse_persisted("future"), None);
    }

    #[test]
    fn serde_uses_stable_strict_wire_labels() {
        for (policy, label) in [
            (AgentPolicy::Fast, "fast"),
            (AgentPolicy::Default, "default"),
            (AgentPolicy::High, "high"),
            (AgentPolicy::Xhigh, "xhigh"),
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
    fn generation_temperature_pins_deterministic_sampling_below_high() {
        assert_eq!(AgentPolicy::Fast.generation_temperature(), Some("0"));
        assert_eq!(AgentPolicy::Default.generation_temperature(), Some("0"));
        assert_eq!(AgentPolicy::High.generation_temperature(), None);
        assert_eq!(AgentPolicy::Xhigh.generation_temperature(), None);
    }
}
