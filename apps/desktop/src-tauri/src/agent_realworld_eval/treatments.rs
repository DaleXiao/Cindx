use serde::Serialize;

pub(super) const LEGACY_SUITE_SCHEMA: &str = "cindx.agent-realworld-suite.v3";
pub(super) const SUITE_SCHEMA: &str = "cindx.agent-realworld-suite.v4";
pub(super) const LEGACY_RAW_SCHEMA: &str = "cindx.agent-realworld-raw.v3";
pub(super) const RAW_SCHEMA: &str = "cindx.agent-realworld-raw.v4";

const LEGACY_TREATMENTS: [&str; 4] = ["direct", "fast", "auto", "pro"];
const CURRENT_TREATMENTS: [&str; 4] = ["oracle_reference", "grounded_direct", "auto", "pro"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(super) enum Treatment {
    #[serde(rename = "direct")]
    LegacyDirect,
    #[serde(rename = "oracle_reference")]
    OracleReference,
    #[serde(rename = "grounded_direct")]
    GroundedDirect,
    #[serde(rename = "fast")]
    Fast,
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "pro")]
    Pro,
}

impl Treatment {
    pub(super) fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "direct" => Ok(Self::LegacyDirect),
            "oracle_reference" => Ok(Self::OracleReference),
            "grounded_direct" => Ok(Self::GroundedDirect),
            "fast" => Ok(Self::Fast),
            "auto" => Ok(Self::Auto),
            "pro" => Ok(Self::Pro),
            other => Err(format!("unsupported treatment {other}")),
        }
    }

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::LegacyDirect => "direct",
            Self::OracleReference => "oracle_reference",
            Self::GroundedDirect => "grounded_direct",
            Self::Fast => "fast",
            Self::Auto => "auto",
            Self::Pro => "pro",
        }
    }

    pub(super) const fn product_effort(self) -> Option<&'static str> {
        match self {
            Self::LegacyDirect | Self::OracleReference => None,
            Self::Fast => Some("fast"),
            Self::GroundedDirect | Self::Auto => Some("auto"),
            Self::Pro => Some("pro"),
        }
    }

    pub(super) const fn profile_effort(self) -> Option<&'static str> {
        match self {
            Self::Auto => Some("auto"),
            Self::Pro => Some("pro"),
            _ => None,
        }
    }

    pub(super) const fn is_oracle_reference(self) -> bool {
        matches!(self, Self::LegacyDirect | Self::OracleReference)
    }

    pub(super) const fn is_grounded_direct(self) -> bool {
        matches!(self, Self::GroundedDirect)
    }
}

pub(super) fn expected_treatments(schema: &str) -> Option<&'static [&'static str]> {
    match schema {
        LEGACY_SUITE_SCHEMA => Some(&LEGACY_TREATMENTS),
        SUITE_SCHEMA => Some(&CURRENT_TREATMENTS),
        _ => None,
    }
}

pub(super) fn raw_schema(schema: &str) -> Option<&'static str> {
    match schema {
        LEGACY_SUITE_SCHEMA => Some(LEGACY_RAW_SCHEMA),
        SUITE_SCHEMA => Some(RAW_SCHEMA),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_grounded_direct_is_an_auto_budget_product_treatment() {
        assert_eq!(Treatment::GroundedDirect.product_effort(), Some("auto"));
        assert!(Treatment::GroundedDirect.is_grounded_direct());
        assert!(!Treatment::GroundedDirect.is_oracle_reference());
        assert_eq!(
            expected_treatments(SUITE_SCHEMA),
            Some(CURRENT_TREATMENTS.as_slice())
        );
    }

    #[test]
    fn legacy_labels_remain_parseable_without_entering_the_v5_protocol() {
        assert_eq!(Treatment::parse("direct").unwrap(), Treatment::LegacyDirect);
        assert_eq!(Treatment::parse("fast").unwrap(), Treatment::Fast);
        assert_eq!(raw_schema(LEGACY_SUITE_SCHEMA), Some(LEGACY_RAW_SCHEMA));
    }
}
