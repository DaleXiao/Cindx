fn main() {
    if let Err(error) = cindx_desktop::run_delivery_verification_v2_authorize() {
        eprintln!("delivery verification v2 authorization failed: {error}");
        std::process::exit(1);
    }
}
