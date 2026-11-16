use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use findex_core::{Drive, DriveKind};

const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);

const LOCAL_FS: &[&str] = &[
    "ext2", "ext3", "ext4", "btrfs", "xfs", "vfat", "exfat", "ntfs", "ntfs3", "fuseblk", "f2fs", "zfs", "bcachefs", "jfs",
    "reiserfs", "hfsplus",
];
const OPTICAL_FS: &[&str] = &["iso9660", "udf"];
const NET_FS: &[&str] = &["nfs", "nfs4", "cifs", "smb3", "smbfs", "fuse.sshfs", "fuse.rclone", "9p", "davfs", "fuse.davfs2"];

struct Mount {
    majmin: String,
    mount_point: PathBuf,
    fstype: String,
    source: String,
}

/// Decodes the octal escapes (`\040` for space) used in mountinfo.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            out.push((b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0'));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_mountinfo(text: &str) -> Vec<Mount> {
    text.lines()
        .filter_map(|line| {
            let (left, right) = line.split_once(" - ")?;
            let l: Vec<&str> = left.split(' ').collect();
            let r: Vec<&str> = right.split(' ').collect();
            Some(Mount {
                majmin: l.get(2)?.to_string(),
                mount_point: PathBuf::from(unescape(l.get(4)?)),
                fstype: r.first()?.to_string(),
                source: unescape(r.get(1)?),
            })
        })
        .collect()
}

fn user_visible(mp: &Path) -> bool {
    let s = mp.to_string_lossy();
    s == "/" || s == "/home" || ["/media/", "/run/media/", "/mnt/"].iter().any(|p| s.starts_with(p))
}

fn is_removable(majmin: &str) -> bool {
    let base = PathBuf::from(format!("/sys/dev/block/{majmin}"));
    let real = std::fs::canonicalize(&base).unwrap_or_default();
    if real.to_string_lossy().contains("/usb") {
        return true;
    }
    [base.join("removable"), base.join("../removable")]
        .iter()
        .any(|p| std::fs::read_to_string(p).is_ok_and(|s| s.trim() == "1"))
}

/// udev properties of a block device (`ID_FS_LABEL`, `ID_FS_TYPE`, ...).
fn udev_props(majmin: &str) -> HashMap<String, String> {
    std::fs::read_to_string(format!("/run/udev/data/b{majmin}"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.strip_prefix("E:")?.split_once('=').map(|(k, v)| (k.into(), v.into())))
        .collect()
}

/// (total, free) bytes, or zeros when the probe fails or times out.
fn space(path: PathBuf) -> (u64, u64) {
    crate::with_timeout(PROBE_TIMEOUT, move || {
        let s = rustix::fs::statvfs(&path).ok()?;
        Some((s.f_blocks * s.f_frsize, s.f_bavail * s.f_frsize))
    })
    .flatten()
    .unwrap_or((0, 0))
}

/// Mounted volumes, network shares and unmounted partitions. Every free-space probe is timeout-guarded.
pub fn drives() -> Vec<Drive> {
    let mounts = parse_mountinfo(&std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default());
    let mut out = Vec::new();
    let mut mounted_dev = HashSet::new();
    for m in &mounts {
        mounted_dev.insert(m.majmin.clone());
        let fs = m.fstype.as_str();
        let kind = if NET_FS.contains(&fs) {
            DriveKind::Network
        } else if OPTICAL_FS.contains(&fs) {
            DriveKind::Optical
        } else if LOCAL_FS.contains(&fs) && user_visible(&m.mount_point) {
            if is_removable(&m.majmin) { DriveKind::Removable } else { DriveKind::Fixed }
        } else {
            continue;
        };
        let label = udev_props(&m.majmin).remove("ID_FS_LABEL").filter(|l| !l.is_empty()).unwrap_or_else(|| {
            match m.mount_point.to_str() {
                Some("/") => "File System".into(),
                _ => m.mount_point.file_name().map_or(m.source.clone(), |n| n.to_string_lossy().into_owned()),
            }
        });
        let (total, free) = space(m.mount_point.clone());
        out.push(Drive {
            label: format!("{label} ({})", m.mount_point.display()),
            path: m.mount_point.clone(),
            device: m.source.starts_with("/dev/").then(|| m.source.clone()),
            kind,
            total,
            free,
        });
    }
    // GVfs shares (smb://, sftp://, mtp://) appear as folders under the gvfs FUSE mount.
    let gvfs = PathBuf::from(format!("/run/user/{}/gvfs", rustix::process::getuid().as_raw()));
    if let Ok(rd) = std::fs::read_dir(&gvfs) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().replace("server=", "").replace(",share=", "/");
            out.push(Drive { label: name, path: e.path(), device: None, kind: DriveKind::Network, total: 0, free: 0 });
        }
    }
    out.extend(unmounted(&mounted_dev));
    out
}

