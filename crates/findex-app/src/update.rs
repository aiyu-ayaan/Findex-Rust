//! Self-update from GitHub releases on a channel (alpha, beta, stable), the same flow as Domus.
//! The release workflow tags versions `v0.0.2-alpha.1`, `v0.0.2-beta.1` and `v0.0.2`.
//! An AppImage replaces its own file and restarts; a .deb or the Windows zip is saved to Downloads and opened.
//!
//! shortcut: HTTP goes through the system `curl` (Linux desktops and Windows 10 1803+ ship it), which keeps
//! TLS out of the binary; move to an HTTP crate if a supported system turns up without curl.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const RELEASES: &str = "https://api.github.com/repos/aiyu-ayaan/Findex-Rust/releases?per_page=30";
pub const REPO_URL: &str = "https://github.com/aiyu-ayaan/Findex-Rust";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Channel names by stage; settings store the index.
const CHANNELS: [&str; 3] = ["alpha", "beta", "stable"];

/// `1.2.3-beta.4` as [major, minor, patch, stage, number] (stage 0 alpha, 1 beta, 2 stable); None for any other shape.
/// A stable release has no number and sorts above every one of its pre-releases.
fn version_key(text: &str) -> Option<[u64; 5]> {
    let t = text.trim();
    let t = t.strip_prefix('v').unwrap_or(t);
    let (core, pre) = match t.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (t, None),
    };
    let num = |s: &str| -> Option<u64> {
        s.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| s.parse().ok())?
    };
    let mut parts = core.split('.');
    let (a, b, c) = (
        num(parts.next()?)?,
        num(parts.next()?)?,
        num(parts.next()?)?,
    );
    if parts.next().is_some() {
        return None;
    }
    let (stage, n) = match pre {
        None => (2, u64::MAX),
        Some(p) => {
            let (name, n) = p.split_once('.')?;
            (
                CHANNELS[..2].iter().position(|c| *c == name)? as u64,
                num(n)?,
            )
        }
    };
    Some([a, b, c, stage, n])
}

/// The channel a build belongs to, the default: whoever installed an alpha wants alphas.
pub fn channel_of(version: &str) -> i32 {
    version_key(version).map_or(2, |k| k[3] as i32)
}

/// A master build that the release workflow did not version updates by rebuilding.
pub fn is_dev() -> bool {
    VERSION == "0.0.0"
}

/// Test and screenshot runs never reach the network on their own.
pub fn auto_check_allowed() -> bool {
    !is_dev()
        && ["FINDEX_UI_TEST", "FINDEX_SNAPSHOT", "FINDEX_NO_IPC"]
            .iter()
            .all(|v| std::env::var_os(v).is_none())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Offer {
    pub version: String,
    pub url: String,
    pub size: u64,
    /// Release notes as written on GitHub (Markdown, shown as plain text).
    pub notes: String,
}

/// The newest release on `channel` (which also takes every steadier one) that is newer than
/// `installed` and has an asset ending in `suffix`. Releases without one are passed over.
fn pick(
    releases: &serde_json::Value,
    installed: &str,
    channel: i32,
    suffix: &str,
) -> Option<Offer> {
    let current = version_key(installed)?;
    let mut best: Option<([u64; 5], Offer)> = None;
    for r in releases.as_array()? {
        let Some(tag) = r["tag_name"].as_str() else {
            continue;
        };
        let Some(key) = version_key(tag) else {
            continue;
        };
        if (key[3] as i32) < channel || key <= best.as_ref().map_or(current, |b| b.0) {
            continue;
        }
        let asset = r["assets"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|a| a["name"].as_str().is_some_and(|n| n.ends_with(suffix)));
        let Some(url) = asset.and_then(|a| a["browser_download_url"].as_str()) else {
            continue;
        };
        let offer = Offer {
            version: tag.trim_start_matches('v').to_string(),
            url: url.to_string(),
            size: asset.and_then(|a| a["size"].as_u64()).unwrap_or(0),
            notes: r["body"].as_str().unwrap_or("").trim().to_string(),
        };
        best = Some((key, offer));
    }
    best.map(|b| b.1)
}

/// The AppImage this process runs from, which an update replaces in place.
fn appimage() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
}

/// True when installing an update restarts Findex (an AppImage), false when it saves an installer.
pub fn self_install() -> bool {
    appimage().is_some()
}

fn curl() -> Command {
    let mut c = findex_platform::background_command("curl");
    c.args(["-fsSL", "--connect-timeout", "15"]);
    c
}

