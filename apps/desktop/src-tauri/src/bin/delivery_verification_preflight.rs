fn main() {
    if let Err(error) = cindx_desktop::run_delivery_verification_preflight() {
        eprintln!("delivery verification preflight failed: {error}");
        std::process::exit(1);
    }
}
