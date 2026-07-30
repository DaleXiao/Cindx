use std::process::ExitCode;

fn main() -> ExitCode {
    match cindx_desktop::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Cindx startup failed: {error}");
            ExitCode::FAILURE
        }
    }
}
