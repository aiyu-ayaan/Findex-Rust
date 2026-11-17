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
