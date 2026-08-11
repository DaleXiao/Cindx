fn main() {
    if let Err(error) = cindx_desktop::run_delivery_verification_v4_preflight() {
        eprintln!("delivery verification v4 preflight failed: {error}");
        std::process::exit(1);
    }
}
