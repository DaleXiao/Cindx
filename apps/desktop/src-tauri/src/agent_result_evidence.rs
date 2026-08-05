use agent_core::{Event, MessageRole, Metadata, ToolOutcomeStatus, ToolRisk, ToolSource, ToolSpec};
use agent_runtime::{AgentGoalDelta, AgentLoopState, AgentToolRequest, ContractEvidenceKind};
use orchestrator::{
    IndependentQualitySource, LearningAttribution, LearningEvidenceV1, LearningTermination,
    LearningUsageCompleteness,
};
use std::collections::BTreeMap;

const TOOL_EVIDENCE_SCHEMA: &str = "cindx.tool_evidence.v1";
const TOOL_EVIDENCE_PROVENANCE: &str = "runtime_dispatch";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CompletionToolEvidence {
    pub(crate) grounded_count: usize,
    pub(crate) verified_postcondition_count: usize,
    pub(crate) trusted_contract_sequences: Vec<u64>,
}

pub(crate) fn annotate_latest_tool_observation(
    runtime: &mut AgentLoopState,
    tools: &[ToolSpec],
    call: &AgentToolRequest,
    status: &ToolOutcomeStatus,
    risk: Option<&ToolRisk>,
    steer_epoch: u64,
    postcondition_verified: bool,
    goal_delta: Option<&AgentGoalDelta>,
) {
    let source = tools
        .iter()
        .find(|tool| tool.name == call.tool_name)
        .map(|tool| tool_source_label(&tool.source))
        .unwrap_or("unknown");
    let contract_evidence_sequence = runtime
        .task_contract
        .evidence()
        .last()
        .filter(|evidence| evidence.source == call.tool_name)
        .map(|evidence| evidence.sequence.to_string());
    let Some(message) = runtime
        .messages
        .last_mut()
        .filter(|message| message.role == MessageRole::Tool)
    else {
        return;
    };
    message.metadata.extend([
        (
            "tool_evidence_schema".to_string(),
            TOOL_EVIDENCE_SCHEMA.to_string(),
        ),
        (
            "tool_evidence_provenance".to_string(),
            TOOL_EVIDENCE_PROVENANCE.to_string(),
        ),
        ("tool_name".to_string(), call.tool_name.clone()),
        (
            "tool_status".to_string(),
            tool_status_label(status).to_string(),
        ),
        ("tool_source".to_string(), source.to_string()),
        (
            "tool_risk".to_string(),
            risk.map(tool_risk_label).unwrap_or("unknown").to_string(),
        ),
        ("steer_epoch".to_string(), steer_epoch.to_string()),
        (
            "postcondition_verified".to_string(),
            postcondition_verified.to_string(),
        ),
    ]);
    if let Some(sequence) = contract_evidence_sequence {
        message
            .metadata
            .insert("contract_evidence_sequence".to_string(), sequence);
    }
    if let Some(delta) = goal_delta {
        message.metadata.extend([
            ("goal_delta_schema".to_string(), delta.schema().to_string()),
            ("goal_delta_kinds".to_string(), delta.kind_labels()),
            (
                "goal_delta_fingerprint".to_string(),
                delta.fingerprint().to_string(),
            ),
        ]);
    }
}

pub(crate) fn completion_tool_evidence(
    runtime: &AgentLoopState,
    steer_epoch: u64,
) -> CompletionToolEvidence {
    let successful_evidence = runtime
        .messages
        .iter()
        .filter(|message| trusted_successful_tool_message(message, steer_epoch))
        .filter_map(|message| {
            let key = (
                message.metadata.get("tool_name")?.clone(),
                message
                    .metadata
                    .get("contract_evidence_sequence")?
                    .parse::<u64>()
                    .ok()?,
            );
            let postcondition_verified = message
                .metadata
                .get("postcondition_verified")
                .is_some_and(|value| value == "true");
            Some((key, postcondition_verified))
        })
        .collect::<BTreeMap<_, _>>();
    if successful_evidence.is_empty() {
        return CompletionToolEvidence::default();
    }

    let trusted_contract_evidence = runtime
        .task_contract
        .evidence()
        .iter()
        .filter(|evidence| {
            successful_evidence.contains_key(&(evidence.source.clone(), evidence.sequence))
        })
        .collect::<Vec<_>>();
    let verified_postcondition_count = trusted_contract_evidence
        .iter()
        .filter(|evidence| match evidence.kind {
            ContractEvidenceKind::Verification => {
                runtime.successful_mutations() > 0 && runtime.verified_after_last_mutation()
            }
            ContractEvidenceKind::InteractionObservation => {
                successful_evidence
                    .get(&(evidence.source.clone(), evidence.sequence))
                    .copied()
                    == Some(true)
            }
            _ => false,
        })
        .count();

    CompletionToolEvidence {
        grounded_count: successful_evidence.len(),
        verified_postcondition_count,
        trusted_contract_sequences: trusted_contract_evidence
            .into_iter()
            .map(|evidence| evidence.sequence)
            .collect(),
    }
}

