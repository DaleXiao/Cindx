fn main() {
    if let Err(error) = cindx_desktop::run_agent_realworld_eval() {
        eprintln!("Cindx Agent Real-World evaluation failed: {error}");
        std::process::exit(1);
    }
}
