use super::collaboration_successor_protocol::{clean_source_head_tree, now_millis};
use super::delivery_verification::DeliveryVerificationModelFailure;
use super::delivery_verification_authorization::*;
use super::delivery_verification_campaign::{
    execute_fixed_campaign, CallOutcome, CaseOutcome, CaseStatus, CompletedCall,
    DeliveryVerificationRuntime, PreparedDeliveryCall,
};
use super::delivery_verification_execution_journal::*;
use super::delivery_verification_preflight::{
    parse_and_validate_receipt, protocol_snapshot, provider_binding, validate_current_authority,
    DeliveryVerificationPreflightReceipt, DeliveryVerificationSourceBindingReceipt,
};
use super::delivery_verification_protocol::{
    parse_and_validate_protocol, CalibrationDecision, HoldoutDecisionResult, MatchedPairCounts,
    ProtocolBudget, ValidatedProtocol, CONSUMED_DELIVERY_VERIFICATION_PROTOCOL_V1_ID,
    DELIVERY_VERIFICATION_PROTOCOL_ID, DELIVERY_VERIFICATION_PROTOCOL_RELATIVE_PATH,
    DELIVERY_VERIFICATION_SUITE_RELATIVE_PATH,
};
use super::delivery_verification_runner_binary::{
    read_current_delivery_execute, read_delivery_execute_sibling, DeliveryVerificationRunnerBinary,
};
use crate::configuration_models::ProviderConfig;
use agent_core::{
    ModelResponse, ModelResponseDisposition, ModelResponseTermination, ModelRole,
    ProviderFailureClass,
};
use model_provider::{ModelError, OpenAiCompatibleConfig, OpenAiCompatibleProvider};
use orchestrator::sha256_hex;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

const PREFLIGHT_RECEIPT_ENV: &str = "CINDX_DELIVERY_VERIFICATION_V2_PREFLIGHT_RECEIPT";
const AUTHORIZATION_ENV: &str = "CINDX_DELIVERY_VERIFICATION_V2_AUTHORIZATION";
const OUTPUT_ROOT_ENV: &str = "CINDX_DELIVERY_VERIFICATION_V2_OUTPUT_ROOT";
const AUTHORIZE_FLAG: &str = "--authorize-once";

pub(in super::super) fn run_authorize() -> Result<(), String> {
    require_authorize_arguments(std::env::args_os())?;
    reject_consumed_delivery_protocol(DELIVERY_VERIFICATION_PROTOCOL_ID)?;
    let runner = read_delivery_execute_sibling()?;
    let inputs = StaticInputs::load(true, &runner)?;
    let provider = crate::configuration_persistence::load_provider_config();
    inputs.validate_current_authority(&provider, &runner)?;
    let issued_at_ms = now_millis()?;
    let expires_at_ms = issued_at_ms
        .checked_add(DELIVERY_AUTHORIZATION_TTL_MS)
        .ok_or_else(|| "delivery authorization expiry overflowed".to_string())?;
    let nonce = authorization_nonce(
        issued_at_ms,
        &inputs.authorization_path,
        &inputs.output_root,
        &runner,
    );
    let authorization =
        issue_delivery_verification_authorization(DeliveryVerificationAuthorizationIssue {
            protocol: &inputs.protocol(),
            preflight: &inputs.preflight,
            repo_root: &inputs.repo_root,
            preflight_path: &inputs.preflight_path,
            authorization_path: &inputs.authorization_path,
            output_root: &inputs.output_root,
            runner: &runner,
            issued_at_ms,
            expires_at_ms,
            nonce: &nonce,
            credential: &provider.api_key,
        })?;
    inputs.validate_current_authority(&provider, &runner)?;
    write_delivery_verification_authorization_new(
        &inputs.repo_root,
        &inputs.authorization_path,
        &authorization,
    )?;
    eprintln!(
        "[delivery-verification-authorize] authorization={} digest={} runner={} expires_at_ms={} provider_calls=0",
        inputs.authorization_path.display(),
        authorization.authorization_sha256,
        authorization.runner_sha256,
        expires_at_ms,
    );
    Ok(())
}

