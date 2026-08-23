use super::test_support::test_message;
use super::*;
use crate::agent_preparation_runtime::{
    effective_prompt_objective_for_messages, image_generation_objective_for_preparation,
    preparation_prompt_parts, remove_stale_preparation_context, route_requirements_for_preparation,
};
use agent_core::run_decision_enums::AgentEffectAuthority;
use agent_core::AgentToolRequirement;

#[test]
fn goal2_preparation_replay_keeps_each_steer_once_and_replans_the_latest_prompt() {
    let mut runtime = start_agent_loop(
        TaskId("preparation-replay".to_string()),
        "original request",
        AgentRuntimeConfig::default(),
    );
    for (queue_id, prompt) in [
        ("steer-one", "preserve compatibility"),
        ("steer-two", "also run the focused tests"),
    ] {
        AgentKernel::new(&mut runtime, &[]).apply_steer(
            prompt,
            [
                ("queue_id".to_string(), queue_id.to_string()),
                ("queue_mode".to_string(), "steer".to_string()),
                ("model_content".to_string(), prompt.to_string()),
            ]
            .into_iter()
            .collect(),
        );
    }

    let (history, active) = preparation_prompt_parts(&runtime.messages)
        .expect("the latest steer should be the active planning objective");
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].content, "original request");
    assert_eq!(history[1].content, "preserve compatibility");
    assert_eq!(active.content, "also run the focused tests");
    assert_eq!(
        active.metadata.get("queue_id").map(String::as_str),
        Some("steer-two")
    );
    let queue_ids = history
        .iter()
        .chain(std::iter::once(&active))
        .filter_map(|message| message.metadata.get("queue_id"))
        .collect::<Vec<_>>();
    assert_eq!(queue_ids, vec!["steer-one", "steer-two"]);
    assert_eq!(
        effective_prompt_objective_for_messages("original request", &runtime.messages),
        "Initial request:\noriginal request\n\nAccepted steering 1:\npreserve compatibility\n\nAccepted steering 2:\nalso run the focused tests"
    );
}

#[test]
fn goal2_effective_objective_reclaims_short_steer_budget_for_initial_constraints() {
    let initial = format!("{}CRITICAL_END_CONSTRAINT", "A".repeat(5_900));
    let mut runtime = start_agent_loop(
        TaskId("effective-objective-budget".to_string()),
        &initial,
        AgentRuntimeConfig::default(),
    );
    for (index, prompt) in ["continue", "preserve data", "keep it fast", "run tests"]
        .into_iter()
        .enumerate()
    {
        AgentKernel::new(&mut runtime, &[]).apply_steer(
            prompt,
            [
                ("queue_id".to_string(), format!("steer-{index}")),
                ("queue_mode".to_string(), "steer".to_string()),
                ("model_content".to_string(), prompt.to_string()),
            ]
            .into_iter()
            .collect(),
        );
    }

    let objective = effective_prompt_objective_for_messages(&initial, &runtime.messages);

    assert!(objective.chars().count() <= 6_000);
    assert!(objective.chars().count() > 5_800);
    assert!(objective.contains("CRITICAL_END_CONSTRAINT"));
    for prompt in ["continue", "preserve data", "keep it fast", "run tests"] {
        assert!(objective.contains(prompt));
    }
}

#[test]
fn goal2_replanning_replaces_stale_preparation_context_but_keeps_durable_history() {
    let mut history = vec![
        Message {
            role: MessageRole::System,
            content: "old memory".to_string(),
            metadata: [
                ("internal".to_string(), "true".to_string()),
                ("kind".to_string(), "project_memory".to_string()),
            ]
            .into_iter()
            .collect(),
        },
        Message {
            role: MessageRole::System,
            content: "old collaboration".to_string(),
            metadata: [
                ("internal".to_string(), "true".to_string()),
                ("collaboration_stage".to_string(), "guidance".to_string()),
            ]
            .into_iter()
            .collect(),
        },
        Message {
            role: MessageRole::System,
            content: "durable artifact manifest".to_string(),
            metadata: [
                ("internal".to_string(), "true".to_string()),
                ("kind".to_string(), "artifact_manifest".to_string()),
            ]
            .into_iter()
            .collect(),
        },
        Message {
            role: MessageRole::Assistant,
            content: "completed work evidence".to_string(),
            metadata: Metadata::new(),
        },
    ];

    remove_stale_preparation_context(&mut history);

    assert_eq!(history.len(), 2);
    assert_eq!(history[0].content, "durable artifact manifest");
    assert_eq!(history[1].content, "completed work evidence");
}

