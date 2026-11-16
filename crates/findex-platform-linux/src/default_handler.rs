//! Default file manager on Linux: a `.desktop` file plus the `inode/directory` default in `mimeapps.list`.
//!
//! GNOME binds Super+E to the `home` media key (`<Super>e`), which opens the home folder with the
//! `inode/directory` handler, so setting the default is enough there. [?] KDE and XFCE need their own
//! keybinding work (spike S5).

use std::io;
use std::path::PathBuf;
use std::process::Command;

const DESKTOP_ID: &str = "findex.desktop";

fn backup_file() -> PathBuf {
    dirs::config_dir().unwrap_or_default().join("findex/previous-file-manager")
}

fn query_default() -> Option<String> {
    let out = Command::new("xdg-mime").args(["query", "default", "inode/directory"]).output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn set_mime_default(desktop_id: &str) -> io::Result<()> {
    let st = Command::new("xdg-mime").args(["default", desktop_id, "inode/directory"]).status()?;
    if st.success() { Ok(()) } else { Err(io::Error::other("xdg-mime default failed")) }
}

pub fn is_default_file_manager() -> bool {
    query_default().as_deref() == Some(DESKTOP_ID)
}

/// Installs the `.desktop` entry and makes Findex the folder handler, or reverts to the saved previous handler.
/// Reversible: the previous handler is saved before anything changes.
pub fn set_default_file_manager(enable: bool) -> io::Result<String> {
    let backup = backup_file();
    if enable {
        let exe = std::env::current_exe()?;
        let apps = dirs::data_dir().ok_or_else(|| io::Error::other("no XDG data dir"))?.join("applications");
        std::fs::create_dir_all(&apps)?;
        std::fs::write(
            apps.join(DESKTOP_ID),
            format!(
                "[Desktop Entry]\nType=Application\nName=Findex\nGenericName=File Manager\nComment=Browse files and folders\n\
                 Exec=\"{}\" %U\nIcon=system-file-manager\nTerminal=false\nCategories=System;FileTools;FileManager;\n\
                 MimeType=inode/directory;\nStartupWMClass=findex\n",
                exe.display()
            ),
        )?;
        if let Some(prev) = query_default().filter(|p| p != DESKTOP_ID) {
            std::fs::create_dir_all(backup.parent().unwrap())?;
            std::fs::write(&backup, prev)?;
        }
        set_mime_default(DESKTOP_ID)?;
        Ok("Findex is now the default file manager. Super+E opens it on GNOME.".into())
    } else {
        let prev = std::fs::read_to_string(&backup).unwrap_or_else(|_| "org.gnome.Nautilus.desktop".into());
        set_mime_default(prev.trim())?;
        let _ = std::fs::remove_file(&backup);
        Ok(format!("Restored {} as the default file manager.", prev.trim()))
    }
}
