#![cfg_attr(windows, windows_subsystem = "windows")]
#[cfg(windows)]
mod windows;

fn main() {
    #[cfg(windows)]
    windows::run();
    #[cfg(not(windows))]
    eprintln!("This native dialog test application requires Windows.");
}