/// Asks GitHub for the newest release on `channel`. Blocking: call it from a worker.
pub fn check(channel: i32) -> Result<Option<Offer>, String> {
    if is_dev() {
        return Ok(None);
    }
    let out = curl()
        .args([
            "--max-time",
            "30",
            "-H",
            "Accept: application/vnd.github+json",
            RELEASES,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("could not run curl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "GitHub could not be reached ({})",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("GitHub sent an unreadable answer: {e}"))?;
    let suffix = if self_install() {
        ".AppImage"
    } else {
        findex_platform::UPDATE_ASSET
    };
    Ok(pick(&json, VERSION, channel, suffix))
}

pub enum Installed {
    /// The AppImage was replaced; start this file and quit.
    Relaunch(PathBuf),
    /// The installer was saved here; open it.
    Saved(PathBuf),
}

/// Downloads the offer and puts it in place. `progress` gets 0..1 while it runs. Blocking: call it from a worker.
pub fn install(offer: &Offer, progress: &dyn Fn(f32)) -> Result<Installed, String> {
    let appimage = appimage();
    let target = match &appimage {
        Some(p) => p.clone(),
        None => {
            let dir = dirs::download_dir()
                .or_else(dirs::home_dir)
                .ok_or("no Downloads folder")?;
            let name = offer.url.rsplit('/').next().unwrap_or("");
            if name.is_empty() || name == ".." {
                return Err("the download has no file name".into());
            }
            dir.join(name)
        }
    };
    let mut part = target.clone().into_os_string();
    part.push(".part");
    let part = PathBuf::from(part);
    let r = download(offer, &part, progress).and_then(|_| {
        if let Some(old) = &appimage {
            // An error page saved under the right name must not become the app.
            let mut head = [0u8; 4];
            let elf = std::fs::File::open(&part)
                .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut head))
                .is_ok_and(|_| &head == b"\x7fELF");
            if !elf {
                return Err("the download is not an AppImage".into());
            }
            let perms = std::fs::metadata(old)
                .map_err(|e| e.to_string())?
                .permissions();
            std::fs::set_permissions(&part, perms).map_err(|e| e.to_string())?;
        }
        // A rename within one folder: a failure before it leaves the old build in place.
        std::fs::rename(&part, &target).map_err(|e| e.to_string())
    });
    if let Err(e) = r {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    Ok(match appimage {
        Some(_) => Installed::Relaunch(target),
        None => Installed::Saved(target),
    })
}

fn download(offer: &Offer, part: &Path, progress: &dyn Fn(f32)) -> Result<(), String> {
    let mut child = curl()
        .args(["--max-time", "1800", "-o"])
        .arg(part)
        .arg(&offer.url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not run curl: {e}"))?;
    // The asset size comes from GitHub, so the bar needs no Content-Length.
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break s;
        }
        let done = std::fs::metadata(part).map_or(0, |m| m.len());
        progress(done as f32 / offer.size.max(1) as f32);
        std::thread::sleep(Duration::from_millis(150));
    };
    let got = std::fs::metadata(part).map_or(0, |m| m.len());
    if !status.success() {
        return Err("the download failed".into());
    }
    if offer.size != 0 && got != offer.size {
        return Err(format!(
            "the download is incomplete ({got} of {} bytes)",
            offer.size
        ));
    }
    progress(1.0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, names: &[&str]) -> serde_json::Value {
        let assets: Vec<_> = names
            .iter()
            .map(|n| serde_json::json!({ "name": n, "browser_download_url": format!("u/{n}"), "size": 7 }))
            .collect();
        serde_json::json!({ "tag_name": format!("v{tag}"), "assets": assets, "body": "notes" })
    }

    fn picked(
        releases: &serde_json::Value,
        installed: &str,
        channel: i32,
        suffix: &str,
    ) -> Option<(String, String)> {
        pick(releases, installed, channel, suffix).map(|o| (o.version, o.url))
    }

    /// The same cases as Domus's `apps/desktop/src/updates.js --check`.
    #[test]
    fn picks_the_newest_release_on_the_channel() {
        let rs = serde_json::Value::Array(vec![
            release("0.0.2-alpha.1", &["a.AppImage"]),
            release("0.0.1", &["b.AppImage", "b_amd64.deb"]),
            release("0.0.1-beta.2", &["c.AppImage"]),
            release("0.0.3", &["d.apk"]),
        ]);
        let p = |v: &str, u: &str| Some((v.to_string(), u.to_string()));
        assert_eq!(
            picked(&rs, "0.0.1-alpha.1", 0, ".AppImage"),
            p("0.0.2-alpha.1", "u/a.AppImage")
        );
        // Beta takes the stable release too; the alpha is not for it, and 0.0.3 has no desktop build.
        assert_eq!(
            picked(&rs, "0.0.1-beta.2", 1, ".AppImage"),
            p("0.0.1", "u/b.AppImage")
        );
        assert_eq!(
            picked(&rs, "0.0.1-beta.2", 1, "_amd64.deb"),
            p("0.0.1", "u/b_amd64.deb")
        );
        assert_eq!(picked(&rs, "0.0.1", 2, ".AppImage"), None);
        assert_eq!(picked(&rs, "not-a-version", 0, ".AppImage"), None);
        let full = pick(&rs, "0.0.1-alpha.1", 0, ".AppImage").unwrap();
        assert_eq!((full.size, full.notes.as_str()), (7, "notes"));
    }

    #[test]
    fn orders_versions_and_channels() {
        assert_eq!(channel_of("0.0.1-alpha.1"), 0);
        assert_eq!(channel_of("0.0.1-beta.3"), 1);
        assert_eq!(channel_of("1.2.3"), 2);
        assert!(version_key("1.0.0") > version_key("1.0.0-beta.9"));
        assert!(version_key("1.0.0-beta.1") > version_key("1.0.0-alpha.12"));
        assert!(version_key("1.0.0-alpha.10") > version_key("1.0.0-alpha.9"));
        assert_eq!(version_key("1.0"), None);
        assert_eq!(version_key("1.0.0-rc.1"), None);
        assert_eq!(version_key("1.+0.0"), None);
    }
}
