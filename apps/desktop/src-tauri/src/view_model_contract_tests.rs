use crate::queue_service::{
    QueuedAgentMessageActionReceipt, QueuedAgentMessageReceipt, QueuedAgentMessageView,
};
use crate::view_models::{
    AgentAttachmentView, AgentRunBudgetView, AgentRunBudgetsView, AgentState, ChatMessageView,
    ModelStreamDelta, PermissionReviewItem, PermissionReviewState, ProjectSessionState,
    ProjectView, ProviderConfigInput, ProviderConfigState, RuntimeStatus, SessionView,
    TimelineEntry, ToolApprovalView,
};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const CONTRACT_SCHEMA: &str = "cindx.desktop-dto-contract.v1";
const CONTRACT_JSON: &str = include_str!("../../contracts/tauri-dto-v1.json");

fn serialized<T: Serialize>(value: T) -> Value {
    serde_json::to_value(value).expect("contract value should serialize")
}

fn queue_message(mode: &str, updated_at_ms: u64) -> QueuedAgentMessageView {
    QueuedAgentMessageView {
        id: "queue-a".to_string(),
        session_id: "session-a".to_string(),
        prompt: "Continue contract".to_string(),
        attachments: vec![AgentAttachmentView {
            id: "queue-attachment".to_string(),
            name: "queued.txt".to_string(),
            path: "/contract/queued.txt".to_string(),
            mime_type: "text/plain".to_string(),
            size_bytes: 6,
        }],
        effort: "default".to_string(),
        mode: mode.to_string(),
        plan_mode: false,
        created_at_ms: 53,
        updated_at_ms,
    }
}

fn provider_state(auth_verified_at_ms: Option<u64>) -> ProviderConfigState {
    let configured = auth_verified_at_ms.is_some();
    ProviderConfigState {
        provider_id: if configured { "openai" } else { "custom" }.to_string(),
        provider_resource: String::new(),
        base_url: if configured {
            "https://api.openai.com/v1"
        } else {
            "https://example.test/v1"
        }
        .to_string(),
        model: if configured {
            "gpt-contract"
        } else {
            "custom-contract"
        }
        .to_string(),
        conductor_model: if configured {
            "gpt-contract"
        } else {
            "custom-contract"
        }
        .to_string(),
        planner_model: if configured {
            "gpt-contract"
        } else {
            "custom-contract"
        }
        .to_string(),
        executor_model: if configured {
            "gpt-contract"
        } else {
            "custom-contract"
        }
        .to_string(),
        reviewer_model: if configured {
            "gpt-contract"
        } else {
            "custom-contract"
        }
        .to_string(),
        summarizer_model: if configured {
            "gpt-contract"
        } else {
            "custom-contract"
        }
        .to_string(),
        fast_model: String::new(),
        auto_model: String::new(),
        pro_model: String::new(),
        embedding_model: if configured {
            "text-embedding-contract"
        } else {
            "embedding-contract"
        }
        .to_string(),
        image_model: if configured { "image-contract" } else { "" }.to_string(),
        image_endpoint: String::new(),
        voice_model: if configured { "voice-contract" } else { "" }.to_string(),
        collaboration_policy: if configured { "auto_router" } else { "single" }.to_string(),
        direct_judge_fail_closed: false,
        context_window_tokens: if configured { 128_000 } else { 4_096 },
        agent_system_prompt: if configured { "Contract prompt" } else { "" }.to_string(),
        ready: configured,
        api_key_set: configured,
        auth_verified: configured,
        auth_verified_at_ms,
        enabled_models: Vec::new(),
    }
}

