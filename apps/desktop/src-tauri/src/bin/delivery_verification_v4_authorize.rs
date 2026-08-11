fn main() {
    if let Err(error) = cindx_desktop::run_delivery_verification_v4_authorize() {
        eprintln!("delivery verification v4 authorization failed: {error}");
        std::process::exit(1);
    }
}
