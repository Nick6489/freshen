//! Authenticated desktop updates without a GUI or ownership of the host event loop.
//!
//! Operations are blocking: call them on a worker thread, not a GUI event thread.
//! No async runtime, window handle, or toolkit is required.
#![doc = include_str!("../README.md")]

mod archive;
mod error;
mod fsutil;
mod helper;
mod install;
mod manifest;
pub mod shared;
#[cfg(test)]
mod tests;
mod transport;
mod updater;

pub use error::{Error, Result};
pub use helper::{Handoff, confirm_startup, run_helper};
pub use install::{
    InstallLayout, InstallPolicy, InstallState, InstallStatus, discard_finished_installation,
    installation_status, recover_installation,
};
pub use manifest::signing_message;
pub use manifest::{
    Artifact, PackageFile, PackageKind, ReleaseManifest, SignedManifest, TrustStore,
};
pub use transport::{Cancellation, Event, HttpTransport, Transport};
pub use updater::{Candidate, PreparedUpdate, ReleaseSource, Updater};
