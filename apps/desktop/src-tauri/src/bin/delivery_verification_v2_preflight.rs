fn main() {
    if let Err(error) = cindx_desktop::run_delivery_verification_v2_preflight() {
        eprintln!("delivery verification v2 preflight failed: {error}");
        std::process::exit(1);
    }
}