pub(in super::super) fn run_execute() -> Result<(), String> {
    if std::env::args_os().len() != 1 {
        return Err("delivery verification execute accepts no command-line arguments".into());
    }
    reject_consumed_delivery_protocol(DELIVERY_VERIFICATION_PROTOCOL_ID)?;
    let runner = read_current_delivery_execute()?;
    let repo_root = repository_root()?;
    let raw_output_root = required_path(OUTPUT_ROOT_ENV)?;
    if raw_output_root.exists() {
        let output_root =
            canonical_external_path(&raw_output_root, &repo_root, true, "delivery output root")?;
        let recovery = DeliveryVerificationExecutionJournal::recover(&output_root, now_millis()?)?;
        return Err(format!(
            "delivery output root is already consumed and cannot execute again: {recovery:?}"
        ));
    }

    let inputs = StaticInputs::load(false, &runner)?;
    let provider = crate::configuration_persistence::load_provider_config();
    inputs.validate_current_authority(&provider, &runner)?;
    let validated = load_and_validate_delivery_verification_authorization(
        DeliveryVerificationAuthorizationValidation {
            protocol: &inputs.protocol(),
            preflight: &inputs.preflight,
            repo_root: &inputs.repo_root,
            preflight_path: &inputs.preflight_path,
            authorization_path: &inputs.authorization_path,
            output_root: &inputs.output_root,
            runner: &runner,
            credential: &provider.api_key,
            now_ms: now_millis()?,
        },
    )?;
    if sha256_hex(&runner.bytes) != validated.authorization.runner_sha256
        || runner.code_directory_sha256 != validated.authorization.runner_code_directory_sha256
    {
        return Err("delivery execute binary differs from its authorization".into());
    }
    inputs.validate_current_authority(&provider, &runner)?;
    let tombstone = consume_delivery_verification_authorization_once(&validated, now_millis()?)?;
    let mut journal = DeliveryVerificationExecutionJournal::create_new(
        &inputs.output_root,
        &validated,
        &tombstone,
    )?;
    journal.reserve_campaign(now_millis()?)?;
    let result = {
        let protocol = inputs.protocol();
        let mut runtime = JournalRuntime::new(&mut journal, provider, protocol.budget().clone());
        execute_fixed_campaign(&protocol, &mut runtime)
    };
    if let Err(error) = &result {
        if !journal.is_terminal() {
            let reason = format!("execution_error:{}", sha256_hex(error.as_bytes()));
            let _ = journal.freeze(
                &reason,
                DeliveryVerificationCampaignDispositionV1::Censored,
                now_millis().unwrap_or(1),
            );
        }
    }
    result.map(|_| ())
}

pub(super) fn require_authorize_arguments(
    args: impl IntoIterator<Item = OsString>,
) -> Result<(), String> {
    let args = args.into_iter().collect::<Vec<_>>();
    if args.len() == 3 && args[1] == OsStr::new(AUTHORIZE_FLAG) {
        let protocol_id = args[2]
            .to_str()
            .ok_or_else(|| "delivery authorization protocol id is not UTF-8".to_string())?;
        reject_consumed_delivery_protocol(protocol_id)?;
        if protocol_id == DELIVERY_VERIFICATION_PROTOCOL_ID {
            return Ok(());
        }
    }
    Err(format!(
        "delivery authorization requires exactly `{AUTHORIZE_FLAG} {DELIVERY_VERIFICATION_PROTOCOL_ID}`"
    ))
}

