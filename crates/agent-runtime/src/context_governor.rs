use crate::context_engine::{
    estimate_model_message_tokens, estimate_text_tokens, ContextSourceKind, CONTEXT_SOURCE_SCHEMA,
};
use crate::context_projection::{
    build_omitted_context_digest, fit_message_to_budget, fit_message_to_budget_with_estimate,
    CONTEXT_GOVERNOR_SCHEMA,
};
use agent_core::{Message, MessageRole, Metadata, ToolSpec};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextBudgetAllocation {
    pub core_tokens: u64,
    pub current_request_tokens: u64,
    pub protected_context_tokens: u64,
    pub supplemental_context_tokens: u64,
    pub recent_conversation_tokens: u64,
    pub archive_digest_tokens: u64,
    pub unused_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextGovernorReport {
    pub applied: bool,
    pub context_window_tokens: u64,
    pub input_budget_tokens: u64,
    pub estimated_original_tokens: u64,
    pub estimated_projected_tokens: u64,
    pub original_messages: usize,
    pub projected_messages: usize,
    pub omitted_messages: usize,
    pub truncated_messages: usize,
    pub hard_limit_satisfied: bool,
    pub selected_context_sources: Vec<String>,
    pub omitted_context_sources: Vec<String>,
    pub selected_source_tokens: BTreeMap<String, u64>,
    pub omitted_source_tokens: BTreeMap<String, u64>,
    pub current_request_preserved: bool,
    pub protected_sources_satisfied: bool,
    pub tool_round_integrity_satisfied: bool,
    pub allocation: ContextBudgetAllocation,
}

impl ContextGovernorReport {
    pub(crate) fn insert_metadata(&self, metadata: &mut Metadata) {
        metadata.insert(
            "context_governor_schema".to_string(),
            CONTEXT_GOVERNOR_SCHEMA.to_string(),
        );
        metadata.insert(
            "context_governor_applied".to_string(),
            self.applied.to_string(),
        );
        metadata.insert(
            "context_input_budget_tokens".to_string(),
            self.input_budget_tokens.to_string(),
        );
        metadata.insert(
            "context_original_tokens".to_string(),
            self.estimated_original_tokens.to_string(),
        );
        metadata.insert(
            "context_projected_tokens".to_string(),
            self.estimated_projected_tokens.to_string(),
        );
        metadata.insert(
            "context_omitted_messages".to_string(),
            self.omitted_messages.to_string(),
        );
        metadata.insert(
            "context_truncated_messages".to_string(),
            self.truncated_messages.to_string(),
        );
        metadata.insert(
            "context_hard_limit_satisfied".to_string(),
            self.hard_limit_satisfied.to_string(),
        );
        metadata.insert(
            "context_source_schema".to_string(),
            CONTEXT_SOURCE_SCHEMA.to_string(),
        );
        metadata.insert(
            "context_selected_sources_json".to_string(),
            serde_json::to_string(&self.selected_context_sources)
                .unwrap_or_else(|_| "[]".to_string()),
        );
        metadata.insert(
            "context_omitted_sources_json".to_string(),
            serde_json::to_string(&self.omitted_context_sources)
                .unwrap_or_else(|_| "[]".to_string()),
        );
        metadata.insert(
            "context_selected_source_tokens_json".to_string(),
            serde_json::to_string(&self.selected_source_tokens)
                .unwrap_or_else(|_| "{}".to_string()),
        );
        metadata.insert(
            "context_omitted_source_tokens_json".to_string(),
            serde_json::to_string(&self.omitted_source_tokens).unwrap_or_else(|_| "{}".to_string()),
        );
        metadata.insert(
            "context_current_request_preserved".to_string(),
            self.current_request_preserved.to_string(),
        );
        metadata.insert(
            "context_protected_sources_satisfied".to_string(),
            self.protected_sources_satisfied.to_string(),
        );
        metadata.insert(
            "context_tool_round_integrity_satisfied".to_string(),
            self.tool_round_integrity_satisfied.to_string(),
        );
        metadata.insert(
            "context_budget_allocation_json".to_string(),
            serde_json::to_string(&[
                ("core", self.allocation.core_tokens),
                ("current_request", self.allocation.current_request_tokens),
                (
                    "protected_context",
                    self.allocation.protected_context_tokens,
                ),
                (
                    "supplemental_context",
                    self.allocation.supplemental_context_tokens,
                ),
                (
                    "recent_conversation",
                    self.allocation.recent_conversation_tokens,
                ),
                ("archive_digest", self.allocation.archive_digest_tokens),
                ("unused", self.allocation.unused_tokens),
            ])
            .unwrap_or_else(|_| "[]".to_string()),
        );
    }
}

pub fn bounded_max_output_tokens(context_window_tokens: u64, requested: u64) -> u64 {
    let context_window_tokens = context_window_tokens.max(4_096);
    requested.max(1).min((context_window_tokens / 4).max(1_024))
}

pub(crate) fn govern_model_messages(
    state_messages: &[Message],
    system_prompt: String,
    tools: &[ToolSpec],
    context_window_tokens: u64,
    max_output_tokens: u64,
) -> (Vec<Message>, ContextGovernorReport) {
    let context_window_tokens = context_window_tokens.max(4_096);
    let input_budget_tokens = input_budget_tokens(context_window_tokens, max_output_tokens);
    let system_message = Message {
        role: MessageRole::System,
        content: system_prompt,
        metadata: Metadata::new(),
    };
    let system_tokens = estimate_model_message_tokens(&system_message);
    let state_message_tokens = state_messages
        .iter()
        .map(estimate_model_message_tokens)
        .collect::<Vec<_>>();
    let tool_tokens = estimate_tool_tokens(tools);
    let estimated_original_tokens = system_tokens
        .saturating_add(state_message_tokens.iter().copied().sum::<u64>())
        .saturating_add(tool_tokens);
    if estimated_original_tokens <= input_budget_tokens {
        let mut messages = Vec::with_capacity(state_messages.len() + 1);
        messages.push(system_message);
        messages.extend(state_messages.iter().cloned());
        let selected_context_sources = context_source_labels(&messages);
        let selected_source_tokens = context_source_token_ledger(&messages);
        let current_request_preserved = current_request_preserved(state_messages, &messages);
        let protected_sources_satisfied = protected_sources_satisfied(state_messages, &messages);
        let tool_round_integrity_satisfied = tool_round_integrity_satisfied(&messages);
        return (
            messages,
            ContextGovernorReport {
                applied: false,
                context_window_tokens,
                input_budget_tokens,
                estimated_original_tokens,
                estimated_projected_tokens: estimated_original_tokens,
                original_messages: state_messages.len(),
                projected_messages: state_messages.len() + 1,
                omitted_messages: 0,
                truncated_messages: 0,
                hard_limit_satisfied: true,
                selected_context_sources,
                omitted_context_sources: Vec::new(),
                selected_source_tokens,
                omitted_source_tokens: BTreeMap::new(),
                current_request_preserved,
                protected_sources_satisfied,
                tool_round_integrity_satisfied,
                allocation: ContextBudgetAllocation {
                    core_tokens: fixed_core_tokens(system_tokens, tool_tokens),
                    current_request_tokens: state_messages
                        .iter()
                        .rposition(is_user_turn_start)
                        .map(|index| state_message_tokens[index])
                        .unwrap_or_default(),
                    protected_context_tokens: selected_system_context_tokens(
                        state_messages,
                        &state_message_tokens,
                        true,
                    ),
                    supplemental_context_tokens: selected_system_context_tokens(
                        state_messages,
                        &state_message_tokens,
                        false,
                    ),
                    recent_conversation_tokens: conversation_tokens_except_current(
                        state_messages,
                        &state_message_tokens,
                    ),
                    archive_digest_tokens: 0,
                    unused_tokens: input_budget_tokens.saturating_sub(estimated_original_tokens),
                },
            },
        );
    }

    let fixed_tokens = system_tokens.saturating_add(tool_tokens);
    let available_tokens = input_budget_tokens.saturating_sub(fixed_tokens);
    let mut selected = BTreeSet::new();
    let mut replacements = BTreeMap::new();
    let mut truncated_messages = 0usize;

    let current_user_index = state_messages.iter().rposition(is_user_turn_start);
    let mut selected_conversation_tokens = 0u64;
    if let Some(index) = current_user_index {
        let maximum_user_budget = available_tokens;
        let preferred_user_budget = (available_tokens.saturating_mul(45) / 100)
            .max(256)
            .min(maximum_user_budget);
        let fitted = fit_message_to_budget_with_estimate(
            &state_messages[index],
            preferred_user_budget,
            state_message_tokens[index],
        )
        .or_else(|| {
            fit_required_user_message_to_budget(&state_messages[index], maximum_user_budget)
        });
        if let Some((message, truncated)) = fitted {
            selected.insert(index);
            selected_conversation_tokens = estimate_model_message_tokens(&message);
            if truncated {
                replacements.insert(index, message);
                truncated_messages += 1;
            }
        }
    }

    let system_context_budget = available_tokens
        .saturating_sub(selected_conversation_tokens)
        .min(available_tokens.saturating_mul(35) / 100);
    select_system_contexts(
        state_messages,
        &state_message_tokens,
        system_context_budget,
        current_user_index.map(|index| state_messages[index].content.as_str()),
        &mut selected,
        &mut replacements,
        &mut truncated_messages,
    );
    let selected_system_tokens = selected
        .iter()
        .filter(|index| matches!(state_messages[**index].role, MessageRole::System))
        .map(|index| {
            replacements.get(index).map_or_else(
                || state_message_tokens[*index],
                estimate_model_message_tokens,
            )
        })
        .sum::<u64>();

    let digest_reserve = (available_tokens / 10).clamp(128, 8_192);
    let mut recent_budget = available_tokens
        .saturating_sub(selected_system_tokens)
        .saturating_sub(selected_conversation_tokens)
        .saturating_sub(digest_reserve);
    if let Some(current_user_index) = current_user_index {
        select_recent_current_turn_rounds(
            state_messages,
            &state_message_tokens,
            current_user_index,
            &mut recent_budget,
            &mut selected,
        );
        select_prior_user_turns(
            state_messages,
            &state_message_tokens,
            current_user_index,
            &mut recent_budget,
            &mut selected,
        );
    } else {
        select_recent_messages(
            state_messages,
            &state_message_tokens,
            &mut recent_budget,
            &mut selected,
        );
    }

    let selected_tokens = selected
        .iter()
        .map(|index| {
            replacements.get(index).map_or_else(
                || state_message_tokens[*index],
                estimate_model_message_tokens,
            )
        })
        .sum::<u64>();
    let digest_budget = available_tokens.saturating_sub(selected_tokens);
    let omitted_indices = (0..state_messages.len())
        .filter(|index| !selected.contains(index))
        .collect::<Vec<_>>();
    let digest = build_omitted_context_digest(state_messages, &omitted_indices, digest_budget);

    let mut messages = Vec::with_capacity(selected.len() + usize::from(digest.is_some()) + 1);
    messages.push(system_message);
    for index in selected.iter().filter(|index| {
        state_messages
            .get(**index)
            .is_some_and(|message| matches!(message.role, MessageRole::System))
    }) {
        if let Some(message) = replacements
            .get(index)
            .or_else(|| state_messages.get(*index))
        {
            messages.push(message.clone());
        }
    }
    if let Some(digest) = digest {
        messages.push(digest);
    }
    for index in selected.iter().filter(|index| {
        state_messages
            .get(**index)
            .is_some_and(|message| !matches!(message.role, MessageRole::System))
    }) {
        if let Some(message) = replacements
            .get(index)
            .or_else(|| state_messages.get(*index))
        {
            messages.push(message.clone());
        }
    }

    let estimated_projected_tokens =
        estimate_messages_tokens(&messages).saturating_add(tool_tokens);
    let selected_context_sources = context_source_labels(&messages);
    let selected_source_tokens = context_source_token_ledger(&messages);
    let omitted_context_sources = omitted_indices
        .iter()
        .filter_map(|index| state_messages.get(*index))
        .filter_map(ContextSourceKind::from_message)
        .map(ContextSourceKind::as_str)
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let omitted_source_tokens = context_source_token_ledger_for_indices(
        state_messages,
        &state_message_tokens,
        &omitted_indices,
    );
    let current_request_preserved = current_request_preserved(state_messages, &messages);
    let protected_sources_satisfied = protected_sources_satisfied(state_messages, &messages);
    let tool_round_integrity_satisfied = tool_round_integrity_satisfied(&messages);
    let archive_digest_tokens = messages
        .iter()
        .find(|message| {
            message.metadata.get("kind").map(String::as_str) == Some("context_governor_digest")
        })
        .map(estimate_model_message_tokens)
        .unwrap_or_default();
    let protected_context_tokens =
        selected_context_tokens_by_protection(state_messages, &selected, &replacements, true);
    let supplemental_context_tokens =
        selected_context_tokens_by_protection(state_messages, &selected, &replacements, false);
    let recent_conversation_tokens = selected
        .iter()
        .filter(|index| Some(**index) != current_user_index)
        .filter(|index| !matches!(state_messages[**index].role, MessageRole::System))
        .map(|index| {
            replacements.get(index).map_or_else(
                || state_message_tokens[*index],
                estimate_model_message_tokens,
            )
        })
        .sum::<u64>();
    let report = ContextGovernorReport {
        applied: true,
        context_window_tokens,
        input_budget_tokens,
        estimated_original_tokens,
        estimated_projected_tokens,
        original_messages: state_messages.len(),
        projected_messages: messages.len(),
        omitted_messages: omitted_indices.len(),
        truncated_messages,
        hard_limit_satisfied: estimated_projected_tokens <= input_budget_tokens,
        selected_context_sources,
        omitted_context_sources,
        selected_source_tokens,
        omitted_source_tokens,
        current_request_preserved,
        protected_sources_satisfied,
        tool_round_integrity_satisfied,
        allocation: ContextBudgetAllocation {
            core_tokens: fixed_core_tokens(system_tokens, tool_tokens),
            current_request_tokens: selected_conversation_tokens,
            protected_context_tokens,
            supplemental_context_tokens,
            recent_conversation_tokens,
            archive_digest_tokens,
            unused_tokens: input_budget_tokens.saturating_sub(estimated_projected_tokens),
        },
    };
    (messages, report)
}

fn input_budget_tokens(context_window_tokens: u64, requested_output_tokens: u64) -> u64 {
    let output_tokens = bounded_max_output_tokens(context_window_tokens, requested_output_tokens);
    let safety_tokens = (context_window_tokens / 20).clamp(512, 8_192);
    context_window_tokens
        .saturating_sub(output_tokens)
        .saturating_sub(safety_tokens)
        .max(1_024)
}

fn estimate_messages_tokens(messages: &[Message]) -> u64 {
    messages.iter().map(estimate_model_message_tokens).sum()
}

fn estimate_tool_tokens(tools: &[ToolSpec]) -> u64 {
    tools
        .iter()
        .map(|tool| {
            estimate_text_tokens(&tool.name)
                .saturating_add(estimate_text_tokens(&tool.description))
                .saturating_add(estimate_text_tokens(&tool.input_schema_json))
                .saturating_add(16)
        })
        .sum()
}

fn is_user_turn_start(message: &Message) -> bool {
    matches!(message.role, MessageRole::User)
        && message.metadata.get("kind").map(String::as_str) != Some("tool_observation")
}

fn context_source_labels(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .skip(1)
        .filter_map(ContextSourceKind::from_message)
        .map(ContextSourceKind::as_str)
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn fixed_core_tokens(system_tokens: u64, tool_tokens: u64) -> u64 {
    system_tokens.saturating_add(tool_tokens)
}

fn context_source_token_ledger(messages: &[Message]) -> BTreeMap<String, u64> {
    let mut ledger = BTreeMap::new();
    for message in messages.iter().skip(1) {
        let Some(source) = ContextSourceKind::from_message(message) else {
            continue;
        };
        let entry = ledger.entry(source.as_str().to_string()).or_insert(0u64);
        *entry = (*entry).saturating_add(estimate_model_message_tokens(message));
    }
    ledger
}

fn context_source_token_ledger_for_indices(
    messages: &[Message],
    message_tokens: &[u64],
    indices: &[usize],
) -> BTreeMap<String, u64> {
    let mut ledger = BTreeMap::new();
    for index in indices {
        let Some(message) = messages.get(*index) else {
            continue;
        };
        let Some(source) = ContextSourceKind::from_message(message) else {
            continue;
        };
        let entry = ledger.entry(source.as_str().to_string()).or_insert(0u64);
        *entry = (*entry).saturating_add(message_tokens.get(*index).copied().unwrap_or_default());
    }
    ledger
}

fn selected_system_context_tokens(
    messages: &[Message],
    message_tokens: &[u64],
    protected: bool,
) -> u64 {
    messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| {
            let source = ContextSourceKind::from_message(message)?;
            (source.is_protected() == protected).then_some(message_tokens[index])
        })
        .sum()
}

fn selected_context_tokens_by_protection(
    messages: &[Message],
    selected: &BTreeSet<usize>,
    replacements: &BTreeMap<usize, Message>,
    protected: bool,
) -> u64 {
    selected
        .iter()
        .filter_map(|index| {
            let message = messages.get(*index)?;
            let source = ContextSourceKind::from_message(message)?;
            (source.is_protected() == protected).then(|| {
                replacements.get(index).map_or_else(
                    || estimate_model_message_tokens(message),
                    estimate_model_message_tokens,
                )
            })
        })
        .sum()
}

fn conversation_tokens_except_current(messages: &[Message], message_tokens: &[u64]) -> u64 {
    let current = messages.iter().rposition(is_user_turn_start);
    messages
        .iter()
        .enumerate()
        .filter(|(index, message)| {
            Some(*index) != current && !matches!(message.role, MessageRole::System)
        })
        .map(|(index, _)| message_tokens[index])
        .sum()
}

fn current_request_preserved(original: &[Message], projected: &[Message]) -> bool {
    let Some(current) = original.iter().rfind(|message| is_user_turn_start(message)) else {
        return true;
    };
    let Some(selected) = projected
        .iter()
        .rfind(|message| is_user_turn_start(message))
    else {
        return false;
    };
    let prefix = current.content.chars().take(64).collect::<String>();
    prefix.is_empty() || selected.content.starts_with(&prefix)
}

fn protected_sources_satisfied(original: &[Message], projected: &[Message]) -> bool {
    let required = original
        .iter()
        .filter_map(ContextSourceKind::from_message)
        .filter(|source| source.is_protected())
        .collect::<BTreeSet<_>>();
    let selected = projected
        .iter()
        .filter_map(ContextSourceKind::from_message)
        .filter(|source| source.is_protected())
        .collect::<BTreeSet<_>>();
    required.is_subset(&selected)
}

fn tool_round_integrity_satisfied(messages: &[Message]) -> bool {
    let mut proposed = BTreeSet::new();
    let mut observed = BTreeSet::new();
    for message in messages {
        if let Some(raw_calls) = message.metadata.get("raw_tool_calls_json") {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw_calls) {
                if let Some(calls) = value.as_array() {
                    proposed.extend(calls.iter().filter_map(|call| {
                        call.get("id")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string)
                    }));
                }
            }
        }
        if matches!(message.role, MessageRole::Tool) {
            if let Some(call_id) = message.metadata.get("tool_call_id") {
                observed.insert(call_id.clone());
            }
        }
    }
    proposed == observed
}