fn project_session_values() -> Vec<Value> {
    let project = ProjectView {
        id: "project-a".to_string(),
        name: "Project A".to_string(),
        root: "/contract/workspace".to_string(),
        detail: "Contract project".to_string(),
        status: "Ready".to_string(),
        active: true,
        created_at_ms: 10,
        updated_at_ms: 20,
    };
    let session = SessionView {
        id: "session-a".to_string(),
        project_id: "project-a".to_string(),
        name: "Session A".to_string(),
        title_state: "automatic".to_string(),
        detail: "Contract session".to_string(),
        effort: "default".to_string(),
        agent_model: String::new(),
        status: "Ready".to_string(),
        activity: "idle".to_string(),
        attention_reason: None,
        unseen_result: false,
        latest_sequence: 7,
        active: true,
        archived: false,
        archived_at_ms: None,
        created_at_ms: 10,
        updated_at_ms: 20,
    };
    let archived_session = SessionView {
        id: "session-b".to_string(),
        project_id: "project-a".to_string(),
        name: "Session B".to_string(),
        title_state: "manual".to_string(),
        detail: "Archived contract session".to_string(),
        effort: "high".to_string(),
        agent_model: String::new(),
        status: "Complete".to_string(),
        activity: "attention".to_string(),
        attention_reason: Some("Unread result".to_string()),
        unseen_result: true,
        latest_sequence: 8,
        active: false,
        archived: true,
        archived_at_ms: Some(21),
        created_at_ms: 11,
        updated_at_ms: 21,
    };
    vec![
        serialized(ProjectSessionState {
            projects: vec![project],
            sessions: vec![session, archived_session],
            active_project_id: "project-a".to_string(),
            active_session_id: "session-a".to_string(),
            last_error: None,
        }),
        serialized(ProjectSessionState {
            projects: Vec::new(),
            sessions: Vec::new(),
            active_project_id: String::new(),
            active_session_id: String::new(),
            last_error: Some("contract failure".to_string()),
        }),
    ]
}

fn permission_values() -> Vec<Value> {
    vec![serialized(PermissionReviewState {
        pending: vec![
            PermissionReviewItem {
                request_id: "permission-a".to_string(),
                action: "shell.run".to_string(),
                risk: "execute".to_string(),
                reason: "Contract permission".to_string(),
                scope: "/contract/workspace".to_string(),
                source: "agent".to_string(),
                project_id: Some("project-a".to_string()),
                project_name: Some("Project A".to_string()),
                session_id: Some("session-a".to_string()),
                session_name: Some("Session A".to_string()),
                input: "echo contract".to_string(),
                requested_at_ms: 30,
                can_allow_session: true,
            },
            PermissionReviewItem {
                request_id: "permission-b".to_string(),
                action: "file.write".to_string(),
                risk: "write".to_string(),
                reason: "Contract nullable context".to_string(),
                scope: ".".to_string(),
                source: "tool".to_string(),
                project_id: None,
                project_name: None,
                session_id: None,
                session_name: None,
                input: String::new(),
                requested_at_ms: 31,
                can_allow_session: false,
            },
        ],
    })]
}

