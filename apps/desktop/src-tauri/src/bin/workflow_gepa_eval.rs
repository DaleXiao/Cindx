fn main() {
    if let Err(error) = cindx_desktop::run_workflow_gepa_eval() {
        eprintln!("Cindx Workflow GEPA evaluation failed: {error}");
        std::process::exit(1);
    }
}
