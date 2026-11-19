//! Build flavour, like an Android product flavour.
//!
//! The **dev** flavour (every debug build, or a release build with `--features dev`) runs next to an
//! installed Findex without touching it: its own config dir (settings, Link identity, paired phones),
//! its own single-instance socket, no `org.freedesktop.FileManager1` name, no default-file-manager
//! changes, a different preferred Link port, and "Dev" in the window title and the name phones see.

pub const DEV: bool = cfg!(any(debug_assertions, feature = "dev"));

/// Folder name under the config and cache dirs, and the single-instance socket name.
pub const APP_DIR: &str = if DEV { "findex-dev" } else { "findex" };

/// Shown in the window title and to paired phones.
pub const DISPLAY_NAME: &str = if DEV { "Findex Dev" } else { "Findex" };
