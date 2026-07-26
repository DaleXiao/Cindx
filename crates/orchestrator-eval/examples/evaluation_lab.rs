use orchestrator_eval::{
    evaluate_agent_benchmark, parse_agent_benchmark_baseline, parse_agent_benchmark_observations,
    parse_agent_benchmark_suite,
};
use std::env;
use std::fs;
use std::path::PathBuf;

const DEFAULT_SUITE: &str = include_str!("../../../benchmarks/agent/core-v2.json");
const DEFAULT_BASELINE: &str = include_str!("../../../benchmarks/agent/core-v2-baseline.json");

#[derive(Debug, Default)]
struct Options {
    suite: Option<PathBuf>,
    baseline: Option<PathBuf>,
    observations: Option<PathBuf>,
    report: Option<PathBuf>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Cindx agent benchmark failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let options = parse_options()?;
    let suite_source = read_or_default(options.suite.as_ref(), DEFAULT_SUITE)?;
    let baseline_source = read_or_default(options.baseline.as_ref(), DEFAULT_BASELINE)?;
    let observation_source = options
        .observations
        .as_ref()
        .map(fs::read_to_string)
        .transpose()
        .map_err(|error| format!("could not read observations: {error}"))?
        .unwrap_or_default();
    let suite = parse_agent_benchmark_suite(&suite_source)?;
    let baseline = parse_agent_benchmark_baseline(&baseline_source)?;
    let observations = parse_agent_benchmark_observations(&observation_source)?;
    let report = evaluate_agent_benchmark(&suite, &baseline, &observations)
        .map_err(|errors| errors.join("\n"))?;

    println!(
        "Cindx agent benchmark {}-v{}: Auto {}/{} passed ({:.1}%), over={}, under={}",
        report.suite_id,
        report.suite_version,
        report.contract.passed,
        report.contract.cases,
        report.contract.pass_rate * 100.0,
        report.contract.over_orchestrated,
        report.contract.under_orchestrated
    );
    for mode in &report.modes {
        let observed = report
            .observations
            .iter()
            .find(|summary| summary.mode == mode.mode)
            .expect("all benchmark modes have an observation summary");
        let observation_text = if observed.runs == 0 {
            "quality=not_observed".to_string()
        } else {
            format!(
                "quality_pass={:.1}% completion={:.1}% latency={:.0}ms tokens={:.0} cost={}microusd",
                observed.quality_pass_rate.unwrap_or_default() * 100.0,
                observed.completion_rate.unwrap_or_default() * 100.0,
                observed.average_latency_ms.unwrap_or_default(),
                observed.average_total_tokens.unwrap_or_default(),
                observed.average_estimated_cost_microusd.unwrap_or_default()
            )
        };
        println!(
            "  {:<12} calls={:.2} latency_units={:.2} cost_units={:.2} less/same/more={}/{}/{} {}",
            mode.mode.label(),
            mode.average_estimated_model_calls,
            mode.average_estimated_latency_units,
            mode.average_estimated_cost_units,
            mode.less_orchestration_than_contract,
            mode.same_orchestration_as_contract,
            mode.more_orchestration_than_contract,
            observation_text
        );
    }

    if let Some(path) = options.report {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)
                .map_err(|error| format!("could not create report directory: {error}"))?;
        }
        let json = serde_json::to_string_pretty(&report)
            .map_err(|error| format!("could not serialize report: {error}"))?;
        fs::write(&path, json)
            .map_err(|error| format!("could not write {}: {error}", path.display()))?;
        println!("Report: {}", path.display());
    }

    if !report.baseline_passed {
        for failure in &report.baseline_failures {
            eprintln!("BASELINE: {failure}");
        }
        for failure in &report.contract.failures {
            eprintln!("CONTRACT: {failure}");
        }
        return Err("regression baseline failed".to_string());
    }
    Ok(())
}

fn read_or_default(path: Option<&PathBuf>, default: &str) -> Result<String, String> {
    path.map(fs::read_to_string)
        .transpose()
        .map_err(|error| format!("could not read benchmark input: {error}"))
        .map(|source| source.unwrap_or_else(|| default.to_string()))
}

fn parse_options() -> Result<Options, String> {
    let mut options = Options::default();
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let value = match argument.as_str() {
            "--suite" | "--baseline" | "--observations" | "--report" => arguments
                .next()
                .ok_or_else(|| format!("{argument} requires a path"))?,
            "--help" | "-h" => {
                println!(
                    "Usage: evaluation_lab [--suite PATH] [--baseline PATH] [--observations JSONL] [--report PATH]"
                );
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument {argument}")),
        };
        match argument.as_str() {
            "--suite" => options.suite = Some(value.into()),
            "--baseline" => options.baseline = Some(value.into()),
            "--observations" => options.observations = Some(value.into()),
            "--report" => options.report = Some(value.into()),
            _ => unreachable!(),
        }
    }
    Ok(options)
}
