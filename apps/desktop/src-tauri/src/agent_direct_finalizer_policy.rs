use crate::prompt_profile_serving::restore_prompt_profile_selection;
use agent_core::Metadata;
use orchestrator::{
    prompt_genome_sha256, sha256_hex, AgentExecutionMode, AgentPolicy, AgentRunDecision,
    ConductorPromptGenome, DirectFinalizerPromptPhenotype, PromptVerification,
    DIRECT_FINALIZER_PHENOTYPE_SCHEMA,
};
use serde::Serialize;

pub(crate) const DIRECT_FINALIZER_RECEIPT_SCHEMA: &str =
    "cindx.direct-finalizer-phenotype-receipt.v1";
const DIRECT_FINALIZER_RECEIPT_DOMAIN: &[u8] = b"cindx.direct-finalizer-phenotype-receipt.v1\0";
const DIRECT_FINALIZER_INSTRUCTION_DOMAIN: &[u8] = b"cindx.direct-finalizer-instruction.v1\0";

const PHENOTYPE_SCHEMA_KEY: &str = "direct_finalizer_phenotype_schema";
const PHENOTYPE_SHA256_KEY: &str = "direct_finalizer_phenotype_sha256";
const VERIFICATION_KEY: &str = "direct_finalizer_verification";

#[derive(Debug, Clone)]
pub(crate) struct DirectFinalizerPolicySelection {
    pub(crate) phenotype: DirectFinalizerPromptPhenotype,
    pub(crate) receipt_json: String,
    pub(crate) receipt_sha256: String,
}

#[derive(Debug, Serialize)]
struct DirectFinalizerPolicyReceipt<'a> {
    schema: &'static str,
    surface: &'static str,
    verification: &'a str,
    prompt_profile_assignment_sha256: &'a str,
    profile_sha256: &'a str,
    phenotype_sha256: &'a str,
    instruction_sha256: String,
    steer_epoch: u64,
}

pub(crate) fn install_direct_finalizer_policy_metadata(
    run_context: &mut Metadata,
    effort: AgentPolicy,
    execution: AgentExecutionMode,
    genome: &ConductorPromptGenome,
) -> Result<(), String> {
    for key in [PHENOTYPE_SCHEMA_KEY, PHENOTYPE_SHA256_KEY, VERIFICATION_KEY] {
        run_context.remove(key);
    }
    if execution != AgentExecutionMode::Direct
        || !matches!(effort, AgentPolicy::Auto | AgentPolicy::Pro)
    {
        return Ok(());
    }
    let phenotype = genome.direct_finalizer_phenotype();
    run_context.insert(
        PHENOTYPE_SCHEMA_KEY.to_string(),
        DIRECT_FINALIZER_PHENOTYPE_SCHEMA.to_string(),
    );
    run_context.insert(PHENOTYPE_SHA256_KEY.to_string(), phenotype.sha256()?);
    run_context.insert(
        VERIFICATION_KEY.to_string(),
        verification_label(phenotype.verification).to_string(),
    );
    Ok(())
}

pub(crate) fn selected_direct_finalizer_policy(
    run_context: &Metadata,
) -> Option<DirectFinalizerPolicySelection> {
    let effort = run_context.get("agent_effort")?.trim();
    if !matches!(effort, "auto" | "pro")
        || run_context.get("collaboration_profile").map(String::as_str) != Some("direct")
    {
        return None;
    }
    let decision =
        serde_json::from_str::<AgentRunDecision>(run_context.get("run_decision")?).ok()?;
    if decision.execution != AgentExecutionMode::Direct {
        return None;
    }
    let scope = run_context
        .get("project_id")
        .map(String::as_str)
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
        .unwrap_or("global");
    let selection = restore_prompt_profile_selection(effort, scope, run_context)
        .ok()
        .flatten()?;
    let phenotype = selection.genome.direct_finalizer_phenotype();
    let phenotype_sha256 = phenotype.sha256().ok()?;
    if run_context.get(PHENOTYPE_SCHEMA_KEY).map(String::as_str)
        != Some(DIRECT_FINALIZER_PHENOTYPE_SCHEMA)
        || run_context.get(PHENOTYPE_SHA256_KEY) != Some(&phenotype_sha256)
        || run_context.get(VERIFICATION_KEY).map(String::as_str)
            != Some(verification_label(phenotype.verification))
    {
        return None;
    }
    let assignment_sha256 = run_context.get("prompt_profile_assignment_sha256")?;
    let profile_sha256 = prompt_genome_sha256(&selection.genome).ok()?;
    let instruction_sha256 = sha256_hex(
        &[
            DIRECT_FINALIZER_INSTRUCTION_DOMAIN,
            phenotype.directive().unwrap_or_default().as_bytes(),
        ]
        .concat(),
    );
    let receipt = DirectFinalizerPolicyReceipt {
        schema: DIRECT_FINALIZER_RECEIPT_SCHEMA,
        surface: "direct_finalizer",
        verification: verification_label(phenotype.verification),
        prompt_profile_assignment_sha256: assignment_sha256,
        profile_sha256: &profile_sha256,
        phenotype_sha256: &phenotype_sha256,
        instruction_sha256,
        steer_epoch: run_context
            .get("steer_epoch")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or_default(),
    };
    let receipt_json = serde_json::to_string(&receipt).ok()?;
    let receipt_sha256 =
        sha256_hex(&[DIRECT_FINALIZER_RECEIPT_DOMAIN, receipt_json.as_bytes()].concat());
    Some(DirectFinalizerPolicySelection {
        phenotype,
        receipt_json,
        receipt_sha256,
    })
}

