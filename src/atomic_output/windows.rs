use std::ffi::c_void;
use std::fs::OpenOptions;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;

const FILE_RENAME_INFO_EX: i32 = 22;
const FILE_RENAME_REPLACE_IF_EXISTS: u32 = 0x1;
const FILE_RENAME_POSIX_SEMANTICS: u32 = 0x2;
const DELETE_ACCESS: u32 = 0x0001_0000;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
const FILE_ATTRIBUTE_TEMPORARY: u32 = 0x100;

#[cfg(test)]
mod tests;

#[repr(C)]
struct RenameInfo {
    flags: u32,
    root: *mut c_void,
    name_bytes: u32,
    name: [u16; 1],
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetFileAttributesW(path: *const u16, attributes: u32) -> i32;
    fn SetFileInformationByHandle(
        file: *mut c_void,
        class: i32,
        info: *const c_void,
        size: u32,
    ) -> i32;
}

pub(super) fn persist_file(source: &Path, destination: &Path) -> io::Result<()> {
    let mut name: Vec<u16> = source.as_os_str().encode_wide().collect();
    if name.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "source contains NUL",
        ));
    }
    name.push(0);
    if unsafe { SetFileAttributesW(name.as_ptr(), FILE_ATTRIBUTE_NORMAL) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if let Err(error) = replace_file(source, destination) {
        unsafe {
            SetFileAttributesW(name.as_ptr(), FILE_ATTRIBUTE_TEMPORARY);
        }
        return Err(error);
    }
    Ok(())
}

pub(super) fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    let name: Vec<u16> = std::path::absolute(destination)?
        .as_os_str()
        .encode_wide()
        .collect();
    if name.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "destination contains NUL",
        ));
    }
    let name_bytes = name
        .len()
        .checked_mul(2)
        .and_then(|len| u32::try_from(len).ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "destination is too long"))?;
    let size = std::mem::size_of::<RenameInfo>()
        .checked_add(name_bytes as usize)
        .and_then(|len| u32::try_from(len).ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "rename buffer is too large"))?;
    let mut buffer = vec![0_usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
    let info = buffer.as_mut_ptr().cast::<RenameInfo>();
    unsafe {
        (*info).flags = FILE_RENAME_REPLACE_IF_EXISTS | FILE_RENAME_POSIX_SEMANTICS;
        (*info).root = std::ptr::null_mut();
        (*info).name_bytes = name_bytes;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            std::ptr::addr_of_mut!((*info).name).cast::<u16>(),
            name.len(),
        );
    }
    let file = OpenOptions::new().access_mode(DELETE_ACCESS).open(source)?;
    let success = unsafe {
        SetFileInformationByHandle(file.as_raw_handle(), FILE_RENAME_INFO_EX, info.cast(), size)
    };
    if success == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
