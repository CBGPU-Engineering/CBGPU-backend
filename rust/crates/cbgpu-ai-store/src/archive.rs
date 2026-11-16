//! ZIP extraction is native Rust; path traversal, symlinks, duplicate/clobber and bombs fail closed.
use crate::{Result, available, fail, private_dir, safe_relative};
use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
};
pub fn extract_zip(source: &Path, destination: &Path) -> Result<()> {
    extract_zip_with_floor(source, destination, 10 * 1024 * 1024 * 1024)
}
pub fn extract_zip_with_floor(source: &Path, destination: &Path, min_free: u64) -> Result<()> {
    if std::fs::symlink_metadata(destination).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(fail("ZIP destination must not be a symlink", 400));
    }
    private_dir(destination)?;
    let file = File::open(source)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|_| fail("Invalid ZIP archive", 400))?;
    if archive.len() > 10000 {
        return Err(fail("ZIP contains too many files", 400));
    }
    let mut total = 0u64;
    for i in 0..archive.len() {
        let mut item = archive
            .by_index(i)
            .map_err(|_| fail("Invalid ZIP entry", 400))?;
        let entry_name = item
            .name()
            .map_err(|_| fail("Invalid ZIP entry name", 400))?;
        let relative = safe_relative(&entry_name)?;
        if item.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
            return Err(fail("ZIP symlinks are not supported", 400));
        }
        total = total
            .checked_add(item.size())
            .ok_or_else(|| fail("Expanded ZIP exceeds 2 GB", 413))?;
        if total > 2 * 1024 * 1024 * 1024 {
            return Err(fail("Expanded ZIP exceeds 2 GB", 413));
        }
        if item.is_dir() {
            create_beneath(destination, &relative, true)?;
            continue;
        }
        let mut target = create_beneath(destination, &relative, false)?;
        let mut copied = 0u64;
        let mut bytes = [0u8; 65536];
        loop {
            let n = item
                .read(&mut bytes)
                .map_err(|_| fail("Invalid ZIP content", 400))?;
            if n == 0 {
                break;
            }
            if available(destination)? < min_free {
                return Err(fail("AI storage is running low", 507));
            }
            copied += n as u64;
            if copied > item.size() || copied > 2 * 1024 * 1024 * 1024 {
                return Err(fail("ZIP file is too large", 413));
            }
            target.write_all(&bytes[..n])?;
        }
        target.sync_all()?;
    }
    Ok(())
}
#[cfg(unix)]
fn create_beneath(root: &Path, relative: &Path, directory: bool) -> Result<File> {
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    };
    let mut current = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(root)?;
    let parts = relative
        .components()
        .filter_map(|p| {
            if let std::path::Component::Normal(p) = p {
                Some(p)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    for (i, part) in parts.iter().enumerate() {
        let component =
            std::ffi::CString::new(part.as_bytes()).map_err(|_| fail("Invalid ZIP path", 400))?;
        let last = i + 1 == parts.len();
        let isdir = !last || directory;
        if isdir {
            let rc = unsafe { libc::mkdirat(current.as_raw_fd(), component.as_ptr(), 0o700) };
            if rc != 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err(fail("Cannot create ZIP directory", 400));
            }
        }
        let flags = libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | if isdir {
                libc::O_RDONLY | libc::O_DIRECTORY
            } else {
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL
            };
        let fd = unsafe { libc::openat(current.as_raw_fd(), component.as_ptr(), flags, 0o600) };
        if fd < 0 {
            return Err(fail("Unsafe or duplicate ZIP destination", 400));
        }
        let next = unsafe { File::from_raw_fd(fd) };
        if last {
            return Ok(next);
        }
        current = next;
    }
    Err(fail("Invalid ZIP entry", 400))
}
#[cfg(not(unix))]
fn create_beneath(_: &Path, _: &Path, _: bool) -> Result<File> {
    Err(fail("Secure extraction unavailable on this host", 503))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn archive_extracts_and_blocks_traversal() {
        let t = tempfile::tempdir().unwrap();
        let source = t.path().join("good.zip");
        let mut zip = zip::ZipWriter::new(File::create(&source).unwrap());
        zip.start_file("src/main.py", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"print(1)").unwrap();
        zip.finish().unwrap();
        extract_zip_with_floor(&source, &t.path().join("good"), 0).unwrap();
        assert_eq!(
            std::fs::read_to_string(t.path().join("good/src/main.py")).unwrap(),
            "print(1)"
        );
        let bad = t.path().join("bad.zip");
        let mut zip = zip::ZipWriter::new(File::create(&bad).unwrap());
        zip.start_file("../escape", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"bad").unwrap();
        zip.finish().unwrap();
        assert!(extract_zip_with_floor(&bad, &t.path().join("bad"), 0).is_err());
        assert!(!t.path().join("escape").exists());
    }
    #[cfg(unix)]
    #[test]
    fn archive_cannot_follow_preexisting_directory_symlink() {
        let t = tempfile::tempdir().unwrap();
        let source = t.path().join("good.zip");
        let mut zip = zip::ZipWriter::new(File::create(&source).unwrap());
        zip.start_file("linked/escape", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"bad").unwrap();
        zip.finish().unwrap();
        std::fs::create_dir(t.path().join("out")).unwrap();
        std::fs::create_dir(t.path().join("outside")).unwrap();
        std::os::unix::fs::symlink(t.path().join("outside"), t.path().join("out/linked")).unwrap();
        assert!(extract_zip_with_floor(&source, &t.path().join("out"), 0).is_err());
        assert!(!t.path().join("outside/escape").exists());
    }
}
