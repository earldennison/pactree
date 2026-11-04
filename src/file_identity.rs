use std::fs::File;
use std::io;
use std::path::Path;

pub(crate) fn path_id(path: &Path) -> io::Result<(u64, u128)> {
    file_id(&File::open(path)?)
}

#[cfg(unix)]
pub(crate) fn file_id(file: &File) -> io::Result<(u64, u128)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok((metadata.dev(), u128::from(metadata.ino())))
}

#[cfg(windows)]
pub(crate) fn file_id(file: &File) -> io::Result<(u64, u128)> {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;

    #[repr(C)]
    struct FileIdInfo {
        volume: u64,
        identifier: [u8; 16],
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandleEx(
            file: *mut c_void,
            class: i32,
            info: *mut c_void,
            size: u32,
        ) -> i32;
    }

    let mut info = FileIdInfo {
        volume: 0,
        identifier: [0; 16],
    };
    const FILE_ID_INFO_CLASS: i32 = 18;
    let success = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FILE_ID_INFO_CLASS,
            (&mut info as *mut FileIdInfo).cast(),
            std::mem::size_of::<FileIdInfo>() as u32,
        )
    };
    if success == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((info.volume, u128::from_le_bytes(info.identifier)))
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn file_id(_file: &File) -> io::Result<(u64, u128)> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "physical file identity is unavailable",
    ))
}
