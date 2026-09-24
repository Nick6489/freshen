#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    match freshen::run_helper(std::env::args_os().skip(1)) {
        Ok(true) => std::process::ExitCode::SUCCESS,
        Ok(false) => std::process::ExitCode::from(2),
        Err(_) => std::process::ExitCode::FAILURE,
    }
}
