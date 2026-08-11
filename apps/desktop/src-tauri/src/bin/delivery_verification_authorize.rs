fn main() {
    if let Err(error) = cindx_desktop::run_delivery_verification_authorize() {
        eprintln!("delivery verification authorization failed: {error}");
        std::process::exit(1);
    }
}
