use orchestrator_eval::{
    build_fugu_pilot_run_plan, evaluate_fugu_pilot, fugu_pilot_cells_from_external_effect_report,
    fugu_pilot_synthetic_external_report_json, parse_fugu_pilot_protocol, parse_fugu_pilot_suite,
    project_fugu_pilot_cells, render_fugu_pilot_card, FuguEvaluationObservation,
    FuguPilotCellAggregate,
};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_SUITE: &str = include_str!("../../../benchmarks/fugu/fugu-pilot-v1.json");
const DEFAULT_PROTOCOL: &str = include_str!("../../../benchmarks/fugu/fugu-pilot-protocol-v1.json");

#[derive(Debug, Default)]
struct Options {
    suite: Option<PathBuf>,
    protocol: Option<PathBuf>,
    selftest: bool,
    raw_report: Option<PathBuf>,
    cells: Option<PathBuf>,
    observations: Option<PathBuf>,
    run_plan: Option<PathBuf>,
    report: Option<PathBuf>,
    require_ready: bool,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Cindx Fugu pilot evaluation failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let options = parse_options()?;
    let suite_source = read_optional(options.suite.as_deref(), DEFAULT_SUITE, "Fugu pilot suite")?;
    let protocol_source = read_optional(
        options.protocol.as_deref(),
        DEFAULT_PROTOCOL,
        "Fugu pilot protocol",
    )?;
    let suite = parse_fugu_pilot_suite(&suite_source)?;
    let protocol = parse_fugu_pilot_protocol(&protocol_source)?;
    suite.validate().map_err(|errors| errors.join("; "))?;
    protocol
        .validate(suite_source.as_bytes(), &suite)
        .map_err(|errors| errors.join("; "))?;
    let plan = build_fugu_pilot_run_plan(&suite).map_err(|errors| errors.join("; "))?;
    println!(
        "Cindx Fugu pilot {} v{}: planned={} execution_enabled={} authorized={}",
        suite.id,
        suite.version,
        plan.runs.len(),
        plan.execution_enabled,
        protocol.execution_authorized
    );
    if let Some(run_plan_path) = &options.run_plan {
        write_output(
            run_plan_path,
            &serde_json::to_vec_pretty(&plan).map_err(|error| error.to_string())?,
        )?;
        println!("  run plan: {}", run_plan_path.display());
    }

    let observations = if options.selftest {
        run_selftest(&suite_source, &suite, &protocol, &options)?
    } else if let Some(raw_report_path) = &options.raw_report {
        let report_bytes =
            fs::read(raw_report_path).map_err(|error| format!("read raw report: {error}"))?;
        let cells = fugu_pilot_cells_from_external_effect_report(&suite, &protocol, &report_bytes)
            .map_err(|errors| errors.join("; "))?;
        project_fugu_pilot_cells(&suite, &cells).map_err(|errors| errors.join("; "))?
    } else if let Some(cells_path) = &options.cells {
        let cells_source =
            fs::read_to_string(cells_path).map_err(|error| format!("read cells: {error}"))?;
        let cells: Vec<FuguPilotCellAggregate> = serde_json::from_str(&cells_source)
            .map_err(|error| format!("invalid pilot cells JSON: {error}"))?;
        let projected =
            project_fugu_pilot_cells(&suite, &cells).map_err(|errors| errors.join("; "))?;
        if let Some(observations_path) = &options.observations {
            write_output(
                observations_path,
                &serde_json::to_vec_pretty(&projected).map_err(|error| error.to_string())?,
            )?;
            println!("  observations: {}", observations_path.display());
        }
        projected
    } else if let Some(observations_path) = &options.observations {
        let source = fs::read_to_string(observations_path)
            .map_err(|error| format!("read observations: {error}"))?;
        serde_json::from_str::<Vec<FuguEvaluationObservation>>(&source)
            .map_err(|error| format!("invalid pilot observations JSON: {error}"))?
    } else {
        Vec::new()
    };

    let report = evaluate_fugu_pilot(&suite, &observations).map_err(|errors| errors.join("; "))?;
    println!("{}", render_fugu_pilot_card(&report));
    if let Some(report_path) = &options.report {
        write_output(
            report_path,
            &serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?,
        )?;
        println!("  report: {}", report_path.display());
    }
    if options.require_ready && !report.ready {
        return Err("pilot report is not ready".to_string());
    }
    Ok(())
}

