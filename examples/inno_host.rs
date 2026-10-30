//! Minimal console integration harness, not the external reference GUI application.
use base64::{Engine, engine::general_purpose::STANDARD};
use freshen::{inno::InnoPolicy, *};
use std::{path::PathBuf, time::Duration};
fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    if run_helper(std::env::args_os().skip(1))? {
        return Ok(());
    }
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--version") {
        println!(
            "{}",
            option_env!("FRESHEN_EXAMPLE_VERSION").unwrap_or("1.0.0")
        );
        return Ok(());
    }
    if args.len() != 6 {
        println!(
            "inno_host CURRENT_VERSION MANIFEST_URL PUBLIC_KEY_FILE STATE_DIRECTORY INSTALL_DIRECTORY basic|enhanced"
        );
        return Ok(());
    }
    let key: [u8; 32] = STANDARD
        .decode(std::fs::read_to_string(&args[2])?.trim())?
        .try_into()
        .map_err(|_| "public key must be 32 bytes")?;
    let updater = Updater {
        product: "freshen-inno-example".into(),
        channel: "stable".into(),
        current_version: args[0].parse()?,
        target: "x86_64-pc-windows-msvc".into(),
        trust: TrustStore::new(vec![key])?,
        transport: HttpTransport::new(Duration::from_secs(300))?,
        max_download: 512 * 1024 * 1024,
        max_unpacked: 512 * 1024 * 1024,
    };
    let cancel = Cancellation::default();
    let source = ReleaseSource::Manifest {
        document: args[1].parse()?,
        signature: format!("{}.sig", args[1]).parse()?,
    };
    let mut events = |e| println!("{e:?}");
    let Some(candidate) = updater.check(&source, &cancel, &mut events)? else {
        println!("No update");
        return Ok(());
    };
    let state = PathBuf::from(&args[3]);
    std::fs::create_dir_all(&state)?;
    let prepared = updater.prepare(candidate, &state, &cancel, &mut events)?;
    let policy = InstallPolicy {
        root: state,
        layout: InstallLayout::Inno(InnoPolicy {
            installer: "setup.exe".into(),
            app_id: "FreshenInnoExample".into(),
            directory: PathBuf::from(&args[4]),
            executable: "inno_host.exe".into(),
            machine_wide: false,
            enhanced: match args[5].as_str() {
                "basic" => false,
                "enhanced" => true,
                _ => return Err("choose basic or enhanced".into()),
            },
            timeout_secs: 1800,
            relaunch: false,
        }),
        exit_timeout_secs: 120,
        confirmation_timeout_secs: 120,
    };
    let helper = std::env::current_exe()?.with_file_name("freshen-helper.exe");
    let handoff = prepared.arm(policy, &helper, vec![])?;
    println!(
        "Helper {} ready; host exiting. Inspect inno::status from a surviving UI or next launch.",
        handoff.helper_pid()
    );
    Ok(())
}
