fn main() {
    if let Err(error) = cindx_desktop::run_collaboration_successor_authorize() {
        eprintln!("collaboration successor authorization failed: {error}");
        std::process::exit(1);
    }
}