fn system_context_priority(message: &Message) -> u8 {
    ContextSourceKind::from_message(message)
        .map(ContextSourceKind::priority)
        .unwrap_or_default()
}

fn system_context_is_protected(message: &Message) -> bool {
    ContextSourceKind::from_message(message).is_some_and(ContextSourceKind::is_protected)
}

fn context_relevance_score(message: &Message, objective_terms: &BTreeSet<String>) -> u16 {
    if objective_terms.is_empty() {
        return 0;
    }
    let message_terms = context_relevance_terms(&message.content);
    let overlap = objective_terms.intersection(&message_terms).count();
    if overlap == 0 {
        return 0;
    }
    let identifier_overlap = objective_terms
        .intersection(&message_terms)
        .filter(|term| is_context_identifier(term))
        .count();
    let coverage = overlap.saturating_mul(30) / objective_terms.len().max(1);
    overlap
        .saturating_mul(12)
        .saturating_add(coverage)
        .saturating_add(identifier_overlap.saturating_mul(50))
        .min(u16::MAX as usize) as u16
}

fn context_relevance_terms(text: &str) -> BTreeSet<String> {
    text.split(|character: char| {
        !(character.is_alphanumeric() || matches!(character, '_' | '-' | '.' | '/' | ':' | '@'))
    })
    .map(|term| {
        term.trim_matches(|character: char| matches!(character, '.' | '/' | ':' | '-' | '@'))
            .to_lowercase()
    })
    .filter(|term| {
        !term.is_empty()
            && (term.chars().count() > 1 || term.chars().any(char::is_numeric))
            && !matches!(
                term.as_str(),
                "a" | "an"
                    | "and"
                    | "are"
                    | "as"
                    | "at"
                    | "be"
                    | "by"
                    | "for"
                    | "from"
                    | "in"
                    | "is"
                    | "it"
                    | "of"
                    | "on"
                    | "or"
                    | "that"
                    | "the"
                    | "this"
                    | "to"
                    | "was"
                    | "what"
                    | "when"
                    | "where"
                    | "which"
                    | "with"
            )
    })
    .collect()
}

