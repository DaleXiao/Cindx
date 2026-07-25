use super::*;

fn bounded_workflow_context(value: &str, max_chars: usize) -> String {
    let total_chars = value.chars().count();
    if total_chars <= max_chars {
        return value.to_string();
    }

    let tail_chars = max_chars / 4;
    let head_chars = max_chars.saturating_sub(tail_chars);
    let head = value.chars().take(head_chars).collect::<String>();
    let mut tail = value.chars().rev().take(tail_chars).collect::<Vec<_>>();
    tail.reverse();
    format!(
        "{head}\n[... {} characters omitted by the workflow context boundary ...]\n{}",
        total_chars.saturating_sub(max_chars),
        tail.into_iter().collect::<String>()
    )
}

pub fn validate_adaptive_workflow(
    workflow: &AdaptiveWorkflow,
    allowed_models: &[String],
) -> Result<(), String> {
    if workflow.steps.is_empty() {
        return Err("adaptive workflow must contain at least one step".to_string());
    }
    if workflow.steps.len() > MAX_ADAPTIVE_WORKFLOW_STEPS {
        return Err(format!(
            "adaptive workflow exceeds the {MAX_ADAPTIVE_WORKFLOW_STEPS}-step budget"
        ));
    }

    let allowed_models = allowed_models
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut seen_ids = BTreeSet::new();
    let mut selected_models = BTreeSet::new();
    for step in &workflow.steps {
        if step.id.trim().is_empty() {
            return Err("adaptive workflow step id is empty".to_string());
        }
        if seen_ids.contains(step.id.as_str()) {
            return Err(format!(
                "adaptive workflow step id is duplicated: {}",
                step.id
            ));
        }
        if !allowed_models.contains(step.model.as_str()) {
            return Err(format!(
                "adaptive workflow selected an unknown model: {}",
                step.model
            ));
        }
        selected_models.insert(step.model.as_str());
        if step.subtask.trim().is_empty() {
            return Err(format!(
                "adaptive workflow step {} has an empty subtask",
                step.id
            ));
        }
        if !matches!(
            step.role.as_str(),
            "thinker" | "worker" | "verifier" | "synthesizer"
        ) {
            return Err(format!(
                "adaptive workflow step {} selected an unknown role: {}",
                step.id, step.role
            ));
        }

        let mut unique_access = BTreeSet::new();
        for dependency in &step.access {
            if !unique_access.insert(dependency.as_str()) {
                return Err(format!(
                    "adaptive workflow step {} repeats dependency {dependency}",
                    step.id
                ));
            }
            if !seen_ids.contains(dependency.as_str()) {
                return Err(format!(
                    "adaptive workflow step {} must only access earlier steps: {dependency}",
                    step.id
                ));
            }
        }
        seen_ids.insert(step.id.as_str());
    }
    if selected_models.len() > MAX_ADAPTIVE_WORKFLOW_AGENTS {
        return Err(format!(
            "adaptive workflow exceeds the {MAX_ADAPTIVE_WORKFLOW_AGENTS}-agent budget"
        ));
    }

    if workflow.steps.len() > 1
        && workflow
            .steps
            .last()
            .is_some_and(|step| step.access.is_empty())
    {
        return Err("the final adaptive workflow step must synthesize prior work".to_string());
    }
    if workflow
        .steps
        .last()
        .is_some_and(|step| step.role != "synthesizer")
    {
        return Err("the final adaptive workflow step must use the synthesizer role".to_string());
    }

    let indexes = workflow
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| (step.id.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    let mut included = BTreeSet::new();
    let mut pending = vec![workflow.steps.len() - 1];
    while let Some(index) = pending.pop() {
        if !included.insert(index) {
            continue;
        }
        for dependency in &workflow.steps[index].access {
            if let Some(dependency_index) = indexes.get(dependency.as_str()) {
                pending.push(*dependency_index);
            }
        }
    }
    if included.len() != workflow.steps.len() {
        let omitted = workflow
            .steps
            .iter()
            .enumerate()
            .filter(|(index, _)| !included.contains(index))
            .map(|(_, step)| step.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "the final adaptive workflow must incorporate every branch: {omitted}"
        ));
    }

    adaptive_workflow_layers(workflow)?;
    Ok(())
}

