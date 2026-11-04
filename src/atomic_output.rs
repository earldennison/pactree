use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{PactError, PactResult};

#[cfg(windows)]
mod windows;

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
            #[cfg(windows)]
            {
                let mut staged = staged;
                windows::persist_file(staged.path(), destination)?;
                staged.disable_cleanup(true);
            }
            #[cfg(not(windows))]
            staged
                .persist(destination)
                .map_err(|error| PactError::Io(error.error))?;
        }
    }
    sync_parent_directory(&parent)
}

pub(crate) fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        windows::replace_file(source, destination)
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(source, destination)
    }
}

pub(crate) fn truncate_for_append(
    path: &Path,
    file: &mut File,
    len: u64,
    chunk_bytes: usize,
) -> PactResult<u64> {
    if file.metadata()?.len() == len {
        return Ok(0);
    }
    match file.set_len(len) {
        Ok(()) => Ok(0),
        #[cfg(windows)]
        Err(error) if error.raw_os_error() == Some(1224) => {
            use std::io::{Read, Seek, SeekFrom};
            let identity = crate::file_identity::file_id(file)?;
            let mut staged = tempfile::NamedTempFile::new_in(destination_parent(path)?)?;
            file.seek(SeekFrom::Start(0))?;
            let mut remaining = len;
            let budget = crate::pact_file::memory_accounting::Budget::new(chunk_bytes.max(1));
            let _lease = budget.reserve_tracked(chunk_bytes.max(1));
            let mut buffer = vec![0; chunk_bytes.max(1)];
            while remaining > 0 {
                let take = remaining.min(buffer.len() as u64) as usize;
                file.read_exact(&mut buffer[..take])?;
                staged.write_all(&buffer[..take])?;
                remaining -= take as u64;
            }
            staged.as_file().sync_all()?;
            let replacement_id = crate::file_identity::file_id(staged.as_file())?;
            if crate::file_identity::path_id(path)? != identity {
                return Err(PactError::invalid(
                    "Pact file was replaced under tail recovery",
                ));
            }
            publish_staged(staged, path, PublishMode::ReplaceExisting)?;
            let replacement = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)?;
            if crate::file_identity::file_id(&replacement)? != replacement_id {
                return Err(PactError::invalid(
                    "Pact file was replaced after tail recovery",
                ));
            }
            *file = replacement;
            Ok(len)
        }
        Err(error) => {
            let _ = (path, chunk_bytes);
            Err(error.into())
        }
    }
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