pub(super) fn reject_consumed_delivery_protocol(protocol_id: &str) -> Result<(), String> {
    if protocol_id == CONSUMED_DELIVERY_VERIFICATION_PROTOCOL_V1_ID {
        return Err(format!(
            "delivery verification protocol `{protocol_id}` is consumed and cannot be preflighted, authorized, or executed again; use the separately frozen successor protocol"
        ));
    }
    Ok(())
}

fn authorization_nonce(
    now_ms: u64,
    authorization: &Path,
    output: &Path,
    runner: &DeliveryVerificationRunnerBinary,
) -> String {
    sha256_hex(
        format!(
            "cindx.delivery-verification-authorization-nonce.v2\0{now_ms}\0{}\0{}\0{}\0{}",
            authorization.display(),
            output.display(),
            sha256_hex(&runner.bytes),
            runner.code_directory_sha256,
        )
        .as_bytes(),
    )
}

struct StaticInputs {
    repo_root: PathBuf,
    manifest_bytes: Vec<u8>,
    suite_bytes: Vec<u8>,
    preflight_path: PathBuf,
    preflight: DeliveryVerificationPreflightReceipt,
    authorization_path: PathBuf,
    output_root: PathBuf,
}

impl StaticInputs {
    fn load(
        authorization_must_be_new: bool,
        runner: &DeliveryVerificationRunnerBinary,
    ) -> Result<Self, String> {
        let repo_root = repository_root()?;
        let manifest_bytes = fs::read(repo_root.join(DELIVERY_VERIFICATION_PROTOCOL_RELATIVE_PATH))
            .map_err(|error| format!("failed to read delivery protocol: {error}"))?;
        let suite_bytes = fs::read(repo_root.join(DELIVERY_VERIFICATION_SUITE_RELATIVE_PATH))
            .map_err(|error| format!("failed to read delivery suite: {error}"))?;
        let protocol = parse_and_validate_protocol(&manifest_bytes, &suite_bytes)?;
        let output_root = canonical_external_path(
            &required_path(OUTPUT_ROOT_ENV)?,
            &repo_root,
            false,
            "delivery output root",
        )?;
        if output_root.exists() {
            return Err("delivery output root must be new".into());
        }
        let preflight_path = canonical_external_path(
            &required_path(PREFLIGHT_RECEIPT_ENV)?,
            &repo_root,
            true,
            "delivery preflight receipt",
        )?;
        let authorization_path = canonical_external_path(
            &required_path(AUTHORIZATION_ENV)?,
            &repo_root,
            !authorization_must_be_new,
            "delivery authorization",
        )?;
        if preflight_path == authorization_path
            || preflight_path == output_root
            || authorization_path == output_root
            || preflight_path.starts_with(&output_root)
            || authorization_path.starts_with(&output_root)
        {
            return Err("delivery control-plane paths must be separate".into());
        }
        let preflight_bytes = read_private_file(&preflight_path, "delivery preflight receipt")?;
        let (head, tree) = clean_source_head_tree(&repo_root)?;
        let config = crate::configuration_persistence::load_provider_config();
        let preflight = parse_and_validate_receipt(
            &protocol_snapshot(&protocol),
            &preflight_bytes,
            &DeliveryVerificationSourceBindingReceipt { head, tree },
            &provider_binding(&config)?,
            runner,
            &output_root,
        )?;
        Ok(Self {
            repo_root,
            manifest_bytes,
            suite_bytes,
            preflight_path,
            preflight,
            authorization_path,
            output_root,
        })
    }

    fn protocol(&self) -> ValidatedProtocol<'_> {
        parse_and_validate_protocol(&self.manifest_bytes, &self.suite_bytes)
            .expect("validated delivery protocol remains valid")
    }

    fn validate_current_authority(
        &self,
        provider: &ProviderConfig,
        runner: &DeliveryVerificationRunnerBinary,
    ) -> Result<(), String> {
        validate_current_authority(
            &protocol_snapshot(&self.protocol()),
            &self.preflight,
            &self.repo_root,
            provider,
            runner,
            &self.output_root,
        )
    }
}