fn agent_state_values() -> Vec<Value> {
    let attachment = AgentAttachmentView {
        id: "attachment-a".to_string(),
        name: "contract.txt".to_string(),
        path: "/contract/contract.txt".to_string(),
        mime_type: "text/plain".to_string(),
        size_bytes: 8,
    };
    let timeline = vec![
        TimelineEntry {
            sequence: 7,
            label: "Tool proposed".to_string(),
            detail: "shell.run".to_string(),
            kind: "tool".to_string(),
            tool_name: Some("shell.run".to_string()),
            state: "pending".to_string(),
            timestamp_ms: 51,
        },
        TimelineEntry {
            sequence: 8,
            label: "Permission requested".to_string(),
            detail: "Awaiting approval".to_string(),
            kind: "permission".to_string(),
            tool_name: None,
            state: "pending".to_string(),
            timestamp_ms: 52,
        },
        TimelineEntry {
            sequence: 9,
            label: "Workflow checkpoint".to_string(),
            detail: "Contract nullable workflow fields".to_string(),
            kind: "model".to_string(),
            tool_name: None,
            state: "idle".to_string(),
            timestamp_ms: 53,
        },
    ];
    let messages = vec![
        ChatMessageView {
            sequence: 1,
            role: "user".to_string(),
            content: "Run the contract".to_string(),
            timestamp_ms: 50,
            run_id: Some("run-a".to_string()),
            queue_id: Some("queue-a".to_string()),
            attachments: vec![attachment],
        },
        ChatMessageView {
            sequence: 2,
            role: "assistant".to_string(),
            content: "Waiting".to_string(),
            timestamp_ms: 51,
            run_id: None,
            queue_id: None,
            attachments: Vec::new(),
        },
    ];
    vec![
        serialized(AgentState {
            task_id: "phase-16-agent-loop".to_string(),
            project_id: Some("project-a".to_string()),
            project_name: Some("Project A".to_string()),
            session_id: Some("session-a".to_string()),
            session_name: Some("Session A".to_string()),
            status: "waiting_for_permission".to_string(),
            turn_count: 2,
            max_turns: 8,
            transcript_messages: 4,
            context_tokens_used: 1_024,
            context_window_tokens: 128_000,
            context_remaining_percent: 99.2,
            context_usage_estimated: false,
            run_started_at_ms: 50,
            run_budget_ms: 2_700_000,
            run_model_call_budget: 128,
            run_tool_call_budget: 256,
            can_cancel: true,
            can_retry: false,
            can_continue: false,
            event_count: 9,
            latest_sequence: 9,
            oldest_sequence: 1,
            has_older_history: false,
            timeline,
            messages,
            pending_approvals: vec![ToolApprovalView {
                request_id: "permission-a".to_string(),
                invocation_id: "invoke-a".to_string(),
                tool_name: "shell.run".to_string(),
                risk: "execute".to_string(),
                reason: "Contract permission".to_string(),
                scope: "/contract/workspace".to_string(),
                input: "echo contract".to_string(),
                requested_at_ms: 52,
                can_allow_session: true,
                subagent: false,
            }],
            queued_messages: vec![queue_message("queue", 53)],
            pending_plan_confirmation: None,
            latest_answer: Some("Waiting".to_string()),
            last_error: None,
        }),
        serialized(AgentState {
            task_id: "phase-16-agent-loop".to_string(),
            project_id: None,
            project_name: None,
            session_id: None,
            session_name: None,
            status: "idle".to_string(),
            turn_count: 0,
            max_turns: 0,
            transcript_messages: 0,
            context_tokens_used: 0,
            context_window_tokens: 0,
            context_remaining_percent: 100.0,
            context_usage_estimated: true,
            run_started_at_ms: 0,
            run_budget_ms: 0,
            run_model_call_budget: 0,
            run_tool_call_budget: 0,
            can_cancel: false,
            can_retry: false,
            can_continue: false,
            event_count: 0,
            latest_sequence: 0,
            oldest_sequence: 0,
            has_older_history: false,
            timeline: Vec::new(),
            messages: Vec::new(),
            pending_approvals: Vec::new(),
            queued_messages: Vec::new(),
            pending_plan_confirmation: None,
            latest_answer: None,
            last_error: Some("contract failure".to_string()),
        }),
    ]
}

fn output_contract_values() -> BTreeMap<&'static str, Vec<Value>> {
    let budgets = AgentRunBudgetsView {
        fast: AgentRunBudgetView {
            max_duration_ms: 300_000,
            max_model_calls: 24,
            max_tool_calls: 48,
        },
        default: AgentRunBudgetView {
            max_duration_ms: 2_700_000,
            max_model_calls: 128,
            max_tool_calls: 256,
        },
        high: AgentRunBudgetView {
            max_duration_ms: 14_400_000,
            max_model_calls: 384,
            max_tool_calls: 768,
        },
        xhigh: AgentRunBudgetView {
            max_duration_ms: 21_600_000,
            max_model_calls: 512,
            max_tool_calls: 1024,
        },
    };
    BTreeMap::from([
        (
            "RuntimeStatus",
            vec![serialized(RuntimeStatus {
                app_version: "0.1.contract".to_string(),
                kernel_status: "online".to_string(),
                provider_ready: true,
                workspace_root: "/contract/workspace".to_string(),
                orchestration_modes: vec!["single".to_string(), "auto_router".to_string()],
                registered_tools: vec!["file.read".to_string(), "shell.run".to_string()],
                agent_run_budgets: budgets,
            })],
        ),
        ("ProjectSessionState", project_session_values()),
        ("PermissionReviewState", permission_values()),
        (
            "ProviderConfigState",
            vec![
                serialized(provider_state(Some(40))),
                serialized(provider_state(None)),
            ],
        ),
        ("AgentState", agent_state_values()),
        (
            "ModelStreamDelta",
            vec![
                serialized(ModelStreamDelta {
                    task_id: "phase-16-agent-loop".to_string(),
                    request_id: "request-a".to_string(),
                    session_id: Some("session-a".to_string()),
                    delta: "contract".to_string(),
                    done: false,
                    reset: false,
                    error: None,
                }),
                serialized(ModelStreamDelta {
                    task_id: "phase-16-agent-loop".to_string(),
                    request_id: "request-b".to_string(),
                    session_id: None,
                    delta: String::new(),
                    done: true,
                    reset: true,
                    error: Some("contract failure".to_string()),
                }),
            ],
        ),
        (
            "QueuedAgentMessageReceipt",
            vec![serialized(QueuedAgentMessageReceipt {
                message: queue_message("queue", 53),
                event_count: 10,
                latest_sequence: 10,
                latest_timestamp_ms: 54,
            })],
        ),
        (
            "QueuedAgentMessageActionReceipt",
            vec![
                serialized(QueuedAgentMessageActionReceipt {
                    queue_id: "queue-a".to_string(),
                    message: Some(queue_message("steer", 55)),
                    event_count: 11,
                    latest_sequence: 11,
                    latest_timestamp_ms: 55,
                    cancelled_active_run: true,
                    steer_committed: true,
                }),
                serialized(QueuedAgentMessageActionReceipt {
                    queue_id: "queue-b".to_string(),
                    message: None,
                    event_count: 12,
                    latest_sequence: 12,
                    latest_timestamp_ms: 56,
                    cancelled_active_run: false,
                    steer_committed: false,
                }),
            ],
        ),
    ])
}