fn is_context_identifier(term: &str) -> bool {
    term.chars()
        .any(|character| matches!(character, '_' | '-' | '.' | '/' | ':' | '@'))
        || term.chars().any(char::is_numeric)
}

fn system_context_key(message: &Message, index: usize) -> String {
    message
        .metadata
        .get("kind")
        .or_else(|| message.metadata.get("collaboration_stage"))
        .cloned()
        .unwrap_or_else(|| format!("system-{index}"))
}

fn select_system_contexts(
    messages: &[Message],
    message_tokens: &[u64],
    budget: u64,
    current_objective: Option<&str>,
    selected: &mut BTreeSet<usize>,
    replacements: &mut BTreeMap<usize, Message>,
    truncated_messages: &mut usize,
) {
    let objective_terms = current_objective
        .map(context_relevance_terms)
        .unwrap_or_default();
    let mut seen = BTreeSet::new();
    let mut candidates = messages
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, message)| matches!(message.role, MessageRole::System))
        .filter(|(index, message)| seen.insert(system_context_key(message, *index)))
        .map(|(index, message)| {
            (
                index,
                system_context_is_protected(message),
                context_relevance_score(message, &objective_terms),
                system_context_priority(message),
            )
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then(right.2.cmp(&left.2))
            .then(right.3.cmp(&left.3))
            .then(right.0.cmp(&left.0))
    });

    let mut remaining = budget;
    for (index, _, _, _) in candidates {
        if remaining < 64 {
            break;
        }
        let message = &messages[index];
        let per_message_budget = remaining.min((budget / 2).max(256));
        let Some((fitted, truncated)) =
            fit_message_to_budget_with_estimate(message, per_message_budget, message_tokens[index])
        else {
            continue;
        };
        let tokens = estimate_model_message_tokens(&fitted);
        if tokens > remaining {
            continue;
        }
        selected.insert(index);
        if truncated {
            replacements.insert(index, fitted);
            *truncated_messages += 1;
        }
        remaining = remaining.saturating_sub(tokens);
    }
}

