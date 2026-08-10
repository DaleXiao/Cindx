use super::authorization::*;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

const LOCK_FILE_NAME: &str = "successor-execution.lock";
const ROOT_RECOVERY_FILE_NAME: &str = "successor-execution-recovery.json";
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in super::super) enum ArmKindV1 {
    Direct,
    Workflow,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResourceTotalsV1 {
    pub(super) runs: u64,
    pub(super) duration_ms: u64,
    pub(super) model_calls: u64,
    pub(super) tool_calls: u64,
    pub(super) agent_turns: u64,
    pub(super) physical_model_attempts: u64,
    pub(super) total_tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in super::super) struct ArmObservedUsageV1 {
    pub(in super::super) duration_ms: u64,
    pub(in super::super) model_calls: u64,
    pub(in super::super) tool_calls: u64,
    pub(in super::super) agent_turns: u64,
    pub(in super::super) physical_model_attempts: u64,
    pub(in super::super) total_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum JournalPhaseV1 {
    AuthorizationConsumed,
    CampaignReserved,
    Executing,
    Terminal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CellPhaseV1 {
    Planned,
    Reserved,
    PairCommitted,
    Committed,
    Censored,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum ArmStateV1 {
    Planned,
    Reserved {
        reservation_sha256: String,
    },
    Terminal {
        terminal_status: String,
        evidence_sha256: String,
        usage: Option<ArmObservedUsageV1>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalArmV1 {
    kind: ArmKindV1,
    state: ArmStateV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalCellV1 {
    binding: AuthorizationCellV1,
    phase: CellPhaseV1,
    arms: Vec<JournalArmV1>,
    pair_sha256: Option<String>,
    commit_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum TerminalDispositionV1 {
    ReadyForIndependentReview,
    Frozen,
    Censored,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalReceiptV1 {
    disposition: TerminalDispositionV1,
    reason: String,
    terminal_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalDocumentV1 {
    schema: String,
    revision: u64,
    authorization: AuthorizationV1,
    tombstone_sha256: String,
    phase: JournalPhaseV1,
    cells: Vec<JournalCellV1>,
    charged: ResourceTotalsV1,
    observed: ResourceTotalsV1,
    terminal: Option<TerminalReceiptV1>,
    journal_sha256: String,
}

pub(in super::super) struct SuccessorExecutionJournal {
    root: PathBuf,
    document: JournalDocumentV1,
    _lock: File,
}

impl Drop for SuccessorExecutionJournal {
    fn drop(&mut self) {
        let _ = File::unlock(&self._lock);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum JournalRecoveryV1 {
    Terminal(TerminalDispositionV1),
    Frozen,
}

impl SuccessorExecutionJournal {
    pub(super) fn create_new(
        output_root: &Path,
        validated: &ValidatedAuthorizationV1,
        tombstone: &ConsumedTombstoneV1,
    ) -> Result<Self, String> {
        tombstone.validate_for(&validated.authorization)?;
        let root = canonical_bound_path(output_root, true)?;
        require_private_directory(&root, "successor output root")?;
        let lock = acquire_execution_lock(&root)?;
        match fs::symlink_metadata(root.join(ROOT_RECOVERY_FILE_NAME)) {
            Ok(_) => return Err("successor output root is already recovery-terminal".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "failed to inspect successor root recovery marker: {error}"
                ))
            }
        }
        if root != validated.output_root
            || path_sha256(&root) != validated.authorization.output_root_sha256
        {
            return Err(
                "successor journal output root does not match its consumed authorization".into(),
            );
        }
        let stored_tombstone = read_consumed_tombstone(&root)?;
        if &stored_tombstone != tombstone {
            return Err("successor consumed tombstone differs from the durable root lock".into());
        }
        let cells = validated
            .authorization
            .cells
            .iter()
            .map(|binding| JournalCellV1 {
                binding: binding.clone(),
                phase: CellPhaseV1::Planned,
                arms: binding
                    .execution_order()
                    .into_iter()
                    .map(|kind| JournalArmV1 {
                        kind,
                        state: ArmStateV1::Planned,
                    })
                    .collect(),
                pair_sha256: None,
                commit_sha256: None,
            })
            .collect();
        let mut document = JournalDocumentV1 {
            schema: JOURNAL_SCHEMA.into(),
            revision: 0,
            authorization: validated.authorization.clone(),
            tombstone_sha256: tombstone.tombstone_sha256.clone(),
            phase: JournalPhaseV1::AuthorizationConsumed,
            cells,
            charged: ResourceTotalsV1::default(),
            observed: ResourceTotalsV1::default(),
            terminal: None,
            journal_sha256: String::new(),
        };
        document.reseal()?;
        document.validate()?;
        super::preflight::write_new_private_file_atomically(
            &root.join(JOURNAL_FILE_NAME),
            &canonical_json(&document, "successor execution journal")?,
        )?;
        Ok(Self {
            root,
            document,
            _lock: lock,
        })
    }

    pub(super) fn recover(output_root: &Path) -> Result<(Option<Self>, JournalRecoveryV1), String> {
        let root = canonical_bound_path(output_root, true)?;
        require_private_directory(&root, "successor output root")?;
        let lock = acquire_execution_lock(&root)?;
        let journal_path = root.join(JOURNAL_FILE_NAME);
        if !journal_path.exists() {
            write_root_only_recovery(&root)?;
            return Ok((None, JournalRecoveryV1::Frozen));
        }
        let bytes = read_private_file(&journal_path, "successor execution journal")?;
        let document: JournalDocumentV1 = serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid successor execution journal JSON: {error}"))?;
        if canonical_json(&document, "successor execution journal")? != bytes {
            return Err("successor execution journal is not canonical JSON".into());
        }
        document.validate()?;
        let tombstone = read_consumed_tombstone(&root)?;
        tombstone.validate_for(&document.authorization)?;
        if tombstone.tombstone_sha256 != document.tombstone_sha256
            || path_sha256(&root) != document.authorization.output_root_sha256
        {
            return Err(
                "successor execution journal is not anchored to its durable root lock".into(),
            );
        }
        let mut journal = Self {
            root,
            document,
            _lock: lock,
        };
        let recovery = if let Some(terminal) = &journal.document.terminal {
            JournalRecoveryV1::Terminal(terminal.disposition.clone())
        } else {
            journal.freeze("interrupted_execution", TerminalDispositionV1::Censored)?;
            JournalRecoveryV1::Frozen
        };
        Ok((Some(journal), recovery))
    }

    pub(super) fn reserve_campaign(&mut self) -> Result<(), String> {
        self.transition(|document| {
            if document.phase != JournalPhaseV1::AuthorizationConsumed {
                return Err("successor campaign reservation is out of order".into());
            }
            document.phase = JournalPhaseV1::CampaignReserved;
            Ok(())
        })
    }

    pub(super) fn reserve_cell(&mut self, ordinal: usize) -> Result<(), String> {
        self.transition(|document| {
            require_live_phase(document)?;
            let index = cell_index(document, ordinal)?;
            if document.cells[index].phase != CellPhaseV1::Planned
                || document.cells[..index]
                    .iter()
                    .any(|cell| cell.phase != CellPhaseV1::Committed)
            {
                return Err("successor cell reservation is out of order".into());
            }
            document.cells[index].phase = CellPhaseV1::Reserved;
            document.phase = JournalPhaseV1::Executing;
            Ok(())
        })
    }

    pub(in super::super) fn reserve_arm(
        &mut self,
        ordinal: usize,
        kind: ArmKindV1,
        reservation_sha256: String,
    ) -> Result<(), String> {
        require_sha256(&reservation_sha256, "successor arm reservation")?;
        self.transition(|document| {
            require_live_phase(document)?;
            let index = cell_index(document, ordinal)?;
            let arm_index = {
                let cell = &document.cells[index];
                if cell.phase != CellPhaseV1::Reserved {
                    return Err("successor arm requires a reserved cell".into());
                }
                cell.arms
                    .iter()
                    .position(|arm| arm.kind == kind)
                    .ok_or_else(|| "successor arm is outside the frozen cell".to_string())?
            };
            if arm_index > 0 && !document.cells[index].arms[arm_index - 1].is_complete() {
                return Err("successor arm order differs from the frozen protocol".into());
            }
            if !matches!(
                document.cells[index].arms[arm_index].state,
                ArmStateV1::Planned
            ) {
                return Err("successor arm has already been started".into());
            }
            document.cells[index].arms[arm_index].state =
                ArmStateV1::Reserved { reservation_sha256 };
            document.charged = document
                .charged
                .checked_add(document.authorization.run_budget.maximum_charge()?)?;
            require_within_campaign(
                document.charged,
                &document.authorization.campaign_budget,
                "charged",
            )
        })
    }

    pub(in super::super) fn record_arm_terminal(
        &mut self,
        ordinal: usize,
        kind: ArmKindV1,
        terminal_status: String,
        evidence_sha256: String,
        usage: Option<ArmObservedUsageV1>,
    ) -> Result<(), String> {
        require_sha256(&evidence_sha256, "successor arm evidence")?;
        if terminal_status.trim().is_empty() {
            return Err("successor arm terminal status is empty".into());
        }
        self.transition(|document| {
            require_live_phase(document)?;
            let index = cell_index(document, ordinal)?;
            let arm_index = document.cells[index]
                .arms
                .iter()
                .position(|arm| arm.kind == kind)
                .ok_or_else(|| "successor arm is outside the frozen cell".to_string())?;
            if !matches!(
                document.cells[index].arms[arm_index].state,
                ArmStateV1::Reserved { .. }
            ) {
                return Err("successor arm terminal has no durable reservation".into());
            }
            if let Some(actual) = usage {
                actual.validate_against(&document.authorization.run_budget)?;
                document.observed = document.observed.checked_add(actual.as_totals())?;
                require_within_campaign(
                    document.observed,
                    &document.authorization.campaign_budget,
                    "observed",
                )?;
            }
            document.cells[index].arms[arm_index].state = ArmStateV1::Terminal {
                terminal_status: terminal_status.clone(),
                evidence_sha256,
                usage,
            };
            if terminal_status != "completed" || usage.is_none() {
                terminalize(document, TerminalDispositionV1::Censored, "incomplete_arm")?;
            }
            Ok(())
        })
    }

    pub(super) fn commit_pair(
        &mut self,
        ordinal: usize,
        pair_sha256: String,
    ) -> Result<(), String> {
        require_sha256(&pair_sha256, "successor pair")?;
        self.transition(|document| {
            require_live_phase(document)?;
            let index = cell_index(document, ordinal)?;
            let cell = &mut document.cells[index];
            if cell.phase != CellPhaseV1::Reserved
                || !cell.arms.iter().all(JournalArmV1::is_complete)
            {
                return Err("successor pair is incomplete or out of order".into());
            }
            cell.pair_sha256 = Some(pair_sha256);
            cell.phase = CellPhaseV1::PairCommitted;
            Ok(())
        })
    }

    pub(super) fn commit_cell(
        &mut self,
        ordinal: usize,
        commit_sha256: String,
    ) -> Result<(), String> {
        require_sha256(&commit_sha256, "successor cell commit")?;
        self.transition(|document| {
            require_live_phase(document)?;
            let index = cell_index(document, ordinal)?;
            let cell = &mut document.cells[index];
            if cell.phase != CellPhaseV1::PairCommitted {
                return Err("successor cell commit has no committed pair".into());
            }
            cell.commit_sha256 = Some(commit_sha256);
            cell.phase = CellPhaseV1::Committed;
            Ok(())
        })
    }

    pub(super) fn finish_ready_for_independent_review(
        &mut self,
        decision_sha256: String,
    ) -> Result<(), String> {
        require_sha256(&decision_sha256, "successor terminal decision")?;
        self.transition(|document| {
            require_live_phase(document)?;
            if document
                .cells
                .iter()
                .any(|cell| cell.phase != CellPhaseV1::Committed)
            {
                return Err("successor campaign cannot finish before all cells commit".into());
            }
            terminalize(
                document,
                TerminalDispositionV1::ReadyForIndependentReview,
                &format!("ready_for_independent_review:{decision_sha256}"),
            )
        })
    }

    pub(super) fn freeze(
        &mut self,
        reason: &str,
        disposition: TerminalDispositionV1,
    ) -> Result<(), String> {
        if reason.trim().is_empty()
            || disposition == TerminalDispositionV1::ReadyForIndependentReview
        {
            return Err("successor freeze requires a frozen or censored disposition".into());
        }
        self.transition(|document| terminalize(document, disposition, reason))
    }

    pub(super) fn charged(&self) -> ResourceTotalsV1 {
        self.document.charged
    }

    pub(super) fn observed(&self) -> ResourceTotalsV1 {
        self.document.observed
    }

    pub(super) fn arm_is_planned(&self, ordinal: usize, kind: ArmKindV1) -> bool {
        cell_index(&self.document, ordinal)
            .ok()
            .and_then(|index| {
                self.document.cells[index]
                    .arms
                    .iter()
                    .find(|arm| arm.kind == kind)
            })
            .is_some_and(|arm| matches!(arm.state, ArmStateV1::Planned))
    }

    pub(super) fn terminal_disposition(&self) -> Option<TerminalDispositionV1> {
        self.document
            .terminal
            .as_ref()
            .map(|terminal| terminal.disposition.clone())
    }

    fn transition(
        &mut self,
        apply: impl FnOnce(&mut JournalDocumentV1) -> Result<(), String>,
    ) -> Result<(), String> {
        if self.document.terminal.is_some() {
            return Err("successor execution journal is terminal".into());
        }
        let mut next = self.document.clone();
        apply(&mut next)?;
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or_else(|| "successor journal revision overflowed".to_string())?;
        next.reseal()?;
        next.validate()?;
        tools::write_private_file_atomically(
            &self.root.join(JOURNAL_FILE_NAME),
            &canonical_json(&next, "successor execution journal")?,
        )
        .map_err(|error| format!("failed to persist successor execution journal: {error}"))?;
        self.document = next;
        Ok(())
    }
}

fn acquire_execution_lock(root: &Path) -> Result<File, String> {
    let path = root.join(LOCK_FILE_NAME);
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options
        .open(&path)
        .map_err(|error| format!("failed to open successor execution lock: {error}"))?;
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("failed to inspect successor execution lock: {error}"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("successor execution lock is not a regular file".into());
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o777 != 0o600 {
        return Err("successor execution lock permissions must be 0600".into());
    }
    file.try_lock()
        .map_err(|error| format!("successor execution is already active: {error}"))?;
    Ok(file)
}

impl AuthorizationRunBudgetV1 {
    pub(super) fn maximum_charge(&self) -> Result<ResourceTotalsV1, String> {
        Ok(ResourceTotalsV1 {
            runs: 1,
            duration_ms: self.max_duration_ms,
            model_calls: u64::try_from(self.max_model_calls)
                .map_err(|_| "successor model-call budget overflowed")?,
            tool_calls: u64::try_from(self.max_tool_calls)
                .map_err(|_| "successor tool-call budget overflowed")?,
            agent_turns: u64::try_from(self.max_agent_turns)
                .map_err(|_| "successor turn budget overflowed")?,
            physical_model_attempts: u64::try_from(self.max_physical_model_attempts)
                .map_err(|_| "successor physical-attempt budget overflowed")?,
            total_tokens: self.max_total_tokens,
        })
    }
}

fn journal_digest(value: &JournalDocumentV1) -> Result<String, String> {
    let mut payload = value.clone();
    payload.journal_sha256.clear();
    domain_digest(JOURNAL_HASH_DOMAIN, &payload, "successor execution journal")
}

fn terminal_digest(value: &TerminalReceiptV1) -> Result<String, String> {
    let mut payload = value.clone();
    payload.terminal_sha256.clear();
    domain_digest(
        b"cindx.collaboration-successor-terminal.v1\0",
        &payload,
        "successor terminal receipt",
    )
}

impl ArmObservedUsageV1 {
    fn as_totals(self) -> ResourceTotalsV1 {
        ResourceTotalsV1 {
            runs: 1,
            duration_ms: self.duration_ms,
            model_calls: self.model_calls,
            tool_calls: self.tool_calls,
            agent_turns: self.agent_turns,
            physical_model_attempts: self.physical_model_attempts,
            total_tokens: self.total_tokens,
        }
    }

    fn validate_against(&self, budget: &AuthorizationRunBudgetV1) -> Result<(), String> {
        if !self.as_totals().within(budget.maximum_charge()?) {
            return Err("successor arm resources exceed the frozen run budget".into());
        }
        Ok(())
    }
}

impl ResourceTotalsV1 {
    pub(super) fn checked_add(self, other: Self) -> Result<Self, String> {
        Ok(Self {
            runs: checked_add(self.runs, other.runs, "run")?,
            duration_ms: checked_add(self.duration_ms, other.duration_ms, "duration")?,
            model_calls: checked_add(self.model_calls, other.model_calls, "model-call")?,
            tool_calls: checked_add(self.tool_calls, other.tool_calls, "tool-call")?,
            agent_turns: checked_add(self.agent_turns, other.agent_turns, "turn")?,
            physical_model_attempts: checked_add(
                self.physical_model_attempts,
                other.physical_model_attempts,
                "physical-attempt",
            )?,
            total_tokens: checked_add(self.total_tokens, other.total_tokens, "token")?,
        })
    }

    pub(super) fn checked_mul(self, factor: u64) -> Result<Self, String> {
        Ok(Self {
            runs: checked_mul(self.runs, factor, "run")?,
            duration_ms: checked_mul(self.duration_ms, factor, "duration")?,
            model_calls: checked_mul(self.model_calls, factor, "model-call")?,
            tool_calls: checked_mul(self.tool_calls, factor, "tool-call")?,
            agent_turns: checked_mul(self.agent_turns, factor, "turn")?,
            physical_model_attempts: checked_mul(
                self.physical_model_attempts,
                factor,
                "physical-attempt",
            )?,
            total_tokens: checked_mul(self.total_tokens, factor, "token")?,
        })
    }

    fn within(self, limit: Self) -> bool {
        self.runs <= limit.runs
            && self.duration_ms <= limit.duration_ms
            && self.model_calls <= limit.model_calls
            && self.tool_calls <= limit.tool_calls
            && self.agent_turns <= limit.agent_turns
            && self.physical_model_attempts <= limit.physical_model_attempts
            && self.total_tokens <= limit.total_tokens
    }

    pub(super) fn matches_campaign(self, budget: &AuthorizationCampaignBudgetV1) -> bool {
        self.runs == budget.runs as u64
            && self.duration_ms == budget.max_duration_ms
            && self.model_calls == budget.max_model_calls as u64
            && self.tool_calls == budget.max_tool_calls as u64
            && self.agent_turns == budget.max_agent_turns as u64
            && self.physical_model_attempts == budget.max_physical_model_attempts as u64
            && self.total_tokens == budget.max_total_tokens
    }
}

impl JournalArmV1 {
    fn is_complete(&self) -> bool {
        matches!(
            &self.state,
            ArmStateV1::Terminal {
                terminal_status,
                usage: Some(_),
                ..
            } if terminal_status == "completed"
        )
    }
}

impl JournalDocumentV1 {
    fn reseal(&mut self) -> Result<(), String> {
        self.journal_sha256.clear();
        self.journal_sha256 = journal_digest(self)?;
        Ok(())
    }

    fn validate(&self) -> Result<(), String> {
        self.authorization.validate_static()?;
        require_sha256(&self.tombstone_sha256, "successor consumed tombstone")?;
        if self.schema != JOURNAL_SCHEMA
            || self.journal_sha256 != journal_digest(self)?
            || self.cells.len() != PAIR_COUNT
        {
            return Err("successor execution journal envelope is invalid".into());
        }
        let mut charged = ResourceTotalsV1::default();
        let mut observed = ResourceTotalsV1::default();
        for (index, cell) in self.cells.iter().enumerate() {
            if cell.binding != self.authorization.cells[index]
                || cell.arms.len() != 2
                || cell.arms.iter().map(|arm| arm.kind).collect::<Vec<_>>()
                    != cell.binding.execution_order()
                || (cell.phase == CellPhaseV1::Committed
                    && (cell.pair_sha256.is_none() || cell.commit_sha256.is_none()))
                || (cell.phase == CellPhaseV1::PairCommitted && cell.pair_sha256.is_none())
            {
                return Err("successor execution journal cell is invalid".into());
            }
            let states_are_planned = cell
                .arms
                .iter()
                .all(|arm| matches!(arm.state, ArmStateV1::Planned));
            let states_are_terminal = cell
                .arms
                .iter()
                .all(|arm| matches!(arm.state, ArmStateV1::Terminal { .. }));
            if (cell.phase == CellPhaseV1::Planned
                && (!states_are_planned
                    || cell.pair_sha256.is_some()
                    || cell.commit_sha256.is_some()))
                || (matches!(
                    cell.phase,
                    CellPhaseV1::PairCommitted | CellPhaseV1::Committed
                ) && !states_are_terminal)
            {
                return Err("successor execution journal cell phase contradicts its arms".into());
            }
            for arm in &cell.arms {
                match &arm.state {
                    ArmStateV1::Planned => {}
                    ArmStateV1::Reserved { reservation_sha256 } => {
                        require_sha256(reservation_sha256, "successor arm reservation")?;
                        charged =
                            charged.checked_add(self.authorization.run_budget.maximum_charge()?)?;
                    }
                    ArmStateV1::Terminal {
                        terminal_status,
                        evidence_sha256,
                        usage,
                    } => {
                        if terminal_status.trim().is_empty() {
                            return Err("successor arm terminal status is empty".into());
                        }
                        require_sha256(evidence_sha256, "successor arm evidence")?;
                        charged =
                            charged.checked_add(self.authorization.run_budget.maximum_charge()?)?;
                        if let Some(actual) = usage {
                            actual.validate_against(&self.authorization.run_budget)?;
                            observed = observed.checked_add(actual.as_totals())?;
                        }
                    }
                }
            }
        }
        require_within_campaign(charged, &self.authorization.campaign_budget, "charged")?;
        require_within_campaign(observed, &self.authorization.campaign_budget, "observed")?;
        if charged != self.charged || observed != self.observed {
            return Err("successor execution journal resource ledger is invalid".into());
        }
        match (&self.phase, &self.terminal) {
            (JournalPhaseV1::Terminal, Some(terminal)) => {
                if terminal.terminal_sha256 != terminal_digest(terminal)? {
                    return Err("successor terminal receipt digest is invalid".into());
                }
                if terminal.disposition == TerminalDispositionV1::ReadyForIndependentReview
                    && self
                        .cells
                        .iter()
                        .any(|cell| cell.phase != CellPhaseV1::Committed)
                {
                    return Err("review-ready successor terminal omitted a committed cell".into());
                }
            }
            (JournalPhaseV1::Terminal, None) | (_, Some(_)) => {
                return Err("successor execution journal terminal state is inconsistent".into())
            }
            _ => {}
        }
        if self.terminal.is_none() {
            let mut saw_active_or_planned = false;
            for cell in &self.cells {
                match cell.phase {
                    CellPhaseV1::Committed if !saw_active_or_planned => {}
                    CellPhaseV1::Planned => saw_active_or_planned = true,
                    CellPhaseV1::Reserved | CellPhaseV1::PairCommitted
                        if !saw_active_or_planned =>
                    {
                        saw_active_or_planned = true;
                    }
                    _ => {
                        return Err("successor execution journal cell order is inconsistent".into())
                    }
                }
            }
        }
        Ok(())
    }
}

fn require_live_phase(document: &JournalDocumentV1) -> Result<(), String> {
    if !matches!(
        document.phase,
        JournalPhaseV1::CampaignReserved | JournalPhaseV1::Executing
    ) || document.terminal.is_some()
    {
        return Err("successor execution journal is not live".into());
    }
    Ok(())
}

fn cell_index(document: &JournalDocumentV1, ordinal: usize) -> Result<usize, String> {
    let index = ordinal
        .checked_sub(1)
        .ok_or_else(|| "successor cell ordinal is invalid".to_string())?;
    if document
        .cells
        .get(index)
        .is_none_or(|cell| cell.binding.ordinal != ordinal)
    {
        return Err("successor cell ordinal is outside the frozen matrix".into());
    }
    Ok(index)
}

fn terminalize(
    document: &mut JournalDocumentV1,
    disposition: TerminalDispositionV1,
    reason: &str,
) -> Result<(), String> {
    if document.terminal.is_some() || reason.trim().is_empty() {
        return Err("successor execution journal is already terminal".into());
    }
    if disposition != TerminalDispositionV1::ReadyForIndependentReview {
        if let Some(cell) = document.cells.iter_mut().find(|cell| {
            matches!(
                cell.phase,
                CellPhaseV1::Reserved | CellPhaseV1::PairCommitted
            )
        }) {
            cell.phase = CellPhaseV1::Censored;
        }
    }
    let mut terminal = TerminalReceiptV1 {
        disposition,
        reason: reason.to_string(),
        terminal_sha256: String::new(),
    };
    terminal.terminal_sha256 = terminal_digest(&terminal)?;
    document.phase = JournalPhaseV1::Terminal;
    document.terminal = Some(terminal);
    Ok(())
}

fn require_within_campaign(
    totals: ResourceTotalsV1,
    budget: &AuthorizationCampaignBudgetV1,
    label: &str,
) -> Result<(), String> {
    let limit = ResourceTotalsV1 {
        runs: budget.runs as u64,
        duration_ms: budget.max_duration_ms,
        model_calls: budget.max_model_calls as u64,
        tool_calls: budget.max_tool_calls as u64,
        agent_turns: budget.max_agent_turns as u64,
        physical_model_attempts: budget.max_physical_model_attempts as u64,
        total_tokens: budget.max_total_tokens,
    };
    if !totals.within(limit) {
        return Err(format!(
            "successor {label} resources exceed the frozen campaign budget"
        ));
    }
    Ok(())
}

fn checked_add(left: u64, right: u64, label: &str) -> Result<u64, String> {
    left.checked_add(right)
        .ok_or_else(|| format!("successor {label} accounting overflowed"))
}

fn checked_mul(value: u64, factor: u64, label: &str) -> Result<u64, String> {
    value
        .checked_mul(factor)
        .ok_or_else(|| format!("successor {label} accounting overflowed"))
}
