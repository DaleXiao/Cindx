use orchestrator::{
    build_fugu_evaluation_run_plan, evaluate_fugu_evaluation, parse_fugu_evaluation_observations,
    parse_fugu_evaluation_suite, render_fugu_evaluation_card,
};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_SUITE: &str = include_str!("../../../benchmarks/fugu/fugu-v1.json");

#[derive(Debug, Default)]
struct Options {
    suite: Option<PathBuf>,
    observations: Option<PathBuf>,
    run_plan: Option<PathBuf>,
    report: Option<PathBuf>,
    card: Option<PathBuf>,
    require_ready: bool,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Cindx Fugu v1 evaluation failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let options = parse_options()?;
    let suite_source = read_optional(options.suite.as_deref(), DEFAULT_SUITE, "Fugu suite")?;
    let observation_source =
        read_optional(options.observations.as_deref(), "", "Fugu observations")?;
    let suite = parse_fugu_evaluation_suite(&suite_source)?;
    let observations = parse_fugu_evaluation_observations(&observation_source)?;
    let plan = build_fugu_evaluation_run_plan(&suite).map_err(|errors| errors.join("\n"))?;
    let report =
        evaluate_fugu_evaluation(&suite, &observations).map_err(|errors| errors.join("\n"))?;

    println!(
        "Cindx Fugu evaluation {}-v{}: planned={} observed={} ready={}",
        suite.id,
        suite.version,
        plan.runs.len(),
        observations.len(),
        report.ready_for_scientific_comparison
    );
    println!(
        "  safety: execution_enabled={} default={:?} workspace={:?} network={:?}",
        plan.execution_enabled,
        suite.safety_policy.default_mode,
        suite.safety_policy.source_workspace_access,
        suite.safety_policy.network_default
    );
    for track in &report.tracks {
        println!(
            "  {:<32} observed={:>4}/{:<4} ready={}",
            track.track_id, track.observed_runs, track.required_runs, track.ready
        );
    }

    if let Some(path) = options.run_plan {
        write_json(&path, &plan, "run plan")?;
        println!("Run plan: {}", path.display());
    }
    if let Some(path) = options.report {
        write_json(&path, &report, "evaluation report")?;
        println!("Report: {}", path.display());
    }
    if let Some(path) = options.card {
        write_text(
            &path,
            &render_fugu_evaluation_card(&suite, &report),
            "evaluation card",
        )?;
        println!("Evaluation card: {}", path.display());
    }
    if options.require_ready && !report.ready_for_scientific_comparison {
        return Err("the frozen provider-backed evaluation matrix is incomplete".to_string());
    }
    Ok(())
}

fn read_optional(path: Option<&Path>, fallback: &str, label: &str) -> Result<String, String> {
    path.map(fs::read_to_string)
        .transpose()
        .map_err(|error| format!("could not read {label}: {error}"))
        .map(|source| source.unwrap_or_else(|| fallback.to_string()))
}

fn write_json(path: &Path, value: &impl serde::Serialize, label: &str) -> Result<(), String> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|error| format!("could not serialize {label}: {error}"))?;
    write_text(path, &format!("{json}\n"), label)
}

fn write_text(path: &Path, value: &str, label: &str) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {label} directory: {error}"))?;
    }
    fs::write(path, value).map_err(|error| format!("could not write {}: {error}", path.display()))
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
            "--run-plan" => {
                options.run_plan = Some(required_value(&mut arguments, "--run-plan")?.into())
            }
            "--report" => options.report = Some(required_value(&mut arguments, "--report")?.into()),
            "--card" => options.card = Some(required_value(&mut arguments, "--card")?.into()),
            "--require-ready" => options.require_ready = true,
            "--help" | "-h" => {
                println!(
                    "Usage: fugu_evaluation_lab [--suite PATH] [--observations JSONL] [--run-plan PATH] [--report PATH] [--card PATH] [--require-ready]"
                );
                println!("This command validates evidence and emits plans/reports. It never executes benchmarks.");
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