/// Partitions with a filesystem that are not mounted (skips swap, EFI, loop, zram).
fn unmounted(mounted: &HashSet<String>) -> Vec<Drive> {
    const EFI: &str = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";
    let Ok(rd) = std::fs::read_dir("/sys/class/block") else { return Vec::new() };
    let mut v = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if ["loop", "ram", "zram", "dm-", "sr"].iter().any(|p| name.starts_with(p)) || !e.path().join("partition").exists() {
            continue;
        }
        let Ok(majmin) = std::fs::read_to_string(e.path().join("dev")) else { continue };
        let majmin = majmin.trim().to_string();
        if mounted.contains(&majmin) {
            continue;
        }
        let p = udev_props(&majmin);
        let usable = p.get("ID_FS_USAGE").is_some_and(|u| u == "filesystem")
            && p.get("ID_PART_ENTRY_TYPE").is_none_or(|t| t != EFI);
        if !usable {
            continue;
        }
        let total = std::fs::read_to_string(e.path().join("size")).ok().and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(0) * 512;
        let label = p.get("ID_FS_LABEL").filter(|l| !l.is_empty()).cloned().unwrap_or_else(|| name.clone());
        v.push(Drive {
            label: format!("{label} (not mounted)"),
            path: PathBuf::new(),
            device: Some(format!("/dev/{name}")),
            kind: DriveKind::Unmounted,
            total,
            free: 0,
        });
    }
    v
}

// shortcut: shells out to `udisksctl` (udisks2 CLI); switch to the udisks2 D-Bus API when adding hotplug signals.
fn udisksctl(args: &[&str]) -> io::Result<String> {
    let out = Command::new("udisksctl").args(args).arg("--no-user-interaction").output()?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(io::Error::other(String::from_utf8_lossy(&out.stderr).trim().to_string()))
    }
}

/// Mounts a block device through udisks2 and returns the mount point.
pub fn mount(device: &str) -> io::Result<PathBuf> {
    let out = udisksctl(&["mount", "-b", device])?;
    // "Mounted /dev/sdb1 at /media/user/LABEL"
    out.trim().rsplit_once(" at ").map(|(_, p)| PathBuf::from(p.trim_end_matches('.'))).ok_or_else(|| io::Error::other(out))
}

/// Unmounts and powers off a removable device.
pub fn eject(device: &str) -> io::Result<()> {
    udisksctl(&["unmount", "-b", device])?;
    let disk = device.trim_end_matches(|c: char| c.is_ascii_digit()).trim_end_matches('p');
    let _ = udisksctl(&["power-off", "-b", disk]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mountinfo_with_escapes() {
        let t = "36 35 98:0 / /media/me/My\\040Disk rw,noatime master:1 - ext4 /dev/sdb1 rw\n";
        let m = parse_mountinfo(t);
        assert_eq!(m[0].mount_point, PathBuf::from("/media/me/My Disk"));
        assert_eq!(m[0].fstype, "ext4");
        assert_eq!(m[0].majmin, "98:0");
    }
}
