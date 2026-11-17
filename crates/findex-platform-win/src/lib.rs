//! Windows platform layer. Compiles to nothing on other targets.
#![cfg(windows)]

use std::ffi::{OsStr, OsString};
use std::io::{self, Read, Write};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use findex_core::{Cancel, Drive, DriveKind, Entry, FLAG_HIDDEN, FLAG_SYMLINK};
use windows_sys::Win32::Foundation::{HWND, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Graphics::Dwm::{DwmExtendFrameIntoClientArea, DwmSetWindowAttribute};
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::UI::Controls::MARGINS;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

const FIRST_CHUNK: usize = 256;
const CHUNK: usize = 2000;
const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

fn filetime_to_unix(lo: u32, hi: u32) -> i64 {
    let t = ((hi as u64) << 32) | lo as u64;
    if t == 0 {
        0
    } else {
        (t as i64 - 116_444_736_000_000_000) / 10_000_000
    }
}

/// Runs `f` on its own thread and gives up after `timeout`. A hung share only leaks that thread.
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

/// Streams a directory with `FindFirstFileExW(FindExInfoBasic, FIND_FIRST_EX_LARGE_FETCH)`:
/// name, size, time and attributes in one call, no per-entry stat.
pub fn list_dir(path: &Path, cancel: &Cancel, sink: &mut dyn FnMut(Vec<Entry>)) -> io::Result<()> {
    let pattern = wide(path.join("*").as_os_str());
    let mut data: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
    let h = unsafe {
        FindFirstFileExW(
            pattern.as_ptr(),
            FindExInfoBasic,
            &mut data as *mut _ as *mut _,
            FindExSearchNameMatch,
            std::ptr::null(),
            FIND_FIRST_EX_LARGE_FETCH,
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let mut chunk = Vec::with_capacity(FIRST_CHUNK);
    let mut limit = FIRST_CHUNK;
    loop {
        if cancel.is_cancelled() {
            break;
        }
        let len = data
            .cFileName
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(data.cFileName.len());
        let name = OsString::from_wide(&data.cFileName[..len]);
        if name != "." && name != ".." {
            let a = data.dwFileAttributes;
            let is_dir = a & FILE_ATTRIBUTE_DIRECTORY != 0;
            let mut flags = 0;
            if a & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0 {
                flags |= FLAG_HIDDEN;
            }
            if a & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                flags |= FLAG_SYMLINK;
            }
            let size = ((data.nFileSizeHigh as u64) << 32) | data.nFileSizeLow as u64;
            let mtime = filetime_to_unix(
                data.ftLastWriteTime.dwLowDateTime,
                data.ftLastWriteTime.dwHighDateTime,
            );
            chunk.push(Entry::new(
                name,
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
        if unsafe { FindNextFileW(h, &mut data) } == 0 {
            break;
        }
    }
    unsafe { FindClose(h) };
    if !chunk.is_empty() {
        sink(chunk);
    }
    Ok(())
}

// shortcut: buffered copy with progress; switch to CopyFile2 (COPY_FILE_RESTARTABLE + progress routine) after spike S4.
/// Copies one file. `progress` gets bytes written and returns `false` to cancel; it may block to pause.
/// The destination must not exist. A cancelled or failed copy removes the partial file.
pub fn copy_file(src: &Path, dst: &Path, progress: &mut dyn FnMut(u64) -> bool) -> io::Result<()> {
    let mut input = std::fs::File::open(src)?;
    let meta = input.metadata()?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dst)?;
    let result = (|| {
        let mut buf = vec![0u8; 1 << 20];
        let mut done = 0u64;
        loop {
            let n = input.read(&mut buf)?;
            if n == 0 {
                break;
            }
            output.write_all(&buf[..n])?;
            done += n as u64;
            if !progress(done) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
        }
        if let Ok(m) = meta.modified() {
            output.set_modified(m)?;
        }
        Ok(())
    })();
    if result.is_err() {
        drop(output);
        let _ = std::fs::remove_file(dst);
    }
    result
}

pub fn copy_symlink(src: &Path, dst: &Path) -> io::Result<()> {
    let target = std::fs::read_link(src)?;
    if src.is_dir() {
        std::os::windows::fs::symlink_dir(target, dst)
    } else {
        std::os::windows::fs::symlink_file(target, dst)
    }
}

/// Logical drives with type, label and timeout-guarded free space.
pub fn drives() -> Vec<Drive> {
    let mut buf = [0u16; 512];
    let n = unsafe { GetLogicalDriveStringsW(buf.len() as u32, buf.as_mut_ptr()) } as usize;
    buf[..n.min(buf.len())]
        .split(|&c| c == 0)
        .filter(|s| !s.is_empty())
        .map(|root| {
            let root_w: Vec<u16> = root.iter().copied().chain(Some(0)).collect();
            let path = PathBuf::from(OsString::from_wide(root));
            let kind = match unsafe { GetDriveTypeW(root_w.as_ptr()) } {
                2 => DriveKind::Removable, // DRIVE_REMOVABLE
                4 => DriveKind::Network,   // DRIVE_REMOTE
                5 => DriveKind::Optical,   // DRIVE_CDROM
                _ => DriveKind::Fixed,
            };
            let mut label = [0u16; 261];
            let ok = unsafe {
                GetVolumeInformationW(
                    root_w.as_ptr(),
                    label.as_mut_ptr(),
                    label.len() as u32,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    0,
                )
            };
            let len = label.iter().position(|&c| c == 0).unwrap_or(0);
            let mut name = if ok != 0 && len > 0 {
                String::from_utf16_lossy(&label[..len])
            } else {
                String::new()
            };
            if name.is_empty() {
                name = match kind {
                    DriveKind::Removable => "USB Drive",
                    DriveKind::Network => "Network Drive",
                    DriveKind::Optical => "DVD Drive",
                    _ => "Local Disk",
                }
                .into();
            }
            let letter = path.to_string_lossy().trim_end_matches('\\').to_string();
            let (total, free) = with_timeout(PROBE_TIMEOUT, move || {
                let (mut free, mut total) = (0u64, 0u64);
                let ok = unsafe {
                    GetDiskFreeSpaceExW(
                        root_w.as_ptr(),
                        &mut free,
                        &mut total,
                        std::ptr::null_mut(),
                    )
                };
                if ok != 0 { (total, free) } else { (0, 0) }
            })
            .unwrap_or((0, 0));
            Drive {
                label: format!("{name} ({letter})"),
                path,
                device: None,
                kind,
                total,
                free,
            }
        })
        .collect()
}

pub fn mount(_device: &str) -> io::Result<PathBuf> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Windows mounts volumes automatically",
    ))
}

// shortcut: eject needs CM_Request_Device_EjectW; not wired yet.
pub fn eject(_device: &str) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Use Safely Remove Hardware for now",
    ))
}

