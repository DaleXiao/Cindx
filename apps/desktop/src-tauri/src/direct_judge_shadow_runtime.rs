use super::*;

pub(crate) const DIRECT_JUDGE_SHADOW_JOURNAL_CAPACITY: usize = 256;
const DIRECT_JUDGE_SHADOW_JOURNAL_FILE: &str = "direct-judge-shadow.journal.jsonl";

pub(crate) fn direct_judge_shadow_journal_path() -> PathBuf {
    direct_judge_shadow_journal_path_for(&app_data_root())
}

pub(crate) fn direct_judge_shadow_journal_path_for(data_root: &Path) -> PathBuf {
    data_root
        .join("prompt-evolution")
        .join(DIRECT_JUDGE_SHADOW_JOURNAL_FILE)
}

/// Projects the terminal judge disposition plus task-contract mutation facts
/// into the shadow outcome receipt and appends its fitness signal to the
/// private journal. Best-effort: recording failures never disturb delivery.
pub(crate) fn record_direct_judge_shadow_fitness(
    runtime: &agent_runtime::AgentLoopState,
    disposition: &str,
    grounded_basis_label: &str,
) {
    if let Err(error) = record_direct_judge_shadow_fitness_to(
        &direct_judge_shadow_journal_path(),
        runtime,
        disposition,
        grounded_basis_label,
    ) {
        eprintln!("direct judge shadow fitness unavailable: {error}");
    }
}

pub(crate) fn record_direct_judge_shadow_fitness_to(
    journal_path: &Path,
    runtime: &agent_runtime::AgentLoopState,
    disposition: &str,
    grounded_basis_label: &str,
) -> Result<agent_application::DirectJudgeFitnessSignalV1, String> {
    let signal = project_direct_judge_shadow_signal(runtime, disposition, grounded_basis_label)?;
    append_direct_judge_shadow_signal(journal_path, &signal)?;
    Ok(signal)
}

pub(crate) fn project_direct_judge_shadow_signal(
    runtime: &agent_runtime::AgentLoopState,
    disposition: &str,
    grounded_basis_label: &str,
) -> Result<agent_application::DirectJudgeFitnessSignalV1, String> {
    let contract = &runtime.task_contract;
    let facts = agent_application::DirectJudgeCompletionFacts {
        task_identity: runtime.task_id.0.as_str(),
        disposition,
        successful_mutations: contract.successful_mutations() as u64,
        latest_mutation_verified: contract.latest_mutation_verified(),
        workspace_verification_required: matches!(
            contract.workspace_verification_policy(),
            agent_runtime::WorkspaceVerificationPolicy::RequiredAfterMutation
        ),
        grounded_basis: grounded_basis_label,
    };
    let outcome = agent_application::DirectJudgeOutcomeV1::from_completion_facts(&facts)
        .map_err(|error| error.to_string())?;
    agent_application::DirectJudgeFitnessSignalV1::from_outcome(&outcome)
        .map_err(|error| error.to_string())
}

fn append_direct_judge_shadow_signal(
    path: &Path,
    signal: &agent_application::DirectJudgeFitnessSignalV1,
) -> Result<(), String> {
    let mut lines = read_direct_judge_shadow_journal_lines(path);
    let encoded = signal.to_json().map_err(|error| error.to_string())?;
    lines.push(encoded);
    if lines.len() > DIRECT_JUDGE_SHADOW_JOURNAL_CAPACITY {
        let excess = lines.len() - DIRECT_JUDGE_SHADOW_JOURNAL_CAPACITY;
        lines.drain(..excess);
    }
    let mut payload = lines.join("\n");
    payload.push('\n');
    write_private_file_atomically(
        path,
        payload.as_bytes(),
        "direct judge shadow fitness journal",
    )
}

fn read_direct_judge_shadow_journal_lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .map(|content| {
            content
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}


/// Reads the shadow journal back. Test-only: production records the journal but
/// never reads it (inert measurement plumbing).
#[cfg(test)]
pub(crate) fn load_direct_judge_shadow_signals(
    path: &Path,
) -> Result<Vec<agent_application::DirectJudgeFitnessSignalV1>, String> {
    let mut signals = Vec::new();
    for line in read_direct_judge_shadow_journal_lines(path) {
        let signal = agent_application::DirectJudgeFitnessSignalV1::from_json(&line)
            .map_err(|error| error.to_string())?;
        signals.push(signal);
    }
    Ok(signals)
}