fn select_recent_current_turn_rounds(
    messages: &[Message],
    message_tokens: &[u64],
    current_user_index: usize,
    budget: &mut u64,
    selected: &mut BTreeSet<usize>,
) {
    let assistant_starts = messages
        .iter()
        .enumerate()
        .skip(current_user_index + 1)
        .filter_map(|(index, message)| {
            matches!(message.role, MessageRole::Assistant).then_some(index)
        })
        .collect::<Vec<_>>();
    for position in (0..assistant_starts.len()).rev() {
        let start = assistant_starts[position];
        let end = assistant_starts
            .get(position + 1)
            .copied()
            .unwrap_or(messages.len());
        let indices = (start..end)
            .filter(|index| !matches!(messages[*index].role, MessageRole::System))
            .collect::<Vec<_>>();
        let tokens = indices
            .iter()
            .map(|index| message_tokens[*index])
            .sum::<u64>();
        if tokens > *budget {
            break;
        }
        selected.extend(indices);
        *budget = budget.saturating_sub(tokens);
    }
}

fn select_prior_user_turns(
    messages: &[Message],
    message_tokens: &[u64],
    current_user_index: usize,
    budget: &mut u64,
    selected: &mut BTreeSet<usize>,
) {
    let user_starts = messages[..current_user_index]
        .iter()
        .enumerate()
        .filter_map(|(index, message)| is_user_turn_start(message).then_some(index))
        .collect::<Vec<_>>();
    for position in (0..user_starts.len()).rev() {
        let start = user_starts[position];
        let end = user_starts
            .get(position + 1)
            .copied()
            .unwrap_or(current_user_index);
        let indices = (start..end)
            .filter(|index| !matches!(messages[*index].role, MessageRole::System))
            .collect::<Vec<_>>();
        let tokens = indices
            .iter()
            .map(|index| message_tokens[*index])
            .sum::<u64>();
        if tokens > *budget {
            break;
        }
        selected.extend(indices);
        *budget = budget.saturating_sub(tokens);
    }
}

