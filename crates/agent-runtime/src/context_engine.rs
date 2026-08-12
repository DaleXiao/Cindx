use crate::execution::{
    AGENT_EVIDENCE_CONTEXT_SCHEMA, COLLABORATION_GUIDANCE_SCHEMA, WORKFLOW_EXECUTION_CONTEXT_SCHEMA,
};
use agent_core::{Message, MessageRole};

const CONTEXT_BASE_TOKENS: u64 = 512;
const MESSAGE_ENVELOPE_TOKENS: u64 = 6;
pub const CONTEXT_SOURCE_SCHEMA: &str = "cindx.context-source.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ContextSourceKind {
    ImageGenerationPolicy,
    CollaborationTrustPolicy,
    CognitiveState,
    WorkflowExecutionContract,
    GroundingEvidence,
    AgentEvidence,
    RestorePack,
    ArtifactManifest,
    WorkspaceKnowledge,
    ProjectMemory,
    ProjectInstructions,
    Skill,
    SingleModelPolicy,
    Collaboration,
    Other,
}

impl ContextSourceKind {
    pub fn from_message(message: &Message) -> Option<Self> {
        let reviewer_context = matches!(message.role, MessageRole::Reviewer)
            && message.metadata.get("internal").map(String::as_str) == Some("true");
        let reviewer_grounding = reviewer_context
            && message
                .metadata
                .get("required_grounding")
                .map(String::as_str)
                == Some("true")
            && (message
                .metadata
                .get("requirement_id")
                .is_some_and(|value| !value.trim().is_empty())
                || message
                    .metadata
                    .get("requirement_ids_json")
                    .is_some_and(|value| value != "[]"))
            && match message.metadata.get("kind").map(String::as_str) {
                Some("grounding_evidence_capsule") => {
                    message.metadata.get("evidence_schema").map(String::as_str)
                        == Some("cindx.grounding-evidence.v1")
                }
                Some("knowledge_context") => {
                    message
                        .metadata
                        .get("context_source_schema")
                        .map(String::as_str)
                        == Some(CONTEXT_SOURCE_SCHEMA)
                }
                Some("collaboration_tool_evidence") => {
                    message.metadata.get("evidence_schema").map(String::as_str)
                        == Some("cindx.collaboration-tool-evidence.v1")
                }
                _ => false,
            };
        if reviewer_grounding {
            return Some(Self::GroundingEvidence);
        }
        if reviewer_context
            && message.metadata.get("trust").map(String::as_str) == Some("untrusted_model_output")
        {
            match message.metadata.get("kind").map(String::as_str) {
                Some("collaboration_guidance")
                    if message.metadata.get("context_schema").map(String::as_str)
                        == Some(COLLABORATION_GUIDANCE_SCHEMA) =>
                {
                    return Some(Self::Collaboration);
                }
                Some("agent_evidence_packet")
                    if message.metadata.get("context_schema").map(String::as_str)
                        == Some(AGENT_EVIDENCE_CONTEXT_SCHEMA)
                        && message.metadata.get("evidence_schema").map(String::as_str)
                            == Some("cindx.agent-evidence.v1") =>
                {
                    return Some(Self::AgentEvidence);
                }
                Some("workflow_execution_contract")
                    if message.metadata.get("context_schema").map(String::as_str)
                        == Some(WORKFLOW_EXECUTION_CONTEXT_SCHEMA) =>
                {
                    return Some(Self::WorkflowExecutionContract);
                }
                _ => {}
            }
        }
        if reviewer_context
            && message.metadata.get("kind").map(String::as_str) == Some("knowledge_context")
            && message
                .metadata
                .get("context_source_schema")
                .map(String::as_str)
                == Some(CONTEXT_SOURCE_SCHEMA)
        {
            return Some(Self::WorkspaceKnowledge);
        }
        if reviewer_context
            && message.metadata.get("kind").map(String::as_str)
                == Some("collaboration_tool_evidence")
            && message.metadata.get("evidence_schema").map(String::as_str)
                == Some("cindx.collaboration-tool-evidence.v1")
        {
            return Some(Self::Collaboration);
        }
        if !matches!(message.role, MessageRole::System) {
            return None;
        }
        if message
            .metadata
            .get("required_grounding")
            .map(String::as_str)
            == Some("true")
        {
            return Some(Self::GroundingEvidence);
        }
        Some(match message.metadata.get("kind").map(String::as_str) {
            Some("image_generation_policy") => Self::ImageGenerationPolicy,
            Some("collaboration_trust_policy") => Self::CollaborationTrustPolicy,
            Some("cognitive_state")
                if message
                    .metadata
                    .get("cognitive_state_schema")
                    .map(String::as_str)
                    == Some("cindx.agent.cognitive-state.v1") =>
            {
                Self::CognitiveState
            }
            Some("workflow_execution_contract") => Self::WorkflowExecutionContract,
            Some("agent_evidence_packet") => Self::AgentEvidence,
            Some("context_restore_pack") => Self::RestorePack,
            Some("artifact_manifest") => Self::ArtifactManifest,
            Some("knowledge_context") => Self::WorkspaceKnowledge,
            Some("project_memory") => Self::ProjectMemory,
            Some("project_instructions") => Self::ProjectInstructions,
            Some("skill_context") => Self::Skill,
            Some("single_model_policy_guidance") => Self::SingleModelPolicy,
            _ if message.metadata.contains_key("collaboration_stage") => Self::Collaboration,
            _ => Self::Other,
        })
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ImageGenerationPolicy => "image_generation_policy",
            Self::CollaborationTrustPolicy => "collaboration_trust_policy",
            Self::CognitiveState => "cognitive_state",
            Self::WorkflowExecutionContract => "workflow_execution_contract",
            Self::GroundingEvidence => "grounding_evidence",
            Self::AgentEvidence => "agent_evidence_packet",
            Self::RestorePack => "context_restore_pack",
            Self::ArtifactManifest => "artifact_manifest",
            Self::WorkspaceKnowledge => "knowledge_context",
            Self::ProjectMemory => "project_memory",
            Self::ProjectInstructions => "project_instructions",
            Self::Skill => "skill_context",
            Self::SingleModelPolicy => "single_model_policy_guidance",
            Self::Collaboration => "collaboration",
            Self::Other => "other_system_context",
        }
    }

    pub(crate) const fn priority(self) -> u8 {
        match self {
            Self::ImageGenerationPolicy => 100,
            Self::CollaborationTrustPolicy => 100,
            Self::CognitiveState => 100,
            Self::WorkflowExecutionContract => 99,
            Self::GroundingEvidence => 98,
            Self::AgentEvidence => 98,
            Self::RestorePack => 95,
            Self::ArtifactManifest => 90,
            Self::WorkspaceKnowledge => 85,
            // This context was recalled specifically for the current request
            // and can contain durable user requirements. Keep it ahead of
            // bulk workspace evidence when the input budget is tight.
            Self::ProjectMemory => 96,
            // Bounded project instruction files re-enter every preparation as
            // durable guidance. Keep them protected, but rank them below the
            // current-recall and restore sources.
            Self::ProjectInstructions => 94,
            Self::Skill => 75,
            Self::SingleModelPolicy => 70,
            Self::Collaboration => 68,
            Self::Other => 50,
        }
    }

    pub(crate) const fn is_protected(self) -> bool {
        matches!(
            self,
            Self::ImageGenerationPolicy
                | Self::CollaborationTrustPolicy
                | Self::CognitiveState
                | Self::WorkflowExecutionContract
                | Self::GroundingEvidence
                | Self::AgentEvidence
                | Self::RestorePack
                | Self::ArtifactManifest
                | Self::ProjectMemory
                | Self::ProjectInstructions
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextCompactionPolicy {
    pub trigger_percent: u64,
    pub recent_target_percent: u64,
    pub recent_max_tokens: u64,
    pub recent_max_messages: usize,
    pub reuse_percent: u64,
    pub reuse_max_tokens: u64,
    pub reuse_max_messages: usize,
}

impl Default for ContextCompactionPolicy {
    fn default() -> Self {
        Self {
            trigger_percent: 65,
            recent_target_percent: 28,
            recent_max_tokens: 48_000,
            recent_max_messages: 32,
            reuse_percent: 42,
            reuse_max_tokens: 64_000,
            reuse_max_messages: 48,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextCompactionPlan {
    pub estimated_history_tokens: u64,
    pub estimated_request_tokens: u64,
    pub recent_budget_tokens: u64,
    pub recent_start: usize,
    pub recent_tokens: u64,
    pub should_compact: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextEngine {
    policy: ContextCompactionPolicy,
}

impl Default for ContextEngine {
    fn default() -> Self {
        Self::new(ContextCompactionPolicy::default())
    }
}

impl ContextEngine {
    pub const fn new(policy: ContextCompactionPolicy) -> Self {
        Self { policy }
    }

    pub const fn policy(&self) -> ContextCompactionPolicy {
        self.policy
    }

    pub fn compaction_plan(
        &self,
        history: &[Message],
        context_window_tokens: u64,
    ) -> ContextCompactionPlan {
        let context_window_tokens = context_window_tokens.max(1);
        let estimated_history_tokens = estimate_context_tokens(history);
        let estimated_request_tokens =
            estimated_history_tokens.saturating_add(context_prompt_reserve(context_window_tokens));
        let should_compact = estimated_request_tokens
            >= context_window_tokens.saturating_mul(self.policy.trigger_percent) / 100
            || history.len() > 80;
        let recent_floor = 8_000.min(context_window_tokens / 2).max(1);
        let recent_budget_tokens =
            (context_window_tokens.saturating_mul(self.policy.recent_target_percent) / 100)
                .min(self.policy.recent_max_tokens)
                .max(recent_floor);
        let (recent_start, recent_tokens) =
            self.recent_history_start(history, recent_budget_tokens);
        ContextCompactionPlan {
            estimated_history_tokens,
            estimated_request_tokens,
            recent_budget_tokens,
            recent_start,
            recent_tokens,
            should_compact,
        }
    }

    pub fn checkpoint_is_reusable(
        &self,
        covered_messages: usize,
        history: &[Message],
        plan: ContextCompactionPlan,
        context_window_tokens: u64,
    ) -> bool {
        if covered_messages > history.len() {
            return false;
        }
        let retained = &history[covered_messages..];
        if retained.len() > self.policy.reuse_max_messages {
            return false;
        }
        let retained_tokens = retained.iter().map(estimate_message_tokens).sum::<u64>();
        let reuse_budget = (context_window_tokens
            .max(1)
            .saturating_mul(self.policy.reuse_percent)
            / 100)
            .min(self.policy.reuse_max_tokens)
            .max(plan.recent_budget_tokens);
        retained_tokens <= reuse_budget
    }

    pub fn recent_history_start(&self, history: &[Message], token_budget: u64) -> (usize, u64) {
        if history.is_empty() {
            return (0, 0);
        }
        let mut start = history.len();
        let mut selected = 0usize;
        let mut tokens = 0_u64;
        while start > 0 && selected < self.policy.recent_max_messages {
            let message_tokens = estimate_message_tokens(&history[start - 1]);
            if selected > 0 && tokens.saturating_add(message_tokens) > token_budget {
                break;
            }
            start -= 1;
            selected += 1;
            tokens = tokens.saturating_add(message_tokens);
        }
        if start > 0 && !is_user_turn_start(&history[start]) {
            if let Some(offset) = history[start..].iter().position(is_user_turn_start) {
                start += offset;
            } else if let Some(previous_turn) =
                history[..start].iter().rposition(is_user_turn_start)
            {
                start = previous_turn;
            }
        }
        if start == history.len() {
            start = history.len() - 1;
        }
        let tokens = history[start..].iter().map(estimate_message_tokens).sum();
        (start, tokens)
    }
}

pub fn estimate_text_tokens(value: &str) -> u64 {
    if value.is_ascii() {
        return (value.len() as u64)
            .saturating_add(2)
            .checked_div(3)
            .unwrap_or_default()
            .saturating_add(u64::from(!value.is_empty()));
    }

    let mut ascii = 0_u64;
    let mut non_ascii = 0_u64;
    for character in value.chars() {
        if character.is_ascii() {
            ascii += 1;
        } else {
            non_ascii += 1;
        }
    }
    ascii
        .saturating_add(2)
        .checked_div(3)
        .unwrap_or_default()
        .saturating_add(non_ascii)
        .saturating_add(u64::from(!value.is_empty()))
}

pub fn estimate_message_tokens(message: &Message) -> u64 {
    estimate_message_tokens_with_overhead(message, MESSAGE_ENVELOPE_TOKENS)
}

pub(crate) fn estimate_model_message_tokens(message: &Message) -> u64 {
    estimate_message_tokens_with_overhead(message, MESSAGE_ENVELOPE_TOKENS + 2)
}

fn estimate_message_tokens_with_overhead(message: &Message, overhead: u64) -> u64 {
    let tool_call_tokens = message
        .metadata
        .get("raw_tool_calls_json")
        .map(|value| estimate_text_tokens(value))
        .unwrap_or_default();
    let image_tokens = message
        .metadata
        .get("image_paths")
        .map(|paths| paths.lines().filter(|path| !path.trim().is_empty()).count() as u64 * 1_024)
        .unwrap_or_default();
    estimate_text_tokens(&message.content)
        .saturating_add(tool_call_tokens)
        .saturating_add(image_tokens)
        .saturating_add(overhead)
}

pub fn estimate_context_tokens(messages: &[Message]) -> u64 {
    if messages.is_empty() {
        return 0;
    }
    CONTEXT_BASE_TOKENS.saturating_add(messages.iter().map(estimate_message_tokens).sum::<u64>())
}

pub fn context_prompt_reserve(context_window_tokens: u64) -> u64 {
    let context_window_tokens = context_window_tokens.max(1);
    (context_window_tokens / 8)
        .clamp(2_048, 16_384)
        .min(context_window_tokens / 4)
}

pub fn is_user_turn_start(message: &Message) -> bool {
    matches!(message.role, MessageRole::User)
        && message.metadata.get("kind").map(String::as_str) != Some("tool_observation")
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::Metadata;

    fn message(role: MessageRole, content: &str) -> Message {
        Message {
            role,
            content: content.to_string(),
            metadata: Metadata::new(),
        }
    }

    #[test]
    fn recent_history_starts_on_complete_user_turn() {
        let history = vec![
            message(MessageRole::User, "older request"),
            message(MessageRole::Assistant, "older response"),
            message(MessageRole::User, "current request"),
            message(MessageRole::Assistant, "working"),
            message(MessageRole::Tool, "result"),
        ];
        let budget = estimate_message_tokens(&history[3]) + estimate_message_tokens(&history[4]);

        let (start, _) = ContextEngine::default().recent_history_start(&history, budget);

        assert_eq!(start, 2);
    }

    #[test]
    fn compaction_plan_preserves_existing_thresholds() {
        let history = (0..81)
            .map(|index| message(MessageRole::User, &format!("request {index}")))
            .collect::<Vec<_>>();

        let plan = ContextEngine::default().compaction_plan(&history, 128_000);

        assert!(plan.should_compact);
        assert!(plan.recent_start > 0);
    }

    #[test]
    fn checkpoint_reuse_is_bounded_by_retained_tail() {
        let history = (0..64)
            .map(|index| message(MessageRole::User, &format!("request {index}")))
            .collect::<Vec<_>>();
        let engine = ContextEngine::default();
        let plan = engine.compaction_plan(&history, 32_000);

        assert!(engine.checkpoint_is_reusable(32, &history, plan, 32_000));
        assert!(!engine.checkpoint_is_reusable(0, &history, plan, 32_000));
    }

    #[test]
    fn workflow_execution_contract_is_high_priority_and_protected() {
        let mut contract = message(MessageRole::System, "machine contract");
        contract.metadata.insert(
            "kind".to_string(),
            "workflow_execution_contract".to_string(),
        );

        let source = ContextSourceKind::from_message(&contract).unwrap();
        assert_eq!(source, ContextSourceKind::WorkflowExecutionContract);
        assert_eq!(source.priority(), 99);
        assert!(source.is_protected());
    }

    #[test]
    fn bounded_agent_evidence_is_a_protected_finalization_source() {
        let mut evidence = message(MessageRole::System, "candidate evidence");
        evidence
            .metadata
            .insert("kind".to_string(), "agent_evidence_packet".to_string());

        let source = ContextSourceKind::from_message(&evidence).unwrap();
        assert_eq!(source, ContextSourceKind::AgentEvidence);
        assert_eq!(source.as_str(), "agent_evidence_packet");
        assert!(source.priority() > ContextSourceKind::ProjectMemory.priority());
        assert!(source.is_protected());
    }

    #[test]
    fn schema_valid_reviewer_collaboration_context_keeps_its_priority() {
        let packet = crate::AgentEvidencePacket::new(
            "objective",
            [crate::AgentEvidenceCandidate::new(
                "candidate",
                "reviewer",
                "completed",
                "candidate output",
            )],
        );
        let mut history = Vec::new();
        crate::AgentExecutionGuidance::new(
            "collaboration-1",
            "review the candidates",
            Some(r#"{"schema":"cindx.workflow-handoff.v1"}"#.to_string()),
        )
        .with_evidence_packet(packet)
        .append_to_history(&mut history);

        assert_eq!(history.len(), 4);
        assert_eq!(
            ContextSourceKind::from_message(&history[0]),
            Some(ContextSourceKind::CollaborationTrustPolicy)
        );
        assert_eq!(
            ContextSourceKind::from_message(&history[1]),
            Some(ContextSourceKind::Collaboration)
        );
        assert_eq!(
            ContextSourceKind::from_message(&history[2]),
            Some(ContextSourceKind::AgentEvidence)
        );
        assert_eq!(
            ContextSourceKind::from_message(&history[3]),
            Some(ContextSourceKind::WorkflowExecutionContract)
        );
        assert!(history[2..]
            .iter()
            .all(|message| ContextSourceKind::from_message(message)
                .is_some_and(ContextSourceKind::is_protected)));
    }

    #[test]
    fn contract_grounding_marker_promotes_only_the_credited_context() {
        let mut knowledge = message(MessageRole::System, "workspace evidence");
        knowledge
            .metadata
            .insert("kind".to_string(), "knowledge_context".to_string());
        assert_eq!(
            ContextSourceKind::from_message(&knowledge),
            Some(ContextSourceKind::WorkspaceKnowledge)
        );

        knowledge
            .metadata
            .insert("required_grounding".to_string(), "true".to_string());
        let source = ContextSourceKind::from_message(&knowledge).unwrap();
        assert_eq!(source, ContextSourceKind::GroundingEvidence);
        assert_eq!(source.as_str(), "grounding_evidence");
        assert!(source.is_protected());
    }

    #[test]
    fn project_instructions_are_protected_below_recalled_memory() {
        let mut instructions = message(MessageRole::System, "workspace guidance");
        instructions
            .metadata
            .insert("kind".to_string(), "project_instructions".to_string());

        let source = ContextSourceKind::from_message(&instructions).unwrap();
        assert_eq!(source, ContextSourceKind::ProjectInstructions);
        assert_eq!(source.as_str(), "project_instructions");
        assert!(source.is_protected());
        assert!(source.priority() < ContextSourceKind::ProjectMemory.priority());
        assert!(source.priority() > ContextSourceKind::WorkspaceKnowledge.priority());
    }

    #[test]
    fn recalled_project_memory_is_protected_above_workspace_knowledge() {
        let mut memory = message(MessageRole::System, "durable user requirement");
        memory
            .metadata
            .insert("kind".to_string(), "project_memory".to_string());
        let mut knowledge = message(MessageRole::System, "workspace evidence");
        knowledge
            .metadata
            .insert("kind".to_string(), "knowledge_context".to_string());

        let memory_source = ContextSourceKind::from_message(&memory).unwrap();
        let knowledge_source = ContextSourceKind::from_message(&knowledge).unwrap();
        assert!(memory_source.is_protected());
        assert!(memory_source.priority() > knowledge_source.priority());
    }
}
