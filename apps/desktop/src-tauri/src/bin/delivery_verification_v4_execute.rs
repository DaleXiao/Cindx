fn main() {
    if let Err(error) = cindx_desktop::run_delivery_verification_v4_execute() {
        eprintln!("delivery verification v4 execution failed: {error}");
        std::process::exit(1);
    }
}
