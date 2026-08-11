fn main() {
    if let Err(error) = cindx_desktop::run_delivery_verification_v3_authorize() {
        eprintln!("delivery verification v3 authorization failed: {error}");
        std::process::exit(1);
    }
}
