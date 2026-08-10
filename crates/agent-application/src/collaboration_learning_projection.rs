use crate::collaboration_learning_policy::{
    collaboration_learning_sha256, validate_collaboration_learning_sha256,
    CollaborationLearningError, CollaborationLearningPolicyV1, CollaborationRepairV1,
    CollaborationSpecialistInvocationV1, CollaborationVerificationV1,
};
use crate::outcome_evidence::ExternallyVerifiedOutcomeV1;
use agent_core::{
    decode_event_type, DecodedEventType, Event, EventKind, EventTypeV1, Metadata,
    AGENT_ACTOR_METADATA_KEY, AGENT_ATTRIBUTION_COMPONENT_METADATA_KEY,
    AGENT_ATTRIBUTION_LEGACY_ROLE_METADATA_KEY, AGENT_ATTRIBUTION_MODEL_METADATA_KEY,
    AGENT_EFFECT_AUTHORITY_METADATA_KEY, AGENT_MODEL_ATTRIBUTION_SCHEMA,
    AGENT_MODEL_ATTRIBUTION_SCHEMA_METADATA_KEY, AGENT_MODEL_PROFILE_METADATA_KEY,
    AGENT_OUTPUT_TRUST_METADATA_KEY, AGENT_SERVICE_METADATA_KEY, AGENT_STAGE_METADATA_KEY,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const COLLABORATION_LEARNING_ASSIGNMENT_SUMMARY: &str =
    "Collaboration learning policy assigned";
pub const COLLABORATION_LEARNING_ASSIGNMENT_SCHEMA: &str =
    "cindx.agent-collaboration-learning-assignment.v1";
pub const COLLABORATION_LEARNING_EXERCISE_SCHEMA: &str =
    "cindx.agent-collaboration-learning-exercise.v1";
pub const COLLABORATION_LEARNING_CONTEXT_RECEIPT_SCHEMA: &str =
    "cindx.agent-collaboration-context-receipt.v1";
pub const COLLABORATION_LEARNING_CONTEXT_COMMITTED_SUMMARY: &str =
    "Collaboration learning context committed";
pub const COLLABORATION_LEARNING_ASSIGNMENT_SCHEMA_METADATA_KEY: &str =
    "collaboration_learning_assignment_schema";
pub const COLLABORATION_LEARNING_POLICY_JSON_METADATA_KEY: &str =
    "collaboration_learning_policy_json";
pub const COLLABORATION_LEARNING_POLICY_SHA256_METADATA_KEY: &str =
    "collaboration_learning_policy_sha256";
pub const COLLABORATION_LEARNING_CASE_BINDING_SHA256_METADATA_KEY: &str =
    "collaboration_learning_case_binding_sha256";
pub const COLLABORATION_LEARNING_PLAN_REQUIRED_INDEPENDENT_VERIFIER_METADATA_KEY: &str =
    "collaboration_learning_plan_required_independent_verifier";
pub const COLLABORATION_LEARNING_SPECIALIST_STEP_ID_METADATA_KEY: &str =
    "collaboration_learning_specialist_step_id";
pub const COLLABORATION_LEARNING_SPECIALIST_OUTPUT_KIND_METADATA_KEY: &str =
    "collaboration_learning_specialist_output_kind";
pub const COLLABORATION_LEARNING_SPECIALIST_MODEL_METADATA_KEY: &str =
    "collaboration_learning_specialist_model";
pub const COLLABORATION_LEARNING_SPECIALIST_REPAIR_MODEL_METADATA_KEY: &str =
    "collaboration_learning_specialist_repair_model";
pub const COLLABORATION_LEARNING_VERIFIER_STEP_ID_METADATA_KEY: &str =
    "collaboration_learning_verifier_step_id";
pub const COLLABORATION_LEARNING_VERIFIER_MODEL_METADATA_KEY: &str =
    "collaboration_learning_verifier_model";
pub const COLLABORATION_LEARNING_VERIFIER_REPAIR_MODEL_METADATA_KEY: &str =
    "collaboration_learning_verifier_repair_model";
pub const COLLABORATION_LEARNING_CONTEXT_SCHEMA_METADATA_KEY: &str =
    "collaboration_learning_context_schema";
pub const COLLABORATION_LEARNING_CONTEXT_BUDGET_BPS_METADATA_KEY: &str =
    "collaboration_learning_context_budget_bps";
pub const COLLABORATION_LEARNING_CONTEXT_PAYLOAD_SHA256_METADATA_KEY: &str =
    "collaboration_learning_context_payload_sha256";
pub const COLLABORATION_LEARNING_CONTEXT_BYTES_METADATA_KEY: &str =
    "collaboration_learning_context_bytes";
pub const COLLABORATION_LEARNING_WORKER_TURN_ORDINAL_METADATA_KEY: &str =
    "collaboration_learning_worker_turn_ordinal";

const HASH_DOMAIN: &[u8] = b"cindx.agent-collaboration-learning-exercise.v1\0";
const CONTEXT_AGGREGATE_HASH_DOMAIN: &[u8] = b"cindx.agent-collaboration-context-aggregate.v1\0";
const CONTEXT_AGGREGATE_SCHEMA: &str = "cindx.agent-collaboration-context-aggregate.v1";
const MAX_JSON_BYTES: usize = 128 * 1024;
const RUN: &str = "agent_run_id";
const EPOCH: &str = "steer_epoch";
const PLAN: &str = "execution_plan_semantic_sha256";
const COLLABORATION: &str = "collaboration_id";
const REQUEST: &str = "request_id";
const STEP: &str = "workflow_step_id";
const OUTPUT: &str = "output_kind";
const RECOVERY: &str = "recovery";
const RECOVERY_ATTEMPT: &str = "recovery_attempt";
const REQUEST_PAYLOAD: &str = "request_payload_sha256";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationLearningLaneActorV1 {
    Specialist,
    IndependentVerifier,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationLearningOutputKindV1 {
    Analysis,
    Evidence,
    Verification,
}

impl CollaborationLearningOutputKindV1 {
    fn parse(value: &str) -> Result<Self, CollaborationLearningError> {
        match value {
            "analysis" => Ok(Self::Analysis),
            "evidence" => Ok(Self::Evidence),
            "verification" => Ok(Self::Verification),
            _ => Err(err("unsupported worker output kind")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollaborationLearningLaneAssignmentV1 {
    pub actor: CollaborationLearningLaneActorV1,
    pub workflow_step_id: String,
    pub output_kind: CollaborationLearningOutputKindV1,
    pub initial_model: String,
    pub repair_model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollaborationLearningAssignmentV1 {
    pub schema: String,
    pub agent_run_id: String,
    pub steer_epoch: u64,
    pub execution_plan_semantic_sha256: String,
    pub policy_json: String,
    pub policy_sha256: String,
    pub case_binding_sha256: String,
    pub assignment_sequence: u64,
    pub plan_required_independent_verifier: bool,
    pub collaboration_id: Option<String>,
    pub specialist: Option<CollaborationLearningLaneAssignmentV1>,
    pub verifier: Option<CollaborationLearningLaneAssignmentV1>,
}

impl CollaborationLearningAssignmentV1 {
    pub fn policy(&self) -> Result<CollaborationLearningPolicyV1, CollaborationLearningError> {
        let policy = CollaborationLearningPolicyV1::from_json(&self.policy_json)?;
        if policy.to_json()? != self.policy_json || policy.policy_sha256 != self.policy_sha256 {
            return Err(err("assignment policy JSON or digest is not canonical"));
        }
        Ok(policy)
    }

    fn validate(&self) -> Result<CollaborationLearningPolicyV1, CollaborationLearningError> {
        if self.schema != COLLABORATION_LEARNING_ASSIGNMENT_SCHEMA
            || self.agent_run_id.trim().is_empty()
        {
            return Err(err("assignment identity is invalid"));
        }
        for (digest, label) in [
            (&self.execution_plan_semantic_sha256, "semantic plan"),
            (&self.policy_sha256, "policy"),
            (&self.case_binding_sha256, "case binding"),
        ] {
            validate_collaboration_learning_sha256(digest, label)?;
        }
        let policy = self.policy()?;
        match policy.specialist_invocation {
            CollaborationSpecialistInvocationV1::DirectOwnerOnly => {
                if self.plan_required_independent_verifier
                    || self.collaboration_id.is_some()
                    || self.specialist.is_some()
                    || self.verifier.is_some()
                {
                    return Err(err("direct assignment is not Owner-only"));
                }
            }
            CollaborationSpecialistInvocationV1::OneReadOnlySpecialist => {
                if self.collaboration_id.as_deref().is_none_or(str::is_empty) {
                    return Err(err("workflow assignment is missing collaboration id"));
                }
                validate_assignment_lane(
                    self.specialist
                        .as_ref()
                        .ok_or_else(|| err("workflow assignment is missing Specialist"))?,
                    &policy,
                    CollaborationLearningLaneActorV1::Specialist,
                )?;
                if let Some(verifier) = &self.verifier {
                    validate_assignment_lane(
                        verifier,
                        &policy,
                        CollaborationLearningLaneActorV1::IndependentVerifier,
                    )?;
                    let specialist = self.specialist.as_ref().expect("validated above");
                    if lane_models(specialist)
                        .intersection(&lane_models(verifier))
                        .next()
                        .is_some()
                    {
                        return Err(err("Specialist and Verifier models are not distinct"));
                    }
                }
                match policy.verification {
                    CollaborationVerificationV1::PlanRequiredOnly
                        if self.verifier.is_some() != self.plan_required_independent_verifier =>
                    {
                        return Err(err(
                            "PlanRequiredOnly assignment disagrees with the plan requirement",
                        ));
                    }
                    CollaborationVerificationV1::AlwaysIndependent if self.verifier.is_none() => {
                        return Err(err("policy requires an Independent Verifier"));
                    }
                    _ => {}
                }
            }
        }
        Ok(policy)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollaborationLearningContextReceiptV1 {
    pub budget_bps: u16,
    pub payload_sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollaborationLearningWorkerAttemptV1 {
    pub request_id: String,
    pub model: String,
    pub started_sequence: u64,
    pub finished_sequence: u64,
    pub recovery_attempt: Option<u8>,
    pub context: CollaborationLearningContextReceiptV1,
    pub provider_response_observed: bool,
    pub succeeded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollaborationLearningLaneExerciseV1 {
    pub actor: CollaborationLearningLaneActorV1,
    pub workflow_step_id: String,
    pub output_kind: CollaborationLearningOutputKindV1,
    pub attempts: Vec<CollaborationLearningWorkerAttemptV1>,
    pub final_success: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationDerivedStopReasonV1 {
    DirectOwnerTerminal,
    RequiredLanesCompleted,
    RequiredLanesCompletedAfterRepair,
    OwnerFallbackAfterLaneFailure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollaborationLearningExerciseV1 {
    pub schema: String,
    pub outcome_receipt_sha256: String,
    pub assignment: CollaborationLearningAssignmentV1,
    pub lanes: Vec<CollaborationLearningLaneExerciseV1>,
    pub stop_reason: CollaborationDerivedStopReasonV1,
    pub receipt_sha256: String,
}

impl CollaborationLearningExerciseV1 {
    pub fn from_events(
        events: &[Event],
        outcome: &ExternallyVerifiedOutcomeV1,
    ) -> Result<Self, CollaborationLearningError> {
        project(events, outcome).map_err(|error| {
            err(format!(
                "exercise is censored because trusted projection failed: {error}"
            ))
        })
    }

    pub fn from_json(encoded: &str) -> Result<Self, CollaborationLearningError> {
        if encoded.len() > MAX_JSON_BYTES {
            return Err(err("exercise JSON exceeds its size bound"));
        }
        let exercise: Self = serde_json::from_str(encoded)
            .map_err(|error| err(format!("exercise JSON is invalid: {error}")))?;
        exercise.validate()?;
        Ok(exercise)
    }

    pub fn to_json(&self) -> Result<String, CollaborationLearningError> {
        self.validate()?;
        let encoded = serde_json::to_string(self)
            .map_err(|error| err(format!("exercise JSON encoding failed: {error}")))?;
        if encoded.len() > MAX_JSON_BYTES {
            return Err(err("exercise JSON exceeds its size bound"));
        }
        Ok(encoded)
    }

    pub fn validate(&self) -> Result<(), CollaborationLearningError> {
        let policy = self.validate_payload()?;
        validate_collaboration_learning_sha256(&self.receipt_sha256, "exercise receipt")?;
        if self.receipt_sha256 != exercise_digest(self)? {
            return Err(err("exercise digest is invalid"));
        }
        validate_lanes(&self.assignment, &policy, &self.lanes)
    }

    pub fn validate_against(
        &self,
        policy: &CollaborationLearningPolicyV1,
        outcome: &ExternallyVerifiedOutcomeV1,
    ) -> Result<(), CollaborationLearningError> {
        self.validate()?;
        policy.validate()?;
        outcome
            .validate()
            .map_err(|error| err(format!("outcome is invalid: {error}")))?;
        if self.assignment.policy()? != *policy
            || self.outcome_receipt_sha256 != outcome.receipt_sha256
            || self.assignment.agent_run_id != outcome.lifecycle.agent_run_id
            || self.assignment.steer_epoch != outcome.lifecycle.steer_epoch
            || self.assignment.execution_plan_semantic_sha256
                != outcome.lifecycle.execution_plan_semantic_sha256
            || self.assignment.assignment_sequence <= outcome.lifecycle.decision_sequence
            || self.assignment.assignment_sequence >= outcome.lifecycle.terminal_sequence
        {
            return Err(err("exercise does not bind its policy and outcome"));
        }
        if self
            .lanes
            .iter()
            .flat_map(|lane| &lane.attempts)
            .any(|attempt| {
                attempt.started_sequence <= self.assignment.assignment_sequence
                    || attempt.finished_sequence >= outcome.lifecycle.terminal_sequence
            })
        {
            return Err(err("worker attempt is outside the bound lifecycle"));
        }
        validate_exposure(self, policy, outcome)
    }

    fn validate_payload(
        &self,
    ) -> Result<CollaborationLearningPolicyV1, CollaborationLearningError> {
        if self.schema != COLLABORATION_LEARNING_EXERCISE_SCHEMA {
            return Err(err("exercise schema is unsupported"));
        }
        validate_collaboration_learning_sha256(&self.outcome_receipt_sha256, "outcome receipt")?;
        let policy = self.assignment.validate()?;
        validate_lanes(&self.assignment, &policy, &self.lanes)?;
        if self.stop_reason != derive_stop(&policy, &self.assignment, &self.lanes)? {
            return Err(err("stop reason is not derived from actual attempts"));
        }
        Ok(policy)
    }
}

#[derive(Clone)]
struct Actual {
    actor: CollaborationLearningLaneActorV1,
    request: String,
    step: String,
    output: CollaborationLearningOutputKindV1,
    model: String,
    collaboration: String,
    stage: String,
    profile: String,
    component: String,
    legacy_role: String,
    runtime_stage: Option<String>,
    recovery: Option<u8>,
    sequence: u64,
    context: Option<CollaborationLearningContextReceiptV1>,
}

fn project(
    events: &[Event],
    outcome: &ExternallyVerifiedOutcomeV1,
) -> Result<CollaborationLearningExerciseV1, CollaborationLearningError> {
    outcome
        .validate()
        .map_err(|error| err(format!("outcome is invalid: {error}")))?;
    let assignments = events
        .iter()
        .filter(|event| in_run(event, outcome))
        .filter(|event| {
            event.summary == COLLABORATION_LEARNING_ASSIGNMENT_SUMMARY
                || event
                    .metadata
                    .contains_key(COLLABORATION_LEARNING_ASSIGNMENT_SCHEMA_METADATA_KEY)
        })
        .collect::<Vec<_>>();
    let [assignment_event] = assignments.as_slice() else {
        return Err(err(if assignments.is_empty() {
            "trusted assignment is missing"
        } else {
            "trusted assignment is duplicated"
        }));
    };
    if assignment_event.sequence <= outcome.lifecycle.decision_sequence
        || assignment_event.sequence >= outcome.lifecycle.terminal_sequence
    {
        return Err(err("trusted assignment is outside the bound lifecycle"));
    }
    let assignment = parse_assignment(assignment_event, outcome)?;
    let policy = assignment.validate()?;
    let scoped = events
        .iter()
        .filter(|event| in_run(event, outcome))
        .filter(|event| {
            event.sequence > outcome.lifecycle.decision_sequence
                && event.sequence < outcome.lifecycle.terminal_sequence
        })
        .collect::<Vec<_>>();
    if scoped
        .windows(2)
        .any(|pair| pair[0].sequence >= pair[1].sequence)
    {
        return Err(err("events are duplicated or out of sequence"));
    }
    if events
        .iter()
        .filter(|event| in_run(event, outcome))
        .any(|event| {
            lane_model_event(event, &assignment)
                && (event.sequence <= outcome.lifecycle.decision_sequence
                    || event.sequence >= outcome.lifecycle.terminal_sequence)
        })
    {
        return Err(err("worker event is outside the bound lifecycle"));
    }
    let mut starts = BTreeMap::new();
    for event in &scoped {
        if event.kind == EventKind::ModelRequestStarted && lane_model_event(event, &assignment) {
            if event.sequence <= assignment.assignment_sequence {
                return Err(err("worker started before assignment"));
            }
            typed(event, EventTypeV1::AgentModelTurnStarted)?;
            let actual = parse_actual(event, true, &assignment, &policy)?;
            if starts.insert(actual.request.clone(), actual).is_some() {
                return Err(err("worker start is duplicated"));
            }
        }
    }
    let mut committed_contexts =
        committed_contexts(events, outcome, &assignment, &policy, &starts)?;
    let mut attempts = BTreeMap::<CollaborationLearningLaneActorV1, Vec<_>>::new();
    let mut finished = BTreeSet::new();
    for event in &scoped {
        if event.kind != EventKind::ModelRequestFinished || !lane_model_event(event, &assignment) {
            continue;
        }
        typed(event, EventTypeV1::AgentModelTurnFinished)?;
        let actual = parse_actual(event, false, &assignment, &policy)?;
        let start = starts
            .get(&actual.request)
            .ok_or_else(|| err("worker finish is orphaned"))?;
        if !finished.insert(actual.request.clone()) || actual.sequence <= start.sequence {
            return Err(err("worker finish is duplicated or out of sequence"));
        }
        if (
            actual.actor,
            &actual.step,
            actual.output,
            &actual.model,
            &actual.collaboration,
            &actual.stage,
            &actual.profile,
            &actual.component,
            &actual.legacy_role,
            &actual.runtime_stage,
            actual.recovery,
        ) != (
            start.actor,
            &start.step,
            start.output,
            &start.model,
            &start.collaboration,
            &start.stage,
            &start.profile,
            &start.component,
            &start.legacy_role,
            &start.runtime_stage,
            start.recovery,
        ) {
            return Err(err("worker attribution changed between start and finish"));
        }
        let status = req(&event.metadata, "status", "worker terminal status")?;
        let succeeded = status == "completed";
        if !matches!(status, "completed" | "degraded" | "interrupted")
            || (succeeded && event.metadata.get("output").is_none_or(String::is_empty))
        {
            return Err(err("worker terminal receipt is invalid"));
        }
        let context = attempt_context(
            start,
            &actual,
            committed_contexts
                .remove(&actual.request)
                .unwrap_or_default(),
            &assignment,
            policy.context_budget_bps,
        )?;
        attempts
            .entry(start.actor)
            .or_default()
            .push(CollaborationLearningWorkerAttemptV1 {
                request_id: start.request.clone(),
                model: start.model.clone(),
                started_sequence: start.sequence,
                finished_sequence: actual.sequence,
                recovery_attempt: start.recovery,
                context,
                provider_response_observed: provider_response_observed(event)?,
                succeeded,
            });
    }
    if finished.len() != starts.len() {
        return Err(err("worker attempt is unfinished"));
    }
    if !committed_contexts.is_empty() {
        return Err(err("committed context is orphaned"));
    }
    let mut lanes = Vec::new();
    for actor in [
        CollaborationLearningLaneActorV1::Specialist,
        CollaborationLearningLaneActorV1::IndependentVerifier,
    ] {
        if let Some(mut actual) = attempts.remove(&actor) {
            actual.sort_by_key(|attempt| attempt.started_sequence);
            let declared =
                assigned(&assignment, actor).ok_or_else(|| err("worker was not assigned"))?;
            lanes.push(CollaborationLearningLaneExerciseV1 {
                actor,
                workflow_step_id: declared.workflow_step_id.clone(),
                output_kind: declared.output_kind,
                final_success: actual.last().is_some_and(|attempt| attempt.succeeded),
                attempts: actual,
            });
        }
    }
    validate_lanes(&assignment, &policy, &lanes)?;
    let mut exercise = CollaborationLearningExerciseV1 {
        schema: COLLABORATION_LEARNING_EXERCISE_SCHEMA.to_string(),
        outcome_receipt_sha256: outcome.receipt_sha256.clone(),
        stop_reason: derive_stop(&policy, &assignment, &lanes)?,
        assignment,
        lanes,
        receipt_sha256: String::new(),
    };
    exercise.receipt_sha256 = exercise_digest(&exercise)?;
    exercise.validate_against(&policy, outcome)?;
    Ok(exercise)
}

fn parse_assignment(
    event: &Event,
    outcome: &ExternallyVerifiedOutcomeV1,
) -> Result<CollaborationLearningAssignmentV1, CollaborationLearningError> {
    if event.kind != EventKind::TaskStatusChanged
        || event.summary != COLLABORATION_LEARNING_ASSIGNMENT_SUMMARY
        || event
            .metadata
            .get(COLLABORATION_LEARNING_ASSIGNMENT_SCHEMA_METADATA_KEY)
            .map(String::as_str)
            != Some(COLLABORATION_LEARNING_ASSIGNMENT_SCHEMA)
    {
        return Err(err("assignment event schema is invalid"));
    }
    let run = req(&event.metadata, RUN, "assignment run")?;
    let epoch = number(&event.metadata, EPOCH, "assignment epoch")?;
    let plan = digest(&event.metadata, PLAN, "assignment plan")?;
    if run != outcome.lifecycle.agent_run_id
        || epoch != outcome.lifecycle.steer_epoch
        || plan != outcome.lifecycle.execution_plan_semantic_sha256
    {
        return Err(err(
            "assignment does not bind run, epoch, and semantic plan",
        ));
    }
    let policy_json = req(
        &event.metadata,
        COLLABORATION_LEARNING_POLICY_JSON_METADATA_KEY,
        "policy JSON",
    )?
    .to_string();
    let policy = CollaborationLearningPolicyV1::from_json(&policy_json)?;
    let assignment = CollaborationLearningAssignmentV1 {
        schema: COLLABORATION_LEARNING_ASSIGNMENT_SCHEMA.to_string(),
        agent_run_id: run.to_string(),
        steer_epoch: epoch,
        execution_plan_semantic_sha256: plan,
        policy_sha256: digest(
            &event.metadata,
            COLLABORATION_LEARNING_POLICY_SHA256_METADATA_KEY,
            "policy",
        )?,
        policy_json,
        case_binding_sha256: digest(
            &event.metadata,
            COLLABORATION_LEARNING_CASE_BINDING_SHA256_METADATA_KEY,
            "case binding",
        )?,
        assignment_sequence: event.sequence,
        plan_required_independent_verifier: boolean(
            &event.metadata,
            COLLABORATION_LEARNING_PLAN_REQUIRED_INDEPENDENT_VERIFIER_METADATA_KEY,
            "plan-required Independent Verifier",
        )?,
        collaboration_id: event.metadata.get(COLLABORATION).cloned(),
        specialist: parse_lane(event, CollaborationLearningLaneActorV1::Specialist)?,
        verifier: parse_lane(event, CollaborationLearningLaneActorV1::IndependentVerifier)?,
    };
    if policy.policy_sha256 != assignment.policy_sha256
        || policy.to_json()? != assignment.policy_json
    {
        return Err(err("assignment policy was changed after canonicalization"));
    }
    assignment.validate()?;
    Ok(assignment)
}

fn parse_lane(
    event: &Event,
    actor: CollaborationLearningLaneActorV1,
) -> Result<Option<CollaborationLearningLaneAssignmentV1>, CollaborationLearningError> {
    let (step_key, output_key, model_key, repair_key) = match actor {
        CollaborationLearningLaneActorV1::Specialist => (
            COLLABORATION_LEARNING_SPECIALIST_STEP_ID_METADATA_KEY,
            Some(COLLABORATION_LEARNING_SPECIALIST_OUTPUT_KIND_METADATA_KEY),
            COLLABORATION_LEARNING_SPECIALIST_MODEL_METADATA_KEY,
            COLLABORATION_LEARNING_SPECIALIST_REPAIR_MODEL_METADATA_KEY,
        ),
        CollaborationLearningLaneActorV1::IndependentVerifier => (
            COLLABORATION_LEARNING_VERIFIER_STEP_ID_METADATA_KEY,
            None,
            COLLABORATION_LEARNING_VERIFIER_MODEL_METADATA_KEY,
            COLLABORATION_LEARNING_VERIFIER_REPAIR_MODEL_METADATA_KEY,
        ),
    };
    let present = event.metadata.contains_key(step_key)
        || event.metadata.contains_key(model_key)
        || output_key.is_some_and(|key| event.metadata.contains_key(key));
    if !present {
        if event.metadata.contains_key(repair_key) {
            return Err(err("orphan repair assignment"));
        }
        return Ok(None);
    }
    let output_kind = output_key
        .map(|key| {
            req(&event.metadata, key, "assigned output kind")
                .and_then(CollaborationLearningOutputKindV1::parse)
        })
        .transpose()?
        .unwrap_or(CollaborationLearningOutputKindV1::Verification);
    Ok(Some(CollaborationLearningLaneAssignmentV1 {
        actor,
        workflow_step_id: req(&event.metadata, step_key, "assigned step")?.to_string(),
        output_kind,
        initial_model: req(&event.metadata, model_key, "assigned model")?.to_string(),
        repair_model: event.metadata.get(repair_key).cloned(),
    }))
}

fn parse_actual(
    event: &Event,
    start: bool,
    assignment: &CollaborationLearningAssignmentV1,
    policy: &CollaborationLearningPolicyV1,
) -> Result<Actual, CollaborationLearningError> {
    let actor = worker(event).ok_or_else(|| err("worker actor is missing"))?;
    let stage = req(&event.metadata, AGENT_STAGE_METADATA_KEY, "actor stage")?;
    let profile = req(
        &event.metadata,
        AGENT_MODEL_PROFILE_METADATA_KEY,
        "model profile",
    )?;
    let component = req(
        &event.metadata,
        AGENT_ATTRIBUTION_COMPONENT_METADATA_KEY,
        "attribution component",
    )?;
    let legacy_role = req(
        &event.metadata,
        AGENT_ATTRIBUTION_LEGACY_ROLE_METADATA_KEY,
        "legacy attribution role",
    )?;
    if event
        .metadata
        .get(AGENT_MODEL_ATTRIBUTION_SCHEMA_METADATA_KEY)
        .map(String::as_str)
        != Some(AGENT_MODEL_ATTRIBUTION_SCHEMA)
        || event
            .metadata
            .get(AGENT_SERVICE_METADATA_KEY)
            .map(String::as_str)
            != Some("none")
        || event
            .metadata
            .get(AGENT_EFFECT_AUTHORITY_METADATA_KEY)
            .map(String::as_str)
            != Some("read_only")
        || event
            .metadata
            .get(AGENT_OUTPUT_TRUST_METADATA_KEY)
            .map(String::as_str)
            != Some("untrusted_model_output")
    {
        return Err(err("worker attribution is invalid"));
    }
    let recovery = match (
        event.metadata.get(RECOVERY).map(String::as_str),
        event.metadata.get(RECOVERY_ATTEMPT),
    ) {
        (None, None) => None,
        (Some("true"), Some(value)) if value == "2" => Some(2),
        _ => return Err(err("worker recovery metadata is invalid")),
    };
    let model = req(
        &event.metadata,
        AGENT_ATTRIBUTION_MODEL_METADATA_KEY,
        "worker model",
    )?;
    let declared =
        assigned(assignment, actor).ok_or_else(|| err("actual worker was not assigned"))?;
    let expected_model = recovery
        .and(declared.repair_model.as_deref())
        .unwrap_or(declared.initial_model.as_str());
    let step = req(&event.metadata, STEP, "worker step")?;
    let output =
        CollaborationLearningOutputKindV1::parse(req(&event.metadata, OUTPUT, "worker output")?)?;
    let valid_actor = match (actor, output) {
        (
            CollaborationLearningLaneActorV1::Specialist,
            CollaborationLearningOutputKindV1::Analysis,
        ) => stage == "plan" && matches!(profile, "primary" | "reasoning"),
        (
            CollaborationLearningLaneActorV1::Specialist,
            CollaborationLearningOutputKindV1::Evidence,
        ) => stage == "evidence" && matches!(profile, "primary" | "reasoning"),
        (
            CollaborationLearningLaneActorV1::IndependentVerifier,
            CollaborationLearningOutputKindV1::Verification,
        ) => stage == "verify" && profile == "verifier",
        _ => false,
    };
    let collaboration = req(&event.metadata, COLLABORATION, "collaboration id")?;
    if !valid_actor
        || model != expected_model
        || event.metadata.get("model").map(String::as_str) != Some(model)
        || step != declared.workflow_step_id
        || output != declared.output_kind
        || Some(collaboration) != assignment.collaboration_id.as_deref()
    {
        return Err(err("declared and actual worker assignment disagree"));
    }
    let context = start
        .then(|| legacy_context(event, policy.context_budget_bps))
        .transpose()?;
    Ok(Actual {
        actor,
        request: req(&event.metadata, REQUEST, "request id")?.to_string(),
        step: step.to_string(),
        output,
        model: model.to_string(),
        collaboration: collaboration.to_string(),
        stage: stage.to_string(),
        profile: profile.to_string(),
        component: component.to_string(),
        legacy_role: legacy_role.to_string(),
        runtime_stage: event.metadata.get("stage").cloned(),
        recovery,
        sequence: event.sequence,
        context: context.flatten(),
    })
}

#[derive(Debug, Clone)]
struct CommittedContext {
    sequence: u64,
    ordinal: u64,
    payload_sha256: String,
    bytes: u64,
}

fn committed_contexts(
    events: &[Event],
    outcome: &ExternallyVerifiedOutcomeV1,
    assignment: &CollaborationLearningAssignmentV1,
    policy: &CollaborationLearningPolicyV1,
    starts: &BTreeMap<String, Actual>,
) -> Result<BTreeMap<String, Vec<CommittedContext>>, CollaborationLearningError> {
    let mut committed = BTreeMap::<String, Vec<CommittedContext>>::new();
    for event in events
        .iter()
        .filter(|event| context_committed_marker(event))
    {
        let request = event.metadata.get(REQUEST).map(String::as_str);
        if !in_run(event, outcome) {
            if request.is_some_and(|request| starts.contains_key(request)) {
                return Err(err("committed context changed run or epoch"));
            }
            continue;
        }
        if event.sequence <= outcome.lifecycle.decision_sequence
            || event.sequence >= outcome.lifecycle.terminal_sequence
        {
            return Err(err("committed context is outside the bound lifecycle"));
        }
        if event.kind != EventKind::TaskStatusChanged
            || event.summary != COLLABORATION_LEARNING_CONTEXT_COMMITTED_SUMMARY
            || event
                .metadata
                .get(COLLABORATION_LEARNING_CONTEXT_SCHEMA_METADATA_KEY)
                .map(String::as_str)
                != Some(COLLABORATION_LEARNING_CONTEXT_RECEIPT_SCHEMA)
        {
            return Err(err("committed context event schema is invalid"));
        }
        let request = req(&event.metadata, REQUEST, "committed context request")?;
        let start = starts
            .get(request)
            .ok_or_else(|| err("committed context is orphaned"))?;
        if event.sequence <= start.sequence {
            return Err(err("committed context was not recorded after worker start"));
        }
        let plan = digest(&event.metadata, PLAN, "committed context plan")?;
        let budget = number(
            &event.metadata,
            COLLABORATION_LEARNING_CONTEXT_BUDGET_BPS_METADATA_KEY,
            "committed context budget",
        )?;
        let payload_sha256 = digest(
            &event.metadata,
            COLLABORATION_LEARNING_CONTEXT_PAYLOAD_SHA256_METADATA_KEY,
            "committed context payload",
        )?;
        let bytes = number(
            &event.metadata,
            COLLABORATION_LEARNING_CONTEXT_BYTES_METADATA_KEY,
            "committed context bytes",
        )?;
        let ordinal = number(
            &event.metadata,
            COLLABORATION_LEARNING_WORKER_TURN_ORDINAL_METADATA_KEY,
            "committed context turn ordinal",
        )?;
        let runtime_stage = start
            .runtime_stage
            .as_deref()
            .ok_or_else(|| err("worker start is missing its runtime stage"))?;
        if plan != assignment.execution_plan_semantic_sha256
            || budget != u64::from(policy.context_budget_bps)
            || bytes == 0
            || ordinal == 0
            || req(&event.metadata, COLLABORATION, "committed collaboration")?
                != start.collaboration
            || req(&event.metadata, "stage", "committed runtime stage")? != runtime_stage
            || req(&event.metadata, "model", "committed model")? != start.model
            || event.metadata.get(REQUEST_PAYLOAD).map(String::as_str)
                != Some(payload_sha256.as_str())
            || req(&event.metadata, STEP, "committed workflow step")? != start.step
        {
            return Err(err(
                "committed context disagrees with its plan, budget, step, or model",
            ));
        }
        committed
            .entry(request.to_string())
            .or_default()
            .push(CommittedContext {
                sequence: event.sequence,
                ordinal,
                payload_sha256,
                bytes,
            });
    }
    Ok(committed)
}

fn context_committed_marker(event: &Event) -> bool {
    event.summary == COLLABORATION_LEARNING_CONTEXT_COMMITTED_SUMMARY
        || event
            .metadata
            .contains_key(COLLABORATION_LEARNING_WORKER_TURN_ORDINAL_METADATA_KEY)
}

fn attempt_context(
    start: &Actual,
    finish: &Actual,
    mut committed: Vec<CommittedContext>,
    assignment: &CollaborationLearningAssignmentV1,
    expected_bps: u16,
) -> Result<CollaborationLearningContextReceiptV1, CollaborationLearningError> {
    if committed.is_empty() {
        return start
            .context
            .clone()
            .ok_or_else(|| err("trusted actual context receipt is missing"));
    }
    if start.context.is_some() {
        return Err(err("actual context receipt is duplicated"));
    }
    committed.sort_by_key(|receipt| receipt.ordinal);
    if committed
        .iter()
        .enumerate()
        .any(|(index, receipt)| receipt.ordinal != u64::try_from(index + 1).unwrap_or(u64::MAX))
        || committed
            .windows(2)
            .any(|pair| pair[0].ordinal == pair[1].ordinal || pair[0].sequence >= pair[1].sequence)
        || committed
            .iter()
            .any(|receipt| receipt.sequence >= finish.sequence)
    {
        return Err(err(
            "committed context turns are duplicated, non-contiguous, or outside the attempt",
        ));
    }
    let bytes = committed.iter().try_fold(0u64, |total, receipt| {
        total
            .checked_add(receipt.bytes)
            .ok_or_else(|| err("committed context byte count overflowed"))
    })?;
    let payloads = committed
        .iter()
        .map(|receipt| {
            (
                receipt.ordinal,
                receipt.sequence,
                receipt.payload_sha256.as_str(),
                receipt.bytes,
            )
        })
        .collect::<Vec<_>>();
    let payload_sha256 = collaboration_learning_sha256(
        CONTEXT_AGGREGATE_HASH_DOMAIN,
        &(
            CONTEXT_AGGREGATE_SCHEMA,
            assignment.agent_run_id.as_str(),
            assignment.steer_epoch,
            assignment.execution_plan_semantic_sha256.as_str(),
            start.collaboration.as_str(),
            start.request.as_str(),
            start.step.as_str(),
            start.model.as_str(),
            start.sequence,
            finish.sequence,
            expected_bps,
            &payloads,
        ),
        "aggregate committed context",
    )?;
    Ok(CollaborationLearningContextReceiptV1 {
        budget_bps: expected_bps,
        payload_sha256,
        bytes,
    })
}

fn legacy_context(
    event: &Event,
    expected_bps: u16,
) -> Result<Option<CollaborationLearningContextReceiptV1>, CollaborationLearningError> {
    let present = [
        COLLABORATION_LEARNING_CONTEXT_SCHEMA_METADATA_KEY,
        COLLABORATION_LEARNING_CONTEXT_BUDGET_BPS_METADATA_KEY,
        COLLABORATION_LEARNING_CONTEXT_PAYLOAD_SHA256_METADATA_KEY,
        COLLABORATION_LEARNING_CONTEXT_BYTES_METADATA_KEY,
    ]
    .into_iter()
    .any(|key| event.metadata.contains_key(key));
    present.then(|| context(event, expected_bps)).transpose()
}

fn context(
    event: &Event,
    expected_bps: u16,
) -> Result<CollaborationLearningContextReceiptV1, CollaborationLearningError> {
    if event
        .metadata
        .get(COLLABORATION_LEARNING_CONTEXT_SCHEMA_METADATA_KEY)
        .map(String::as_str)
        != Some(COLLABORATION_LEARNING_CONTEXT_RECEIPT_SCHEMA)
    {
        return Err(err("trusted actual context receipt is missing"));
    }
    let budget = number(
        &event.metadata,
        COLLABORATION_LEARNING_CONTEXT_BUDGET_BPS_METADATA_KEY,
        "context budget",
    )?;
    let bytes = number(
        &event.metadata,
        COLLABORATION_LEARNING_CONTEXT_BYTES_METADATA_KEY,
        "context bytes",
    )?;
    let receipt = CollaborationLearningContextReceiptV1 {
        budget_bps: u16::try_from(budget).map_err(|_| err("context budget is invalid"))?,
        payload_sha256: digest(
            &event.metadata,
            COLLABORATION_LEARNING_CONTEXT_PAYLOAD_SHA256_METADATA_KEY,
            "context payload",
        )?,
        bytes,
    };
    if receipt.budget_bps != expected_bps || receipt.bytes == 0 {
        return Err(err("actual context receipt disagrees with assigned budget"));
    }
    Ok(receipt)
}

fn provider_response_observed(event: &Event) -> Result<bool, CollaborationLearningError> {
    if let Some(raw) = event.metadata.get("worker_model_responses") {
        return raw
            .parse::<usize>()
            .map(|count| count > 0)
            .map_err(|_| err("worker provider response count is invalid"));
    }
    if let Some(receipt) = event.metadata.get("request_payload_sha256") {
        validate_collaboration_learning_sha256(receipt, "worker request payload")?;
        return Ok(true);
    }
    Ok(event.summary == "Agent model turn finished"
        || event.metadata.get("status").map(String::as_str) == Some("completed")
        || event.metadata.contains_key("output"))
}

fn validate_assignment_lane(
    lane: &CollaborationLearningLaneAssignmentV1,
    policy: &CollaborationLearningPolicyV1,
    actor: CollaborationLearningLaneActorV1,
) -> Result<(), CollaborationLearningError> {
    let kind_ok = match actor {
        CollaborationLearningLaneActorV1::Specialist => {
            lane.output_kind != CollaborationLearningOutputKindV1::Verification
        }
        CollaborationLearningLaneActorV1::IndependentVerifier => {
            lane.output_kind == CollaborationLearningOutputKindV1::Verification
        }
    };
    if lane.actor != actor
        || lane.workflow_step_id.is_empty()
        || lane.initial_model.is_empty()
        || !kind_ok
    {
        return Err(err("lane assignment is invalid"));
    }
    let repair_ok = match policy.repair {
        CollaborationRepairV1::FailFast => lane.repair_model.is_none(),
        CollaborationRepairV1::SameModelOnce => {
            lane.repair_model.as_deref() == Some(lane.initial_model.as_str())
        }
        CollaborationRepairV1::AlternateModelOnce => lane
            .repair_model
            .as_deref()
            .is_some_and(|model| !model.is_empty() && model != lane.initial_model),
    };
    repair_ok
        .then_some(())
        .ok_or_else(|| err("repair assignment disagrees with policy"))
}

fn validate_lanes(
    assignment: &CollaborationLearningAssignmentV1,
    policy: &CollaborationLearningPolicyV1,
    lanes: &[CollaborationLearningLaneExerciseV1],
) -> Result<(), CollaborationLearningError> {
    if policy.specialist_invocation == CollaborationSpecialistInvocationV1::DirectOwnerOnly {
        return lanes
            .is_empty()
            .then_some(())
            .ok_or_else(|| err("Direct exercised a worker"));
    }
    if lanes.first().map(|lane| lane.actor) != Some(CollaborationLearningLaneActorV1::Specialist)
        || lanes.len() > 2
    {
        return Err(err("Workflow does not have exactly one Specialist lane"));
    }
    for lane in lanes {
        let declared =
            assigned(assignment, lane.actor).ok_or_else(|| err("lane was not assigned"))?;
        if lane.workflow_step_id != declared.workflow_step_id
            || lane.output_kind != declared.output_kind
            || lane.attempts.is_empty()
            || lane.attempts.len() > 2
            || lane.final_success
                != lane
                    .attempts
                    .last()
                    .is_some_and(|attempt| attempt.succeeded)
        {
            return Err(err("actual lane disagrees with assignment"));
        }
        let first = &lane.attempts[0];
        if first.recovery_attempt.is_some() || first.model != declared.initial_model {
            return Err(err("initial lane attempt is invalid"));
        }
        if let Some(repair) = lane.attempts.get(1) {
            if first.succeeded
                || repair.recovery_attempt != Some(2)
                || Some(repair.model.as_str()) != declared.repair_model.as_deref()
                || repair.started_sequence <= first.finished_sequence
            {
                return Err(err("repair is not one same-lane second attempt"));
            }
        } else if !first.succeeded && policy.repair != CollaborationRepairV1::FailFast {
            return Err(err("assigned repair was skipped after failure"));
        }
        if lane.attempts.iter().any(|attempt| {
            attempt.request_id.is_empty()
                || attempt.finished_sequence <= attempt.started_sequence
                || attempt.context.budget_bps != policy.context_budget_bps
                || attempt.context.bytes == 0
                || validate_collaboration_learning_sha256(
                    &attempt.context.payload_sha256,
                    "context",
                )
                .is_err()
        }) {
            return Err(err("worker attempt or context is invalid"));
        }
    }
    let specialist = &lanes[0];
    let verifier = lanes.get(1);
    if verifier
        .is_some_and(|lane| lane.actor != CollaborationLearningLaneActorV1::IndependentVerifier)
        || (assignment.verifier.is_some() && specialist.final_success) != verifier.is_some()
        || (!specialist.final_success && verifier.is_some())
    {
        return Err(err("Verifier exercise disagrees with serial assignment"));
    }
    if let Some(verifier) = verifier {
        if verifier.attempts[0].started_sequence
            <= specialist.attempts.last().unwrap().finished_sequence
            || specialist
                .attempts
                .iter()
                .map(|attempt| &attempt.model)
                .collect::<BTreeSet<_>>()
                .intersection(
                    &verifier
                        .attempts
                        .iter()
                        .map(|attempt| &attempt.model)
                        .collect(),
                )
                .next()
                .is_some()
        {
            return Err(err("Verifier is not serial and model-distinct"));
        }
    }
    Ok(())
}

fn derive_stop(
    policy: &CollaborationLearningPolicyV1,
    assignment: &CollaborationLearningAssignmentV1,
    lanes: &[CollaborationLearningLaneExerciseV1],
) -> Result<CollaborationDerivedStopReasonV1, CollaborationLearningError> {
    if policy.specialist_invocation == CollaborationSpecialistInvocationV1::DirectOwnerOnly {
        return Ok(CollaborationDerivedStopReasonV1::DirectOwnerTerminal);
    }
    let specialist = lanes
        .first()
        .ok_or_else(|| err("stop is missing Specialist evidence"))?;
    let verifier = lanes.get(1);
    let complete = specialist.final_success
        && (assignment.verifier.is_none() || verifier.is_some_and(|lane| lane.final_success));
    Ok(if !complete {
        CollaborationDerivedStopReasonV1::OwnerFallbackAfterLaneFailure
    } else if lanes.iter().any(|lane| lane.attempts.len() == 2) {
        CollaborationDerivedStopReasonV1::RequiredLanesCompletedAfterRepair
    } else {
        CollaborationDerivedStopReasonV1::RequiredLanesCompleted
    })
}

fn validate_exposure(
    exercise: &CollaborationLearningExerciseV1,
    policy: &CollaborationLearningPolicyV1,
    outcome: &ExternallyVerifiedOutcomeV1,
) -> Result<(), CollaborationLearningError> {
    let attempts = exercise
        .lanes
        .iter()
        .flat_map(|lane| &lane.attempts)
        .collect::<Vec<_>>();
    let successful = |actor| {
        exercise
            .lanes
            .iter()
            .filter(move |lane| lane.actor == actor)
            .flat_map(|lane| &lane.attempts)
            .filter(|attempt| attempt.provider_response_observed)
            .collect::<Vec<_>>()
    };
    let specialist = successful(CollaborationLearningLaneActorV1::Specialist);
    let verifier = successful(CollaborationLearningLaneActorV1::IndependentVerifier);
    let models = |values: &[&CollaborationLearningWorkerAttemptV1]| {
        values
            .iter()
            .map(|attempt| attempt.model.clone())
            .collect::<BTreeSet<_>>()
    };
    let workflow =
        policy.specialist_invocation == CollaborationSpecialistInvocationV1::OneReadOnlySpecialist;
    let exposure = &outcome.exposure;
    if exposure.worker_model_calls != attempts.len()
        || exposure.successful_specialist_model_calls != specialist.len()
        || exposure.successful_independent_verifier_model_calls != verifier.len()
        || exposure.successful_workflow_specialist_model_calls != specialist.len()
        || exposure.successful_workflow_verifier_model_calls != verifier.len()
        || exposure.worker_models != models(&attempts)
        || exposure.successful_workflow_specialist_models != models(&specialist)
        || exposure.successful_workflow_verifier_models != models(&verifier)
        || exposure.workflow_planned != workflow
        || exposure.workflow_completed
            != (workflow
                && exercise.stop_reason
                    != CollaborationDerivedStopReasonV1::OwnerFallbackAfterLaneFailure)
    {
        return Err(err(
            "actual exercise disagrees with outcome treatment exposure",
        ));
    }
    Ok(())
}

fn exercise_digest(
    exercise: &CollaborationLearningExerciseV1,
) -> Result<String, CollaborationLearningError> {
    collaboration_learning_sha256(
        HASH_DOMAIN,
        &(
            exercise.schema.as_str(),
            exercise.outcome_receipt_sha256.as_str(),
            &exercise.assignment,
            &exercise.lanes,
            exercise.stop_reason,
        ),
        "exercise",
    )
}

fn assigned(
    assignment: &CollaborationLearningAssignmentV1,
    actor: CollaborationLearningLaneActorV1,
) -> Option<&CollaborationLearningLaneAssignmentV1> {
    match actor {
        CollaborationLearningLaneActorV1::Specialist => assignment.specialist.as_ref(),
        CollaborationLearningLaneActorV1::IndependentVerifier => assignment.verifier.as_ref(),
    }
}

fn lane_models(lane: &CollaborationLearningLaneAssignmentV1) -> BTreeSet<&str> {
    [
        Some(lane.initial_model.as_str()),
        lane.repair_model.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn worker(event: &Event) -> Option<CollaborationLearningLaneActorV1> {
    match event
        .metadata
        .get(AGENT_ACTOR_METADATA_KEY)
        .map(String::as_str)
    {
        Some("specialist") => Some(CollaborationLearningLaneActorV1::Specialist),
        Some("independent_verifier") => Some(CollaborationLearningLaneActorV1::IndependentVerifier),
        _ => None,
    }
}

fn lane_model_event(event: &Event, assignment: &CollaborationLearningAssignmentV1) -> bool {
    if !matches!(
        event.kind,
        EventKind::ModelRequestStarted | EventKind::ModelRequestFinished
    ) {
        return false;
    }
    if worker(event).is_some() {
        return true;
    }
    let step = event.metadata.get(STEP).map(String::as_str);
    [assignment.specialist.as_ref(), assignment.verifier.as_ref()]
        .into_iter()
        .flatten()
        .any(|lane| Some(lane.workflow_step_id.as_str()) == step)
}

fn in_run(event: &Event, outcome: &ExternallyVerifiedOutcomeV1) -> bool {
    event.metadata.get(RUN).map(String::as_str) == Some(outcome.lifecycle.agent_run_id.as_str())
        && event
            .metadata
            .get(EPOCH)
            .and_then(|value| value.parse().ok())
            == Some(outcome.lifecycle.steer_epoch)
}

fn typed(event: &Event, expected: EventTypeV1) -> Result<(), CollaborationLearningError> {
    matches!(decode_event_type(event), DecodedEventType::V1(value) if value.event_type() == expected)
        .then_some(())
        .ok_or_else(|| err("worker model lifecycle type is invalid"))
}

fn req<'a>(
    metadata: &'a Metadata,
    key: &str,
    label: &str,
) -> Result<&'a str, CollaborationLearningError> {
    metadata
        .get(key)
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| err(format!("{label} is missing")))
}

fn number(metadata: &Metadata, key: &str, label: &str) -> Result<u64, CollaborationLearningError> {
    req(metadata, key, label)?
        .parse()
        .map_err(|_| err(format!("{label} is invalid")))
}

fn boolean(
    metadata: &Metadata,
    key: &str,
    label: &str,
) -> Result<bool, CollaborationLearningError> {
    match req(metadata, key, label)? {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(err(format!("{label} is invalid"))),
    }
}

fn digest(
    metadata: &Metadata,
    key: &str,
    label: &str,
) -> Result<String, CollaborationLearningError> {
    let value = req(metadata, key, label)?;
    validate_collaboration_learning_sha256(value, label)?;
    Ok(value.to_string())
}

fn err(message: impl Into<String>) -> CollaborationLearningError {
    CollaborationLearningError::new(format!("collaboration learning {}", message.into()))
}

#[cfg(test)]
#[path = "collaboration_learning_projection_tests.rs"]
mod tests;
#[cfg(test)]
pub(crate) use tests::test_support;