fn run_selftest(
    suite_source: &str,
    suite: &orchestrator_eval::FuguPilotSuite,
    protocol: &orchestrator_eval::FuguPilotProtocol,
    options: &Options,
) -> Result<Vec<FuguEvaluationObservation>, String> {
    let report_bytes = fugu_pilot_synthetic_external_report_json();
    let cells = fugu_pilot_cells_from_external_effect_report(suite, protocol, &report_bytes)
        .map_err(|errors| errors.join("; "))?;
    let observations =
        project_fugu_pilot_cells(suite, &cells).map_err(|errors| errors.join("; "))?;
    let report = evaluate_fugu_pilot(suite, &observations).map_err(|errors| errors.join("; "))?;
    let failures: Vec<String> = [
        (!report.ready, "pilot report is not ready"),
        (report.planned_runs != 3, "planned runs drifted from three"),
        (
            report.observed_runs != 3,
            "observed runs drifted from three",
        ),
        (
            report.claim_guard != "harness_link_verification_only",
            "claim guard drifted",
        ),
        (protocol.execution_authorized, "protocol became authorized"),
    ]
    .into_iter()
    .filter(|(failed, _)| *failed)
    .map(|(_, label)| format!("selftest failed: {label}"))
    .collect();
    if !failures.is_empty() {
        return Err(failures.join("; "));
    }

    let mut drifted: serde_json::Value =
        serde_json::from_slice(&report_bytes).map_err(|error| error.to_string())?;
    drifted["runs"].as_array_mut().expect("runs").pop();
    let drifted_bytes = serde_json::to_vec(&drifted).map_err(|error| error.to_string())?;
    if fugu_pilot_cells_from_external_effect_report(suite, protocol, &drifted_bytes).is_ok() {
        return Err("selftest: missing run was accepted".to_string());
    }
    let mut drifted: serde_json::Value =
        serde_json::from_slice(&report_bytes).map_err(|error| error.to_string())?;
    drifted["sources"][0]["file_sha256"] = serde_json::json!("0".repeat(64));
    let drifted_bytes = serde_json::to_vec(&drifted).map_err(|error| error.to_string())?;
    if fugu_pilot_cells_from_external_effect_report(suite, protocol, &drifted_bytes).is_ok() {
        return Err("selftest: drifted case authority was accepted".to_string());
    }
    let mut drifted: serde_json::Value =
        serde_json::from_slice(&report_bytes).map_err(|error| error.to_string())?;
    drifted["runs"][0]["treatment"] = serde_json::json!("unknown_arm");
    let drifted_bytes = serde_json::to_vec(&drifted).map_err(|error| error.to_string())?;
    if fugu_pilot_cells_from_external_effect_report(suite, protocol, &drifted_bytes).is_ok() {
        return Err("selftest: unknown treatment label was accepted".to_string());
    }

    if let Some(observations_path) = &options.observations {
        write_output(
            observations_path,
            &serde_json::to_vec_pretty(&observations).map_err(|error| error.to_string())?,
        )?;
        println!("  observations: {}", observations_path.display());
    }
    println!("{}", suite.schema);
    println!("{}", protocol.schema);
    println!(
        "pilot selftest ok (suite digest {})",
        sha256_display(suite_source)
    );
    Ok(observations)
}

fn sha256_display(source: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(source.as_bytes()))
}

fn parse_options() -> Result<Options, String> {
    let mut options = Options::default();
    let mut args = env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--suite" => options.suite = Some(next_path(&mut args, "--suite")?),
            "--protocol" => options.protocol = Some(next_path(&mut args, "--protocol")?),
            "--selftest" => options.selftest = true,
            "--raw-report" => options.raw_report = Some(next_path(&mut args, "--raw-report")?),
            "--cells" => options.cells = Some(next_path(&mut args, "--cells")?),
            "--observations" => {
                options.observations = Some(next_path(&mut args, "--observations")?)
            }
            "--run-plan" => options.run_plan = Some(next_path(&mut args, "--run-plan")?),
            "--report" => options.report = Some(next_path(&mut args, "--report")?),
            "--require-ready" => options.require_ready = true,
            other => return Err(format!("unknown option {other}")),
        }
    }
    if options.cells.is_some() && options.observations.is_none() && options.report.is_none() {
        return Err(
            "--cells requires --observations or --report to receive the projection".to_string(),
        );
    }
    if options.selftest && options.raw_report.is_some() {
        return Err("--selftest and --raw-report are exclusive".to_string());
    }
    Ok(options)
}

fn next_path(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<PathBuf, String> {
    args.next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("{flag} requires a path"))
}

fn read_optional(path: Option<&Path>, default: &str, label: &str) -> Result<String, String> {
    match path {
        Some(path) => fs::read_to_string(path).map_err(|error| format!("read {label}: {error}")),
        None => Ok(default.to_string()),
    }
}

fn write_output(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("create output dir: {error}"))?;
    }
    fs::write(path, bytes).map_err(|error| format!("write {}: {error}", path.display()))
}