fn select_recent_messages(
    messages: &[Message],
    message_tokens: &[u64],
    budget: &mut u64,
    selected: &mut BTreeSet<usize>,
) {
    for index in (0..messages.len()).rev() {
        if matches!(messages[index].role, MessageRole::System) {
            continue;
        }
        let tokens = message_tokens[index];
        if tokens > *budget {
            break;
        }
        selected.insert(index);
        *budget = budget.saturating_sub(tokens);
    }
}

fn fit_required_user_message_to_budget(message: &Message, budget: u64) -> Option<(Message, bool)> {
    if budget < 8 {
        return None;
    }

    let mut projected = message.clone();
    projected.metadata.remove("raw_tool_calls_json");
    let image_paths = projected
        .metadata
        .get("image_paths")
        .map(|value| {
            value
                .lines()
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if !image_paths.is_empty() {
        let mut retained = image_paths.len();
        loop {
            if retained == 0 {
                projected.metadata.remove("image_paths");
            } else {
                projected.metadata.insert(
                    "image_paths".to_string(),
                    image_paths[..retained].join("\n"),
                );
            }
            let mut empty = projected.clone();
            empty.content.clear();
            if estimate_model_message_tokens(&empty) < budget || retained == 0 {
                break;
            }
            retained -= 1;
        }
        if retained < image_paths.len() {
            projected.metadata.insert(
                "context_omitted_image_count".to_string(),
                image_paths.len().saturating_sub(retained).to_string(),
            );
        }
    }

    fit_message_to_budget(&projected, budget).map(|(message, _)| (message, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context_projection::compact_excerpt;

    fn message(role: MessageRole, content: impl Into<String>) -> Message {
        Message {
            role,
            content: content.into(),
            metadata: Metadata::new(),
        }
    }

    fn tool() -> ToolSpec {
        ToolSpec::builtin(
            "file.read",
            "file",
            "Read a workspace file",
            agent_core::ToolRisk::ReadOnly,
            r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#,
        )
    }

    #[test]
    fn small_context_is_preserved_verbatim() {
        let history = vec![message(MessageRole::User, "Inspect README")];
        let (projected, report) =
            govern_model_messages(&history, "system".to_string(), &[tool()], 128_000, 4_096);

        assert!(!report.applied);
        assert_eq!(projected.len(), 2);
        assert_eq!(projected[1], history[0]);
    }

    #[test]
    fn model_request_reports_selected_context_provenance() {
        let mut knowledge = message(MessageRole::System, "retrieved evidence");
        knowledge
            .metadata
            .insert("kind".to_string(), "knowledge_context".to_string());
        let history = vec![knowledge, message(MessageRole::User, "Inspect README")];

        let (_, report) =
            govern_model_messages(&history, "system".to_string(), &[tool()], 128_000, 4_096);
        let mut metadata = Metadata::new();
        report.insert_metadata(&mut metadata);

        assert_eq!(
            report.selected_context_sources,
            vec!["knowledge_context".to_string()]
        );
        assert_eq!(metadata["context_source_schema"], CONTEXT_SOURCE_SCHEMA);
        assert_eq!(
            metadata["context_selected_sources_json"],
            r#"["knowledge_context"]"#
        );
    }

    #[test]
    fn token_estimate_is_conservative_for_cjk() {
        let chinese = "上下文压缩".repeat(200);
        assert!(estimate_text_tokens(&chinese) >= 1_000);
    }

    #[test]
    fn compact_excerpt_preserves_normalized_excerpt_semantics() {
        assert_eq!(
            compact_excerpt("  alpha\n beta\tgamma  ", 64),
            "alpha beta gamma"
        );
        assert_eq!(compact_excerpt("abc   ", 3), "abc");
        assert_eq!(compact_excerpt("abc def", 4), "abc ...");
        assert_eq!(compact_excerpt("   ", 0), "");
        assert_eq!(compact_excerpt("content", 0), "...");
    }

    #[test]
    fn compact_excerpt_stops_on_unicode_character_boundaries() {
        assert_eq!(compact_excerpt("上下文 压缩之后", 5), "上下文 压...");
    }

    #[test]
    fn oversized_context_keeps_goal_critical_context_and_latest_tool_round() {
        let mut artifact = message(MessageRole::System, "artifact ".repeat(8_000));
        artifact
            .metadata
            .insert("kind".to_string(), "artifact_manifest".to_string());
        let mut history = vec![
            artifact,
            message(MessageRole::User, "old requirement ".repeat(5_000)),
            message(MessageRole::Assistant, "old answer ".repeat(5_000)),
            message(
                MessageRole::User,
                "current goal: finish the verified implementation",
            ),
        ];
        for index in 0..5 {
            let mut assistant = message(MessageRole::Assistant, format!("tool round {index}"));
            assistant.metadata.insert(
                "raw_tool_calls_json".to_string(),
                format!(
                    r#"[{{"id":"call-{index}","type":"function","function":{{"name":"file_read","arguments":"{{\"path\":\"file-{index}\"}}"}}}}]"#
                ),
            );
            let mut observation = message(
                MessageRole::Tool,
                if index == 4 {
                    "latest verified evidence".to_string()
                } else {
                    "older evidence ".repeat(4_000)
                },
            );
            observation
                .metadata
                .insert("tool_call_id".to_string(), format!("call-{index}"));
            history.push(assistant);
            history.push(observation);
        }
        let original = history.clone();

        let (projected, report) =
            govern_model_messages(&history, "system".repeat(300), &[tool()], 16_384, 2_048);

        assert!(report.applied);
        assert!(report.hard_limit_satisfied);
        assert!(report.omitted_messages > 0);
        assert!(report.current_request_preserved);
        assert!(report.protected_sources_satisfied);
        assert!(report.tool_round_integrity_satisfied);
        assert!(report
            .selected_source_tokens
            .contains_key("artifact_manifest"));
        assert!(projected.iter().any(|message| {
            message
                .content
                .contains("current goal: finish the verified implementation")
        }));
        assert!(projected.iter().any(|message| {
            message.metadata.get("kind").map(String::as_str) == Some("artifact_manifest")
        }));
        let assistant_index = projected
            .iter()
            .position(|message| {
                message
                    .metadata
                    .get("raw_tool_calls_json")
                    .is_some_and(|value| value.contains("call-4"))
            })
            .expect("latest assistant tool call should remain");
        let tool_index = projected
            .iter()
            .position(|message| {
                message.metadata.get("tool_call_id").map(String::as_str) == Some("call-4")
            })
            .expect("latest tool evidence should remain");
        assert!(assistant_index < tool_index);
        assert_eq!(
            history, original,
            "canonical runtime history must remain lossless"
        );
    }

    #[test]
    fn supplemental_context_selection_prefers_current_objective_coverage() {
        let mut generic = message(
            MessageRole::System,
            "workspace architecture overview with unrelated deployment notes ".repeat(200),
        );
        generic
            .metadata
            .insert("kind".to_string(), "knowledge_context".to_string());
        let mut relevant = message(
            MessageRole::System,
            "max_turns terminal reserve preserves the completed final answer ".repeat(200),
        );
        relevant
            .metadata
            .insert("kind".to_string(), "project_memory".to_string());
        let messages = vec![generic, relevant];
        let tokens = messages
            .iter()
            .map(estimate_model_message_tokens)
            .collect::<Vec<_>>();
        let mut selected = BTreeSet::new();
        let mut replacements = BTreeMap::new();
        let mut truncated = 0;

        select_system_contexts(
            &messages,
            &tokens,
            128,
            Some("Why did max_turns discard the final answer?"),
            &mut selected,
            &mut replacements,
            &mut truncated,
        );

        assert!(selected.contains(&1));
        assert!(!selected.contains(&0));
    }

    #[test]
    fn context_report_accounts_for_selected_and_omitted_sources() {
        let mut restore = message(MessageRole::System, "restore ".repeat(4_000));
        restore
            .metadata
            .insert("kind".to_string(), "context_restore_pack".to_string());
        let mut knowledge = message(MessageRole::System, "knowledge ".repeat(7_000));
        knowledge
            .metadata
            .insert("kind".to_string(), "knowledge_context".to_string());
        let history = vec![
            restore,
            knowledge,
            message(
                MessageRole::User,
                "Keep the restored objective and finish it",
            ),
        ];

        let (_, report) =
            govern_model_messages(&history, "system".to_string(), &[tool()], 8_192, 1_024);

        assert!(report.applied);
        assert!(report.current_request_preserved);
        assert!(report.protected_sources_satisfied);
        assert!(report
            .selected_source_tokens
            .contains_key("context_restore_pack"));
        assert!(report
            .selected_source_tokens
            .values()
            .chain(report.omitted_source_tokens.values())
            .all(|tokens| *tokens > 0));
        let accounted = report
            .allocation
            .core_tokens
            .saturating_add(report.allocation.current_request_tokens)
            .saturating_add(report.allocation.protected_context_tokens)
            .saturating_add(report.allocation.supplemental_context_tokens)
            .saturating_add(report.allocation.recent_conversation_tokens)
            .saturating_add(report.allocation.archive_digest_tokens)
            .saturating_add(report.allocation.unused_tokens);
        assert_eq!(accounted, report.input_budget_tokens);
    }

    #[test]
    fn oversized_current_user_attachments_are_bounded_without_mutating_history() {
        let mut current = message(
            MessageRole::User,
            "Compare every attached reference and preserve the stated constraints. ".repeat(500),
        );
        current.metadata.insert(
            "image_paths".to_string(),
            (0..10)
                .map(|index| format!("/tmp/reference-{index}.png"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let history = vec![current];
        let canonical = history.clone();

        let (projected, report) =
            govern_model_messages(&history, "system".to_string(), &[tool()], 8_192, 1_024);

        assert!(report.applied);
        assert!(report.hard_limit_satisfied);
        let projected_user = projected
            .iter()
            .find(|message| matches!(message.role, MessageRole::User))
            .expect("current user request should remain");
        let retained_images = projected_user
            .metadata
            .get("image_paths")
            .map(|paths| paths.lines().count())
            .unwrap_or_default();
        assert!(retained_images < 10);
        assert_eq!(
            projected_user
                .metadata
                .get("context_omitted_image_count")
                .and_then(|value| value.parse::<usize>().ok()),
            Some(10 - retained_images)
        );
        assert_eq!(
            history, canonical,
            "canonical attachment metadata must remain lossless"
        );
    }

    #[test]
    fn output_budget_is_bounded_by_the_context_window() {
        assert_eq!(bounded_max_output_tokens(32_000, 32_768), 8_000);
        assert_eq!(bounded_max_output_tokens(128_000, 32_768), 32_000);
        assert_eq!(bounded_max_output_tokens(1_000_000, 32_768), 32_768);
    }

    #[test]
    #[ignore = "performance diagnostic; run through the quality-gate performance profile"]
    fn long_history_context_governor_scaling_diagnostic() {
        let mut history = Vec::with_capacity(8_001);
        for index in 0..4_000 {
            history.push(message(
                MessageRole::User,
                format!(
                    "historical requirement {index}: {}",
                    "constraint ".repeat(12)
                ),
            ));
            history.push(message(
                MessageRole::Assistant,
                format!("historical response {index}: {}", "evidence ".repeat(12)),
            ));
        }
        history.push(message(
            MessageRole::User,
            "current goal: preserve UX and complete the verified task",
        ));
        let history_payload_bytes = history
            .iter()
            .map(|entry| entry.content.len())
            .sum::<usize>();
        let canonical_messages = history.len();
        let mut samples = Vec::with_capacity(11);
        let mut projected_messages = 0usize;
        let mut estimated_original_tokens = 0u64;
        let mut estimated_projected_tokens = 0u64;
        for _ in 0..11 {
            let started_at = std::time::Instant::now();
            let (projected, report) =
                govern_model_messages(&history, "system".to_string(), &[tool()], 32_768, 4_096);
            samples.push(started_at.elapsed().as_micros());
            assert!(report.applied);
            assert!(report.hard_limit_satisfied);
            assert!(projected.iter().any(|entry| entry
                .content
                .contains("current goal: preserve UX and complete the verified task")));
            projected_messages = report.projected_messages;
            estimated_original_tokens = report.estimated_original_tokens;
            estimated_projected_tokens = report.estimated_projected_tokens;
        }
        assert_eq!(history.len(), canonical_messages);
        assert!(projected_messages < canonical_messages / 10);
        samples.sort_unstable();
        let percentile = |value: usize| samples[(samples.len().saturating_sub(1) * value) / 100];
        let p50_micros = percentile(50);
        let p95_micros = percentile(95);
        let max_micros = samples.last().copied().unwrap_or_default();
        println!(
            "{{\"schema\":\"cindx.context-governor-diagnostic.v1\",\"history_messages\":{canonical_messages},\"history_payload_bytes\":{history_payload_bytes},\"projected_messages\":{projected_messages},\"estimated_original_tokens\":{estimated_original_tokens},\"estimated_projected_tokens\":{estimated_projected_tokens},\"sample_count\":{},\"p50_micros\":{p50_micros},\"p95_micros\":{p95_micros},\"max_micros\":{max_micros}}}",
            samples.len()
        );
    }
}
