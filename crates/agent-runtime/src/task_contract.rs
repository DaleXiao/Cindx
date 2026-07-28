use crate::{AgentFailure, InteractionSurface};
use agent_core::{ToolOutcomeStatus, ToolRisk, ToolSpec};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const MAX_CONTRACT_EVIDENCE: usize = 128;
const MAX_COMPLETION_GATE_ATTEMPTS: usize = 2;
const MAX_CONTEXT_TARGETS: usize = 8;
const MAX_CONTEXT_EVIDENCE: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContractEvidenceKind {
    RequiredTool,
    Read,
    Mutation,
    Verification,
    InteractionAction,
    InteractionObservation,
    OtherTool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceVerificationPolicy {
    #[default]
    NotRequired,
    RequiredAfterMutation,
}

impl WorkspaceVerificationPolicy {
    pub fn is_required(self) -> bool {
        self == Self::RequiredAfterMutation
    }

    fn merge(self, other: Self) -> Self {
        if self.is_required() || other.is_required() {
            Self::RequiredAfterMutation
        } else {
            Self::NotRequired
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractEvidence {
    pub sequence: u64,
    pub kind: ContractEvidenceKind,
    pub source: String,
    pub input_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskContractContext {
    schema: &'static str,
    unresolved_required_tools: Vec<String>,
    unresolved_any_tool_requirements: Vec<TaskContractAnyToolRequirement>,
    pending_postconditions: Vec<TaskContractPostcondition>,
    unverified_workspace_mutation: Option<TaskContractMutation>,
    recent_evidence: Vec<TaskContractEvidenceSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskContractAnyToolRequirement {
    id: String,
    alternatives: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskContractPostcondition {
    surface: &'static str,
    action: String,
    observers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskContractMutation {
    epoch: u64,
    targets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskContractEvidenceSummary {
    sequence: u64,
    kind: ContractEvidenceKind,
    source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentTaskContract {
    #[serde(default)]
    workspace_verification_policy: WorkspaceVerificationPolicy,
    #[serde(default)]
    required_tool_successes: BTreeSet<String>,
    #[serde(default)]
    required_any_tool_successes: BTreeMap<String, BTreeSet<String>>,
    #[serde(default)]
    successful_tools: BTreeSet<String>,
    #[serde(default)]
    mutation_targets: BTreeSet<String>,
    #[serde(default)]
    mutation_epoch: u64,
    #[serde(default)]
    verified_mutation_epoch: u64,
    #[serde(default)]
    pending_interactions: BTreeMap<InteractionSurface, String>,
    #[serde(default)]
    evidence: Vec<ContractEvidence>,
    #[serde(default)]
    next_sequence: u64,
    #[serde(default)]
    gate_attempts: BTreeMap<String, usize>,
}

impl AgentTaskContract {
    pub fn merge_workspace_verification_policy(&mut self, policy: WorkspaceVerificationPolicy) {
        self.workspace_verification_policy = self.workspace_verification_policy.merge(policy);
    }

    pub fn workspace_verification_policy(&self) -> WorkspaceVerificationPolicy {
        self.workspace_verification_policy
    }

    pub fn require_tool_success(&mut self, tool_name: impl Into<String>) {
        let tool_name = tool_name.into();
        if !tool_name.trim().is_empty() {
            self.required_tool_successes.insert(tool_name);
        }
    }

    pub fn require_any_tool_success<I, S>(&mut self, requirement_id: impl Into<String>, tools: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let requirement_id = requirement_id.into();
        let requirement_id = requirement_id.trim();
        if requirement_id.is_empty() {
            return;
        }
        let alternatives = tools
            .into_iter()
            .map(Into::into)
            .map(|tool: String| tool.trim().to_string())
            .filter(|tool| !tool.is_empty())
            .collect::<BTreeSet<_>>();
        if alternatives.is_empty() {
            return;
        }
        self.required_any_tool_successes
            .entry(requirement_id.to_string())
            .or_default()
            .extend(alternatives);
    }

    pub fn required_tool_satisfied(&self, tool_name: &str) -> bool {
        self.successful_tools.contains(tool_name)
    }

    pub fn successful_mutations(&self) -> usize {
        self.mutation_epoch as usize
    }

    pub fn latest_mutation_verified(&self) -> bool {
        self.mutation_epoch == 0 || self.verified_mutation_epoch >= self.mutation_epoch
    }

    pub fn pending_interactions(&self) -> &BTreeMap<InteractionSurface, String> {
        &self.pending_interactions
    }

    pub fn evidence(&self) -> &[ContractEvidence] {
        &self.evidence
    }

    /// Returns the active task obligations as trusted, bounded runtime data.
    ///
    /// Rendering this context is deliberately side-effect free: repair-attempt
    /// accounting belongs to the completion gate, not to ordinary model turns.
    pub fn model_context(&self, verification_required: bool, tools: &[ToolSpec]) -> Option<String> {
        self.model_context_with_requirement(verification_required, tools)
    }

    pub fn model_context_for_task(&self, tools: &[ToolSpec]) -> Option<String> {
        self.model_context_with_requirement(self.workspace_verification_policy.is_required(), tools)
    }

    fn model_context_with_requirement(
        &self,
        verification_required: bool,
        tools: &[ToolSpec],
    ) -> Option<String> {
        let available_tools = tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<BTreeSet<_>>();
        let unresolved_required_tools = self
            .required_tool_successes
            .iter()
            .filter(|tool_name| !self.successful_tools.contains(*tool_name))
            .cloned()
            .collect::<Vec<_>>();
        let unresolved_any_tool_requirements = self
            .required_any_tool_successes
            .iter()
            .filter(|(_, alternatives)| alternatives.is_disjoint(&self.successful_tools))
            .map(|(id, alternatives)| TaskContractAnyToolRequirement {
                id: id.clone(),
                alternatives: alternatives
                    .iter()
                    .filter(|tool| available_tools.contains(tool.as_str()))
                    .cloned()
                    .collect(),
            })
            .collect::<Vec<_>>();
        let pending_postconditions = self
            .pending_interactions
            .iter()
            .filter_map(|(surface, action)| {
                let observers = interaction_observers(*surface, action)
                    .iter()
                    .copied()
                    .filter(|candidate| available_tools.contains(candidate))
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                (!observers.is_empty()).then(|| TaskContractPostcondition {
                    surface: surface.label(),
                    action: action.clone(),
                    observers,
                })
            })
            .collect::<Vec<_>>();
        let unverified_workspace_mutation = (verification_required
            && self.mutation_epoch > 0
            && !self.latest_mutation_verified()
            && tools.iter().any(tool_can_verify_workspace_change))
        .then(|| TaskContractMutation {
            epoch: self.mutation_epoch,
            targets: self
                .mutation_targets
                .iter()
                .take(MAX_CONTEXT_TARGETS)
                .cloned()
                .collect(),
        });

        if unresolved_required_tools.is_empty()
            && unresolved_any_tool_requirements.is_empty()
            && pending_postconditions.is_empty()
            && unverified_workspace_mutation.is_none()
        {
            return None;
        }

        let recent_evidence = self
            .evidence
            .iter()
            .rev()
            .take(MAX_CONTEXT_EVIDENCE)
            .rev()
            .map(|evidence| TaskContractEvidenceSummary {
                sequence: evidence.sequence,
                kind: evidence.kind,
                source: evidence.source.clone(),
            })
            .collect();
        let context = TaskContractContext {
            schema: "cindx.task-contract.v1",
            unresolved_required_tools,
            unresolved_any_tool_requirements,
            pending_postconditions,
            unverified_workspace_mutation,
            recent_evidence,
        };
        serde_json::to_string(&context).ok()
    }

    pub(crate) fn restore_legacy(
        successful_mutations: usize,
        verified_after_last_mutation: bool,
        pending_interactions: BTreeMap<InteractionSurface, String>,
    ) -> Self {
        Self {
            mutation_epoch: successful_mutations as u64,
            verified_mutation_epoch: if verified_after_last_mutation {
                successful_mutations as u64
            } else {
                0
            },
            pending_interactions,
            ..Self::default()
        }
    }

    pub fn record_tool_outcome(
        &mut self,
        tool_name: &str,
        input_json: &str,
        status: &ToolOutcomeStatus,
        risk: Option<&ToolRisk>,
    ) {
        if !matches!(status, ToolOutcomeStatus::Succeeded) {
            return;
        }

        self.successful_tools.insert(tool_name.to_string());
        for requirement_id in self
            .required_any_tool_successes
            .iter()
            .filter(|(_, alternatives)| alternatives.contains(tool_name))
            .map(|(requirement_id, _)| requirement_id.clone())
            .collect::<Vec<_>>()
        {
            self.gate_attempts
                .remove(&format!("any_tool:{requirement_id}"));
        }
        if self.required_tool_successes.contains(tool_name) {
            self.gate_attempts.remove(&format!("tool:{tool_name}"));
            self.record_evidence(ContractEvidenceKind::RequiredTool, tool_name, input_json);
        }

        if let Some((surface, action)) = interaction_action(tool_name) {
            self.pending_interactions
                .insert(surface, action.to_string());
            self.gate_attempts.remove("interaction_verification");
            self.record_evidence(
                ContractEvidenceKind::InteractionAction,
                tool_name,
                input_json,
            );
            return;
        }
        if let Some(surface) = interaction_observation(tool_name) {
            let verified = self
                .pending_interactions
                .get(&surface)
                .is_some_and(|action| interaction_observation_verifies(action, tool_name));
            if verified {
                self.pending_interactions.remove(&surface);
                self.gate_attempts.remove("interaction_verification");
            }
            self.record_evidence(
                ContractEvidenceKind::InteractionObservation,
                tool_name,
                input_json,
            );
            return;
        }

        match risk.or_else(|| inferred_builtin_tool_risk(tool_name)) {
            Some(ToolRisk::WritesWorkspace | ToolRisk::Destructive) => {
                self.mutation_epoch = self.mutation_epoch.saturating_add(1);
                self.mutation_targets = structured_targets(input_json);
                self.gate_attempts.remove("workspace_verification");
                self.record_evidence(ContractEvidenceKind::Mutation, tool_name, input_json);
            }
            Some(ToolRisk::ReadOnly) => {
                let verifies_mutation = self.mutation_epoch > self.verified_mutation_epoch
                    && verification_targets_match(&self.mutation_targets, input_json);
                if verifies_mutation {
                    self.verified_mutation_epoch = self.mutation_epoch;
                    self.gate_attempts.remove("workspace_verification");
                }
                self.record_evidence(
                    if verifies_mutation {
                        ContractEvidenceKind::Verification
                    } else {
                        ContractEvidenceKind::Read
                    },
                    tool_name,
                    input_json,
                );
            }
            Some(ToolRisk::ExecutesProcess) => {
                let verifies_mutation = self.mutation_epoch > self.verified_mutation_epoch
                    && process_input_looks_like_verification(input_json);
                if verifies_mutation {
                    self.verified_mutation_epoch = self.mutation_epoch;
                    self.gate_attempts.remove("workspace_verification");
                }
                self.record_evidence(
                    if verifies_mutation {
                        ContractEvidenceKind::Verification
                    } else {
                        ContractEvidenceKind::OtherTool
                    },
                    tool_name,
                    input_json,
                );
            }
            _ => self.record_evidence(ContractEvidenceKind::OtherTool, tool_name, input_json),
        }
    }

    pub fn completion_instruction(
        &mut self,
        verification_required: bool,
        tools: &[ToolSpec],
    ) -> Result<Option<String>, AgentFailure> {
        self.completion_instruction_with_requirement(verification_required, tools)
    }

    pub fn completion_instruction_for_task(
        &mut self,
        tools: &[ToolSpec],
    ) -> Result<Option<String>, AgentFailure> {
        self.completion_instruction_with_requirement(
            self.workspace_verification_policy.is_required(),
            tools,
        )
    }

    fn completion_instruction_with_requirement(
        &mut self,
        verification_required: bool,
        tools: &[ToolSpec],
    ) -> Result<Option<String>, AgentFailure> {
        let available_tools = tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<BTreeSet<_>>();

        if let Some(tool_name) = self
            .required_tool_successes
            .iter()
            .find(|tool_name| !self.successful_tools.contains(*tool_name))
            .cloned()
        {
            if !available_tools.contains(tool_name.as_str()) {
                return Err(AgentFailure::contract(
                    "required_tool_unavailable",
                    format!("task contract requires unavailable tool `{tool_name}`"),
                ));
            }
            self.claim_gate(format!("tool:{tool_name}"))?;
            return Ok(Some(format!(
                "The task contract is not satisfied yet. Use `{tool_name}` successfully before finishing. Do not substitute another implementation or merely describe the intended result."
            )));
        }

        if let Some((requirement_id, alternatives)) = self
            .required_any_tool_successes
            .iter()
            .find(|(_, alternatives)| alternatives.is_disjoint(&self.successful_tools))
            .map(|(requirement_id, alternatives)| (requirement_id.clone(), alternatives.clone()))
        {
            let available_alternatives = alternatives
                .iter()
                .filter(|tool| available_tools.contains(tool.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            if available_alternatives.is_empty() {
                return Err(AgentFailure::contract(
                    "required_tool_group_unavailable",
                    format!(
                        "task contract requirement `{requirement_id}` has no available tool alternatives"
                    ),
                ));
            }
            self.claim_gate(format!("any_tool:{requirement_id}"))?;
            let alternatives = available_alternatives
                .iter()
                .map(|tool| format!("`{tool}`"))
                .collect::<Vec<_>>()
                .join(", ");
            return Ok(Some(format!(
                "The task contract is not satisfied yet. Requirement `{requirement_id}` needs substantive evidence from at least one of: {alternatives}. Use one successfully before finishing; discovery-only observations do not satisfy this requirement."
            )));
        }

        if !self.pending_interactions.is_empty() {
            let requirements =
                interaction_requirements(&self.pending_interactions, &available_tools);
            if !requirements.is_empty() {
                self.claim_gate("interaction_verification".to_string())?;
                return Ok(Some(format!(
                    "The task performed interactive actions whose postconditions have not been observed. Before finishing, verify {}. Compare the fresh observation with the requested outcome; retry or replan if it is wrong.",
                    requirements.join("; ")
                )));
            }
        }

        if verification_required
            && self.mutation_epoch > 0
            && !self.latest_mutation_verified()
            && tools.iter().any(tool_can_verify_workspace_change)
        {
            self.claim_gate("workspace_verification".to_string())?;
            return Ok(Some(
                "The task changed the workspace but has no relevant post-change verification evidence. Use the narrowest applicable test, build, lint, diff, or direct read-back of the changed target before finishing. Unrelated tool success is not verification."
                    .to_string(),
            ));
        }

        Ok(None)
    }

    fn claim_gate(&mut self, key: String) -> Result<(), AgentFailure> {
        let attempts = self.gate_attempts.entry(key.clone()).or_default();
        if *attempts >= MAX_COMPLETION_GATE_ATTEMPTS {
            return Err(AgentFailure::contract(
                "task_contract_unsatisfied",
                format!(
                    "task contract requirement `{key}` remained unsatisfied after {attempts} repair attempts"
                ),
            ));
        }
        *attempts += 1;
        Ok(())
    }

    fn record_evidence(&mut self, kind: ContractEvidenceKind, source: &str, input: &str) {
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.evidence.push(ContractEvidence {
            sequence: self.next_sequence,
            kind,
            source: source.to_string(),
            input_fingerprint: fingerprint(input),
        });
        if self.evidence.len() > MAX_CONTRACT_EVIDENCE {
            let excess = self.evidence.len() - MAX_CONTRACT_EVIDENCE;
            self.evidence.drain(..excess);
        }
    }
}

fn fingerprint(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn structured_targets(input_json: &str) -> BTreeSet<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(input_json) else {
        return BTreeSet::new();
    };
    let mut targets = BTreeSet::new();
    collect_targets(&value, None, &mut targets);
    targets
}

fn collect_targets(value: &serde_json::Value, key: Option<&str>, targets: &mut BTreeSet<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                collect_targets(value, Some(key), targets);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_targets(value, key, targets);
            }
        }
        serde_json::Value::String(value)
            if key.is_some_and(|key| {
                matches!(
                    key.to_ascii_lowercase().as_str(),
                    "path" | "file" | "file_path" | "output" | "output_path" | "directory"
                )
            }) =>
        {
            let normalized = value.trim().trim_end_matches('/').to_string();
            if !normalized.is_empty() {
                targets.insert(normalized);
            }
        }
        _ => {}
    }
}

fn verification_targets_match(mutation_targets: &BTreeSet<String>, input_json: &str) -> bool {
    if mutation_targets.is_empty() {
        return false;
    }
    let verification_targets = structured_targets(input_json);
    mutation_targets.iter().any(|mutation| {
        verification_targets.iter().any(|verification| {
            mutation == verification
                || mutation.starts_with(&format!("{verification}/"))
                || verification.starts_with(&format!("{mutation}/"))
        })
    })
}

fn interaction_action(tool_name: &str) -> Option<(InteractionSurface, &str)> {
    match tool_name {
        "browser.open" | "browser.click" | "browser.type" | "browser.scroll"
        | "browser.select_tab" => Some((InteractionSurface::Browser, tool_name)),
        "computer.click" | "computer.type" | "computer.key" | "computer.scroll" => {
            Some((InteractionSurface::Computer, tool_name))
        }
        _ => None,
    }
}

fn interaction_observation(tool_name: &str) -> Option<InteractionSurface> {
    match tool_name {
        "browser.extract_text" | "browser.capture" | "browser.tabs" => {
            Some(InteractionSurface::Browser)
        }
        "computer.screenshot" => Some(InteractionSurface::Computer),
        _ => None,
    }
}

fn interaction_observation_verifies(action_tool: &str, observation_tool: &str) -> bool {
    match action_tool {
        "browser.open" | "browser.select_tab" => matches!(
            observation_tool,
            "browser.extract_text" | "browser.capture" | "browser.tabs"
        ),
        "browser.click" | "browser.type" | "browser.scroll" => {
            matches!(observation_tool, "browser.extract_text" | "browser.capture")
        }
        "computer.click" | "computer.type" | "computer.key" | "computer.scroll" => {
            observation_tool == "computer.screenshot"
        }
        _ => false,
    }
}

fn interaction_requirements(
    pending: &BTreeMap<InteractionSurface, String>,
    available: &BTreeSet<&str>,
) -> Vec<String> {
    pending
        .iter()
        .filter_map(|(surface, action)| {
            let candidates = interaction_observers(*surface, action);
            let observers = candidates
                .iter()
                .copied()
                .filter(|candidate| available.contains(candidate))
                .collect::<Vec<_>>();
            (!observers.is_empty()).then(|| {
                format!(
                    "{} action `{action}` with {}",
                    surface.label(),
                    observers
                        .iter()
                        .map(|tool| format!("`{tool}`"))
                        .collect::<Vec<_>>()
                        .join(" or ")
                )
            })
        })
        .collect()
}

fn interaction_observers(surface: InteractionSurface, action: &str) -> &'static [&'static str] {
    match surface {
        InteractionSurface::Browser if matches!(action, "browser.open" | "browser.select_tab") => {
            &["browser.capture", "browser.extract_text", "browser.tabs"]
        }
        InteractionSurface::Browser => &["browser.capture", "browser.extract_text"],
        InteractionSurface::Computer => &["computer.screenshot"],
    }
}

fn inferred_builtin_tool_risk(tool_name: &str) -> Option<&'static ToolRisk> {
    static READ_ONLY: ToolRisk = ToolRisk::ReadOnly;
    static WRITES_WORKSPACE: ToolRisk = ToolRisk::WritesWorkspace;
    static EXECUTES_PROCESS: ToolRisk = ToolRisk::ExecutesProcess;
    match tool_name {
        "file.read" | "file.read_many" | "file.list" | "file.search" => Some(&READ_ONLY),
        "file.write" => Some(&WRITES_WORKSPACE),
        "shell.run" => Some(&EXECUTES_PROCESS),
        _ => None,
    }
}

fn process_input_looks_like_verification(input_json: &str) -> bool {
    let normalized = input_json.to_ascii_lowercase();
    [
        " test",
        "test ",
        "check",
        "build",
        "lint",
        "verify",
        "pytest",
        "vitest",
        "jest",
        "cargo test",
        "cargo check",
        "swift test",
        "go test",
        "git diff",
        "git status",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

fn tool_can_verify_workspace_change(tool: &ToolSpec) -> bool {
    matches!(tool.risk, ToolRisk::ReadOnly | ToolRisk::ExecutesProcess)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, risk: ToolRisk) -> ToolSpec {
        ToolSpec::builtin(name, "test", "test", risk, r#"{"type":"object"}"#)
    }

    #[test]
    fn required_tool_is_a_hard_completion_contract() {
        let mut contract = AgentTaskContract::default();
        contract.require_tool_success("image.generate");
        let tools = vec![tool("image.generate", ToolRisk::UsesNetwork)];

        assert!(contract
            .completion_instruction_for_task(&tools)
            .unwrap()
            .unwrap()
            .contains("image.generate"));
        contract.record_tool_outcome(
            "image.generate",
            r#"{"prompt":"city"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::UsesNetwork),
        );
        assert_eq!(contract.completion_instruction_for_task(&tools), Ok(None));
    }

    #[test]
    fn any_tool_requirement_accepts_one_successful_alternative() {
        let mut contract = AgentTaskContract::default();
        contract.require_any_tool_success(
            "workspace_content",
            ["file.read", "file.read_many", "file.search"],
        );
        let tools = vec![
            tool("file.read", ToolRisk::ReadOnly),
            tool("file.read_many", ToolRisk::ReadOnly),
            tool("file.search", ToolRisk::ReadOnly),
        ];

        let instruction = contract
            .completion_instruction_for_task(&tools)
            .expect("completion gate")
            .expect("requirement should be active");
        assert!(instruction.contains("workspace_content"));
        assert!(instruction.contains("file.read_many"));

        contract.record_tool_outcome(
            "file.read_many",
            r#"{"paths":["a.md","b.md"]}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
        );
        assert_eq!(contract.completion_instruction_for_task(&tools), Ok(None));
    }

    #[test]
    fn unrelated_discovery_tool_does_not_satisfy_any_tool_requirement() {
        let mut contract = AgentTaskContract::default();
        contract.require_any_tool_success("workspace_content", ["file.read", "file.read_many"]);
        let tools = vec![
            tool("file.list", ToolRisk::ReadOnly),
            tool("file.read", ToolRisk::ReadOnly),
            tool("file.read_many", ToolRisk::ReadOnly),
        ];

        contract.record_tool_outcome(
            "file.list",
            r#"{"path":""}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
        );
        assert!(contract
            .completion_instruction_for_task(&tools)
            .expect("completion gate")
            .is_some());
    }

    #[test]
    fn unrelated_read_does_not_verify_a_mutation() {
        let mut contract = AgentTaskContract::default();
        contract.record_tool_outcome(
            "file.write",
            r#"{"path":"src/main.rs"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::WritesWorkspace),
        );
        contract.record_tool_outcome(
            "file.read",
            r#"{"path":"README.md"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
        );
        assert!(!contract.latest_mutation_verified());

        contract.record_tool_outcome(
            "file.read",
            r#"{"path":"src/main.rs"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
        );
        assert!(contract.latest_mutation_verified());
    }

    #[test]
    fn interaction_action_requires_a_matching_fresh_observation() {
        let mut contract = AgentTaskContract::default();
        contract.record_tool_outcome("computer.click", "{}", &ToolOutcomeStatus::Succeeded, None);
        contract.record_tool_outcome(
            "file.read",
            r#"{"path":"README.md"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
        );
        assert!(!contract.pending_interactions().is_empty());
        contract.record_tool_outcome(
            "computer.screenshot",
            "{}",
            &ToolOutcomeStatus::Succeeded,
            None,
        );
        assert!(contract.pending_interactions().is_empty());
    }

    #[test]
    fn model_context_exposes_active_obligations_without_claiming_a_gate_attempt() {
        let mut contract = AgentTaskContract::default();
        contract.require_tool_success("image.generate");
        let tools = vec![tool("image.generate", ToolRisk::UsesNetwork)];

        let first = contract
            .model_context_for_task(&tools)
            .expect("required tool should be visible");
        let second = contract
            .model_context_for_task(&tools)
            .expect("rendering should be repeatable");

        assert_eq!(first, second);
        assert!(first.contains("image.generate"));
        assert!(contract.gate_attempts.is_empty());
        assert!(contract.completion_instruction_for_task(&tools).is_ok());
    }

    #[test]
    fn model_context_tracks_and_clears_relevant_workspace_verification() {
        let mut contract = AgentTaskContract::default();
        contract.record_tool_outcome(
            "file.write",
            r#"{"path":"src/main.rs"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::WritesWorkspace),
        );
        let tools = vec![tool("file.read", ToolRisk::ReadOnly)];

        assert!(contract.model_context_for_task(&tools).is_none());
        contract.merge_workspace_verification_policy(
            WorkspaceVerificationPolicy::RequiredAfterMutation,
        );

        let pending = contract
            .model_context_for_task(&tools)
            .expect("mutation should require verification");
        assert!(pending.contains("src/main.rs"));
        contract.merge_workspace_verification_policy(WorkspaceVerificationPolicy::NotRequired);
        assert_eq!(
            contract.workspace_verification_policy(),
            WorkspaceVerificationPolicy::RequiredAfterMutation
        );

        contract.record_tool_outcome(
            "file.read",
            r#"{"path":"src/main.rs"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
        );
        assert!(contract.model_context_for_task(&tools).is_none());
    }

    #[test]
    fn explicit_verification_arguments_preserve_legacy_behavior() {
        let mut contract = AgentTaskContract::default();
        contract.record_tool_outcome(
            "file.write",
            r#"{"path":"src/main.rs"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::WritesWorkspace),
        );
        contract.merge_workspace_verification_policy(
            WorkspaceVerificationPolicy::RequiredAfterMutation,
        );
        let tools = vec![tool("file.read", ToolRisk::ReadOnly)];

        assert!(contract.model_context(false, &tools).is_none());
        assert!(contract.model_context(true, &tools).is_some());

        let mut disabled = contract.clone();
        assert_eq!(disabled.completion_instruction(false, &tools), Ok(None));
        assert!(contract
            .completion_instruction(true, &tools)
            .expect("legacy completion gate evaluates")
            .is_some());
    }

    #[test]
    fn model_context_includes_matching_interaction_observers() {
        let mut contract = AgentTaskContract::default();
        contract.record_tool_outcome(
            "computer.click",
            r#"{"x":10,"y":20}"#,
            &ToolOutcomeStatus::Succeeded,
            None,
        );
        let tools = vec![tool("computer.screenshot", ToolRisk::ReadOnly)];

        let context = contract
            .model_context_for_task(&tools)
            .expect("fresh observation should be required");
        assert!(context.contains("computer.click"));
        assert!(context.contains("computer.screenshot"));
    }
}