pub fn open_default(path: &Path) -> io::Result<()> {
    let op = wide(OsStr::new("open"));
    let file = wide(path.as_os_str());
    let r = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            op.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if r as usize > 32 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Windows 11 Mica backdrop plus dark title chrome. Returns false where unsupported (Windows 10).
/// [?] verify on Windows 11 22H2+ that Slint's transparent surface shows the backdrop (spike S1).
pub fn apply_window_effects(handle: raw_window_handle::RawWindowHandle, dark: bool) -> bool {
    let raw_window_handle::RawWindowHandle::Win32(h) = handle else {
        return false;
    };
    let hwnd = h.hwnd.get() as HWND;
    const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
    const DWMWA_SYSTEMBACKDROP_TYPE: u32 = 38;
    const DWMSBT_MAINWINDOW: i32 = 2; // Mica
    unsafe {
        let dark = dark as i32;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const _ as _,
            4,
        );
        let margins = MARGINS {
            cxLeftWidth: -1,
            cxRightWidth: -1,
            cyTopHeight: -1,
            cyBottomHeight: -1,
        };
        DwmExtendFrameIntoClientArea(hwnd, &margins);
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &DWMSBT_MAINWINDOW as *const _ as _,
            4,
        ) == 0
    }
}

pub fn thumbnail_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Findex\\thumbnails")
}

/// The Recycle Bin is a shell folder, not a path; browsing it needs the shell provider (M2).
pub fn trash_dir() -> Option<PathBuf> {
    None
}

/// "Choose your app mode" (HKCU ... Personalize\AppsUseLightTheme == 0 means dark).
pub fn system_prefers_dark() -> bool {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let key = wide(OsStr::new(
        "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize",
    ));
    let name = wide(OsStr::new("AppsUseLightTheme"));
    let mut value = 1u32;
    let mut size = 4u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            &mut value as *mut _ as _,
            &mut size,
        )
    };
    r == 0 && value == 0
}

pub fn is_default_file_manager() -> bool {
    false
}

/// Not implemented until spikes S2 (registry keys) and S3 (Win+E hook) prove the mechanism.
pub fn set_default_file_manager(_enable: bool) -> io::Result<String> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Default file manager on Windows needs spikes S2/S3 first (see docs/04-platform-integration.md)",
    ))
}

/// Single-instance handoff. Named-pipe version still open; until then every launch starts its own process.
pub fn forward_to_running(_request: &str) -> bool {
    false
}

pub fn listen_for_instances(_on_request: impl Fn(String) + Send + 'static) {}
