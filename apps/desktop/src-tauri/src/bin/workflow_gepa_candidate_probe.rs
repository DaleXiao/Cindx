fn main() {
    if let Err(error) = cindx_desktop::run_workflow_gepa_candidate_probe() {
        eprintln!("Cindx Workflow GEPA candidate probe failed: {error}");
        std::process::exit(1);
    }
}
