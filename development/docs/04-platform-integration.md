# 04 — Platform Integration

This document covers the parts that decide whether Findex feels like *the* file manager: being the default handler, the Win+E / Super+E shortcut, and drive discovery. Items marked **[?]** must be proven in a spike before anyone builds on them.

## 1. Goal

When the user opts in, every "open this folder" request from the desktop reaches Findex, and the Win+E (Windows) or Super+E (Linux) shortcut opens a Findex window. The change is reversible in one click and never leaves the system unable to open a folder.

## 2. Linux — set as default

Four independent mechanisms. All four are needed, because different programs use different ones.

### 2.1 Desktop entry

Install `findex.desktop` to `~/.local/share/applications/` (user) or `/usr/share/applications/` (package):

```
[Desktop Entry]
Type=Application
Name=Findex
Exec=findex %U
Icon=findex
MimeType=inode/directory;
Categories=System;FileTools;FileManager;
StartupNotify=true
```

### 2.2 MIME default

Equivalent of `xdg-mime default findex.desktop inode/directory`. Findex writes the `[Default Applications]` entry in `~/.config/mimeapps.list` itself and checks the result with `xdg-mime query default inode/directory`. **[V]** No other MIME type is needed for plain folder opening.

### 2.3 D-Bus `org.freedesktop.FileManager1`

Browsers and many apps call `ShowItems`, `ShowFolders` and `ShowItemProperties` on this bus name instead of reading the MIME default. Whoever owns the name wins. **[V]**

- Findex implements the interface (crate `zbus`) in its main process.
- Install `~/.local/share/dbus-1/services/org.freedesktop.FileManager1.service` with `Exec=findex --dbus-activate`, so the name is activatable when Findex is not running. User-level service files override system-level ones.
- The `Exec=` must match the MIME default. A mismatch is the known cause of "wrong file manager opens" bugs. **[V]**
- The settings page shows which service currently owns the name (`busctl --user introspect`) and warns when it is not Findex.

### 2.4 Keyboard shortcut: Super+E

Wayland has no application-level global hotkeys, and on X11 grabbing Super keys fights the desktop shell. So the shortcut is registered **in the desktop environment**, not by a Findex daemon:

| Desktop | Mechanism | Tag |
|---|---|---|
| GNOME | Custom keybinding via `gsettings`: `org.gnome.settings-daemon.plugins.media-keys custom-keybindings`, command `findex --new-window`, binding `<Super>e`. Also bind the `Explorer` / `XF86Explorer` media key (`home` key in the same schema). | [K] |
| KDE Plasma | Add `X-KDE-Shortcuts=Meta+E` to the desktop entry or write to `kglobalshortcutsrc`. | [K], [?] exact keys |
| XFCE | `xfconf-query -c xfce4-keyboard-shortcuts -p /commands/custom/<Super>e`. | [K] |
| Others (Sway, i3, Hyprland) | Print the exact config line for the user to paste. No auto-edit. | [K] |

Rules:

- Findex checks whether `<Super>e` is already bound, and shows the existing binding before replacing it. It stores the previous value so "undo" restores it.
- The current development machine runs GNOME on X11, so the GNOME path is the first to implement and test.
- Single instance (implemented): the first Findex listens on `$XDG_RUNTIME_DIR/findex.sock` (fallback `/tmp/findex-<uid>.sock`). A later `findex PATH` writes the path as one line to it and exits in a few ms; the running window opens a new tab and comes forward. A stale socket from a crashed instance is detected by a failed connect and replaced. Set `FINDEX_NO_IPC=1` to opt out (UI tests do). A D-Bus `FileManager1` service is the separate next step.

### 2.5 Out of scope on Linux for v1.0

Portal file chooser replacement. It is a P2 feature in `03-features.md`.

## 3. Windows — set as default

### 3.1 What is and is not possible

- Replacing Explorer as the folder handler is a **registry** change under the `Directory` and `Drive` shell keys. **[V]**
- Open/Save dialogs and some programs keep using Explorer. This cannot be changed through supported means. **[V]**
- Shell calls like `SHOpenFolderAndSelectItems` (used by "Show in folder" in browsers) open Explorer directly. Intercepting them needs code injection; we will not do that. Documented limitation.
- Windows 11 may protect or reset some default associations. **[?]**

### 3.2 Mechanism A — shell verb override (folder opening)

Per-user (no admin) writes to `HKCU\Software\Classes`:

- `Directory\shell\open\command` → `"C:\...\findex-launcher.exe" "%1"`
- `Drive\shell\open\command` → same
- Value `(Default)` on `Directory\shell` set to `open` so it is the default verb.
- Remove or neutralize the `DelegateExecute` value that routes the verb back to Explorer's COM handler **[?]**. This is the one detail that needs a VM spike (S2). The Files app does this and calls it an experimental feature. **[V]** (that it exists), **[?]** (exact keys)

### 3.3 Mechanism B — Win+E

Evidence: Windows handles Win+E very early. Explorer re-claims it. `RegisterHotKey` cannot be relied on. Directory Opus and OneCommander ship a dedicated Win+E setting. **[V]**

