use crate::runtime_constants::ADAPTIVE_QUALITY_PASS_SCORE;
use agent_application::{AgentRunEvent, AgentRunStatus};
use agent_core::{Event, EventKind, RoutingOutcome};

pub(crate) use crate::learning_evidence_runtime::learning_budget_fingerprint;
#[cfg(test)]
pub(crate) use crate::learning_evidence_runtime::LEARNING_BUDGET_KEYS;

mod agent_run_projection;

#[cfg(test)]
pub(crate) use agent_run_projection::load_routing_telemetry_read_model;
#[cfg(test)]
pub(crate) use agent_run_projection::{
    load_routing_telemetry_read_model_snapshot, routing_telemetry_from_events,
};

#[cfg_attr(not(test), allow(dead_code))]
fn routing_quality_signals(run_events: &[&Event], terminal: &Event) -> (Option<f32>, Option<bool>) {
    if let Some(delivery) = run_events
        .iter()
        .rev()
        .find(|event| event.summary == "Collaboration workflow completed")
        .filter(|event| event.metadata.contains_key("anytime_selected_candidate"))
    {
        let quality_score = delivery
            .metadata
            .get("anytime_selected_quality_bps")
            .and_then(|score| score.parse::<f32>().ok())
            .map(|score| (score / 10_000.0).clamp(0.0, 1.0));
        let verification_passed = delivery
            .metadata
            .get("anytime_selected_verified")
            .and_then(|verified| verified.parse::<bool>().ok());
        return (quality_score, verification_passed);
    }

    if let Some(gate) = run_events
        .iter()
        .rev()
        .find(|event| event.summary == "Collaboration quality gate evaluated")
    {
        let quality_score = gate
            .metadata
            .get("quality_score")
            .and_then(|score| score.parse::<f32>().ok())
            .map(|score| score.clamp(0.0, 1.0));
        let quality_pass = gate
            .metadata
            .get("quality_pass")
            .and_then(|pass| pass.parse::<bool>().ok());
        let safety_clear = gate
            .metadata
            .get("safety_violations")
            .and_then(|count| count.parse::<usize>().ok())
            .map(|count| count == 0);
        return (
            quality_score,
            quality_pass
                .zip(safety_clear)
                .map(|(pass, clear)| pass && clear),
        );
    }

    let verification_passed = match terminal
        .metadata
        .get("completion_evidence")
        .map(String::as_str)
    {
        Some("verified_mutation") => Some(true),
        Some("unverified_mutation") => Some(false),
        _ => {
            let requested = terminal
                .metadata
                .get("verification_gate_requests")
                .and_then(|count| count.parse::<usize>().ok())
                .unwrap_or(0);
            let pending = terminal
                .metadata
                .get("pending_interaction_verifications")
                .and_then(|count| count.parse::<usize>().ok())
                .unwrap_or(0);
            (requested > 0).then_some(pending == 0)
        }
    };
    (None, verification_passed)
}

pub(crate) fn completion_learning_signal(
    runtime: &agent_runtime::AgentLoopState,
) -> (&'static str, bool) {
    if runtime.successful_mutations() == 0 {
        ("non_mutating", false)
    } else if runtime.verified_after_last_mutation() {
        ("verified_mutation", true)
    } else {
        ("unverified_mutation", false)
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn routing_outcome_for_run(
    run_events: &[&Event],
    terminal: &Event,
) -> Option<RoutingOutcome> {
    match AgentRunEvent::from_event(terminal).map(AgentRunEvent::status) {
        Some(AgentRunStatus::Cancelled) => return Some(RoutingOutcome::UserRejected),
        Some(AgentRunStatus::Failed) => return Some(RoutingOutcome::Failed),
        Some(AgentRunStatus::Completed) => {}
        _ => return None,
    }

    match terminal.metadata.get("routing_learning_eligible") {
        Some(value) if value == "false" => return None,
        Some(_) => {}
        None => {
            let has_legacy_successful_tool = run_events.iter().any(|event| {
                event.kind == EventKind::ToolCallFinished
                    && event.metadata.get("status").map(String::as_str) == Some("succeeded")
            });
            if has_legacy_successful_tool {
                return None;
            }
        }
    }

    if let Some(delivery) = run_events
        .iter()
        .rev()
        .find(|event| event.summary == "Collaboration workflow completed")
        .filter(|event| event.metadata.contains_key("anytime_selected_candidate"))
    {
        if delivery
            .metadata
            .get("anytime_routing_learning_eligible")
            .is_some_and(|eligible| eligible == "false")
        {
            return None;
        }
        let verified = delivery
            .metadata
            .get("anytime_selected_verified")?
            .parse::<bool>()
            .ok()?;
        let quality_bps = delivery
            .metadata
            .get("anytime_selected_quality_bps")?
            .parse::<u16>()
            .ok()?;
        return Some(
            if verified && quality_bps >= (ADAPTIVE_QUALITY_PASS_SCORE * 10_000.0).round() as u16 {
                RoutingOutcome::Succeeded
            } else {
                RoutingOutcome::Failed
            },
        );
    }

    if let Some(gate) = run_events
        .iter()
        .rev()
        .find(|event| event.summary == "Collaboration quality gate evaluated")
    {
        let pass = gate.metadata.get("quality_pass")?.parse::<bool>().ok()?;
        let score = gate.metadata.get("quality_score")?.parse::<f32>().ok()?;
        let safety_violations = gate
            .metadata
            .get("safety_violations")?
            .parse::<usize>()
            .ok()?;
        return Some(
            if pass && score >= ADAPTIVE_QUALITY_PASS_SCORE && safety_violations == 0 {
                RoutingOutcome::Succeeded
            } else {
                RoutingOutcome::Failed
            },
        );
    }

    if run_events
        .iter()
        .any(|event| event.summary == "Collaboration quality gate unavailable")
    {
        return None;
    }

    Some(RoutingOutcome::Succeeded)
}