fn repository_root() -> Result<PathBuf, String> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .map_err(|error| format!("failed to locate repository root: {error}"))
}

fn required_path(name: &str) -> Result<PathBuf, String> {
    let value = std::env::var_os(name).ok_or_else(|| format!("{name} is required"))?;
    if value.is_empty() {
        return Err(format!("{name} is empty"));
    }
    Ok(PathBuf::from(value))
}

struct JournalRuntime<'a> {
    journal: &'a mut DeliveryVerificationExecutionJournal,
    provider: ProviderConfig,
    budget: ProtocolBudget,
    owner_model: String,
    verifier_model: String,
    campaign_started: Instant,
    case_started: Option<Instant>,
    served_models: ServedModelBinding,
    primary_terminal_error: Option<String>,
}

impl<'a> JournalRuntime<'a> {
    fn new(
        journal: &'a mut DeliveryVerificationExecutionJournal,
        provider: ProviderConfig,
        budget: ProtocolBudget,
    ) -> Self {
        let owner_model = provider.model_for_role(&ModelRole::Executor);
        let verifier_model = provider.model_for_role(&ModelRole::Reviewer);
        Self {
            journal,
            provider,
            budget,
            owner_model,
            verifier_model,
            campaign_started: Instant::now(),
            case_started: None,
            served_models: ServedModelBinding::default(),
            primary_terminal_error: None,
        }
    }

    fn retain_primary_terminal_error(&mut self, error: String) -> CallOutcome {
        if self.primary_terminal_error.is_none() {
            self.primary_terminal_error = Some(error.clone());
        }
        CallOutcome::StructuralFailure(error)
    }

    fn time_budget_exhausted(&self) -> bool {
        elapsed_ms(self.campaign_started) >= self.budget.campaign_timeout_ms
            || self
                .case_started
                .is_some_and(|start| elapsed_ms(start) >= self.budget.case_timeout_ms)
    }

    fn terminal_for_response(
        &mut self,
        permit: DeliveryVerificationCallPermitV1,
        response: ModelResponse,
        latency_ms: u64,
        terminal_at_ms: u64,
    ) -> CallOutcome {
        let validated = validate_model_response(&response);
        let (status, failure_class, retryable) = match &validated {
            Ok(_) => (
                DeliveryVerificationCallTerminalStatusV1::Completed,
                None,
                None,
            ),
            Err(error) => (
                DeliveryVerificationCallTerminalStatusV1::InvalidOutput,
                Some(error.clone()),
                Some(false),
            ),
        };
        let terminal = DeliveryVerificationCallTerminalInputV1 {
            permit,
            status,
            failure_class,
            retryable,
            provider_status_code: None,
            latency_ms,
            request_payload_sha256: response.metadata.get("request_payload_sha256").cloned(),
            response_semantic_sha256: response.metadata.get("response_semantic_sha256").cloned(),
            provider_response_id_sha256: response
                .metadata
                .get("provider_response_id")
                .map(|value| sha256_hex(value.as_bytes())),
            provider_response_model_sha256: response
                .metadata
                .get("provider_response_model")
                .map(|value| sha256_hex(value.as_bytes())),
            provider_system_fingerprint_sha256: response
                .metadata
                .get("provider_system_fingerprint")
                .map(|value| sha256_hex(value.as_bytes())),
            provider_receipt_status: response.metadata.get("provider_receipt_status").cloned(),
            usage: exact_usage(&response),
            terminal_at_ms,
        };
        if let Err(error) = self
            .journal
            .record_call_terminal(terminal, Some(response.message.content.as_bytes()))
        {
            return self.retain_primary_terminal_error(error);
        }
        match validated {
            Ok(served_model_sha256) => CallOutcome::Completed(CompletedCall {
                content: response.message.content,
                served_model_sha256,
            }),
            Err(error) => CallOutcome::StructuralFailure(error),
        }
    }
}