pub fn adaptive_workflow_layers(workflow: &AdaptiveWorkflow) -> Result<Vec<Vec<usize>>, String> {
    let mut indexes = BTreeMap::new();
    let mut step_layers = vec![0usize; workflow.steps.len()];
    for (step_index, step) in workflow.steps.iter().enumerate() {
        if indexes.contains_key(step.id.as_str()) {
            return Err(format!(
                "adaptive workflow step id is duplicated: {}",
                step.id
            ));
        }
        let mut layer = 0;
        for dependency in &step.access {
            let dependency_index = indexes.get(dependency.as_str()).copied().ok_or_else(|| {
                format!(
                    "adaptive workflow step {} must only access earlier steps: {dependency}",
                    step.id
                )
            })?;
            layer = layer.max(step_layers[dependency_index] + 1);
        }
        step_layers[step_index] = layer;
        indexes.insert(step.id.as_str(), step_index);
    }

    let mut layers = Vec::<Vec<usize>>::new();
    for (step_index, layer) in step_layers.into_iter().enumerate() {
        if layers.len() <= layer {
            layers.resize_with(layer + 1, Vec::new);
        }
        layers[layer].push(step_index);
    }
    Ok(layers)
}

pub fn adaptive_worker_prompt(
    workflow: &AdaptiveWorkflow,
    step_index: usize,
    user_prompt: &str,
    shared_memory: &str,
    outputs: &BTreeMap<String, String>,
) -> Option<String> {
    let step = workflow.steps.get(step_index)?;
    let (role_instruction, output_contract) = match step.role.as_str() {
        "thinker" => (
            "Explore an independent approach, decompose the problem, and expose assumptions without duplicating implementation work.",
            "Hypotheses; Assumptions; Recommended path; Failure modes.",
        ),
        "verifier" => (
            "Audit supplied work against evidence, identify disagreements, and state exact corrections without inventing a new unsupported solution.",
            "Agreements; Disagreements; Evidence verdicts; Required corrections.",
        ),
        "synthesizer" => (
            "Resolve disagreements and produce one checkable execution brief grounded in the supplied work.",
            "Decision; Integrated execution brief; Evidence basis; Unresolved risks.",
        ),
        _ => (
            "Produce concrete work for the assigned subtask and report evidence and uncertainty rather than repeating the planning branch.",
            "Work product; Evidence used or needed; Risks; Handoff.",
        ),
    };
    let bounded_shared_memory = if shared_memory.trim().is_empty() {
        "(none)".to_string()
    } else {
        bounded_workflow_context(shared_memory, ADAPTIVE_WORKER_SHARED_MEMORY_MAX_CHARS)
    };
    let mut prompt = format!(
        "You are isolated {} {} in a Cindx adaptive multi-model workflow. {} Complete only the assigned subtask. Do not assume you can see other agents unless their output is explicitly included below. Use exposed read-only evidence tools when the subtask depends on workspace facts. Return concrete findings for a later agent, not a user-facing answer. Do not merely restate authorized outputs; transform, test, or reconcile them for your role.\n\nOutput contract:\n{}\n\nUser request:\n{}\n\nAssigned subtask:\n{}\n\nShared memory from earlier user turns:\n{}",
        step.role,
        step.id,
        role_instruction,
        output_contract,
        user_prompt,
        step.subtask,
        bounded_shared_memory
    );
    prompt.push_str("\n\nAuthorized prior step outputs:\n");
    if step.access.is_empty() {
        prompt.push_str("(none - work independently)\n");
    } else {
        let mut remaining_chars = ADAPTIVE_WORKER_AUTHORIZED_OUTPUTS_MAX_CHARS;
        for dependency in &step.access {
            let output = outputs.get(dependency)?;
            if remaining_chars == 0 {
                prompt.push_str(&format!(
                    "[{dependency}]\n[omitted: authorized workflow context budget exhausted]\n\n"
                ));
                continue;
            }
            let output_budget = remaining_chars.min(ADAPTIVE_WORKER_DEPENDENCY_MAX_CHARS);
            let bounded = bounded_workflow_context(output, output_budget);
            remaining_chars =
                remaining_chars.saturating_sub(bounded.chars().count().min(output_budget));
            prompt.push_str(&format!("[{dependency}]\n{bounded}\n\n"));
        }
    }
    Some(prompt)
}