#[test]
fn goal2_prompt_derived_image_contract_is_refreshed_in_both_directions() {
    let config = ProviderConfig {
        base_url: "https://provider.example/v1".to_string(),
        image_model: "image-model".to_string(),
        ..ProviderConfig::default()
    };
    let mut run_context = Metadata::new();

    add_image_generation_run_context(
        &mut run_context,
        &config,
        "Generate an image of a lighthouse",
    );
    assert_eq!(
        run_context
            .get("image_generation_required")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        run_context
            .get("configured_image_model")
            .map(String::as_str),
        Some("image-model")
    );

    add_image_generation_run_context(&mut run_context, &config, "Review this Rust module");
    assert!(!run_context.contains_key("image_generation_required"));
    assert!(!run_context.contains_key("configured_image_model"));
    assert!(!run_context.contains_key("configured_image_endpoint"));

    add_image_generation_run_context(
        &mut run_context,
        &config,
        "Create a picture for the release notes",
    );
    assert_eq!(
        run_context
            .get("image_generation_required")
            .map(String::as_str),
        Some("true")
    );
}

#[test]
fn goal2_short_image_steer_keeps_the_cumulative_image_contract() {
    let config = ProviderConfig {
        base_url: "https://provider.example/v1".to_string(),
        image_model: "image-model".to_string(),
        ..ProviderConfig::default()
    };
    let initial = "Generate an image of a lighthouse";
    let mut runtime = start_agent_loop(
        TaskId("cumulative-image-contract".to_string()),
        initial,
        AgentRuntimeConfig::default(),
    );
    AgentKernel::new(&mut runtime, &[]).apply_steer(
        "Make the background blue",
        [
            ("queue_id".to_string(), "image-steer".to_string()),
            ("queue_mode".to_string(), "steer".to_string()),
        ]
        .into_iter()
        .collect(),
    );
    let planning_objective = effective_prompt_objective_for_messages(initial, &runtime.messages);
    let mut run_context = Metadata::new();

    add_image_generation_run_context(&mut run_context, &config, &planning_objective);
    run_context.insert(
        "effective_prompt_objective".to_string(),
        planning_objective.clone(),
    );

    assert!(planning_objective.contains(initial));
    assert!(planning_objective.contains("Make the background blue"));
    assert_eq!(
        effective_agent_objective(&run_context, "Make the background blue"),
        planning_objective
    );
    assert_eq!(
        run_context
            .get("image_generation_required")
            .map(String::as_str),
        Some("true")
    );
}

