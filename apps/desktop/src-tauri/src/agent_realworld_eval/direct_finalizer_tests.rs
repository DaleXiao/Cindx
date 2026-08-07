use super::direct_finalizer::*;
use agent_core::{Message, MessageRole, Metadata, ModelRole, TaskId};
use agent_runtime::{start_agent_loop, AgentLoopState, AgentRuntimeConfig};
use model_provider::ModelResponse;

fn runtime() -> AgentLoopState {
    let mut runtime = start_agent_loop(
        TaskId("direct-finalizer-eval".to_string()),
        "Return a concise answer grounded in the supplied facts.",
        AgentRuntimeConfig::default(),
    );
    runtime.messages.push(Message {
        role: MessageRole::Assistant,
        content: "The supplied fact is amber.".to_string(),
        metadata: [("internal".to_string(), "true".to_string())]
            .into_iter()
            .collect(),
    });
    runtime
}

fn config() -> DirectFinalizerEvalConfig {
    DirectFinalizerEvalConfig {
        provider_id: "provider-fixed".to_string(),
        model_id: "model-fixed".to_string(),
        system_prompt: Some("Deliver only the grounded final answer.".to_string()),
        runtime_context: Some("Frozen evaluation fixture.".to_string()),
        context_window_tokens: 32_768,
        max_output_tokens: 1_024,
    }
}

fn response(content: &str) -> ModelResponse {
    ModelResponse {
        message: Message {
            role: MessageRole::Assistant,
            content: content.to_string(),
            metadata: Metadata::new(),
        },
        raw_tool_calls_json: None,
        tool_calls: Vec::new(),
        metadata: Metadata::new(),
    }
}

#[test]
fn matched_pair_changes_only_the_direct_finalizer_system_prompt() {
    let pair = prepare_direct_finalizer_pair(
        runtime(),
        config(),
        None,
        Some("Challenge unsupported claims once before delivery."),
    )
    .expect("matched direct-finalizer pair should prepare");

    assert_eq!(
        pair.parent.receipt.pre_treatment_sha256,
        pair.challenger.receipt.pre_treatment_sha256
    );
    assert_eq!(
        pair.parent.receipt.task_contract_sha256,
        pair.challenger.receipt.task_contract_sha256
    );
    assert_ne!(
        pair.parent.receipt.canonical_request_sha256,
        pair.challenger.receipt.canonical_request_sha256
    );
    assert_ne!(
        pair.parent.receipt.pre_treatment_sha256,
        pair.parent.receipt.canonical_request_sha256
    );
    assert!(pair.parent.request().tools.is_empty());
    assert!(pair.challenger.request().tools.is_empty());
    assert_eq!(pair.parent.request().role, ModelRole::Summarizer);
    assert_eq!(pair.challenger.request().role, ModelRole::Summarizer);
    let parent_terminal_policy = pair
        .parent
        .request()
        .messages
        .iter()
        .filter(|message| {
            message.metadata.get("kind").map(String::as_str) == Some("terminal_commit_policy")
        })
        .collect::<Vec<_>>();
    let challenger_terminal_policy = pair
        .challenger
        .request()
        .messages
        .iter()
        .filter(|message| {
            message.metadata.get("kind").map(String::as_str) == Some("terminal_commit_policy")
        })
        .collect::<Vec<_>>();
    assert_eq!(parent_terminal_policy.len(), 1);
    assert_eq!(parent_terminal_policy, challenger_terminal_policy);
}

#[test]
fn canonical_pre_treatment_digest_is_repeatable_and_state_sensitive() {
    let first = prepare_direct_finalizer_pair(
        runtime(),
        config(),
        None,
        Some("Challenge unsupported claims once before delivery."),
    )
    .expect("first pair");
    let second = prepare_direct_finalizer_pair(
        runtime(),
        config(),
        None,
        Some("Challenge unsupported claims once before delivery."),
    )
    .expect("second pair");
    let mut changed_runtime = runtime();
    changed_runtime.messages.push(Message {
        role: MessageRole::User,
        content: "A distinct frozen-state fact.".to_string(),
        metadata: Metadata::new(),
    });
    let changed = prepare_direct_finalizer_pair(
        changed_runtime,
        config(),
        None,
        Some("Challenge unsupported claims once before delivery."),
    )
    .expect("changed pair");

    assert_eq!(
        first.parent.receipt.pre_treatment_sha256,
        second.parent.receipt.pre_treatment_sha256
    );
    assert_ne!(
        first.parent.receipt.pre_treatment_sha256,
        changed.parent.receipt.pre_treatment_sha256
    );
}

#[test]
fn identical_directives_fail_closed_as_no_treatment_effect() {
    let error = prepare_direct_finalizer_pair(
        runtime(),
        config(),
        Some("Use the same final check."),
        Some("Use the same final check."),
    )
    .err()
    .expect("an identical request cannot form a causal pair");

    assert!(error.contains("no request-level effect"));
}

#[test]
fn resolution_uses_production_grounding_and_has_no_fallback() {
    let pair = prepare_direct_finalizer_pair(
        runtime(),
        config(),
        None,
        Some("Challenge unsupported claims once before delivery."),
    )
    .expect("matched pair");

    let parent = pair
        .parent
        .resolve(response("Amber is the supplied fact."))
        .expect("grounded parent response");
    let challenger = pair
        .challenger
        .resolve(response("Amber is the supplied fact."))
        .expect("grounded challenger response");
    assert!(!parent.used_fallback);
    assert!(!challenger.used_fallback);

    let invalid = prepare_direct_finalizer_pair(
        runtime(),
        config(),
        None,
        Some("Challenge unsupported claims once before delivery."),
    )
    .expect("invalid-response pair")
    .parent
    .resolve(response(""))
    .expect_err("empty output cannot be rescued by a fallback");
    assert!(invalid.to_string().contains("no verified fallback"));
}