pub(crate) fn completion_learning_evidence(
    run_context: &Metadata,
    usage: LearningUsageCompleteness,
    steer_epoch: u64,
    tool_evidence: CompletionToolEvidence,
    workflow_terminal: Option<&Event>,
) -> LearningEvidenceV1 {
    let budget_fingerprint =
        crate::routing_learning_runtime::learning_budget_fingerprint(run_context);
    let censored = || {
        LearningEvidenceV1::censored(
            LearningTermination::Completed,
            LearningAttribution::Unknown,
            usage,
            Some(steer_epoch),
            budget_fingerprint.clone(),
        )
    };
    if usage == LearningUsageCompleteness::Missing || budget_fingerprint.is_none() {
        return censored();
    }
    let budget_fingerprint = budget_fingerprint
        .clone()
        .expect("checked learning budget fingerprint");

    if let Some(workflow) = workflow_terminal {
        if workflow
            .metadata
            .get("steer_epoch")
            .and_then(|value| value.parse::<u64>().ok())
            != Some(steer_epoch)
        {
            return censored();
        }
        if workflow
            .metadata
            .contains_key("anytime_routing_learning_eligible")
        {
            if workflow
                .metadata
                .get("anytime_routing_learning_eligible")
                .map(String::as_str)
                != Some("true")
            {
                return censored();
            }
            let Some(verified) = workflow
                .metadata
                .get("anytime_selected_verified")
                .and_then(|value| value.parse::<bool>().ok())
            else {
                return censored();
            };
            let Some(quality_bps) = workflow
                .metadata
                .get("anytime_selected_quality_bps")
                .and_then(|value| value.parse::<u16>().ok())
                .filter(|quality| *quality <= 10_000)
            else {
                return censored();
            };
            let Some(quality_pass) = workflow
                .metadata
                .get("quality_pass")
                .and_then(|value| value.parse::<bool>().ok())
            else {
                return censored();
            };
            let Some(safety_violations) = workflow
                .metadata
                .get("safety_violations")
                .and_then(|value| value.parse::<usize>().ok())
            else {
                return censored();
            };
            let safety_clear = safety_violations == 0;
            return LearningEvidenceV1::independent_quality(
                LearningTermination::Completed,
                LearningAttribution::Workflow,
                usage,
                steer_epoch,
                budget_fingerprint,
                IndependentQualitySource::AnytimeSelector,
                quality_bps,
                verified && quality_pass && safety_clear,
            );
        }

        if let (Some(passed), Some(quality_bps), Some(safety_violations)) = (
            workflow
                .metadata
                .get("quality_pass")
                .and_then(|value| value.parse::<bool>().ok()),
            workflow
                .metadata
                .get("quality_score")
                .and_then(|value| quality_score_bps(value)),
            workflow
                .metadata
                .get("safety_violations")
                .and_then(|value| value.parse::<usize>().ok()),
        ) {
            return LearningEvidenceV1::independent_quality(
                LearningTermination::Completed,
                LearningAttribution::Workflow,
                usage,
                steer_epoch,
                budget_fingerprint,
                IndependentQualitySource::CollaborationQualityGate,
                quality_bps,
                passed && safety_violations == 0,
            );
        }
    }

    if tool_evidence.verified_postcondition_count > 0 {
        return LearningEvidenceV1::verified_postcondition(usage, steer_epoch, budget_fingerprint);
    }
    censored()
}

