use crate::Metadata;
use std::fmt;

pub const AGENT_MODEL_ATTRIBUTION_SCHEMA: &str = "cindx.agent-model-attribution.v1";
pub const AGENT_MODEL_ATTRIBUTION_SCHEMA_METADATA_KEY: &str = "agent_model_attribution_schema";
pub const AGENT_ACTOR_METADATA_KEY: &str = "agent_actor";
pub const AGENT_SERVICE_METADATA_KEY: &str = "agent_service";
pub const AGENT_STAGE_METADATA_KEY: &str = "agent_stage";
pub const AGENT_MODEL_PROFILE_METADATA_KEY: &str = "agent_model_profile";
pub const AGENT_OUTPUT_TRUST_METADATA_KEY: &str = "agent_output_trust";
pub const AGENT_EFFECT_AUTHORITY_METADATA_KEY: &str = "agent_effect_authority";
pub const AGENT_ATTRIBUTION_COMPONENT_METADATA_KEY: &str = "agent_attribution_component";
pub const AGENT_ATTRIBUTION_MODEL_METADATA_KEY: &str = "agent_attribution_model";
pub const AGENT_ATTRIBUTION_LEGACY_ROLE_METADATA_KEY: &str = "agent_attribution_legacy_role";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentActor {
    Owner,
    Specialist,
    IndependentVerifier,
}

impl AgentActor {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Specialist => "specialist",
            Self::IndependentVerifier => "independent_verifier",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentService {
    Conductor,
    LearningUtility,
}

impl AgentService {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Conductor => "conductor",
            Self::LearningUtility => "learning_utility",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStage {
    Plan,
    Evidence,
    Act,
    Verify,
    Finalize,
}

impl AgentStage {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Evidence => "evidence",
            Self::Act => "act",
            Self::Verify => "verify",
            Self::Finalize => "finalize",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentModelProfile {
    Primary,
    Reasoning,
    Verifier,
    Utility,
}

impl AgentModelProfile {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Reasoning => "reasoning",
            Self::Verifier => "verifier",
            Self::Utility => "utility",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentOutputTrust {
    UntrustedModelOutput,
}

impl AgentOutputTrust {
    pub const fn label(self) -> &'static str {
        match self {
            Self::UntrustedModelOutput => "untrusted_model_output",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentEffectAuthority {
    None,
    ReadOnly,
    PermissionGated,
}

impl AgentEffectAuthority {
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::ReadOnly => "read_only",
            Self::PermissionGated => "permission_gated",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentModelAttribution {
    actor: Option<AgentActor>,
    service: Option<AgentService>,
    stage: AgentStage,
    profile: AgentModelProfile,
    output_trust: AgentOutputTrust,
    effect_authority: AgentEffectAuthority,
}

impl AgentModelAttribution {
    pub const fn actor(
        actor: AgentActor,
        stage: AgentStage,
        profile: AgentModelProfile,
        effect_authority: AgentEffectAuthority,
    ) -> Self {
        Self {
            actor: Some(actor),
            service: None,
            stage,
            profile,
            output_trust: AgentOutputTrust::UntrustedModelOutput,
            effect_authority,
        }
    }

    pub const fn service(
        service: AgentService,
        stage: AgentStage,
        profile: AgentModelProfile,
    ) -> Self {
        Self {
            actor: None,
            service: Some(service),
            stage,
            profile,
            output_trust: AgentOutputTrust::UntrustedModelOutput,
            effect_authority: AgentEffectAuthority::None,
        }
    }

    pub fn insert_into(
        self,
        metadata: &mut Metadata,
        model: &str,
        legacy_role: &str,
        component: &str,
    ) -> Result<(), AgentModelAttributionError> {
        let fields = [
            (
                AGENT_MODEL_ATTRIBUTION_SCHEMA_METADATA_KEY,
                AGENT_MODEL_ATTRIBUTION_SCHEMA,
            ),
            (
                AGENT_ACTOR_METADATA_KEY,
                self.actor.map(AgentActor::label).unwrap_or("none"),
            ),
            (
                AGENT_SERVICE_METADATA_KEY,
                self.service.map(AgentService::label).unwrap_or("none"),
            ),
            (AGENT_STAGE_METADATA_KEY, self.stage.label()),
            (AGENT_MODEL_PROFILE_METADATA_KEY, self.profile.label()),
            (AGENT_OUTPUT_TRUST_METADATA_KEY, self.output_trust.label()),
            (
                AGENT_EFFECT_AUTHORITY_METADATA_KEY,
                self.effect_authority.label(),
            ),
            (AGENT_ATTRIBUTION_COMPONENT_METADATA_KEY, component),
            (AGENT_ATTRIBUTION_MODEL_METADATA_KEY, model),
            (AGENT_ATTRIBUTION_LEGACY_ROLE_METADATA_KEY, legacy_role),
        ];
        for (key, value) in fields {
            match metadata.get(key) {
                Some(existing) if existing != value => {
                    return Err(AgentModelAttributionError::ReservedMetadataConflict(key));
                }
                _ => {}
            }
        }
        for (key, value) in fields {
            metadata.insert(key.to_string(), value.to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentModelAttributionError {
    ReservedMetadataConflict(&'static str),
}

impl fmt::Display for AgentModelAttributionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReservedMetadataConflict(key) => {
                write!(
                    formatter,
                    "reserved model attribution metadata conflicts at `{key}`"
                )
            }
        }
    }
}

impl std::error::Error for AgentModelAttributionError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_attribution_preserves_legacy_labels() {
        let mut metadata = [
            ("role".to_string(), "reviewer".to_string()),
            ("stage".to_string(), "quality_gate".to_string()),
        ]
        .into_iter()
        .collect();

        AgentModelAttribution::actor(
            AgentActor::IndependentVerifier,
            AgentStage::Verify,
            AgentModelProfile::Verifier,
            AgentEffectAuthority::None,
        )
        .insert_into(&mut metadata, "review-model", "reviewer", "quality_gate")
        .unwrap();

        assert_eq!(metadata.get("role").map(String::as_str), Some("reviewer"));
        assert_eq!(
            metadata.get("stage").map(String::as_str),
            Some("quality_gate")
        );
        assert_eq!(
            metadata.get(AGENT_ACTOR_METADATA_KEY).map(String::as_str),
            Some("independent_verifier")
        );
        assert_eq!(
            metadata.get(AGENT_STAGE_METADATA_KEY).map(String::as_str),
            Some("verify")
        );
    }

    #[test]
    fn conductor_is_a_service_not_an_actor() {
        let mut metadata = Metadata::new();
        AgentModelAttribution::service(
            AgentService::Conductor,
            AgentStage::Plan,
            AgentModelProfile::Reasoning,
        )
        .insert_into(
            &mut metadata,
            "reasoning-model",
            "planner",
            "route_selection",
        )
        .unwrap();

        assert_eq!(
            metadata.get(AGENT_ACTOR_METADATA_KEY).map(String::as_str),
            Some("none")
        );
        assert_eq!(
            metadata.get(AGENT_SERVICE_METADATA_KEY).map(String::as_str),
            Some("conductor")
        );
    }
}