pub(crate) fn insert_direct_finalizer_receipt_metadata(
    metadata: &mut Metadata,
    selection: &DirectFinalizerPolicySelection,
) {
    metadata.insert(
        "direct_finalizer_phenotype_receipt".to_string(),
        selection.receipt_json.clone(),
    );
    metadata.insert(
        "direct_finalizer_phenotype_receipt_sha256".to_string(),
        selection.receipt_sha256.clone(),
    );
    metadata.insert(
        PHENOTYPE_SHA256_KEY.to_string(),
        selection
            .phenotype
            .sha256()
            .expect("validated direct phenotype must serialize"),
    );
    metadata.insert(
        VERIFICATION_KEY.to_string(),
        verification_label(selection.phenotype.verification).to_string(),
    );
}

fn verification_label(value: PromptVerification) -> &'static str {
    match value {
        PromptVerification::Minimal => "minimal",
        PromptVerification::Evidence => "evidence",
        PromptVerification::Adversarial => "adversarial",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt_profile_serving::{seed_prompt_profile_selection, PromptProfileFallback};
    use agent_core::{AGENT_RUN_ID_METADATA_KEY, LOGICAL_AGENT_RUN_ID_METADATA_KEY};

    fn assigned_context(verification: PromptVerification) -> Metadata {
        let mut context = [
            (
                "project_id".to_string(),
                "direct-finalizer-project".to_string(),
            ),
            ("agent_effort".to_string(), "auto".to_string()),
            ("collaboration_profile".to_string(), "direct".to_string()),
            ("steer_epoch".to_string(), "2".to_string()),
            (
                AGENT_RUN_ID_METADATA_KEY.to_string(),
                "physical-direct-finalizer-run".to_string(),
            ),
            (
                LOGICAL_AGENT_RUN_ID_METADATA_KEY.to_string(),
                "logical-direct-finalizer-run".to_string(),
            ),
        ]
        .into_iter()
        .collect::<Metadata>();
        let mut selection = seed_prompt_profile_selection(
            "auto",
            PromptProfileFallback::EvolutionDisabled,
            &context,
        );
        selection.genome.id = "direct-finalizer-profile".to_string();
        selection.genome.direct_finalizer_verification = verification;
        let profile_sha256 = prompt_genome_sha256(&selection.genome).unwrap();
        selection.receipt.profile_id = selection.genome.id.clone();
        selection.receipt.profile_sha256 = profile_sha256.clone();
        selection.receipt.stable_profile_sha256 = profile_sha256;
        let (receipt, receipt_sha256) = selection.receipt_json_and_sha256().unwrap();
        context.insert("prompt_profile".to_string(), selection.genome.id.clone());
        context.insert(
            "prompt_profile_source".to_string(),
            selection.source_label(),
        );
        context.insert(
            "prompt_genome".to_string(),
            serde_json::to_string(&selection.genome).unwrap(),
        );
        context.insert("prompt_profile_assignment_receipt".to_string(), receipt);
        context.insert(
            "prompt_profile_assignment_sha256".to_string(),
            receipt_sha256,
        );
        context.insert(
            "run_decision".to_string(),
            serde_json::to_string(&AgentRunDecision::direct("executor")).unwrap(),
        );
        install_direct_finalizer_policy_metadata(
            &mut context,
            AgentPolicy::Auto,
            AgentExecutionMode::Direct,
            &selection.genome,
        )
        .unwrap();
        context
    }

    #[test]
    fn validated_direct_assignment_produces_bounded_receipt() {
        let context = assigned_context(PromptVerification::Adversarial);
        let selected = selected_direct_finalizer_policy(&context)
            .expect("validated direct assignment should select a phenotype");
        assert_eq!(
            selected.phenotype.verification,
            PromptVerification::Adversarial
        );
        assert!(selected.phenotype.directive().is_some());
        assert!(selected.receipt_json.len() < 1_024);
        assert_eq!(selected.receipt_sha256.len(), 64);
    }

    #[test]
    fn tampered_assignment_and_workflow_fail_closed() {
        let context = assigned_context(PromptVerification::Adversarial);
        let mut tampered = context.clone();
        tampered.insert(
            "prompt_profile_assignment_sha256".to_string(),
            "0".repeat(64),
        );
        assert!(selected_direct_finalizer_policy(&tampered).is_none());

        let mut workflow = context;
        workflow.insert("collaboration_profile".to_string(), "adaptive".to_string());
        assert!(selected_direct_finalizer_policy(&workflow).is_none());
    }
}