fn quality_score_bps(value: &str) -> Option<u16> {
    let score = value.parse::<f64>().ok()?;
    if !score.is_finite() || !(0.0..=1.0).contains(&score) {
        return None;
    }
    Some((score * 10_000.0).round() as u16)
}

fn trusted_successful_tool_message(message: &agent_core::Message, steer_epoch: u64) -> bool {
    message.role == MessageRole::Tool
        && message
            .metadata
            .get("tool_evidence_schema")
            .map(String::as_str)
            == Some(TOOL_EVIDENCE_SCHEMA)
        && message
            .metadata
            .get("tool_evidence_provenance")
            .map(String::as_str)
            == Some(TOOL_EVIDENCE_PROVENANCE)
        && message.metadata.get("tool_status").map(String::as_str) == Some("succeeded")
        && message
            .metadata
            .get("tool_source")
            .is_some_and(|source| source != "unknown")
        && message
            .metadata
            .get("steer_epoch")
            .and_then(|value| value.parse::<u64>().ok())
            == Some(steer_epoch)
        && message
            .metadata
            .get("synthetic")
            .is_none_or(|value| value != "true")
}

fn tool_status_label(status: &ToolOutcomeStatus) -> &'static str {
    match status {
        ToolOutcomeStatus::Succeeded => "succeeded",
        ToolOutcomeStatus::Failed => "failed",
        ToolOutcomeStatus::Cancelled => "cancelled",
        ToolOutcomeStatus::Denied => "denied",
    }
}

fn tool_source_label(source: &ToolSource) -> &'static str {
    match source {
        ToolSource::BuiltIn => "built_in",
        ToolSource::Mcp { .. } => "mcp",
        ToolSource::Skill { .. } => "skill",
    }
}

