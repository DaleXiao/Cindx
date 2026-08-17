use agent_core::{Message, MessageRole, ModelRole};
use agent_runtime::{
    sanitize_assistant_content, AgentFailure, AgentKernel, AgentRunControl,
    AgentTurnPreparationError,
};
use model_provider::ModelResponse;

#[path = "agent_direct_finalizer_policy.rs"]
pub(crate) mod direct_finalizer_policy;
#[path = "agent_terminal_finalizer_runtime.rs"]
pub(crate) mod terminal_runtime;

#[derive(Debug, Clone)]
pub(crate) struct GroundedFinalizerCandidate {
    pub(crate) content: String,
    pub(crate) receipt: agent_runtime::GroundedCompletionReceipt,
    pub(crate) already_persisted: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct FinalizerResolution {
    pub(crate) candidate: GroundedFinalizerCandidate,
    pub(crate) used_fallback: bool,
}

fn assistant_tool_carrier(message: &Message) -> bool {
    message.role == MessageRole::Assistant
        && (message
            .metadata
            .get("tool_call_count")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or_default()
            > 0
            || message.metadata.contains_key("tool_call_ids"))
}

fn candidate_is_tool_carrier(runtime: &agent_runtime::AgentLoopState, content: &str) -> bool {
    runtime
        .messages
        .iter()
        .any(|message| assistant_tool_carrier(message) && message.content.trim() == content.trim())
}

const LAST_RESORT_VISIBLE_ANSWER_MIN_CHARS: usize = 160;

fn last_resort_visible_answer(runtime: &agent_runtime::AgentLoopState) -> Option<String> {
    let content = runtime
        .messages
        .iter()
        .rev()
        .find(|message| {
            message.role == MessageRole::Assistant
                && message.metadata.get("internal").map(String::as_str) != Some("true")
        })
        .map(|message| message.content.trim().to_string())?;
    (content.chars().count() >= LAST_RESORT_VISIBLE_ANSWER_MIN_CHARS).then_some(content)
}

pub(crate) fn prepare_finalizer_turn(
    runtime: &mut agent_runtime::AgentLoopState,
    system_prompt: Option<&str>,
    direct_finalizer_directive: Option<&str>,
    runtime_context: Option<&str>,
    context_window_tokens: u64,
    max_output_tokens: u64,
) -> Result<agent_runtime::PreparedAgentTurn, AgentTurnPreparationError> {
    let augmented_system_prompt = direct_finalizer_directive.map(|directive| {
        let base = system_prompt.unwrap_or_default().trim();
        if base.is_empty() {
            directive.to_string()
        } else {
            format!("{base}\n\nDirect finalizer policy:\n{directive}")
        }
    });
    let mut prepared = AgentKernel::new(runtime, &[]).prepare_finalizer_turn(
        augmented_system_prompt.as_deref().or(system_prompt),
        runtime_context,
        context_window_tokens,
        max_output_tokens,
    )?;
    prepared.request.role = ModelRole::Summarizer;
    prepared.request.tools.clear();
    prepared
        .request
        .metadata
        .insert("execution_role".to_string(), "finalizer".to_string());
    Ok(prepared)
}

pub(crate) fn grounded_finalizer_fallback(
    runtime: &agent_runtime::AgentLoopState,
    cancellation: &AgentRunControl,
    steer_epoch: u64,
    visible_evidence_sequences: &[u64],
) -> Option<GroundedFinalizerCandidate> {
    if !cancellation.objective_epoch_is_current(steer_epoch) {
        return None;
    }

    let mut candidates = Vec::<String>::new();
    if let Some(best) = cancellation
        .best_known_result()
        .filter(|candidate| candidate.deliverable)
        .filter(|candidate| !candidate_is_tool_carrier(runtime, &candidate.content))
    {
        candidates.push(best.content);
    }
    let partial = cancellation.partial_output();
    if !partial.trim().is_empty() && !candidate_is_tool_carrier(runtime, &partial) {
        candidates.push(partial);
    }
    // Last resort: when the run already holds visible tool evidence but no
    // deliverable candidate, the latest substantive visible assistant text is
    // better than failing the run at the delivery gate.
    if candidates.is_empty() && !visible_evidence_sequences.is_empty() {
        if let Some(last_resort) = last_resort_visible_answer(runtime) {
            candidates.push(last_resort);
        }
    }

    let mut seen = std::collections::BTreeSet::new();
    candidates.into_iter().find_map(|content| {
        let content = content.trim().to_string();
        if content.is_empty() || !seen.insert(content.clone()) {
            return None;
        }
        runtime
            .task_contract
            .grounded_completion_receipt(
                steer_epoch,
                runtime.turn,
                &content,
                visible_evidence_sequences,
            )
            .ok()
            .map(|receipt| GroundedFinalizerCandidate {
                content,
                receipt,
                already_persisted: false,
            })
    })
}

pub(crate) fn resolve_finalizer_response(
    runtime: &mut agent_runtime::AgentLoopState,
    response: ModelResponse,
    fallback: Option<GroundedFinalizerCandidate>,
    steer_epoch: u64,
    visible_evidence_sequences: &[u64],
) -> Result<FinalizerResolution, AgentFailure> {
    let assessment = response.assessment();
    let content = sanitize_assistant_content(&response.message.content);
    let usable = response.tool_calls.is_empty()
        && assessment.disposition == model_provider::ModelResponseDisposition::Usable
        && !content.trim().is_empty();
    if usable {
        match AgentKernel::new(runtime, &[]).decide_grounded_completion(
            steer_epoch,
            &content,
            visible_evidence_sequences,
        ) {
            Ok(agent_runtime::GroundedCompletionDecision::Deliver(receipt)) => {
                return Ok(FinalizerResolution {
                    candidate: GroundedFinalizerCandidate {
                        content,
                        receipt,
                        already_persisted: false,
                    },
                    used_fallback: false,
                });
            }
            Ok(agent_runtime::GroundedCompletionDecision::Repair(_)) | Err(_) => {}
        }
    }

    if fallback.is_some() {
        return resolve_finalizer_fallback(
            runtime,
            fallback,
            steer_epoch,
            visible_evidence_sequences,
        );
    }

    Err(AgentFailure::model_output(
        "finalizer_no_grounded_candidate",
        "the finalizer returned no usable grounded answer and no verified fallback was available",
    ))
}

pub(crate) fn resolve_finalizer_fallback(
    runtime: &agent_runtime::AgentLoopState,
    fallback: Option<GroundedFinalizerCandidate>,
    steer_epoch: u64,
    visible_evidence_sequences: &[u64],
) -> Result<FinalizerResolution, AgentFailure> {
    let fallback = fallback.ok_or_else(|| {
        AgentFailure::model_output(
            "finalizer_no_grounded_candidate",
            "no verified fallback was available for final delivery",
        )
    })?;
    let receipt = runtime
        .task_contract
        .rebind_grounded_completion_receipt(
            &fallback.receipt,
            steer_epoch,
            runtime.turn,
            &fallback.content,
            visible_evidence_sequences,
        )
        .map_err(|_| {
            AgentFailure::contract(
                "finalizer_fallback_lineage_invalid",
                "the finalizer fallback no longer matches the active task contract",
            )
        })?;
    Ok(FinalizerResolution {
        candidate: GroundedFinalizerCandidate {
            receipt,
            ..fallback
        },
        used_fallback: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{Message, Metadata, TaskId};
    use agent_runtime::ResultQuality;
    use model_provider::ModelToolCall;

    fn runtime() -> agent_runtime::AgentLoopState {
        agent_runtime::start_agent_loop(
            TaskId("finalizer-test".to_string()),
            "answer carefully",
            agent_runtime::AgentRuntimeConfig::default(),
        )
    }

    fn response(content: &str, tool_calls: Vec<ModelToolCall>) -> ModelResponse {
        ModelResponse {
            message: Message {
                role: MessageRole::Assistant,
                content: content.to_string(),
                metadata: Metadata::new(),
            },
            raw_tool_calls_json: None,
            tool_calls,
            metadata: Metadata::new(),
        }
    }

    fn fallback(runtime: &agent_runtime::AgentLoopState) -> GroundedFinalizerCandidate {
        let content = "verified actor fallback".to_string();
        let receipt = runtime
            .task_contract
            .grounded_completion_receipt(0, runtime.turn, &content, &[])
            .expect("self-contained fallback should be grounded");
        GroundedFinalizerCandidate {
            content,
            receipt,
            already_persisted: true,
        }
    }

    #[test]
    fn finalizer_request_is_toolless_and_does_not_advance_the_actor_turn() {
        let mut runtime = runtime();
        let before = runtime.turn;
        let prepared = prepare_finalizer_turn(&mut runtime, None, None, None, 8_192, 1_024)
            .expect("finalizer request should prepare");
        assert!(prepared.request.tools.is_empty());
        assert_eq!(prepared.request.role, ModelRole::Summarizer);
        assert_eq!(runtime.turn, before);
    }

    #[test]
    fn direct_policy_changes_only_the_toolless_finalizer_prompt() {
        let mut baseline_runtime = runtime();
        let mut candidate_runtime = baseline_runtime.clone();
        let baseline = prepare_finalizer_turn(
            &mut baseline_runtime,
            Some("base system prompt"),
            None,
            None,
            8_192,
            1_024,
        )
        .expect("baseline finalizer should prepare");
        let directive = "challenge visible evidence before delivery";
        let candidate = prepare_finalizer_turn(
            &mut candidate_runtime,
            Some("base system prompt"),
            Some(directive),
            None,
            8_192,
            1_024,
        )
        .expect("candidate finalizer should prepare");

        assert_eq!(baseline.request.role, candidate.request.role);
        assert!(baseline.request.tools.is_empty());
        assert!(candidate.request.tools.is_empty());
        assert_eq!(baseline_runtime.turn, candidate_runtime.turn);
        assert!(!baseline
            .request
            .messages
            .iter()
            .any(|message| message.content.contains(directive)));
        assert!(candidate
            .request
            .messages
            .iter()
            .any(|message| message.content.contains(directive)));
    }

    #[test]
    fn internal_drafts_are_never_treated_as_already_visible_fallbacks() {
        let mut draft_runtime = runtime();
        draft_runtime.messages.push(Message {
            role: MessageRole::Assistant,
            content: "internal draft".to_string(),
            metadata: [("internal".to_string(), "true".to_string())]
                .into_iter()
                .collect(),
        });
        let control = AgentRunControl::new("fast");
        assert!(control.record_partial_output_at(0, "internal draft"));

        let fallback = grounded_finalizer_fallback(&draft_runtime, &control, 0, &[])
            .expect("the grounded partial can still be delivered through a visible terminal write");
        assert!(!fallback.already_persisted);

        let without_partial = AgentRunControl::new("fast");
        without_partial.record_best_known_result_at(
            0,
            "reviewer",
            "internal-only guidance",
            ResultQuality::Verified,
            1,
            true,
            false,
        );
        assert!(grounded_finalizer_fallback(&runtime(), &without_partial, 0, &[]).is_none());

        let mut tool_runtime = runtime();
        tool_runtime.messages.push(Message {
            role: MessageRole::Assistant,
            content: "I will inspect that now".to_string(),
            metadata: [("tool_call_count".to_string(), "1".to_string())]
                .into_iter()
                .collect(),
        });
        let tool_partial = AgentRunControl::new("fast");
        assert!(tool_partial.record_partial_output_at(0, "I will inspect that now"));
        assert!(grounded_finalizer_fallback(&tool_runtime, &tool_partial, 0, &[]).is_none());
    }

    #[test]
    fn last_resort_delivers_substantive_visible_text_only_with_evidence_and_length() {
        let long_text = format!(
            "Cindx 的配置目录里没有 MCP 配置文件，state.sqlite3 里有 events 与 permission_requests 等表，但没有 MCP 相关的表；projects.conf 里提到 workiq MCP 可用性检测。{}",
            "可见证据与检查步骤补充。".repeat(8)
        );
        assert!(long_text.chars().count() >= super::LAST_RESORT_VISIBLE_ANSWER_MIN_CHARS);
        let mut long_runtime = runtime();
        long_runtime.messages.push(Message {
            role: MessageRole::Assistant,
            content: long_text.clone(),
            metadata: [("tool_call_count".to_string(), "1".to_string())]
                .into_iter()
                .collect(),
        });
        let fallback =
            grounded_finalizer_fallback(&long_runtime, &AgentRunControl::new("fast"), 0, &[7])
                .expect("substantive visible text with visible evidence should deliver");
        assert_eq!(fallback.content, long_text.trim().to_string());

        let mut short_runtime = runtime();
        short_runtime.messages.push(Message {
            role: MessageRole::Assistant,
            content: "让我重新启动并立即轮询".to_string(),
            metadata: [("tool_call_count".to_string(), "1".to_string())]
                .into_iter()
                .collect(),
        });
        assert!(
            grounded_finalizer_fallback(&short_runtime, &AgentRunControl::new("fast"), 0, &[7])
                .is_none(),
            "short progress notes must not be delivered as answers"
        );
        assert!(
            grounded_finalizer_fallback(&long_runtime, &AgentRunControl::new("fast"), 0, &[])
                .is_none(),
            "without visible tool evidence the last resort stays closed"
        );
    }

    #[test]
    fn fallback_ignores_unlineaged_history_and_requires_the_current_control_epoch() {
        let mut historical = runtime();
        historical.messages.push(Message {
            role: MessageRole::Assistant,
            content: "answer from an older run".to_string(),
            metadata: Metadata::new(),
        });
        assert!(
            grounded_finalizer_fallback(&historical, &AgentRunControl::new("fast"), 0, &[],)
                .is_none()
        );

        let stale_control = AgentRunControl::new("fast");
        assert!(stale_control.record_partial_output_at(0, "stale epoch output"));
        assert!(grounded_finalizer_fallback(&runtime(), &stale_control, 1, &[]).is_none());
    }

    #[test]
    fn current_epoch_control_candidates_are_grounded_without_reusing_history_visibility() {
        let mut current = runtime();
        current.messages.push(Message {
            role: MessageRole::Assistant,
            content: "current grounded answer".to_string(),
            metadata: Metadata::new(),
        });
        let control = AgentRunControl::new("fast");
        assert!(control.record_best_known_result_at(
            0,
            "actor",
            "current grounded answer",
            ResultQuality::Grounded,
            0,
            false,
            true,
        ));

        let fallback = grounded_finalizer_fallback(&current, &control, 0, &[])
            .expect("current-epoch deliverable control result should be grounded");
        assert_eq!(fallback.content, "current grounded answer");
        assert!(!fallback.already_persisted);
        assert_eq!(fallback.receipt.steer_epoch, 0);
    }

    #[test]
    fn unusable_finalizer_protocols_return_the_byte_identical_grounded_fallback() {
        let cases = [
            response("", Vec::new()),
            response(
                "must not be delivered",
                vec![ModelToolCall {
                    id: "call-1".to_string(),
                    name: "file.write".to_string(),
                    arguments_json: "{}".to_string(),
                }],
            ),
        ];
        for response in cases {
            let mut runtime = runtime();
            let fallback = fallback(&runtime);
            let resolution =
                resolve_finalizer_response(&mut runtime, response, Some(fallback.clone()), 0, &[])
                    .expect("grounded fallback should be retained");
            assert!(resolution.used_fallback);
            assert_eq!(resolution.candidate.content, fallback.content);
            assert_eq!(resolution.candidate.receipt, fallback.receipt);
        }
    }

    #[test]
    fn stale_or_tampered_fallback_fails_closed() {
        let mut runtime = runtime();
        let mut stale = fallback(&runtime);
        stale.receipt.steer_epoch = 1;
        let error =
            resolve_finalizer_response(&mut runtime, response("", Vec::new()), Some(stale), 0, &[])
                .expect_err("stale fallback must not enter final delivery");
        assert_eq!(error.code, "finalizer_fallback_lineage_invalid");
    }
}
