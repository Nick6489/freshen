use crate::{Error, Result, manifest::check_path};
use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

pub(crate) fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}

pub(crate) fn safe_join(root: &Path, relative: &str) -> Result<PathBuf> {
    check_path(relative)?;
    checked_join(root, relative)
}

pub(crate) fn checked_join(root: &Path, relative: &str) -> Result<PathBuf> {
    let mut result = root.to_path_buf();
    for part in Path::new(relative).components() {
        if !matches!(part, std::path::Component::Normal(_)) {
            return Err(Error::UnsafePath(relative.into()));
        }
        result.push(part);
        match fs::symlink_metadata(&result) {
            Ok(metadata) if is_link(&metadata) => {
                return Err(Error::UnsafePath(result.display().to_string()));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(result)
}

pub(crate) fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::UnsafePath(path.display().to_string()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| Error::Io(error.error))?;
    sync_dir(parent)
}

pub(crate) fn copy_file(source: &Path, destination: &Path) -> Result<()> {
    let parent = destination.parent().unwrap();
    fs::create_dir_all(parent)?;
    let mut input = File::open(source)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    std::io::copy(&mut input, &mut temporary)?;
    temporary
        .as_file()
        .set_permissions(fs::metadata(source)?.permissions())?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(destination)
        .map_err(|error| Error::Io(error.error))?;
    sync_dir(parent)
}

pub(crate) fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if is_link(&metadata) {
        return Err(Error::UnsafePath(source.display().to_string()));
    }
    if metadata.is_file() {
        return copy_file(source, destination);
    }
    if !metadata.is_dir() {
        return Err(Error::UnsafePath(source.display().to_string()));
    }
    fs::create_dir_all(destination)?;
    for item in fs::read_dir(source)? {
        let item = item?;
        copy_tree(&item.path(), &destination.join(item.file_name()))?;
    }
    sync_dir(destination)
}

pub(crate) fn copy_tree_with_retry(source: &Path, destination: &Path) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match copy_tree(source, destination) {
            Err(Error::Io(error))
                if std::time::Instant::now() < deadline
                    && matches!(
                        error.kind(),
                        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::WouldBlock
                    ) =>
            {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            result => return result,
        }
    }
}

pub(crate) fn remove_tree(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if is_link(&metadata) => Err(Error::UnsafePath(path.display().to_string())),
        Ok(metadata) if metadata.is_dir() => {
            fs::remove_dir_all(path)?;
            Ok(())
        }
        Ok(_) => {
            fs::remove_file(path)?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
