//! Whether a path lives on an SSD or a hard disk (the machine profile in the
//! app, D3). Windows only; no admin rights: the volume is opened with access
//! 0, which allows device queries but no reads.

use std::path::{Path, PathBuf};

/// The kind of drive under a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveKind {
    Ssd,
    Hdd,
    Unknown,
}

/// The drive letter of `path` (`C:\x`, `\\?\C:\x`), upper-cased.
pub fn drive_letter(path: &Path) -> Option<char> {
    use std::path::{Component, Prefix};
    match path.components().next()? {
        Component::Prefix(p) => match p.kind() {
            Prefix::Disk(d) | Prefix::VerbatimDisk(d) => Some((d as char).to_ascii_uppercase()),
            _ => None,
        },
        _ => None,
    }
}

/// Seek-penalty answer first (a hard disk incurs one), the TRIM flag when
/// the drive does not answer it (TRIM means flash).
pub fn classify(seek_penalty: Option<bool>, trim: Option<bool>) -> DriveKind {
    match (seek_penalty, trim) {
        (Some(true), _) => DriveKind::Hdd,
        (Some(false), _) | (None, Some(true)) => DriveKind::Ssd,
        (None, _) => DriveKind::Unknown,
    }
}

/// The drive holding `path`. An HDD may need to spin up, so call this off
/// the UI thread.
pub fn drive_kind(path: &Path) -> DriveKind {
    let Some(letter) = drive_letter(&absolute(path)) else { return DriveKind::Unknown };
    sys::drive_kind(letter)
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod sys {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::{
        DEVICE_SEEK_PENALTY_DESCRIPTOR, DEVICE_TRIM_DESCRIPTOR, IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery,
        STORAGE_PROPERTY_ID, STORAGE_PROPERTY_QUERY, StorageDeviceSeekPenaltyProperty, StorageDeviceTrimProperty,
    };

    use super::{DriveKind, classify};

    pub fn drive_kind(letter: char) -> DriveKind {
        let name: Vec<u16> = format!(r"\\.\{letter}:").encode_utf16().chain([0]).collect();
        // SAFETY: `name` is a NUL-terminated UTF-16 string that outlives the
        // call; the other arguments are plain values or null, as CreateFileW allows.
        let h = unsafe {
            CreateFileW(name.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut())
        };
        if h == INVALID_HANDLE_VALUE {
            return DriveKind::Unknown;
        }
        let seek = query::<DEVICE_SEEK_PENALTY_DESCRIPTOR>(h, StorageDeviceSeekPenaltyProperty).map(|d| d.IncursSeekPenalty);
        let trim = query::<DEVICE_TRIM_DESCRIPTOR>(h, StorageDeviceTrimProperty).map(|d| d.TrimEnabled);
        // SAFETY: `h` is the open handle from above, closed once.
        unsafe { CloseHandle(h) };
        classify(seek, trim)
    }

    /// One standard property query of the volume `h`.
    fn query<T: Default + Copy>(h: windows_sys::Win32::Foundation::HANDLE, id: STORAGE_PROPERTY_ID) -> Option<T> {
        let q = STORAGE_PROPERTY_QUERY { PropertyId: id, QueryType: PropertyStandardQuery, AdditionalParameters: [0] };
        let mut out = T::default();
        let mut returned = 0u32;
        // SAFETY: `q` and `out` are live for the call and sized as passed;
        // `returned` is a writable u32; no overlapped IO.
        let ok = unsafe {
            DeviceIoControl(
                h,
                IOCTL_STORAGE_QUERY_PROPERTY,
                (&raw const q).cast(),
                size_of::<STORAGE_PROPERTY_QUERY>() as u32,
                (&raw mut out).cast(),
                size_of::<T>() as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        (ok != 0 && returned as usize >= size_of::<T>()).then_some(out)
    }
}

#[cfg(not(windows))]
mod sys {
    use super::DriveKind;

    pub fn drive_kind(_letter: char) -> DriveKind {
        DriveKind::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_prefers_the_seek_penalty() {
        assert_eq!(classify(Some(true), Some(true)), DriveKind::Hdd);
        assert_eq!(classify(Some(false), None), DriveKind::Ssd);
        assert_eq!(classify(Some(false), Some(false)), DriveKind::Ssd);
        assert_eq!(classify(None, Some(true)), DriveKind::Ssd);
        assert_eq!(classify(None, Some(false)), DriveKind::Unknown);
        assert_eq!(classify(None, None), DriveKind::Unknown);
    }

    #[cfg(windows)]
    #[test]
    fn drive_letters() {
        assert_eq!(drive_letter(Path::new(r"c:\Users\x")), Some('C'));
        assert_eq!(drive_letter(Path::new(r"\\?\D:\a")), Some('D'));
        assert_eq!(drive_letter(Path::new(r"\\server\share\a")), None);
        assert_eq!(drive_letter(Path::new(r"relative\a")), None);
    }
}