fn validate_provider_input(value: Value) {
    let input: ProviderConfigInput =
        serde_json::from_value(value).expect("ProviderConfigInput contract should deserialize");
    assert_eq!(input.provider_id, "azure_openai");
    assert_eq!(input.provider_resource, "contract-resource");
    assert_eq!(input.base_url, "https://contract-resource.openai.azure.com");
    assert_eq!(input.api_key, "contract-key");
    assert_eq!(input.model, "deployment-contract");
    assert_eq!(input.conductor_model, "deployment-contract");
    assert_eq!(input.planner_model, "deployment-contract");
    assert_eq!(input.executor_model, "deployment-contract");
    assert_eq!(input.reviewer_model, "deployment-contract");
    assert_eq!(input.summarizer_model, "deployment-contract");
    assert_eq!(input.embedding_model, "embedding-contract");
    assert_eq!(input.image_model, "image-contract");
    assert_eq!(input.image_endpoint, "");
    assert_eq!(input.voice_model, "voice-contract");
    assert_eq!(input.collaboration_policy, "auto_router");
    assert!(input.direct_judge_fail_closed);
    assert_eq!(input.context_window_tokens, 64_000);
    assert_eq!(input.agent_system_prompt, "Contract prompt");
}

#[test]
fn desktop_dto_contract_matches_committed_wire_values() {
    let contract: Value = serde_json::from_str(CONTRACT_JSON).expect("contract JSON should parse");
    assert_eq!(contract["schema"], CONTRACT_SCHEMA);
    let cases = contract["cases"]
        .as_array()
        .expect("contract cases should be an array");
    assert!(!cases.is_empty(), "contract should contain cases");

    let output_values = output_contract_values();
    let expected_names = output_values
        .keys()
        .copied()
        .chain(std::iter::once("ProviderConfigInput"))
        .collect::<BTreeSet<_>>();
    let mut observed_names = BTreeSet::new();

    for case in cases {
        let name = case["name"].as_str().expect("case name should be a string");
        assert!(
            observed_names.insert(name),
            "duplicate contract case {name}"
        );
        let values = case["values"]
            .as_array()
            .expect("case values should be an array");
        assert!(
            !values.is_empty(),
            "contract case {name} should not be empty"
        );
        match case["direction"].as_str() {
            Some("rust_to_ts") => assert_eq!(
                values,
                output_values
                    .get(name)
                    .unwrap_or_else(|| panic!("unknown Rust output contract {name}")),
                "serialized DTO drifted for {name}"
            ),
            Some("ts_to_rust") if name == "ProviderConfigInput" => {
                assert_eq!(values.len(), 1);
                validate_provider_input(values[0].clone());
            }
            direction => panic!("unsupported contract direction {direction:?} for {name}"),
        }
    }

    assert_eq!(
        observed_names, expected_names,
        "contract case coverage drifted"
    );
}
