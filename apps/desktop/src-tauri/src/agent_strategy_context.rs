use super::{requirements::route_decision_metadata, PlannedAgentRun};
use crate::collaboration_service::truncate_for_collaboration;
use agent_core::Metadata;
use orchestrator::{AgentExecutionMode, AgentToolRequirement, AGENT_ROUTE_OBSERVABILITY_KEYS};

impl PlannedAgentRun {
    pub(crate) fn apply_to_context(&self, run_context: &mut Metadata) -> Result<(), String> {
        let decision = self.execution_plan.action();
        let requested_policy = self.policy.requested_policy();
        let collaboration_policy = decision.policy();
        run_context.insert(
            "task_class".to_string(),
            decision.task_class.label().to_string(),
        );
        run_context.insert(
            "tool_requirement".to_string(),
            match decision.tool_requirement {
                AgentToolRequirement::None => "none",
                AgentToolRequirement::ReadOnly => "read_only",
                AgentToolRequirement::Effects => "effects",
            }
            .to_string(),
        );
        run_context.insert(
            "vision_required".to_string(),
            decision.vision_required.to_string(),
        );
        for key in AGENT_ROUTE_OBSERVABILITY_KEYS {
            run_context.remove(key);
        }
        run_context.extend(route_decision_metadata(self));
        match (decision.calibration, &decision.calibration_reason) {
            (Some(_), Some(reason)) => {
                run_context.insert(
                    "decision_calibration_reason".to_string(),
                    truncate_for_collaboration(reason, 1_200),
                );
            }
            _ => {
                run_context.remove("decision_calibration");
                run_context.remove("decision_calibration_reason");
            }
        }
        run_context.insert(
            "routing_signature".to_string(),
            decision.learning_signature(),
        );
        super::causal_route::apply_causal_route_to_context(&self.execution_plan, run_context)?;
        run_context.insert("agent_effort".to_string(), self.policy.label().to_string());
        run_context.insert(
            "requested_policy".to_string(),
            requested_policy.label().to_string(),
        );
        run_context.insert(
            "collaboration_policy".to_string(),
            collaboration_policy.label().to_string(),
        );
        run_context.insert(
            "collaboration_profile".to_string(),
            if decision.execution == AgentExecutionMode::Workflow {
                "adaptive"
            } else {
                "direct"
            }
            .to_string(),
        );
        run_context.insert(
            "conductor_contract".to_string(),
            self.execution_contract.to_json()?,
        );
        run_context.insert(
            "expected_collaboration_uplift_bps".to_string(),
            self.execution_contract.expected_uplift_bps.to_string(),
        );
        run_context.insert("agent_model".to_string(), decision.primary_model.clone());
        run_context.insert("router_model".to_string(), decision.primary_model.clone());
        run_context.insert("router_examples".to_string(), "0".to_string());
        run_context.insert("router_source".to_string(), self.source.label().to_string());
        run_context.insert(
            "conductor_degraded".to_string(),
            self.degradation_reason.is_some().to_string(),
        );
        if let Some(reason) = &self.degradation_reason {
            run_context.insert(
                "conductor_failure".to_string(),
                truncate_for_collaboration(reason, 1_200),
            );
        } else {
            run_context.remove("conductor_failure");
        }
        run_context.insert(
            "run_decision".to_string(),
            serde_json::to_string(decision)
                .map_err(|error| format!("run decision serialization failed: {error}"))?,
        );
        run_context.insert(
            "execution_plan".to_string(),
            serde_json::to_string(&self.execution_plan)
                .map_err(|error| format!("execution plan serialization failed: {error}"))?,
        );
        run_context.insert(
            "execution_plan_sha256".to_string(),
            self.execution_plan.digest()?,
        );
        run_context.insert(
            "execution_plan_semantic_sha256".to_string(),
            self.execution_plan.semantic_digest()?,
        );
        run_context.insert(
            "execution_plan_authority".to_string(),
            self.execution_plan.authority.label().to_string(),
        );
        run_context.insert(
            "run_decision_attempts".to_string(),
            self.attempts.to_string(),
        );
        run_context.insert(
            "conductor_models_attempted".to_string(),
            self.attempted_conductor_models.join(","),
        );
        run_context.insert(
            "conductor_selected_model".to_string(),
            self.selected_conductor_model.clone().unwrap_or_default(),
        );
        run_context.insert("prompt_profile".to_string(), self.prompt_genome.id.clone());
        run_context.insert(
            "prompt_genome".to_string(),
            serde_json::to_string(&self.prompt_genome)
                .map_err(|error| format!("prompt genome serialization failed: {error}"))?,
        );
        crate::agent_finalizer_runtime::direct_finalizer_policy::install_direct_finalizer_policy_metadata(
            run_context,
            self.policy,
            decision.execution,
            &self.prompt_genome,
        )?;
        Ok(())
    }
}
