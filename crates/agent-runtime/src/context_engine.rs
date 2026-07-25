use agent_core::{Message, MessageRole};

const CONTEXT_BASE_TOKENS: u64 = 512;
const MESSAGE_ENVELOPE_TOKENS: u64 = 6;
pub const CONTEXT_SOURCE_SCHEMA: &str = "cindx.context-source.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ContextSourceKind {
    ImageGenerationPolicy,
    WorkflowExecutionContract,
    RestorePack,
    ArtifactManifest,
    WorkspaceKnowledge,
    ProjectMemory,
    Skill,
    SingleModelPolicy,
    Collaboration,
    Other,
}

impl ContextSourceKind {
    pub fn from_message(message: &Message) -> Option<Self> {
        if !matches!(message.role, MessageRole::System) {
            return None;
        }
        Some(match message.metadata.get("kind").map(String::as_str) {
            Some("image_generation_policy") => Self::ImageGenerationPolicy,
            Some("workflow_execution_contract") => Self::WorkflowExecutionContract,
            Some("context_restore_pack") => Self::RestorePack,
            Some("artifact_manifest") => Self::ArtifactManifest,
            Some("knowledge_context") => Self::WorkspaceKnowledge,
            Some("project_memory") => Self::ProjectMemory,
            Some("skill_context") => Self::Skill,
            Some("single_model_policy_guidance") => Self::SingleModelPolicy,
            _ if message.metadata.contains_key("collaboration_stage") => Self::Collaboration,
            _ => Self::Other,
        })
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ImageGenerationPolicy => "image_generation_policy",
            Self::WorkflowExecutionContract => "workflow_execution_contract",
            Self::RestorePack => "context_restore_pack",
            Self::ArtifactManifest => "artifact_manifest",
            Self::WorkspaceKnowledge => "knowledge_context",
            Self::ProjectMemory => "project_memory",
            Self::Skill => "skill_context",
            Self::SingleModelPolicy => "single_model_policy_guidance",
            Self::Collaboration => "collaboration",
            Self::Other => "other_system_context",
        }
    }

    pub(crate) const fn priority(self) -> u8 {
        match self {
            Self::ImageGenerationPolicy => 100,
            Self::WorkflowExecutionContract => 99,
            Self::RestorePack => 95,
            Self::ArtifactManifest => 90,
            Self::WorkspaceKnowledge => 85,
            Self::ProjectMemory => 80,
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
                | Self::WorkflowExecutionContract
                | Self::RestorePack
                | Self::ArtifactManifest
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
}
