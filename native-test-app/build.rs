use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=resources/dialogs.rc");
    println!("cargo:rerun-if-changed=resources/app.manifest");
    println!("cargo:rerun-if-env-changed=FRESHEN_UI_VERSION");
    println!("cargo:rerun-if-env-changed=FRESHEN_UI_PUBLIC_KEY_FILE");
    let version = env::var("FRESHEN_UI_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let components: Vec<u16> = version
        .split('.')
        .map(|s| s.parse().expect("version components must be u16"))
        .collect();
    assert_eq!(components.len(), 3, "use a three-part version");
    println!("cargo:rustc-env=FRESHEN_UI_VERSION={version}");
    let key = env::var_os("FRESHEN_UI_PUBLIC_KEY_FILE")
        .map(|p| {
            println!("cargo:rerun-if-changed={}", PathBuf::from(&p).display());
            fs::read_to_string(p).expect("read public key")
        })
        .unwrap_or_default();
    println!("cargo:rustc-env=FRESHEN_UI_PUBLIC_KEY={}", key.trim());
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    assert_eq!(
        env::var("CARGO_CFG_TARGET_ENV").as_deref(),
        Ok("msvc"),
        "demo requires MSVC resources"
    );
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let source = fs::read_to_string(root.join("resources/dialogs.rc"))
        .unwrap()
        .replace("@VERSION@", &version)
        .replace("@MAJOR@", &components[0].to_string())
        .replace("@MINOR@", &components[1].to_string())
        .replace("@PATCH@", &components[2].to_string());
    fs::write(out.join("dialogs.rc"), source).unwrap();
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
        "x86" => "x86",
        "aarch64" => "arm64",
        _ => "x64",
    };
    let rc = env::var_os("RC")
        .map(PathBuf::from)
        .or_else(|| {
            let kit = PathBuf::from(
                env::var_os("ProgramFiles(x86)")
                    .unwrap_or_else(|| r"C:\Program Files (x86)".into()),
            )
            .join("Windows Kits/10/bin");
            let mut candidates: Vec<_> = fs::read_dir(kit)
                .ok()?
                .flatten()
                .map(|e| e.path().join(arch).join("rc.exe"))
                .filter(|p| p.is_file())
                .collect();
            candidates.sort();
            candidates.pop()
        })
        .expect("Install Windows SDK resource compiler or set RC to rc.exe");
    let res = out.join("dialogs.res");
    let mut command = Command::new(&rc);
    if let (Some(sdk), Some(version)) = (
        rc.ancestors().nth(4),
        rc.ancestors().nth(2).and_then(|p| p.file_name()),
    ) {
        for directory in ["um", "shared"] {
            command
                .arg("/I")
                .arg(sdk.join("Include").join(version).join(directory));
        }
    }
    let status = command
        .args(["/nologo", "/fo"])
        .arg(&res)
        .arg(out.join("dialogs.rc"))
        .current_dir(root.join("resources"))
        .status()
        .expect("run rc.exe");
    assert!(status.success(), "resource compilation failed");
    println!(
        "cargo:rustc-link-arg-bin=freshen-native-test={}",
        res.display()
    );
}
