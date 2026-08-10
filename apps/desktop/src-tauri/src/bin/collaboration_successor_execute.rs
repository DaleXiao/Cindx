fn main() {
    if let Err(error) = cindx_desktop::run_collaboration_successor_execute() {
        eprintln!("collaboration successor execution failed: {error}");
        std::process::exit(1);
    }
}
