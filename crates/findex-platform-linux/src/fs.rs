use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use findex_core::{Cancel, Entry, FLAG_SYMLINK};
use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags};

const FIRST_CHUNK: usize = 256;
const CHUNK: usize = 2000;

/// Streams a directory in chunks using `getdents64` (via rustix) plus one `statx`-family call per entry.
/// The first chunk is small so the first rows paint quickly.
pub fn list_dir(path: &Path, cancel: &Cancel, sink: &mut dyn FnMut(Vec<Entry>)) -> io::Result<()> {
    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let mut dir = Dir::read_from(&fd)?;
    let mut chunk = Vec::with_capacity(FIRST_CHUNK);
    let mut limit = FIRST_CHUNK;
    while let Some(item) = dir.read() {
        if cancel.is_cancelled() {
            return Ok(());
        }
        let Ok(item) = item else { continue };
        let name = item.file_name();
        let bytes = name.to_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        let os_name = OsStr::from_bytes(bytes).to_os_string();
        let (is_dir, size, mtime, flags) =
            match rustix::fs::statat(&fd, name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(st) => {
                    let ft = FileType::from_raw_mode(st.st_mode);
                    if ft == FileType::Symlink {
                        // Show the target's kind, but never descend through it elsewhere.
                        let target = rustix::fs::statat(&fd, name, AtFlags::empty()).ok();
                        let is_dir = target.as_ref().is_some_and(|t| {
                            FileType::from_raw_mode(t.st_mode) == FileType::Directory
                        });
                        let size = target.map_or(0, |t| t.st_size as u64);
                        (is_dir, size, st.st_mtime, FLAG_SYMLINK)
                    } else {
                        (ft == FileType::Directory, st.st_size as u64, st.st_mtime, 0)
                    }
                }
                Err(_) => (item.file_type() == FileType::Directory, 0, 0, 0),
            };
        chunk.push(Entry::new(
            os_name,
            is_dir,
            if is_dir { 0 } else { size },
            mtime,
            flags,
        ));
        if chunk.len() >= limit {
            sink(std::mem::replace(&mut chunk, Vec::with_capacity(CHUNK)));
            limit = CHUNK;
        }
    }
    if !chunk.is_empty() {
        sink(chunk);
    }
    Ok(())
}

/// Copies one regular file. Tries a reflink (`FICLONE`), then `copy_file_range`, then a buffered copy.
/// `progress` receives bytes written so far and returns `false` to cancel; it may block to pause.
/// The destination must not exist. A cancelled or failed copy removes the partial file.
pub fn copy_file(src: &Path, dst: &Path, progress: &mut dyn FnMut(u64) -> bool) -> io::Result<()> {
    let input = File::open(src)?;
    let meta = input.metadata()?;
    let output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dst)?;
    let result = copy_data(&input, &output, meta.len(), progress).and_then(|_| {
        output.set_permissions(meta.permissions())?;
        if let Ok(m) = meta.modified() {
            output.set_modified(m)?;
        }
        Ok(())
    });
    if result.is_err() {
        drop(output);
        let _ = std::fs::remove_file(dst);
    }
    result
}

fn copy_data(
    mut input: &File,
    mut output: &File,
    len: u64,
    progress: &mut dyn FnMut(u64) -> bool,
) -> io::Result<()> {
    let cancelled = || io::Error::new(io::ErrorKind::Interrupted, "cancelled");
    if len > 0 && rustix::fs::ioctl_ficlone(output, input).is_ok() {
        return if progress(len) {
            Ok(())
        } else {
            Err(cancelled())
        };
    }
    let mut done = 0u64;
    // copy_file_range keeps data in the kernel (and lets NFS/SMB do server-side copies).
    while done < len {
        let want = (len - done).min(8 << 20) as usize;
        match rustix::fs::copy_file_range(input, None, output, None, want) {
            Ok(0) => break,
            Ok(n) => {
                done += n as u64;
                if !progress(done) {
                    return Err(cancelled());
                }
            }
            Err(_) if done == 0 => break, // unsupported here: fall back below
            Err(e) => return Err(e.into()),
        }
    }
    if done >= len {
        return Ok(());
    }
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = input.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        output.write_all(&buf[..n])?;
        done += n as u64;
        if !progress(done) {
            return Err(cancelled());
        }
    }
}

/// Recreates a symlink instead of copying its target.
pub fn copy_symlink(src: &Path, dst: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(std::fs::read_link(src)?, dst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_id_falls_back_to_existing_parent() {
        let dir = tempfile::tempdir().unwrap();
        assert_ne!(device_id(dir.path()), 0);
        assert_eq!(device_id(&dir.path().join("new/x")), device_id(dir.path()));
    }

    #[test]
    fn lists_and_copies() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("a.txt"), vec![7u8; 3_000_000]).unwrap();
        let mut all = Vec::new();
        list_dir(dir.path(), &Cancel::default(), &mut |c| all.extend(c)).unwrap();
        all.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].size, 3_000_000);
        assert!(all[1].is_dir());

        let dst = dir.path().join("b.txt");
        let mut last = 0;
        copy_file(&dir.path().join("a.txt"), &dst, &mut |n| {
            last = n;
            true
        })
        .unwrap();
        assert_eq!(last, 3_000_000);
        assert_eq!(std::fs::read(&dst).unwrap().len(), 3_000_000);
        // destination exists: refuse, never overwrite
        assert!(copy_file(&dir.path().join("a.txt"), &dst, &mut |_| true).is_err());
    }
}

/// Filesystem id (`st_dev`) of `path`, or of its nearest existing parent. Used to queue jobs per device.
pub fn device_id(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    path.ancestors()
        .find_map(|p| std::fs::metadata(p).ok())
        .map_or(0, |m| m.dev())
}
