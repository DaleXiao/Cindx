use orchestrator_eval::{
    build_agent_evaluation_foundation_report, build_agent_evaluation_promotion_report,
    parse_agent_evaluation_baseline, parse_agent_evaluation_dataset,
    parse_agent_evaluation_score_set, AgentEvaluationDataset, AgentEvaluationScoreSet,
};
use std::env;
use std::fs;
use std::path::PathBuf;

const DEFAULT_BASELINE: &str =
    include_str!("../../../benchmarks/agent/evaluation-v2-baseline.json");
const DEFAULT_ROUTING_SUITE: &str = include_str!("../../../benchmarks/agent/core-v1.json");
const DEFAULT_ROUTING_BASELINE: &str =
    include_str!("../../../benchmarks/agent/core-v1-baseline.json");

#[derive(Debug, Default)]
struct Options {
    baseline: Option<PathBuf>,
    feedback: Option<PathBuf>,
    pareto: Option<PathBuf>,
    test: Option<PathBuf>,
    report: Option<PathBuf>,
    baseline_scores: Option<PathBuf>,
    candidate_scores: Option<PathBuf>,
    promotion_report: Option<PathBuf>,
    require_ready: bool,
    require_promotion: bool,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Cindx Evaluation v2 foundation failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let options = parse_options()?;
    let baseline_source = options
        .baseline
        .as_ref()
        .map(fs::read_to_string)
        .transpose()
        .map_err(|error| format!("could not read evaluation baseline: {error}"))?
        .unwrap_or_else(|| DEFAULT_BASELINE.to_string());
    let baseline = parse_agent_evaluation_baseline(&baseline_source)?;
    let datasets = [options.feedback, options.pareto, options.test]
        .into_iter()
        .flatten()
        .map(read_dataset)
        .collect::<Result<Vec<_>, _>>()?;
    let report = build_agent_evaluation_foundation_report(
        &baseline,
        &datasets,
        DEFAULT_ROUTING_SUITE,
        DEFAULT_ROUTING_BASELINE,
    )
    .map_err(|errors| errors.join("\n"))?;

    println!(
        "Cindx Evaluation v2 {}: routing_verified={} quality={:?}",
        report.baseline_id, report.routing_contract_verified, report.quality_evidence_status
    );
    for (split, cases) in &report.split_case_counts {
        println!("  {:<8} cases={cases}", split.label());
    }
    println!(
        "  optimization_ready={} hidden_test_ready={}",
        report.ready_for_optimization, report.ready_for_hidden_test
    );

    if let Some(path) = options.report {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)
                .map_err(|error| format!("could not create report directory: {error}"))?;
        }
        let json = serde_json::to_string_pretty(&report)
            .map_err(|error| format!("could not serialize foundation report: {error}"))?;
        fs::write(&path, json)
            .map_err(|error| format!("could not write {}: {error}", path.display()))?;
        println!("Report: {}", path.display());
    }

    if options.require_ready && !(report.ready_for_optimization && report.ready_for_hidden_test) {
        return Err("evaluation datasets have not reached the frozen promotion gate".to_string());
    }
    let promotion = match (options.baseline_scores, options.candidate_scores) {
        (Some(baseline_scores), Some(candidate_scores)) => {
            let baseline_scores = read_score_set(baseline_scores)?;
            let candidate_scores = read_score_set(candidate_scores)?;
            Some(
                build_agent_evaluation_promotion_report(
                    &baseline,
                    &baseline_scores,
                    &candidate_scores,
                )
                .map_err(|errors| errors.join("\n"))?,
            )
        }
        (None, None) => None,
        _ => {
            return Err(
                "--baseline-scores and --candidate-scores must be supplied together".to_string(),
            )
        }
    };
    if let Some(promotion) = promotion {
        println!(
            "Hidden promotion {} -> {}: eligible={} success_gain={:.3} pairwise_wilson={:.3} max_category_regression={:.3}",
            promotion.baseline.candidate_id,
            promotion.candidate.candidate_id,
            promotion.promotion_eligible,
            promotion.absolute_success_gain,
            promotion.pairwise_wilson_lower_bound,
            promotion.maximum_category_regression,
        );
        if let Some(path) = options.promotion_report {
            write_json_report(&path, &promotion)?;
            println!("Promotion report: {}", path.display());
        }
        if options.require_promotion && !promotion.promotion_eligible {
            return Err("hidden test promotion gate did not pass".to_string());
        }
    } else if options.require_promotion {
        return Err("--require-promotion needs paired hidden score sets".to_string());
    }
    Ok(())
}

fn read_dataset(path: PathBuf) -> Result<AgentEvaluationDataset, String> {
    let source = fs::read_to_string(&path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    parse_agent_evaluation_dataset(&source)
}

fn read_score_set(path: PathBuf) -> Result<AgentEvaluationScoreSet, String> {
    let source = fs::read_to_string(&path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    parse_agent_evaluation_score_set(&source)
}

fn write_json_report(path: &PathBuf, value: &impl serde::Serialize) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("could not create report directory: {error}"))?;
    }
    let json = serde_json::to_string_pretty(value)
        .map_err(|error| format!("could not serialize report: {error}"))?;
    fs::write(path, json).map_err(|error| format!("could not write {}: {error}", path.display()))
}

fn parse_options() -> Result<Options, String> {
    let mut options = Options::default();
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--require-ready" {
            options.require_ready = true;
            continue;
        }
        if argument == "--require-promotion" {
            options.require_promotion = true;
            continue;
        }
        if matches!(argument.as_str(), "--help" | "-h") {
            println!(
                "Usage: evaluation_v2_lab [--baseline PATH] [--feedback PATH] [--pareto PATH] [--test PATH] [--report PATH] [--baseline-scores PATH --candidate-scores PATH --promotion-report PATH] [--require-ready] [--require-promotion]"
            );
            std::process::exit(0);
        }
        let value = arguments
            .next()
            .ok_or_else(|| format!("{argument} requires a path"))?;
        match argument.as_str() {
            "--baseline" => options.baseline = Some(value.into()),
            "--feedback" => options.feedback = Some(value.into()),
            "--pareto" => options.pareto = Some(value.into()),
            "--test" => options.test = Some(value.into()),
            "--report" => options.report = Some(value.into()),
            "--baseline-scores" => options.baseline_scores = Some(value.into()),
            "--candidate-scores" => options.candidate_scores = Some(value.into()),
            "--promotion-report" => options.promotion_report = Some(value.into()),
            _ => return Err(format!("unknown argument {argument}")),
        }
    }
    Ok(options)
}
