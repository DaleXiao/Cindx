fn main() {
    if let Err(error) = cindx_desktop::run_collaboration_successor_preflight() {
        eprintln!("collaboration successor preflight failed: {error}");
        std::process::exit(1);
    }
}
