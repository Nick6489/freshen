//! A minimal "application" for update tests: the executable that an update
//! installs and the helper then restarts.
//!
//! ```text
//! confirm_app MODE INSTALLATION_ROOT
//! ```
//!
//! * `success` confirms startup, as a healthy new version would.
//! * `fail` exits without confirming, so the helper rolls the update back.
//! * `timeout` stays alive without confirming, so the update is recorded as failed.

use std::{env, process::ExitCode, thread, time::Duration};

fn main() -> ExitCode {
    let mut arguments = env::args().skip(1);
    let (Some(mode), Some(root)) = (arguments.next(), arguments.next()) else {
        eprintln!("usage: confirm_app MODE INSTALLATION_ROOT");
        return ExitCode::from(64);
    };
    match mode.as_str() {
        "fail" => ExitCode::FAILURE,
        "timeout" => {
            thread::sleep(Duration::from_secs(3));
            ExitCode::SUCCESS
        }
        _ => match freshen::confirm_startup(std::path::Path::new(&root)) {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::from(2),
            Err(error) => {
                eprintln!("confirm_startup failed: {error}");
                ExitCode::from(3)
            }
        },
    }
}
