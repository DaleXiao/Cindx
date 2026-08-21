use crate::ModelRole;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskClass {
    General,
    Coding,
    Research,
    Retrieval,
    Browser,
    Computer,
}

impl TaskClass {
    pub fn label(&self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Coding => "coding",
            Self::Research => "research",
            Self::Retrieval => "retrieval",
            Self::Browser => "browser",
            Self::Computer => "computer",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCandidate {
    pub name: String,
    pub role: ModelRole,
    pub supports_tools: bool,
    pub supports_vision: bool,
    pub tools_capability_source: ModelCapabilitySource,
    pub vision_capability_source: ModelCapabilitySource,
    pub cost_tier: u8,
    pub latency_tier: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelCapabilitySource {
    ProviderCatalog,
    Configured,
    CompatibilityAssumption,
}

impl ModelCapabilitySource {
    pub const fn label(self) -> &'static str {
        match self {
            Self::ProviderCatalog => "provider_catalog",
            Self::Configured => "configured",
            Self::CompatibilityAssumption => "compatibility_assumption",
        }
    }
}
