use super::workflow_gepa_campaign_contract::ProductRunReceipt;
use crate::runtime_values::current_time_millis;
use orchestrator::sha256_hex;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub(super) const MAX_PRODUCT_RUNS: usize = 35;
pub(super) const MAX_PRODUCT_MODEL_CALLS: usize = 700;
pub(super) const MAX_MUTATION_MODEL_CALLS: usize = 12;
pub(super) const MAX_CAMPAIGN_DURATION_MS: u64 = 2 * 60 * 60 * 1_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct CampaignBudgetReceipt {
    pub(super) max_campaign_duration_ms: u64,
    pub(super) max_product_runs: usize,
    pub(super) max_product_model_calls: usize,
    pub(super) max_mutation_model_calls: usize,
}

impl Default for CampaignBudgetReceipt {
    fn default() -> Self {
        Self {
            max_campaign_duration_ms: MAX_CAMPAIGN_DURATION_MS,
            max_product_runs: MAX_PRODUCT_RUNS,
            max_product_model_calls: MAX_PRODUCT_MODEL_CALLS,
            max_mutation_model_calls: MAX_MUTATION_MODEL_CALLS,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct CampaignUsageReceipt {
    pub(super) reserved_product_runs: usize,
    pub(super) completed_product_runs: usize,
    pub(super) product_model_calls: usize,
    pub(super) product_tokens: u64,
    pub(super) reserved_mutation_model_calls: usize,
    pub(super) completed_mutation_model_calls: usize,
    pub(super) mutation_physical_attempts: u64,
    pub(super) mutation_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PendingAction {
    kind: String,
    label_sha256: String,
    reserved_model_calls: usize,
    started_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ActionReceipt {
    kind: String,
    label_sha256: String,
    completed_at_ms: u64,
    model_calls: usize,
    total_tokens: u64,
    terminal_status: String,
    output_sha256: Option<String>,
    prior_chain_sha256: Option<String>,
    chain_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JournalDocument {
    schema: String,
    source_commit: String,
    suite_sha256: String,
    provider_endpoint_sha256: String,
    configured_models_sha256: Vec<String>,
    created_at_ms: u64,
    updated_at_ms: u64,
    status: String,
    budget: CampaignBudgetReceipt,
    usage: CampaignUsageReceipt,
    pending_action: Option<PendingAction>,
    actions: Vec<ActionReceipt>,
    report_sha256: Option<String>,
}

pub(super) struct CampaignJournal {
    path: PathBuf,
    document: JournalDocument,
}

impl CampaignJournal {
    pub(super) fn create(
        path: &Path,
        source_commit: String,
        suite_sha256: String,
        provider_endpoint_sha256: String,
        configured_models_sha256: Vec<String>,
    ) -> Result<Self, String> {
        if path.exists() {
            return Err(format!(
                "workflow GEPA journal {} already exists; interrupted or completed campaigns must not be replayed in place",
                path.display()
            ));
        }
        let now = current_time_millis();
        let mut journal = Self {
            path: path.to_path_buf(),
            document: JournalDocument {
                schema: "cindx.workflow-gepa-campaign-journal.v1".to_string(),
                source_commit,
                suite_sha256,
                provider_endpoint_sha256,
                configured_models_sha256,
                created_at_ms: now,
                updated_at_ms: now,
                status: "running".to_string(),
                budget: CampaignBudgetReceipt::default(),
                usage: CampaignUsageReceipt::default(),
                pending_action: None,
                actions: Vec::new(),
                report_sha256: None,
            },
        };
        journal.persist()?;
        Ok(journal)
    }

    pub(super) fn budget(&self) -> CampaignBudgetReceipt {
        self.document.budget.clone()
    }

    pub(super) fn usage(&self) -> CampaignUsageReceipt {
        self.document.usage.clone()
    }

    pub(super) fn begin_product(&mut self, label: &str) -> Result<(), String> {
        self.require_idle()?;
        if self.document.usage.reserved_product_runs >= self.document.budget.max_product_runs {
            return Err("workflow GEPA product-run campaign budget is exhausted".to_string());
        }
        self.document.usage.reserved_product_runs += 1;
        self.document.pending_action = Some(PendingAction {
            kind: "product_run".to_string(),
            label_sha256: sha256_hex(label.as_bytes()),
            reserved_model_calls: 20,
            started_at_ms: current_time_millis(),
        });
        self.persist()
    }

    pub(super) fn complete_product(&mut self, run: &ProductRunReceipt) -> Result<(), String> {
        self.complete_action(
            "product_run",
            run.model_calls,
            run.total_tokens,
            &run.terminal_status,
            Some(run.output_sha256.clone()),
        )?;
        self.document.usage.completed_product_runs += 1;
        self.document.usage.product_model_calls = self
            .document
            .usage
            .product_model_calls
            .saturating_add(run.model_calls);
        self.document.usage.product_tokens = self
            .document
            .usage
            .product_tokens
            .saturating_add(run.total_tokens);
        if self.document.usage.product_model_calls > self.document.budget.max_product_model_calls {
            return Err(
                "workflow GEPA observed product model calls exceed the campaign cap".into(),
            );
        }
        self.persist()
    }

    pub(super) fn begin_mutation_search(&mut self) -> Result<(), String> {
        self.require_idle()?;
        if self.document.usage.reserved_mutation_model_calls > 0 {
            return Err("workflow GEPA mutation search may only run once".to_string());
        }
        self.document.usage.reserved_mutation_model_calls =
            self.document.budget.max_mutation_model_calls;
        self.document.pending_action = Some(PendingAction {
            kind: "mutation_search".to_string(),
            label_sha256: sha256_hex(b"observation-driven-candidate-population"),
            reserved_model_calls: self.document.budget.max_mutation_model_calls,
            started_at_ms: current_time_millis(),
        });
        self.persist()
    }

    pub(super) fn complete_mutation_search(
        &mut self,
        model_calls: usize,
        physical_attempts: u64,
        total_tokens: u64,
    ) -> Result<(), String> {
        if model_calls > self.document.budget.max_mutation_model_calls {
            return Err("workflow GEPA mutation search exceeded its model-call cap".to_string());
        }
        self.complete_action(
            "mutation_search",
            model_calls,
            total_tokens,
            "completed",
            None,
        )?;
        self.document.usage.completed_mutation_model_calls = model_calls;
        self.document.usage.mutation_physical_attempts = physical_attempts;
        self.document.usage.mutation_tokens = total_tokens;
        self.persist()
    }

    pub(super) fn finish(&mut self, status: &str, report_sha256: String) -> Result<(), String> {
        self.require_idle()?;
        if self.document.status != "running" {
            return Err("workflow GEPA journal is already terminal".to_string());
        }
        self.document.status = status.to_string();
        self.document.report_sha256 = Some(report_sha256);
        self.persist()
    }

    fn complete_action(
        &mut self,
        expected_kind: &str,
        model_calls: usize,
        total_tokens: u64,
        terminal_status: &str,
        output_sha256: Option<String>,
    ) -> Result<(), String> {
        let pending = self
            .document
            .pending_action
            .take()
            .ok_or_else(|| "workflow GEPA action completion has no reservation".to_string())?;
        if pending.kind != expected_kind || model_calls > pending.reserved_model_calls {
            self.document.pending_action = Some(pending);
            return Err("workflow GEPA action does not match its reservation".to_string());
        }
        let prior_chain_sha256 = self
            .document
            .actions
            .last()
            .map(|receipt| receipt.chain_sha256.clone());
        let completed_at_ms = current_time_millis();
        let chain_payload = serde_json::to_vec(&(
            prior_chain_sha256.as_deref(),
            expected_kind,
            pending.label_sha256.as_str(),
            model_calls,
            total_tokens,
            terminal_status,
            output_sha256.as_deref(),
        ))
        .map_err(|error| format!("failed to encode workflow GEPA action: {error}"))?;
        let chain_sha256 = sha256_hex(&chain_payload);
        self.document.actions.push(ActionReceipt {
            kind: expected_kind.to_string(),
            label_sha256: pending.label_sha256,
            completed_at_ms,
            model_calls,
            total_tokens,
            terminal_status: terminal_status.to_string(),
            output_sha256,
            prior_chain_sha256,
            chain_sha256,
        });
        Ok(())
    }

    fn require_idle(&self) -> Result<(), String> {
        if self.document.status != "running" {
            return Err("workflow GEPA journal is not running".to_string());
        }
        if current_time_millis().saturating_sub(self.document.created_at_ms)
            >= self.document.budget.max_campaign_duration_ms
        {
            return Err("workflow GEPA campaign wall-clock budget is exhausted".to_string());
        }
        if self.document.pending_action.is_some() {
            return Err(
                "workflow GEPA journal contains an interrupted provider action; use a new authorized run path"
                    .to_string(),
            );
        }
        Ok(())
    }

    fn persist(&mut self) -> Result<(), String> {
        self.document.updated_at_ms = current_time_millis();
        let bytes = serde_json::to_vec_pretty(&self.document)
            .map_err(|error| format!("failed to encode workflow GEPA journal: {error}"))?;
        tools::write_private_file_atomically(&self.path, &bytes)
            .map_err(|error| format!("failed to persist workflow GEPA journal: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_receipt(model_calls: usize) -> ProductRunReceipt {
        ProductRunReceipt {
            case_id: "case".to_string(),
            category: "coding".to_string(),
            split: super::super::workflow_gepa_campaign_contract::CampaignSplit::Train,
            replicate: 1,
            treatment: "pro".to_string(),
            execution_mode: "direct".to_string(),
            completed: true,
            terminal_status: "completed".to_string(),
            behavior_checks_passed: 1,
            behavior_checks_total: 1,
            behavior_score: 1.0,
            quality_passed: true,
            safety_violations: 0,
            latency_ms: 1,
            model_calls,
            total_tokens: 10,
            output_sha256: "a".repeat(64),
            profile_id: None,
            profile_sha256: None,
            route_profile_sha256: None,
            execution_plan_sha256: None,
            execution_plan_semantic_sha256: None,
            execution_plan_authority: None,
            workflow_execution_profile_sha256: None,
            route_profile_semantics_exercised: false,
            workflow_profile_exercised: false,
        }
    }

    fn journal(path: &Path) -> CampaignJournal {
        CampaignJournal::create(
            path,
            "b".repeat(40),
            "c".repeat(64),
            "d".repeat(64),
            vec!["e".repeat(64)],
        )
        .unwrap()
    }

    #[test]
    fn provider_action_is_reserved_before_completion() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("journal.json");
        let mut journal = journal(&path);
        journal.begin_product("train-case-seed").unwrap();
        let encoded = std::fs::read_to_string(&path).unwrap();
        assert!(encoded.contains("pending_action"));
        assert!(journal.begin_product("second").is_err());
        journal.complete_product(&run_receipt(2)).unwrap();
        assert_eq!(journal.usage().completed_product_runs, 1);
    }

    #[test]
    fn existing_or_interrupted_journal_cannot_be_replayed() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("journal.json");
        let mut first = journal(&path);
        first.begin_mutation_search().unwrap();
        assert!(CampaignJournal::create(
            &path,
            "b".repeat(40),
            "c".repeat(64),
            "d".repeat(64),
            Vec::new(),
        )
        .is_err());
        assert!(first.begin_product("after-interruption").is_err());
    }

    #[test]
    fn product_run_and_mutation_caps_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let mut mutation_journal = journal(&temp.path().join("journal.json"));
        mutation_journal.begin_mutation_search().unwrap();
        assert!(mutation_journal
            .complete_mutation_search(MAX_MUTATION_MODEL_CALLS + 1, 0, 0)
            .is_err());

        let temp = tempfile::tempdir().unwrap();
        let mut capped_journal = journal(&temp.path().join("journal.json"));
        capped_journal.document.usage.reserved_product_runs = MAX_PRODUCT_RUNS;
        assert!(capped_journal.begin_product("over-cap").is_err());

        let temp = tempfile::tempdir().unwrap();
        let mut expired_journal = journal(&temp.path().join("journal.json"));
        expired_journal.document.created_at_ms =
            current_time_millis().saturating_sub(MAX_CAMPAIGN_DURATION_MS);
        assert!(expired_journal.begin_product("expired").is_err());
    }
}
