//! One API over the platform crates. Code outside `findex-platform*` never uses `cfg(target_os)`.
//!
//! Every function here may block on I/O: call it from a worker thread, never the UI thread.

#[cfg(target_os = "linux")]
pub use findex_platform_linux::*;
#[cfg(windows)]
pub use findex_platform_win::*;

use std::path::PathBuf;

/// Quick-access folders for the navigation pane: (label, path, icon key).
pub fn places() -> Vec<(String, PathBuf, &'static str)> {
    let mut v = Vec::new();
    if let Some(h) = dirs::home_dir() {
        v.push(("Home".into(), h, "home"));
    }
    for (name, dir, icon) in [
        ("Desktop", dirs::desktop_dir(), "desktop"),
        ("Downloads", dirs::download_dir(), "downloads"),
        ("Documents", dirs::document_dir(), "documents"),
        ("Pictures", dirs::picture_dir(), "image"),
        ("Music", dirs::audio_dir(), "music"),
        ("Videos", dirs::video_dir(), "video"),
    ] {
        if let Some(d) = dir.filter(|d| d.is_dir()) {
            v.push((name.into(), d, icon));
        }
    }
    v
}
