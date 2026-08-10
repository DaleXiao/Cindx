use crate::{WorkflowOutputKind, WorkflowPlanIr, WorkflowToolPolicy};
use std::collections::BTreeSet;

pub(crate) fn validate(plan: &WorkflowPlanIr, required_verification: bool) -> Result<(), String> {
    let specialist_steps = plan
        .steps
        .iter()
        .enumerate()
        .filter(|(_, step)| {
            matches!(
                step.contract.output_kind,
                WorkflowOutputKind::Analysis | WorkflowOutputKind::Evidence
            )
        })
        .collect::<Vec<_>>();
    if specialist_steps.len() != 1 {
        return Err(
            "owner execution graph requires exactly one analysis or evidence specialist step"
                .to_string(),
        );
    }

    let verification_steps = plan
        .steps
        .iter()
        .enumerate()
        .filter(|(_, step)| step.contract.output_kind == WorkflowOutputKind::Verification)
        .collect::<Vec<_>>();
    match (required_verification, verification_steps.len()) {
        (true, 1) | (false, 0) => {}
        (true, _) => {
            return Err("owner execution graph requires exactly one verification step".to_string())
        }
        (false, _) => {
            return Err(
                "owner execution graph cannot include verification when it is not required"
                    .to_string(),
            )
        }
    }

    let synthesis_steps = plan
        .steps
        .iter()
        .enumerate()
        .filter(|(_, step)| step.contract.output_kind == WorkflowOutputKind::Synthesis)
        .collect::<Vec<_>>();
    if synthesis_steps.len() != 1
        || synthesis_steps[0].0 + 1 != plan.steps.len()
        || synthesis_steps[0].1.tool_policy != WorkflowToolPolicy::None
    {
        return Err(
            "owner execution graph requires exactly one final tool-free synthesis handoff sink"
                .to_string(),
        );
    }

    let expected_step_count = if required_verification { 3 } else { 2 };
    if plan.steps.len() != expected_step_count {
        return Err(format!(
            "owner execution graph requires exactly {expected_step_count} steps"
        ));
    }

    let (specialist_index, specialist) = specialist_steps[0];
    if specialist_index != 0
        || !specialist.access.is_empty()
        || specialist.contract.input_steps != specialist.access
    {
        return Err(
            "owner execution graph specialist must be the dependency-free first step".to_string(),
        );
    }

    if let Some((verification_index, verification)) = verification_steps.first().copied() {
        if verification_index != 1
            || verification.tool_policy != WorkflowToolPolicy::None
            || verification.access != [specialist.id.clone()]
            || verification.contract.input_steps != verification.access
            || verification.model == specialist.model
        {
            return Err(
                "owner execution graph verifier must use a different configured model and depend only on the specialist without tools"
                    .to_string(),
            );
        }
    }

    let (_, synthesis) = synthesis_steps[0];
    let expected_handoff_input = verification_steps
        .first()
        .map_or(specialist.id.as_str(), |(_, verification)| {
            verification.id.as_str()
        });
    let actual_handoff_inputs = synthesis
        .access
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if actual_handoff_inputs.len() != synthesis.access.len()
        || actual_handoff_inputs != BTreeSet::from([expected_handoff_input])
        || synthesis.contract.input_steps != synthesis.access
    {
        return Err(
            "owner execution graph synthesis handoff must depend only on the final specialist or verifier output"
                .to_string(),
        );
    }

    Ok(())
}
