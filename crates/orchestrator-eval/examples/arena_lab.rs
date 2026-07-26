use orchestrator_eval::{
    evaluate_agent_arena, parse_agent_arena_observations, parse_agent_arena_suite,
};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_SUITE: &str = include_str!("../../../benchmarks/agent/arena-v1.json");

#[derive(Debug, Default)]
struct Options {
    suite: Option<PathBuf>,
    observations: Option<PathBuf>,
    report: Option<PathBuf>,
    require_ready: bool,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Cindx agent arena failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let options = parse_options()?;
    let suite_source = read_optional(options.suite.as_deref(), DEFAULT_SUITE, "arena suite")?;
    let observation_source =
        read_optional(options.observations.as_deref(), "", "arena observations")?;
    let suite = parse_agent_arena_suite(&suite_source)?;
    let observations = parse_agent_arena_observations(&observation_source)?;
    let report = evaluate_agent_arena(&suite, &observations).map_err(|errors| errors.join("\n"))?;

    println!(
        "Cindx agent arena {}-v{}: cases={} observed={}/{} provider_backed={} ready={}",
        report.suite_id,
        report.suite_version,
        report.cases,
        report.observed_runs,
        report.required_observations,
        report.provider_backed_runs,
        report.ready_for_scientific_comparison,
    );
    for mode in &report.modes {
        println!(
            "  {:<12} runs={:<4} success={}",
            format!("{:?}", mode.mode).to_lowercase(),
            mode.runs,
            mode.success_rate
                .map(|value| format!("{:.1}%", value * 100.0))
                .unwrap_or_else(|| "unmeasured".to_string()),
        );
    }
    for failure in &report.readiness_failures {
        println!("  blocked: {failure}");
    }

    if let Some(path) = options.report {
        write_report(&path, &report)?;
        println!("Report: {}", path.display());
    }
    if options.require_ready && !report.ready_for_scientific_comparison {
        return Err("provider-backed paired arena evidence is incomplete".to_string());
    }
    Ok(())
}

fn read_optional(path: Option<&Path>, fallback: &str, label: &str) -> Result<String, String> {
    path.map(fs::read_to_string)
        .transpose()
        .map_err(|error| format!("could not read {label}: {error}"))
        .map(|source| source.unwrap_or_else(|| fallback.to_string()))
}

fn write_report(path: &Path, report: &impl serde::Serialize) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("could not create report directory: {error}"))?;
    }
    let json = serde_json::to_string_pretty(report)
        .map_err(|error| format!("could not serialize arena report: {error}"))?;
    fs::write(path, json).map_err(|error| format!("could not write {}: {error}", path.display()))
}

fn parse_options() -> Result<Options, String> {
    let mut options = Options::default();
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--suite" => options.suite = Some(required_value(&mut arguments, "--suite")?.into()),
            "--observations" => {
                options.observations =
                    Some(required_value(&mut arguments, "--observations")?.into())
            }
            "--report" => options.report = Some(required_value(&mut arguments, "--report")?.into()),
            "--require-ready" => options.require_ready = true,
            "--help" | "-h" => {
                println!(
                    "Usage: arena_lab [--suite PATH] [--observations JSONL] [--report PATH] [--require-ready]"
                );
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument {argument}")),
        }
    }
    Ok(options)
}

fn required_value(
    arguments: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<String, String> {
    arguments
        .next()
        .ok_or_else(|| format!("{flag} requires a value"))
}
