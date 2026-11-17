//! Linux platform layer. Only this crate (and `findex-platform-win`) may call OS-specific APIs.
//! Compiles to nothing on other targets.
#![cfg(target_os = "linux")]

mod clipboard;
mod default_handler;
mod drives;
mod fs;
mod ipc;

pub use clipboard::{get_clipboard_files, path_to_uri, set_clipboard_files};
pub use default_handler::{is_default_file_manager, set_default_file_manager};
pub use drives::{drives, eject, mount};
pub use fs::{copy_file, copy_symlink, device_id, list_dir};
pub use ipc::{forward as forward_to_running, listen as listen_for_instances};

/// Release asset a non-AppImage install updates from (see `.github/workflows/release.yml`).
pub const UPDATE_ASSET: &str = "_amd64.deb";

/// A helper process (such as `curl`) started without a console window.
pub fn background_command(program: &str) -> Command {
    let mut c = Command::new(program);
    c.stdin(Stdio::null());
    c
}

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Runs `f` on its own thread and gives up after `timeout`. A hung mount only leaks that thread.
pub fn with_timeout<T: Send + 'static>(
    timeout: Duration,
    f: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("findex-probe".into())
        .spawn(move || {
            let _ = tx.send(f());
        })
        .ok()?;
    rx.recv_timeout(timeout).ok()
}

/// Opens a file with the user's default application.
pub fn open_default(path: &Path) -> std::io::Result<()> {
    let spawn = |cmd: &str, args: &[&std::ffi::OsStr]| {
        Command::new(cmd)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    };
    spawn("gio", &["open".as_ref(), path.as_os_str()])
        .or_else(|_| spawn("xdg-open", &[path.as_os_str()]))
        .map(|_| ())
}

/// Hides or unhides a file. Linux has no hidden attribute, so this adds or removes the leading dot.
/// Returns the new path.
pub fn set_hidden(path: &Path, hidden: bool) -> std::io::Result<PathBuf> {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let name = path.file_name().unwrap_or_default().as_bytes();
    let new: Vec<u8> = if hidden {
        if name.starts_with(b".") {
            return Ok(path.to_path_buf());
        }
        [b".", name].concat()
    } else {
        let t = name.strip_prefix(b".").unwrap_or(name);
        if t.is_empty() || t == b"." {
            return Ok(path.to_path_buf());
        }
        t.to_vec()
    };
    let to = path.with_file_name(std::ffi::OsString::from_vec(new));
    if to == path {
        return Ok(to);
    }
    if std::fs::symlink_metadata(&to).is_ok() {
        return Err(std::io::Error::other(format!(
            "There is already a file named '{}' here.",
            to.file_name().unwrap_or_default().to_string_lossy()
        )));
    }
    std::fs::rename(path, &to).map(|_| to)
}

/// Dotfiles are hidden on Linux.
pub fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|n| std::os::unix::ffi::OsStrExt::as_bytes(n).starts_with(b"."))
}

/// Read-only means the owner cannot write.
pub fn is_readonly(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o200 == 0
}

/// Sets or clears the owner write bit only. `Permissions::set_readonly(false)` would make the file
/// writable by everyone. Follows symlinks: chmod acts on the target, so read the target's mode too.
pub fn set_readonly(path: &Path, readonly: bool) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perm = std::fs::metadata(path)?.permissions();
    let mode = perm.mode();
    let new = if readonly {
        mode & !0o222
    } else {
        mode | 0o200
    };
    if new != mode {
        perm.set_mode(new);
        std::fs::set_permissions(path, perm)?;
    }
    Ok(())
}

/// Bytes allocated on disk.
pub fn allocated_size(meta: &std::fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(meta.blocks() * 512)
}

/// Display name of the app that opens `path` by default ("Text Editor").
pub fn default_app_name(path: &Path) -> Option<String> {
    let path = path.to_path_buf();
    with_timeout(Duration::from_millis(1500), move || {
        let query = |args: &[&std::ffi::OsStr]| {
            Command::new("xdg-mime")
                .arg("query")
                .args(args)
                .stderr(Stdio::null())
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
        };
        let mime = query(&["filetype".as_ref(), path.as_os_str()])?;
        let desktop = query(&["default".as_ref(), mime.as_ref()])?;
        let dirs = std::iter::once(dirs::data_dir()).flatten().chain(
            std::env::var("XDG_DATA_DIRS")
                .unwrap_or_else(|_| "/usr/local/share:/usr/share".into())
                .split(':')
                .map(PathBuf::from)
                .collect::<Vec<_>>(),
        );
        for d in dirs {
            if let Ok(text) = std::fs::read_to_string(d.join("applications").join(&desktop)) {
                return text
                    .lines()
                    .find_map(|l| l.strip_prefix("Name="))
                    .map(str::to_string);
            }
        }
        Some(desktop.trim_end_matches(".desktop").to_string())
    })
    .flatten()
}

/// Native window backdrop (Mica). Not available on Linux; the UI draws its own translucency.
pub fn apply_window_effects(_handle: raw_window_handle::RawWindowHandle, _dark: bool) -> bool {
    false
}

/// Shared freedesktop thumbnail cache, so Findex reuses thumbnails made by Nautilus, Dolphin and others.
pub fn thumbnail_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("thumbnails")
}

/// Location of the user trash folder for the "Recycle Bin" place.
pub fn trash_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("Trash/files"))
}

/// Desktop dark-mode preference (GNOME `color-scheme`, then the GTK theme name).
pub fn system_prefers_dark() -> bool {
    let get = |key: &str| {
        Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", key])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_ascii_lowercase())
            .unwrap_or_default()
    };
    let scheme = get("color-scheme");
    if scheme.contains("dark") {
        return true;
    }
    !scheme.contains("light") && get("gtk-theme").contains("dark")
}

#[cfg(test)]
mod attr_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn readonly_through_symlink_keeps_target_private() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("secret");
        std::fs::write(&file, b"x").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        set_readonly(&link, true).unwrap();
        let mode = || std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(), 0o400);
        set_readonly(&link, false).unwrap();
        assert_eq!(mode(), 0o600);
    }

    #[test]
    fn hide_and_unhide_rename_with_a_dot() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.txt");
        std::fs::write(&f, b"").unwrap();
        let h = set_hidden(&f, true).unwrap();
        assert_eq!(h, dir.path().join(".a.txt"));
        assert!(is_hidden(&h));
        assert_eq!(set_hidden(&h, false).unwrap(), f);
    }
}
