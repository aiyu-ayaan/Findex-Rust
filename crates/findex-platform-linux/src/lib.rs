//! Linux platform layer. Only this crate (and `findex-platform-win`) may call OS-specific APIs.

mod default_handler;
mod drives;
mod fs;

pub use default_handler::{is_default_file_manager, set_default_file_manager};
pub use drives::{drives, eject, mount};
pub use fs::{copy_file, copy_symlink, list_dir};

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

/// Native window backdrop (Mica). Not available on Linux; the UI draws its own translucency.
pub fn apply_window_effects(_handle: raw_window_handle::RawWindowHandle, _dark: bool) -> bool {
    false
}

/// Shared freedesktop thumbnail cache, so Findex reuses thumbnails made by Nautilus, Dolphin and others.
pub fn thumbnail_cache_dir() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join("thumbnails")
}

/// Location of the user trash folder for the "Recycle Bin" place.
pub fn trash_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("Trash/files"))
}