#[derive(Default)]
pub(super) struct ServedModelBinding {
    owner_sha256: Option<String>,
    verifier_sha256: Option<String>,
}

impl ServedModelBinding {
    pub(super) fn observe(&mut self, role: &ModelRole, sha256: &str) -> Result<(), String> {
        require_sha256(sha256, "delivery served model")?;
        let slot = match role {
            ModelRole::Executor => &mut self.owner_sha256,
            ModelRole::Reviewer => &mut self.verifier_sha256,
            _ => return Err("delivery served model has an invalid role".into()),
        };
        match slot {
            Some(expected) if expected != sha256 => {
                return Err("delivery served model changed during the campaign".into())
            }
            None => *slot = Some(sha256.to_string()),
            Some(_) => {}
        }
        if self.owner_sha256.is_some() && self.owner_sha256 == self.verifier_sha256 {
            return Err("delivery served Owner and Verifier models are not independent".into());
        }
        Ok(())
    }
}

impl DeliveryVerificationRuntime for JournalRuntime<'_> {
    fn configured_model(&self, role: &ModelRole) -> &str {
        match role {
            ModelRole::Executor => &self.owner_model,
            ModelRole::Reviewer => &self.verifier_model,
            _ => "",
        }
    }

    fn begin_case(&mut self, ordinal: usize) -> Result<(), String> {
        self.journal.reserve_case(ordinal, now_millis()?)?;
        self.case_started = Some(Instant::now());
        Ok(())
    }

    fn dispatch(&mut self, call: PreparedDeliveryCall) -> CallOutcome {
        if self.time_budget_exhausted() {
            return CallOutcome::ModelFailure(DeliveryVerificationModelFailure::BudgetExhausted);
        }
        let call_role = call.role.clone();
        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: self.provider.base_url.clone(),
            api_key: self.provider.api_key.clone(),
            model: call.configured_model.clone(),
            embedding_model: self.provider.embedding_model.clone(),
            timeout_seconds: self.budget.model_call_timeout_ms.div_ceil(1_000).max(1),
        });
        let prepared = match provider.prepare_non_streaming_request(&call.request) {
            Ok(prepared) => prepared,
            Err(error) => return self.retain_primary_terminal_error(error.to_string()),
        };
        let (wire_payload_sha256, wire_payload_bytes) = prepared.payload_receipt();
        let wire_payload_sha256 = wire_payload_sha256.to_string();
        let wire_payload_bytes = match u64::try_from(wire_payload_bytes) {
            Ok(value) => value,
            Err(_) => {
                return self
                    .retain_primary_terminal_error("delivery wire payload size overflowed".into())
            }
        };
        let reserved_at_ms = match now_millis() {
            Ok(value) => value,
            Err(error) => return self.retain_primary_terminal_error(error),
        };
        let permit = match self
            .journal
            .reserve_call(DeliveryVerificationCallReservationInputV1 {
                case_ordinal: call.case_ordinal,
                stage: call.stage,
                role: call.role.clone(),
                configured_model_sha256: sha256_hex(call.configured_model.as_bytes()),
                semantic_request_sha256: call.canonical_request_sha256,
                semantic_request_bytes: call.canonical_request_bytes,
                wire_payload_sha256,
                wire_payload_bytes,
                max_output_tokens: call.max_output_tokens,
                reserved_at_ms,
            }) {
            Ok(permit) => permit,
            Err(error) => return self.retain_primary_terminal_error(error),
        };
        let started = Instant::now();
        let response = provider.complete_prepared_non_streaming_request(prepared);
        let latency_ms = elapsed_ms(started);
        let terminal_at_ms = now_millis().unwrap_or(reserved_at_ms);
        match response {
            Ok(response) => {
                let outcome =
                    self.terminal_for_response(permit, response, latency_ms, terminal_at_ms);
                let CallOutcome::Completed(completed) = &outcome else {
                    return outcome;
                };
                if let Err(error) = self
                    .served_models
                    .observe(&call_role, &completed.served_model_sha256)
                {
                    return CallOutcome::StructuralFailure(error);
                }
                if self.time_budget_exhausted() {
                    return CallOutcome::ModelFailure(
                        DeliveryVerificationModelFailure::BudgetExhausted,
                    );
                }
                outcome
            }
            Err(error) => {
                let outcome = model_error_outcome(&error);
                let status = if error.is_cancelled() {
                    DeliveryVerificationCallTerminalStatusV1::Cancelled
                } else {
                    DeliveryVerificationCallTerminalStatusV1::ProviderFailure
                };
                let terminal = DeliveryVerificationCallTerminalInputV1 {
                    status,
                    ..failed_terminal(
                        permit,
                        error.class.label(),
                        error.is_retryable(),
                        error.status_code,
                        latency_ms,
                        terminal_at_ms,
                    )
                };
                if let Err(error) = self.journal.record_call_terminal(terminal, None) {
                    return self.retain_primary_terminal_error(error);
                }
                outcome
            }
        }
    }

    fn record_case(&mut self, outcome: &CaseOutcome) -> Result<(), String> {
        if self.journal.is_terminal() {
            return Ok(());
        }
        if let Some(error) = self.primary_terminal_error.take() {
            return Err(error);
        }
        self.journal
            .record_case_terminal(DeliveryVerificationCaseTerminalInputV1 {
                case_ordinal: outcome.ordinal,
                status: match outcome.status {
                    CaseStatus::Complete => DeliveryVerificationCaseTerminalStatusV1::Complete,
                    CaseStatus::StructuralFailure => {
                        DeliveryVerificationCaseTerminalStatusV1::StructuralFailure
                    }
                    CaseStatus::TreatmentExecutionFailure => {
                        DeliveryVerificationCaseTerminalStatusV1::TreatmentExecutionFailure
                    }
                    CaseStatus::Censored => DeliveryVerificationCaseTerminalStatusV1::Censored,
                },
                control_passed: outcome.control_passed,
                treatment_passed: outcome.treatment_passed,
                owner_draft_sha256: outcome.owner_draft_sha256.clone(),
                owner_draft_bytes: outcome.owner_draft_bytes,
                control_output_sha256: outcome.control_output_sha256.clone(),
                treatment_output_sha256: outcome.treatment_output_sha256.clone(),
                observation_sha256: outcome.observation_sha256.clone(),
                completed_at_ms: now_millis()?,
            })
    }

    fn record_calibration(
        &mut self,
        decision: CalibrationDecision,
        counts: MatchedPairCounts,
    ) -> Result<(), String> {
        self.journal.record_calibration_decision(
            super::delivery_verification_campaign::calibration_label(decision),
            super::delivery_verification_campaign::counts_digest("calibration", counts),
            now_millis()?,
        )
    }

    fn record_holdout(
        &mut self,
        decision: HoldoutDecisionResult,
        counts: MatchedPairCounts,
    ) -> Result<(), String> {
        self.journal.record_holdout_decision(
            super::delivery_verification_campaign::holdout_label(decision),
            super::delivery_verification_campaign::counts_digest("holdout", counts),
            now_millis()?,
        )
    }

    fn finish(
        &mut self,
        disposition: DeliveryVerificationCampaignDispositionV1,
        reason: &str,
        evidence_sha256: String,
    ) -> Result<(), String> {
        self.journal
            .finish(disposition, reason, evidence_sha256, now_millis()?)
    }

    fn is_terminal(&self) -> bool {
        self.journal.is_terminal()
    }
}