#[test]
fn goal2_prompt_derived_image_contract_is_scoped_to_the_steer_epoch() {
    let mut runtime = start_agent_loop(
        phase16_task_id(),
        "Generate the first image",
        AgentRuntimeConfig::default(),
    );
    let mut run_context = [
        ("steer_epoch".to_string(), "0".to_string()),
        ("image_generation_required".to_string(), "true".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    apply_run_task_contract(&mut runtime, &run_context, &[], None)
        .expect("image contract should apply");
    record_tool_outcome_with_risk(
        &mut runtime,
        "image.generate",
        r#"{"prompt":"first"}"#,
        &ToolOutcomeStatus::Succeeded,
        Some(&ToolRisk::UsesNetwork),
    );
    assert!(runtime
        .task_contract
        .required_tool_satisfied("image.generate"));

    run_context.insert("steer_epoch".to_string(), "1".to_string());
    run_context.remove("image_generation_required");
    apply_run_task_contract(&mut runtime, &run_context, &[], None)
        .expect("text contract should apply");
    assert!(runtime.task_contract.model_context_for_task(&[]).is_none());

    run_context.insert("steer_epoch".to_string(), "2".to_string());
    run_context.insert("image_generation_required".to_string(), "true".to_string());
    apply_run_task_contract(&mut runtime, &run_context, &[], None)
        .expect("new image contract should apply");
    assert!(!runtime
        .task_contract
        .required_tool_satisfied("image.generate"));
}

#[test]
fn goal2_knowledge_preparation_is_superseded_without_stopping_the_run() {
    let control = Arc::new(AgentRunControl::new("pro"));
    let epoch = control.steer_epoch();
    assert!(!knowledge_preparation_should_interrupt(&control, epoch));

    assert_eq!(control.request_steer("replace-objective"), Ok(true));
    assert!(knowledge_preparation_should_interrupt(&control, epoch));
    assert_eq!(control.stop_reason(), None);
}

#[test]
fn preparation_route_requirements_recompute_intent_and_active_images() {
    let mut run_context = [(
        "effective_prompt_objective".to_string(),
        "Audit this repository".to_string(),
    )]
    .into_iter()
    .collect::<Metadata>();
    let mut active_user = test_message(MessageRole::User, "Audit this repository");

    let read_only = route_requirements_for_preparation(
        &run_context,
        std::slice::from_ref(&active_user),
        &active_user,
    );
    assert_eq!(
        read_only.minimum_tool_requirement,
        AgentToolRequirement::ReadOnly
    );
    assert_eq!(read_only.effect_authority, AgentEffectAuthority::Allowed);
    assert!(!read_only.image_input_required);

    active_user.metadata.insert(
        "image_paths".to_string(),
        "\n/private/tmp/screenshot.png\n".to_string(),
    );
    let with_image = route_requirements_for_preparation(
        &run_context,
        std::slice::from_ref(&active_user),
        &active_user,
    );
    assert!(with_image.image_input_required);

    run_context.insert("image_generation_required".to_string(), "true".to_string());
    let image_generation = route_requirements_for_preparation(
        &run_context,
        std::slice::from_ref(&active_user),
        &active_user,
    );
    assert_eq!(
        image_generation.minimum_tool_requirement,
        AgentToolRequirement::Effects
    );
    assert_eq!(
        image_generation.effect_authority,
        AgentEffectAuthority::Required
    );

    run_context.insert("steer_epoch".to_string(), "1".to_string());
    run_context.insert(
        "prompt_objective".to_string(),
        "Instead, just explain what an audit is".to_string(),
    );
    run_context.remove("image_generation_required");
    let steered = route_requirements_for_preparation(
        &run_context,
        std::slice::from_ref(&active_user),
        &active_user,
    );
    assert_eq!(steered.minimum_tool_requirement, AgentToolRequirement::None);
    assert_eq!(steered.effect_authority, AgentEffectAuthority::Allowed);

    run_context.insert("steer_epoch".to_string(), "0".to_string());
    run_context.insert(
        "effective_prompt_objective".to_string(),
        "Review this repository; do not modify anything".to_string(),
    );
    run_context.remove("prompt_objective");
    let explicit_read_only = route_requirements_for_preparation(
        &run_context,
        std::slice::from_ref(&active_user),
        &active_user,
    );
    assert_eq!(
        explicit_read_only.effect_authority,
        AgentEffectAuthority::Forbidden
    );
}

#[test]
fn additive_steer_retains_only_current_run_image_requirements() {
    let mut run_context = [
        ("agent_run_id".to_string(), "run-current".to_string()),
        ("steer_epoch".to_string(), "1".to_string()),
        (
            "effective_prompt_objective".to_string(),
            "Initial request:\nInspect this image\n\nAccepted steering 1:\nFocus on the header"
                .to_string(),
        ),
        (
            "prompt_objective".to_string(),
            "Focus on the header".to_string(),
        ),
    ]
    .into_iter()
    .collect::<Metadata>();
    let mut current_image = test_message(MessageRole::User, "Inspect this image");
    current_image
        .metadata
        .insert("agent_run_id".to_string(), "run-current".to_string());
    current_image.metadata.insert(
        "image_paths".to_string(),
        "/private/tmp/current.png".to_string(),
    );
    let mut old_image = test_message(MessageRole::User, "Old image request");
    old_image
        .metadata
        .insert("agent_run_id".to_string(), "run-old".to_string());
    old_image.metadata.insert(
        "image_paths".to_string(),
        "/private/tmp/old.png".to_string(),
    );
    let active_steer = test_message(MessageRole::User, "Focus on the header");

    let additive = route_requirements_for_preparation(
        &run_context,
        &[
            old_image.clone(),
            current_image.clone(),
            active_steer.clone(),
        ],
        &active_steer,
    );
    assert!(additive.image_input_required);

    let old_run_only = route_requirements_for_preparation(
        &run_context,
        &[old_image, active_steer.clone()],
        &active_steer,
    );
    assert!(!old_run_only.image_input_required);

    let mut source_image = test_message(MessageRole::User, "Recovered image request");
    source_image
        .metadata
        .insert("agent_run_id".to_string(), "run-source".to_string());
    source_image.metadata.insert(
        "image_paths".to_string(),
        "/private/tmp/source.png".to_string(),
    );
    run_context.insert("source_agent_run_id".to_string(), "run-source".to_string());
    let recovered_additive = route_requirements_for_preparation(
        &run_context,
        &[source_image.clone(), active_steer.clone()],
        &active_steer,
    );
    assert!(recovered_additive.image_input_required);

    run_context.insert(
        "prompt_objective".to_string(),
        "Instead, ignore the image and explain headers generally".to_string(),
    );
    let replacement = route_requirements_for_preparation(
        &run_context,
        &[current_image, active_steer.clone()],
        &active_steer,
    );
    assert!(!replacement.image_input_required);
    let recovered_replacement = route_requirements_for_preparation(
        &run_context,
        &[source_image, active_steer.clone()],
        &active_steer,
    );
    assert!(!recovered_replacement.image_input_required);
}

#[test]
fn replacement_steer_keeps_route_and_execution_intent_aligned() {
    let replacement = "Instead, just explain how crash diagnosis works";
    let run_context = [
        ("agent_run_id".to_string(), "run-current".to_string()),
        ("steer_epoch".to_string(), "1".to_string()),
        (
            "effective_prompt_objective".to_string(),
            format!(
                "Initial request:\nFix the crash in this image\n\nAccepted steering 1:\n{replacement}"
            ),
        ),
        ("prompt_objective".to_string(), replacement.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let mut initial = test_message(MessageRole::User, "Fix the crash in this image");
    initial
        .metadata
        .insert("agent_run_id".to_string(), "run-current".to_string());
    initial.metadata.insert(
        "image_paths".to_string(),
        "/private/tmp/current.png".to_string(),
    );
    let active_steer = test_message(MessageRole::User, replacement);

    let requirements = route_requirements_for_preparation(
        &run_context,
        &[initial, active_steer.clone()],
        &active_steer,
    );

    assert_eq!(
        requirements.minimum_tool_requirement,
        AgentToolRequirement::None
    );
    assert!(!requirements.image_input_required);
    assert_eq!(
        agent_runtime::prompt_completion_intent(&run_context).tool_requirement,
        agent_runtime::PromptToolRequirement::None
    );
}

#[test]
fn image_generation_route_follows_additive_and_replacement_steers() {
    let config = ProviderConfig {
        base_url: "https://provider.example/v1".to_string(),
        image_model: "image-model".to_string(),
        ..ProviderConfig::default()
    };
    let replacement = "Instead, just explain lighthouse composition";
    let replacement_effective = format!(
        "Initial request:\nGenerate an image of a lighthouse\n\nAccepted steering 1:\n{replacement}"
    );
    let mut replacement_context = [
        ("steer_epoch".to_string(), "1".to_string()),
        (
            "effective_prompt_objective".to_string(),
            replacement_effective.clone(),
        ),
        ("prompt_objective".to_string(), replacement.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let replacement_objective = image_generation_objective_for_preparation(
        &replacement_context,
        &replacement_effective,
        replacement,
    );
    assert_eq!(replacement_objective, replacement);
    add_image_generation_run_context(&mut replacement_context, &config, replacement_objective);
    assert!(!replacement_context.contains_key("image_generation_required"));

    let additive = "Generate an image of the proposed layout";
    let additive_effective =
        format!("Initial request:\nExplain the layout\n\nAccepted steering 1:\n{additive}");
    let mut additive_context = [
        ("steer_epoch".to_string(), "1".to_string()),
        (
            "effective_prompt_objective".to_string(),
            additive_effective.clone(),
        ),
        ("prompt_objective".to_string(), additive.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let additive_objective = image_generation_objective_for_preparation(
        &additive_context,
        &additive_effective,
        additive,
    );
    assert_eq!(additive_objective, additive_effective);
    add_image_generation_run_context(&mut additive_context, &config, additive_objective);
    assert_eq!(
        additive_context
            .get("image_generation_required")
            .map(String::as_str),
        Some("true")
    );
    let active_user = test_message(MessageRole::User, additive);
    assert_eq!(
        route_requirements_for_preparation(
            &additive_context,
            std::slice::from_ref(&active_user),
            &active_user,
        )
        .minimum_tool_requirement,
        AgentToolRequirement::Effects
    );
}
