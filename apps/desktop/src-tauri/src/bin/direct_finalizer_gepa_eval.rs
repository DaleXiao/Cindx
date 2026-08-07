fn main() {
    if let Err(error) = cindx_desktop::run_direct_finalizer_gepa_eval() {
        eprintln!("Cindx Direct-finalizer GEPA evaluation failed: {error}");
        std::process::exit(1);
    }
}
