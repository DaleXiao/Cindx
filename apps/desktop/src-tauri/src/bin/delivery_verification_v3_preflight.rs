fn main() {
    if let Err(error) = cindx_desktop::run_delivery_verification_v3_preflight() {
        eprintln!("delivery verification v3 preflight failed: {error}");
        std::process::exit(1);
    }
}
