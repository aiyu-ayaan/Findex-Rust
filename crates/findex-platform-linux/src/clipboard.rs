//! File clipboard shared with other managers, in the `x-special/gnome-copied-files` format
//! (`copy` or `cut`, then one `file://` URI per line) that Nautilus, Nemo, Caja and Thunar use.
//! Talks to `wl-copy`/`wl-paste` (Wayland) or `xclip` (X11) so no clipboard crate is needed.
//! shortcut: one target per clipboard owner, so KDE's `application/x-kde-cutselection` is not
//! published; Dolphin reads the copy but not the cut flag. Upgrade with an in-process owner.
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const TARGET: &str = "x-special/gnome-copied-files";

fn wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
}

pub fn path_to_uri(p: &Path) -> String {
    let mut s = String::from("file://");
    for &b in p.as_os_str().as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => s.push(b as char),
            _ => s.push_str(&format!("%{b:02X}")),
        }
    }
    s
}

fn encode(paths: &[PathBuf], cut: bool) -> String {
    let mut s = String::from(if cut { "cut" } else { "copy" });
    for p in paths {
        s.push('\n');
        s.push_str(&path_to_uri(p));
    }
    s
}

/// Parses clipboard text into (URIs, is_cut). `None` when it is not a file list.
fn decode(text: &str) -> Option<(Vec<String>, bool)> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let cut = match lines.next()? {
        "cut" => true,
        "copy" => false,
        _ => return None,
    };
    let uris: Vec<String> = lines.filter(|l| l.starts_with("file://")).map(String::from).collect();
    (!uris.is_empty()).then_some((uris, cut))
}

/// Puts the files on the system clipboard. Blocks briefly; call from a worker.
pub fn set_clipboard_files(paths: &[PathBuf], cut: bool) -> bool {
    let mut cmd = if wayland() {
        let mut c = Command::new("wl-copy");
        c.args(["--type", TARGET]);
        c
    } else {
        let mut c = Command::new("xclip");
        c.args(["-selection", "clipboard", "-t", TARGET, "-i"]);
        c
    };
    let Ok(mut child) = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let ok = child
        .stdin
        .take()
        .is_some_and(|mut i| i.write_all(encode(paths, cut).as_bytes()).is_ok());
    // wl-copy and xclip stay alive in the background as the selection owner.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    ok
}

/// File URIs another app put on the clipboard, with the cut flag. Blocks up to 2 s; call from a worker.
pub fn get_clipboard_files() -> Option<(Vec<String>, bool)> {
    crate::with_timeout(Duration::from_secs(2), || {
        let out = if wayland() {
            Command::new("wl-paste").args(["--no-newline", "--type", TARGET]).output()
        } else {
            Command::new("xclip").args(["-selection", "clipboard", "-o", "-t", TARGET]).output()
        }
        .ok()?;
        out.status.success().then(|| decode(&String::from_utf8_lossy(&out.stdout))).flatten()
    })
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_paths_with_spaces() {
        let t = encode(&[PathBuf::from("/a b/ü.txt")], true);
        assert_eq!(t, "cut\nfile:///a%20b/%C3%BC.txt");
        assert_eq!(decode(&t), Some((vec!["file:///a%20b/%C3%BC.txt".into()], true)));
        assert_eq!(decode("hello"), None);
    }
}
