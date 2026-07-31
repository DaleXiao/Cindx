use crate::context_engine::{estimate_model_message_tokens, ContextSourceKind};
use agent_core::Message;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub(crate) fn context_source_token_ledger_for_projection(
    context_overlays: &[Message],
    state_messages: &[Message],
    state_message_tokens: &[u64],
    selected: Option<&BTreeSet<usize>>,
    replacements: &BTreeMap<usize, Message>,
) -> BTreeMap<String, u64> {
    let mut ledger = BTreeMap::new();
    for message in context_overlays {
        add_context_source_tokens(&mut ledger, message, estimate_model_message_tokens(message));
    }
    for (index, message) in state_messages.iter().enumerate() {
        if selected.is_some_and(|selected| !selected.contains(&index)) {
            continue;
        }
        let projected = replacements.get(&index).unwrap_or(message);
        let tokens = replacements.get(&index).map_or_else(
            || state_message_tokens[index],
            estimate_model_message_tokens,
        );
        add_context_source_tokens(&mut ledger, projected, tokens);
    }
    ledger
}

pub(crate) fn context_source_token_ledger_for_indices(
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

pub(crate) fn add_context_source_tokens(
    ledger: &mut BTreeMap<String, u64>,
    message: &Message,
    tokens: u64,
) {
    let Some(source) = ContextSourceKind::from_message(message) else {
        return;
    };
    let entry = ledger.entry(source.as_str().to_string()).or_insert(0u64);
    *entry = (*entry).saturating_add(tokens);
}

/// Non-semantic cache for exact token estimates of the canonical transcript.
///
/// Agent transitions append messages in the steady state. The shallow identities
/// let repeated model turns reuse estimates without rescanning message content;
/// a structural change rebuilds only the affected suffix. Callers that mutate an
/// estimator-relevant `String` in place must invalidate that message explicitly.
#[derive(Default)]
pub(crate) struct ContextTokenLedger {
    identities: Vec<MessageTokenIdentity>,
    tokens: Vec<u64>,
    #[cfg(test)]
    estimate_count: usize,
}

impl fmt::Debug for ContextTokenLedger {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContextTokenLedger")
            .field("cached_messages", &self.tokens.len())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MessageTokenIdentity {
    content: StringIdentity,
    raw_tool_calls_json: Option<StringIdentity>,
    image_paths: Option<StringIdentity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StringIdentity {
    address: usize,
    len: usize,
}

impl StringIdentity {
    fn of(value: &str) -> Self {
        Self {
            address: value.as_ptr() as usize,
            len: value.len(),
        }
    }
}

impl MessageTokenIdentity {
    fn of(message: &Message) -> Self {
        Self {
            content: StringIdentity::of(&message.content),
            raw_tool_calls_json: message
                .metadata
                .get("raw_tool_calls_json")
                .map(|value| StringIdentity::of(value)),
            image_paths: message
                .metadata
                .get("image_paths")
                .map(|value| StringIdentity::of(value)),
        }
    }
}

impl ContextTokenLedger {
    pub(crate) fn synchronize(&mut self, messages: &[Message]) {
        let shared_len = self.identities.len().min(messages.len());
        let first_changed = self
            .identities
            .iter()
            .zip(messages)
            .take(shared_len)
            .position(|(cached, message)| *cached != MessageTokenIdentity::of(message));

        if let Some(first_changed) = first_changed {
            self.identities.truncate(first_changed);
            self.tokens.truncate(first_changed);
        } else if messages.len() < self.identities.len() {
            self.identities.truncate(messages.len());
            self.tokens.truncate(messages.len());
        }

        for message in &messages[self.identities.len()..] {
            self.identities.push(MessageTokenIdentity::of(message));
            self.tokens.push(estimate_model_message_tokens(message));
            #[cfg(test)]
            {
                self.estimate_count += 1;
            }
        }
    }

    pub(crate) fn tokens(&self) -> &[u64] {
        &self.tokens
    }

    pub(crate) fn invalidate_from(&mut self, first_changed_message: usize) {
        let retained = first_changed_message.min(self.identities.len());
        self.identities.truncate(retained);
        self.tokens.truncate(retained);
    }

    #[cfg(test)]
    pub(crate) fn estimate_count(&self) -> usize {
        self.estimate_count
    }
}

// Cloning runtime state must not retain allocation identities from the source
// transcript. The clone lazily reconstructs its cache on its first prepare.
impl Clone for ContextTokenLedger {
    fn clone(&self) -> Self {
        Self::default()
    }
}

// The ledger is derived data and therefore does not participate in semantic
// equality of AgentLoopState.
impl PartialEq for ContextTokenLedger {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl Eq for ContextTokenLedger {}
