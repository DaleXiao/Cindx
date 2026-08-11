fn main() {
    if let Err(error) = cindx_desktop::run_delivery_verification_v3_execute() {
        eprintln!("delivery verification v3 execution failed: {error}");
        std::process::exit(1);
    }
}
