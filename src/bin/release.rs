use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signer, SigningKey};
use freshen::{Artifact, PackageFile, PackageKind, ReleaseManifest, signing_message};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const USAGE: &str = "freshen-release keygen PRIVATE_KEY PUBLIC_KEY\n\
freshen-release pack DIRECTORY ZIP URL TARGET files|mac_bundle PRODUCT VERSION CHANNEL MANIFEST\n\
freshen-release merge OUTPUT_MANIFEST INPUT_MANIFEST...\n\
freshen-release sign PRIVATE_KEY MANIFEST SIGNATURE\n\n\
freshen-release recover INSTALLATION_ROOT\n\n\
Keys and signatures use base64. Keep the private key outside your repository.\n\
Pack writes a ZIP and a single-target manifest. Merge combines target manifests\n\
for the same release before signing. Publish freshen-manifest.json and\n\
freshen-manifest.json.sig alongside the ZIP assets for GitHub discovery.";

fn main() -> std::process::ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    match args.first().map(String::as_str) {
        Some("recover") if args.len() == 2 => {
            let status = freshen::recover_installation(Path::new(&args[1]))?;
            println!("{:?}: {}", status.state, status.detail);
        }
        Some("keygen") if args.len() == 3 => {
            if Path::new(&args[1]).exists() || Path::new(&args[2]).exists() || args[1] == args[2] {
                return Err("key output paths must be distinct and must not already exist".into());
            }
            let mut seed = [0; 32];
            getrandom::fill(&mut seed).map_err(|e| e.to_string())?;
            let key = SigningKey::from_bytes(&seed);
            let mut options = File::options();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut private = options.open(&args[1])?;
            private.write_all(STANDARD.encode(seed).as_bytes())?;
            private.sync_all()?;
            let mut public = File::options()
                .write(true)
                .create_new(true)
                .open(&args[2])?;
            public.write_all(STANDARD.encode(key.verifying_key().to_bytes()).as_bytes())?;
            public.sync_all()?;
            println!("Created signing key and public key.");
        }
        Some("pack") if args.len() == 10 => {
            let directory = fs::canonicalize(&args[1])?;
            let mut paths = Vec::new();
            collect_files(&directory, &directory, &mut paths)?;
            paths.sort();
            let kind = match args[5].as_str() {
                "files" => PackageKind::Files,
                "mac_bundle" => PackageKind::MacBundle,
                _ => return Err("unknown package kind".into()),
            };
            let mut files = Vec::new();
            for relative in &paths {
                let path = directory.join(relative);
                let metadata = fs::metadata(&path)?;
                #[cfg(unix)]
                let executable = {
                    use std::os::unix::fs::PermissionsExt;
                    metadata.permissions().mode() & 0o111 != 0
                };
                #[cfg(not(unix))]
                let executable = relative.ends_with(".exe");
                files.push(PackageFile {
                    path: relative.clone(),
                    sha256: hash(&path)?,
                    size: metadata.len(),
                    executable,
                });
            }
            let mut manifest = ReleaseManifest {
                schema: 1,
                product: args[6].clone(),
                channel: args[8].clone(),
                version: args[7].parse()?,
                notes: String::new(),
                artifacts: vec![Artifact {
                    target: args[4].clone(),
                    kind,
                    url: args[3].parse()?,
                    sha256: "0".repeat(64),
                    size: 1,
                    files,
                }],
            };
            manifest.validate()?;
            let zip_file = File::options()
                .write(true)
                .create_new(true)
                .open(&args[2])?;
            let mut zip = zip::ZipWriter::new(zip_file);
            for entry in &manifest.artifacts[0].files {
                let options = zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated)
                    .unix_permissions(if entry.executable { 0o755 } else { 0o644 });
                zip.start_file(&entry.path, options)?;
                std::io::copy(&mut File::open(directory.join(&entry.path))?, &mut zip)?;
            }
            zip.finish()?.sync_all()?;
            manifest.artifacts[0].sha256 = hash(Path::new(&args[2]))?;
            manifest.artifacts[0].size = fs::metadata(&args[2])?.len();
            write_new(&args[9], &serde_json::to_vec_pretty(&manifest)?)?;
            println!("Created package and unsigned manifest.");
        }
        Some("merge") if args.len() >= 3 => {
            let mut output: ReleaseManifest = serde_json::from_slice(&fs::read(&args[2])?)?;
            for path in &args[3..] {
                let other: ReleaseManifest = serde_json::from_slice(&fs::read(path)?)?;
                other.validate()?;
                if output.product != other.product
                    || output.version != other.version
                    || output.channel != other.channel
                    || output.notes != other.notes
                    || output.schema != other.schema
                {
                    return Err("input manifests must describe the same product, version, channel, schema, and notes".into());
                }
                output.artifacts.extend(other.artifacts);
            }
            output.validate()?;
            write_new(&args[1], &serde_json::to_vec_pretty(&output)?)?;
        }
        Some("sign") if args.len() == 4 => {
            let encoded = fs::read_to_string(&args[1])?;
            let seed: [u8; 32] = STANDARD
                .decode(encoded.trim())?
                .try_into()
                .map_err(|_| "private key must contain 32 bytes")?;
            let document = fs::read(&args[2])?;
            if document.len() > 1024 * 1024 {
                return Err("manifest exceeds 1 MiB".into());
            }
            let manifest: ReleaseManifest = serde_json::from_slice(&document)?;
            manifest.validate()?;
            let signature = SigningKey::from_bytes(&seed).sign(&signing_message(&document));
            write_new(&args[3], STANDARD.encode(signature.to_bytes()).as_bytes())?;
            println!("Signed exact manifest bytes; do not edit the manifest after signing.");
        }
        Some("--help" | "-h") | None => println!("{USAGE}"),
        _ => return Err(USAGE.into()),
    }
    Ok(())
}

fn write_new(path: &str, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = File::options().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn hash(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hex::encode(hash.finalize()))
}

fn collect_files(
    root: &Path,
    folder: &Path,
    paths: &mut Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err("reparse points cannot be packaged".into());
            }
        }
        if metadata.file_type().is_symlink() {
            return Err("symlinks cannot be packaged in schema 1".into());
        }
        if metadata.is_dir() {
            collect_files(root, &entry.path(), paths)?;
        } else if metadata.is_file() {
            let relative: PathBuf = entry.path().strip_prefix(root)?.into();
            paths.push(
                relative
                    .to_str()
                    .ok_or("non-Unicode filename")?
                    .replace('\\', "/"),
            );
        } else {
            return Err("special files cannot be packaged".into());
        }
    }
    Ok(())
}