Design: a tiny background helper, `findex-helper.exe`, started at logon through `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.

1. Installs a low-level keyboard hook (`WH_KEYBOARD_LL`). **[?]** S3 proves this beats Explorer for Win+E on Windows 11 24H2 and later.
2. On Win+E key-down, swallows both the `E` and the paired Win key-up, and injects a harmless no-op key press (a key code that no shortcut uses) so the Start menu does not open when Win is released. This is the same technique AutoHotkey uses for `#e`. **[?]**
3. Sends "new window" to the running Findex instance by named pipe, or starts `findex.exe`.
4. Never logs or stores key events. It looks at one chord only. State this in the README and the settings page.
5. Helper uses under 5 MB of memory, no UI, no network.

Fallback if S3 fails: the AutoHotkey-style approach is the same hook, so there is no weaker fallback. If the hook is blocked (some corporate security tools do), the settings page says so and the user keeps Mechanism A only. Taskbar pin and Start menu entry still work.

### 3.4 Safety (hard requirements)

Replacing the folder handler can lock the user out of their own folders if the replacement is missing. **[V]**

1. Registry entries point to `findex-launcher.exe`, a small stub (no GUI libraries). If `findex.exe` is missing or fails to start within 2 s, the stub runs `explorer.exe` with the same arguments. The system never fails to open a folder.
2. The uninstaller removes the registry entries **before** deleting binaries.
3. Settings has "Restore Windows Explorer as default", which deletes only keys Findex created. It records the original values on first enable (stored under `HKCU\Software\Findex\Backup`) and restores those.
4. `findex --repair` does the same from a command line, so recovery works even when the GUI cannot start.
5. A first-run prompt explains what will change, and the change happens only after an explicit "Yes".
6. Mechanism A and B are separate switches. The user can enable either alone.

### 3.5 Single instance and new-tab behaviour

Named pipe `\\.\pipe\findex-<user-sid>`. `findex.exe <path>` forwards to the running instance and exits. Setting chooses new tab or new window. Win+E always opens a new window.

## 4. Drives and locations

### 4.1 Linux sources (merge by device id)

| Source | Provides | Notes |
|---|---|---|
| `/proc/self/mountinfo` | Every mount including CIFS, NFS, FUSE, bind, tmpfs. | Poll the fd for `POLLPRI`/`EPOLLERR` to detect change. `inotify` does not work on procfs. **[K]** |
| `statvfs` | Free and total space. | Run on a worker thread with a 2 s timeout. Hard-mounted dead NFS blocks forever otherwise. **[V]** (sysinfo warns of this) |
| `/sys/block`, `/dev/disk/by-label`, `by-uuid` | Unmounted block devices, labels, sizes, removable flag. | **[K]** |
| udisks2 over D-Bus (`zbus`) | Mount, unmount, eject, power off, unlock LUKS, hotplug signals. | **[K]**, crate choice **[?]** |
| GVfs / `gio` | `smb://`, `sftp://`, `mtp://`, `dav://` locations, mounted at `/run/user/<uid>/gvfs`. | Optional dependency. If absent, hide those entries. **[?]** |
| Avahi (mDNS) | Network neighbours. | P1. |

Display rules:

- Default view hides pseudo filesystems (`proc`, `sysfs`, `devtmpfs`, `cgroup*`, `tmpfs` except `/tmp` and `/dev/shm` when asked, `overlay`, snaps/squashfs loop mounts).
- "Show all" toggle shows everything.
- Groups: Internal, Removable, Network, Other. Each shows label, mount point, filesystem, used/free bar.
- Never call `sysinfo`'s disk list for network mounts. **[V]**

### 4.2 Windows sources

`GetLogicalDrives`, `GetVolumeInformationW`, `GetDiskFreeSpaceExW` on a worker thread with timeout; `WNetEnumResourceW` for mapped and network drives; `SHGetKnownFolderPath` for Desktop, Documents, Downloads, Pictures, Music, Videos; `SHEmptyRecycleBin` and `SHQueryRecycleBin` for the Recycle Bin; `WM_DEVICECHANGE` for hotplug. **[K]**

## 5. Open-with, protocols and file associations

- Linux: read `mimeapps.list`, `.desktop` files and `shared-mime-info` for type detection (crate choice **[?]**: `mime_guess` for extension, `infer` for magic bytes, or `xdg-mime` shell-out).
- Windows: `SHAssocEnumHandlers` for the "Open with" list, `ShellExecuteExW` for open. **[K]**

## 6. Open questions for spikes

| ID | Question |
|---|---|
| S2 | Exact registry keys to override folder opening on current Windows 11, including `DelegateExecute`, and whether Windows resets them after updates. |
| S3 | Does a `WH_KEYBOARD_LL` hook reliably intercept Win+E on Windows 11, and does the Start menu stay closed? |
| S5 | GNOME 4x custom keybinding via `gsettings` from a normal user process on X11 and Wayland. KDE and XFCE equivalents. |
| S6 | `org.freedesktop.FileManager1` ownership and activation order versus Nautilus' own service file. |