pub(super) fn validate_model_response(response: &ModelResponse) -> Result<String, String> {
    let assessment = response.assessment();
    if assessment.termination != ModelResponseTermination::Complete
        || assessment.disposition != ModelResponseDisposition::Usable
        || !response.tool_calls.is_empty()
        || response.raw_tool_calls_json.is_some()
    {
        return Err("delivery provider response is not a complete tool-free answer".into());
    }
    for key in ["request_payload_sha256", "response_semantic_sha256"] {
        let value = response
            .metadata
            .get(key)
            .ok_or_else(|| format!("delivery provider response lacks {key}"))?;
        require_sha256(value, key)?;
    }
    if response
        .metadata
        .get("provider_receipt_status")
        .map(String::as_str)
        != Some("observed")
    {
        return Err("delivery provider response lacks an observed provider receipt".into());
    }
    let response_id = required_metadata(response, "provider_response_id")?;
    let response_model = required_metadata(response, "provider_response_model")?;
    if response_id.len() > 4_096 || response_model.len() > 512 {
        return Err("delivery provider identity metadata exceeds its bound".into());
    }
    exact_usage(response)
        .ok_or_else(|| "delivery provider response lacks exact usage".to_string())?;
    Ok(sha256_hex(response_model.as_bytes()))
}

