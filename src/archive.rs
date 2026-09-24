use crate::{
    Artifact, Cancellation, Error, Event, Result,
    manifest::{check_path, inventory},
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{Read, Write},
    path::Path,
};

pub(crate) fn hash_file(path: &Path) -> Result<String> {
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

pub(crate) fn extract(
    zip: &Path,
    destination: &Path,
    artifact: &Artifact,
    max_unpacked: u64,
    cancel: &Cancellation,
    events: &mut dyn FnMut(Event),
) -> Result<()> {
    let expected = inventory(artifact);
    let mut archive = zip::ZipArchive::new(File::open(zip)?)?;
    if archive.len() > 200_000 {
        return Err(Error::SizeLimit);
    }
    let mut seen = BTreeSet::new();
    let mut total = 0u64;
    for index in 0..archive.len() {
        cancel.check()?;
        let mut entry = archive.by_index(index)?;
        let name = entry.name().to_owned();
        let path = name.trim_end_matches('/');
        check_path(path)?;
        let mode = entry.unix_mode().unwrap_or(0) & 0o170000;
        if mode != 0 && mode != 0o100000 && mode != 0o040000 {
            return Err(Error::Invalid(
                "archives may contain only regular files and directories".into(),
            ));
        }
        if entry.is_dir() {
            if !expected
                .keys()
                .any(|key| key.starts_with(&format!("{path}/")))
            {
                return Err(Error::Invalid(format!("unlisted directory {path}")));
            }
            continue;
        }
        let file = expected
            .get(path)
            .ok_or_else(|| Error::Invalid(format!("unlisted file {path}")))?;
        if !seen.insert(path.to_ascii_lowercase()) || entry.size() != file.size {
            return Err(Error::Invalid(format!(
                "duplicate or incorrect size for {path}"
            )));
        }
        total = total.checked_add(file.size).ok_or(Error::SizeLimit)?;
        if total > max_unpacked {
            return Err(Error::SizeLimit);
        }
        events(Event::Extracting { path: path.into() });
        let target = destination.join(path);
        fs::create_dir_all(target.parent().unwrap())?;
        let mut output = File::options().write(true).create_new(true).open(&target)?;
        let mut hash = Sha256::new();
        let mut written = 0u64;
        let mut buffer = [0; 64 * 1024];
        loop {
            cancel.check()?;
            let n = entry.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            written += n as u64;
            if written > file.size {
                return Err(Error::SizeLimit);
            }
            hash.update(&buffer[..n]);
            output.write_all(&buffer[..n])?;
        }
        if written != file.size || !hex::encode(hash.finalize()).eq_ignore_ascii_case(&file.sha256)
        {
            return Err(Error::HashMismatch(path.into()));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            output.set_permissions(fs::Permissions::from_mode(if file.executable {
                0o755
            } else {
                0o644
            }))?;
        }
        output.sync_all()?;
    }
    if seen.len() != expected.len() {
        return Err(Error::Invalid("package is missing declared files".into()));
    }
    Ok(())
}
