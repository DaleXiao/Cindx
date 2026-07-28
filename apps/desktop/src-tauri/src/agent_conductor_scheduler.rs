use crate::collaboration_stage_runtime::CollaborationStageError;
use orchestrator::AgentRunDecision;

#[derive(Debug, Clone)]
pub(crate) enum ConductorDecisionOutcome {
    Selected(AgentRunDecision),
    Exhausted,
}

#[derive(Debug, Clone)]
pub(crate) struct ConductorDecisionSchedule {
    pub(crate) outcome: ConductorDecisionOutcome,
    pub(crate) attempted_models: Vec<String>,
    pub(crate) selected_model: Option<String>,
    pub(crate) failure_reasons: Vec<String>,
}

pub(crate) fn schedule_conductor_decision(
    conductor_models: &[String],
    attempts: &mut usize,
    mut attempt: impl FnMut(
        usize,
        &str,
        bool,
        &mut usize,
    ) -> Result<AgentRunDecision, CollaborationStageError>,
) -> Result<ConductorDecisionSchedule, CollaborationStageError> {
    let mut attempted_models = Vec::new();
    let mut failure_reasons = Vec::new();
    for (model_index, conductor_model) in conductor_models.iter().enumerate() {
        attempted_models.push(conductor_model.clone());
        let has_alternate_model = model_index + 1 < conductor_models.len();
        match attempt(model_index, conductor_model, has_alternate_model, attempts) {
            Ok(decision) => {
                return Ok(ConductorDecisionSchedule {
                    outcome: ConductorDecisionOutcome::Selected(decision),
                    attempted_models,
                    selected_model: Some(conductor_model.clone()),
                    failure_reasons,
                });
            }
            Err(CollaborationStageError::RunStopped) => {
                return Err(CollaborationStageError::RunStopped)
            }
            Err(CollaborationStageError::SteerInterrupted) => {
                return Err(CollaborationStageError::SteerInterrupted)
            }
            Err(CollaborationStageError::AttemptDeadline) => {
                failure_reasons.push(format!(
                    "{conductor_model}: conductor response did not start before the failover deadline"
                ));
            }
            Err(CollaborationStageError::StageDeadline) => {
                failure_reasons.push(format!(
                    "{conductor_model}: collaboration stage deadline exhausted"
                ));
                break;
            }
            Err(CollaborationStageError::Failed(error)) => {
                failure_reasons.push(format!("{conductor_model}: {error}"));
            }
        }
    }
    Ok(ConductorDecisionSchedule {
        outcome: ConductorDecisionOutcome::Exhausted,
        attempted_models,
        selected_model: None,
        failure_reasons,
    })
}