fn required_metadata<'a>(response: &'a ModelResponse, key: &str) -> Result<&'a str, String> {
    response
        .metadata
        .get(key)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty() && value.trim() == *value)
        .ok_or_else(|| format!("delivery provider response lacks {key}"))
}

fn exact_usage(response: &ModelResponse) -> Option<DeliveryVerificationCallUsageV1> {
    let metadata = &response.metadata;
    let usage = DeliveryVerificationCallUsageV1 {
        prompt_tokens: metadata.get("prompt_tokens")?.parse().ok()?,
        completion_tokens: metadata.get("completion_tokens")?.parse().ok()?,
        total_tokens: metadata.get("total_tokens")?.parse().ok()?,
        usage_source: metadata.get("usage_source")?.clone(),
        usage_estimated: metadata.get("usage_estimated")?.parse().ok()?,
    };
    (usage.usage_source == "provider"
        && !usage.usage_estimated
        && usage.prompt_tokens.checked_add(usage.completion_tokens)? == usage.total_tokens)
        .then_some(usage)
}

fn failed_terminal(
    permit: DeliveryVerificationCallPermitV1,
    failure_class: &str,
    retryable: bool,
    provider_status_code: Option<u16>,
    latency_ms: u64,
    terminal_at_ms: u64,
) -> DeliveryVerificationCallTerminalInputV1 {
    DeliveryVerificationCallTerminalInputV1 {
        permit,
        status: DeliveryVerificationCallTerminalStatusV1::ProviderFailure,
        failure_class: Some(failure_class.to_string()),
        retryable: Some(retryable),
        provider_status_code,
        latency_ms,
        request_payload_sha256: None,
        response_semantic_sha256: None,
        provider_response_id_sha256: None,
        provider_response_model_sha256: None,
        provider_system_fingerprint_sha256: None,
        provider_receipt_status: None,
        usage: None,
        terminal_at_ms,
    }
}

fn model_error_outcome(error: &ModelError) -> CallOutcome {
    match error.class {
        ProviderFailureClass::Cancelled => CallOutcome::Censored("provider_cancelled".into()),
        ProviderFailureClass::Timeout => {
            CallOutcome::ModelFailure(DeliveryVerificationModelFailure::Timeout)
        }
        ProviderFailureClass::RateLimited
        | ProviderFailureClass::Unavailable
        | ProviderFailureClass::Transport => {
            CallOutcome::ModelFailure(DeliveryVerificationModelFailure::ProviderUnavailable)
        }
        _ => CallOutcome::StructuralFailure(format!(
            "provider_failure:{}:{}",
            error.class.label(),
            sha256_hex(error.message.as_bytes())
        )),
    }
}

fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
#[path = "delivery_verification_runner_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "delivery_verification_loopback_tests.rs"]
mod loopback_tests;
