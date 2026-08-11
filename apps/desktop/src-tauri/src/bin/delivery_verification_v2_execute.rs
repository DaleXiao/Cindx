fn main() {
    if let Err(error) = cindx_desktop::run_delivery_verification_v2_execute() {
        eprintln!("delivery verification v2 execution failed: {error}");
        std::process::exit(1);
    }
}
