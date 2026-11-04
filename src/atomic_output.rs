use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{PactError, PactResult};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[doc(hidden)]
pub enum PublishMode {
    CreateNew,
    ReplaceExisting,
}

#[doc(hidden)]
pub fn publish_bytes(destination: &Path, bytes: &[u8], mode: PublishMode) -> PactResult<()> {
    let staged = stage_bytes(destination, bytes)?;
    publish_staged(staged, destination, mode)
}

fn stage_bytes(destination: &Path, bytes: &[u8]) -> PactResult<tempfile::NamedTempFile> {
    let parent = destination_parent(destination)?;
    let mut staged = tempfile::NamedTempFile::new_in(&parent)?;
    staged.write_all(bytes)?;
    staged.as_file().sync_all()?;

    if std::fs::read(staged.path())? != bytes {
        return Err(PactError::storage(
            "atomic publication candidate did not match requested bytes",
        ));
    }
    Ok(staged)
}

pub(crate) fn publish_staged(
    staged: tempfile::NamedTempFile,
    destination: &Path,
    mode: PublishMode,
) -> PactResult<()> {
    let parent = destination_parent(destination)?;
    match mode {
        PublishMode::CreateNew => {
            staged
                .persist_noclobber(destination)
                .map_err(|error| PactError::Io(error.error))?;
        }
        PublishMode::ReplaceExisting => {
            staged
                .persist(destination)
                .map_err(|error| PactError::Io(error.error))?;
        }
    }
    sync_parent_directory(&parent)
}

fn destination_parent(destination: &Path) -> PactResult<PathBuf> {
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(PactError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("destination directory does not exist: {}", parent.display()),
        )));
    }
    Ok(parent.to_path_buf())
}

fn sync_parent_directory(parent: &Path) -> PactResult<()> {
    #[cfg(unix)]
    {
        File::open(parent)?.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        if let Err(error) = File::open(parent).and_then(|directory| directory.sync_all())
            && !matches!(
                error.kind(),
                std::io::ErrorKind::InvalidInput
                    | std::io::ErrorKind::PermissionDenied
                    | std::io::ErrorKind::Unsupported
            )
        {
            return Err(PactError::Io(error));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
