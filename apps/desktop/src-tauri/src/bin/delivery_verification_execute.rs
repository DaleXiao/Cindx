fn main() {
    if let Err(error) = cindx_desktop::run_delivery_verification_execute() {
        eprintln!("delivery verification execution failed: {error}");
        std::process::exit(1);
    }
}