fn tool_risk_label(risk: &ToolRisk) -> &'static str {
    match risk {
        ToolRisk::ReadOnly => "read_only",
        ToolRisk::WritesWorkspace => "writes_workspace",
        ToolRisk::ExecutesProcess => "executes_process",
        ToolRisk::UsesNetwork => "uses_network",
        ToolRisk::SensitiveContext => "sensitive_context",
        ToolRisk::Destructive => "destructive",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{EventId, EventKind, Message, Metadata, TaskId};
    use agent_runtime::{AgentRunControl, ModelAttemptUsage, ModelUsageSource, RunStageClass};
    use orchestrator::LearningDisposition;

    fn tool_message<const N: usize>(metadata: [(&str, &str); N]) -> Message {
        Message {
            role: MessageRole::Tool,
            content: "tool=file.read\nstatus=succeeded".to_string(),
            metadata: metadata
                .into_iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect::<Metadata>(),
        }
    }

    fn runtime_with_messages(prompt: &str, messages: Vec<Message>) -> AgentLoopState {
        let mut runtime = agent_runtime::start_agent_loop(
            TaskId("task".to_string()),
            prompt,
            agent_runtime::AgentRuntimeConfig { max_turns: 1 },
        );
        runtime.messages = messages;
        runtime
    }

    #[test]
    fn trusted_tool_evidence_requires_success_provenance_and_current_epoch() {
        let valid = tool_message([
            ("tool_evidence_schema", TOOL_EVIDENCE_SCHEMA),
            ("tool_evidence_provenance", TOOL_EVIDENCE_PROVENANCE),
            ("tool_status", "succeeded"),
            ("steer_epoch", "7"),
            ("tool_name", "file.read"),
            ("tool_source", "built_in"),
        ]);
        assert!(trusted_successful_tool_message(&valid, 7));
        assert!(!trusted_successful_tool_message(&valid, 8));

        let mut failed = valid.clone();
        failed
            .metadata
            .insert("tool_status".to_string(), "failed".to_string());
        assert!(!trusted_successful_tool_message(&failed, 7));

        let legacy = Message {
            role: MessageRole::Tool,
            content: "tool=file.read\nstatus=succeeded".to_string(),
            metadata: Metadata::new(),
        };
        assert!(!trusted_successful_tool_message(&legacy, 7));
    }

    #[test]
    fn unverified_read_only_tool_is_grounding_not_verification() {
        let mut runtime = runtime_with_messages(
            "inspect",
            vec![tool_message([
                ("tool_evidence_schema", TOOL_EVIDENCE_SCHEMA),
                ("tool_evidence_provenance", TOOL_EVIDENCE_PROVENANCE),
                ("tool_status", "succeeded"),
                ("steer_epoch", "3"),
                ("tool_name", "file.read"),
                ("tool_source", "built_in"),
                ("contract_evidence_sequence", "1"),
            ])],
        );
        let evidence = completion_tool_evidence(&runtime, 3);
        assert_eq!(evidence.grounded_count, 1);
        assert_eq!(evidence.verified_postcondition_count, 0);

        runtime.messages[0]
            .metadata
            .insert("synthetic".to_string(), "true".to_string());
        assert_eq!(
            completion_tool_evidence(&runtime, 3),
            CompletionToolEvidence::default()
        );
    }

    #[test]
    fn matching_runtime_postcondition_is_verified() {
        let mut runtime = runtime_with_messages("change and test", Vec::new());
        agent_runtime::record_tool_outcome_with_risk(
            &mut runtime,
            "file.write",
            r#"{"path":"src/lib.rs"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::WritesWorkspace),
        );
        agent_runtime::record_tool_outcome_with_risk(
            &mut runtime,
            "process.run",
            r#"{"command":"cargo test"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ExecutesProcess),
        );
        runtime.messages.push(tool_message([
            ("tool_evidence_schema", TOOL_EVIDENCE_SCHEMA),
            ("tool_evidence_provenance", TOOL_EVIDENCE_PROVENANCE),
            ("tool_status", "succeeded"),
            ("steer_epoch", "5"),
            ("tool_name", "file.write"),
            ("tool_source", "built_in"),
            ("contract_evidence_sequence", "1"),
        ]));
        runtime.messages.push(tool_message([
            ("tool_evidence_schema", TOOL_EVIDENCE_SCHEMA),
            ("tool_evidence_provenance", TOOL_EVIDENCE_PROVENANCE),
            ("tool_status", "succeeded"),
            ("steer_epoch", "5"),
            ("tool_name", "process.run"),
            ("tool_source", "built_in"),
            ("contract_evidence_sequence", "2"),
        ]));

        let evidence = completion_tool_evidence(&runtime, 5);
        assert_eq!(evidence.grounded_count, 2);
        assert_eq!(evidence.verified_postcondition_count, 1);
    }

    #[test]
    fn interaction_observation_only_verifies_the_call_that_closed_a_pending_action() {
        let mut runtime = runtime_with_messages("interact", Vec::new());
        agent_runtime::record_tool_outcome_with_risk(
            &mut runtime,
            "browser.click",
            r#"{"role":"button","name":"Submit"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::UsesNetwork),
        );
        agent_runtime::record_tool_outcome_with_risk(
            &mut runtime,
            "browser.capture",
            "{}",
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::UsesNetwork),
        );
        assert_eq!(runtime.verified_interactions, 1);
        agent_runtime::record_tool_outcome_with_risk(
            &mut runtime,
            "browser.capture",
            "{}",
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::UsesNetwork),
        );
        runtime.messages.push(tool_message([
            ("tool_evidence_schema", TOOL_EVIDENCE_SCHEMA),
            ("tool_evidence_provenance", TOOL_EVIDENCE_PROVENANCE),
            ("tool_status", "succeeded"),
            ("steer_epoch", "2"),
            ("tool_name", "browser.capture"),
            ("tool_source", "built_in"),
            ("contract_evidence_sequence", "3"),
            ("postcondition_verified", "false"),
        ]));

        let evidence = completion_tool_evidence(&runtime, 2);
        assert_eq!(evidence.grounded_count, 1);
        assert_eq!(evidence.verified_postcondition_count, 0);
    }

    #[test]
    fn same_tool_name_cannot_reuse_verification_from_an_older_sequence() {
        let mut runtime = runtime_with_messages("change and verify", Vec::new());
        for (tool, input, risk) in [
            (
                "file.write",
                r#"{"path":"src/lib.rs"}"#,
                ToolRisk::WritesWorkspace,
            ),
            (
                "process.run",
                r#"{"command":"cargo test"}"#,
                ToolRisk::ExecutesProcess,
            ),
            (
                "process.run",
                r#"{"command":"echo done"}"#,
                ToolRisk::ExecutesProcess,
            ),
        ] {
            agent_runtime::record_tool_outcome_with_risk(
                &mut runtime,
                tool,
                input,
                &ToolOutcomeStatus::Succeeded,
                Some(&risk),
            );
        }
        runtime.messages.push(tool_message([
            ("tool_evidence_schema", TOOL_EVIDENCE_SCHEMA),
            ("tool_evidence_provenance", TOOL_EVIDENCE_PROVENANCE),
            ("tool_status", "succeeded"),
            ("steer_epoch", "2"),
            ("tool_name", "process.run"),
            ("tool_source", "built_in"),
            ("contract_evidence_sequence", "3"),
        ]));

        let evidence = completion_tool_evidence(&runtime, 2);
        assert_eq!(evidence.grounded_count, 1);
        assert_eq!(evidence.verified_postcondition_count, 0);
    }

    #[test]
    fn workflow_quality_from_an_old_steer_epoch_is_censored() {
        let control = AgentRunControl::new("fast");
        let attempt = control
            .begin_physical_model_attempt("model", 10, 0, RunStageClass::Finalizer)
            .unwrap();
        assert!(control.finish_physical_model_attempt(
            attempt,
            Some(ModelAttemptUsage::new(
                10,
                0,
                10,
                ModelUsageSource::Provider,
            )),
        ));
        let mut run_context = Metadata::new();
        for (index, key) in crate::learning_evidence_runtime::LEARNING_BUDGET_KEYS
            .into_iter()
            .enumerate()
        {
            run_context.insert(key.to_string(), (index + 1).to_string());
        }
        let workflow = Event {
            id: EventId("workflow".to_string()),
            task_id: TaskId("task".to_string()),
            sequence: 1,
            timestamp_ms: 1,
            kind: EventKind::TaskStatusChanged,
            summary: "Collaboration workflow completed".to_string(),
            metadata: [
                ("steer_epoch".to_string(), "1".to_string()),
                ("quality_pass".to_string(), "true".to_string()),
                ("quality_score".to_string(), "0.95".to_string()),
                ("safety_violations".to_string(), "0".to_string()),
            ]
            .into_iter()
            .collect(),
        };

        let evidence = completion_learning_evidence(
            &run_context,
            crate::model_resource_runtime::learning_usage_completeness(&control.resource_usage()),
            2,
            CompletionToolEvidence::default(),
            Some(&workflow),
        );
        assert_eq!(evidence.disposition, LearningDisposition::Censored);
    }

    #[test]
    fn anytime_quality_with_missing_or_invalid_gate_fields_is_censored() {
        let mut run_context = Metadata::new();
        for (index, key) in crate::learning_evidence_runtime::LEARNING_BUDGET_KEYS
            .into_iter()
            .enumerate()
        {
            run_context.insert(key.to_string(), (index + 1).to_string());
        }

        for (field, replacement) in [
            ("quality_pass", None),
            ("quality_pass", Some("not-a-bool")),
            ("safety_violations", None),
            ("safety_violations", Some("not-a-count")),
        ] {
            let mut metadata = [
                ("steer_epoch".to_string(), "2".to_string()),
                (
                    "anytime_routing_learning_eligible".to_string(),
                    "true".to_string(),
                ),
                ("anytime_selected_verified".to_string(), "true".to_string()),
                (
                    "anytime_selected_quality_bps".to_string(),
                    "9500".to_string(),
                ),
                ("quality_pass".to_string(), "true".to_string()),
                ("safety_violations".to_string(), "0".to_string()),
            ]
            .into_iter()
            .collect::<Metadata>();
            if let Some(replacement) = replacement {
                metadata.insert(field.to_string(), replacement.to_string());
            } else {
                metadata.remove(field);
            }
            let workflow = Event {
                id: EventId("workflow".to_string()),
                task_id: TaskId("task".to_string()),
                sequence: 1,
                timestamp_ms: 1,
                kind: EventKind::TaskStatusChanged,
                summary: "Collaboration workflow completed".to_string(),
                metadata,
            };

            let evidence = completion_learning_evidence(
                &run_context,
                LearningUsageCompleteness::Complete,
                2,
                CompletionToolEvidence::default(),
                Some(&workflow),
            );
            assert_eq!(
                evidence.disposition,
                LearningDisposition::Censored,
                "field {field} with replacement {replacement:?} must fail closed"
            );
        }
    }
}
