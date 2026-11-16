# Findex TODO

Legend: `[ ]` open, `[x]` done. P0 = v1.0 blocker, P1 = v1.0 target, P2 = later. Design sources are in `docs/`. No application code exists yet; development has not started.

## Analysis (done)

- [x] Research Windows 11 Explorer feature set (`docs/01-research.md`)
- [x] Research Rust GUI toolkits, directory walking, mounts, thumbnails, trash, cleanup
- [x] Write architecture, features, platform integration, performance, cleanup/thumbnail and UI docs
- [x] Add tabs and Win+E / Super+E to scope
- [x] Write roadmap, spikes and risks

## Decisions needed from the owner

- [ ] Confirm "copy and paste same panel" means one unified transfer panel
- [ ] Choose project licence (affects Slint, see S7)
- [ ] Confirm minimum Windows and Linux versions
- [ ] Confirm per-user installer as default on Windows

## M0 Foundations

- [ ] S1 toolkit spike (1M-row list, icon grid, tab strip, tear-off, file drop)
- [ ] S4 copy engine measurements
- [ ] S7 licence decision, write ADR-001-final
- [ ] Cargo workspace and crate skeletons
- [ ] CI for Linux and Windows (fmt, clippy, test, deny)
- [ ] Benchmark harness and fixture generator
- [ ] `theme` tokens file (light/dark) and contrast check script
- [ ] Verify the `~` design numbers against a real Windows 11 machine

## M1 Browse (P0)

- [ ] Window shell with custom title bar
- [ ] Tab strip: open, close, switch, reorder, overflow, per-tab state
- [ ] Command bar with overflow
- [ ] Address bar with breadcrumbs and editable path
- [ ] Navigation pane tree (lazy)
- [ ] Linux directory reader (`getdents64`) and Windows reader (`FindFirstFileExW`)
- [ ] Streaming listing, cancel on navigate, stale-while-revalidate
- [ ] Virtualized Details, List, Icon, Tiles views
- [ ] Natural sort, group by, column picker
- [ ] Keyboard navigation and type-ahead
- [ ] Status bar
- [ ] Light/dark theme following OS

## M2 Drives and places (P0)

- [ ] Linux: mountinfo parser and change watcher
- [ ] Linux: udisks2 via D-Bus (mount, unmount, eject, hotplug)
- [ ] Linux: GVfs locations (smb, sftp, mtp) as optional
- [ ] Linux: filter pseudo filesystems, "show all" toggle
- [ ] Timeout-guarded free-space probes
- [ ] Windows: logical drives, network drives, known folders, Recycle Bin
- [ ] This PC page with usage bars and groups
- [ ] Trash provider and Trash view with restore

## M3 Operations (P0)

- [ ] Clipboard interop (Windows `CF_HDROP`, Linux uri-list plus gnome/kde cut formats)
- [ ] Drag and drop inside app and with other apps
- [ ] Unified transfer panel with graph, pause, resume, cancel
- [ ] Copy, move, delete engine with per-device queue
- [ ] Reflink / `copy_file_range` / `CopyFile2` paths
- [ ] Conflict dialog with compare and apply-to-all
- [ ] Undo and redo journal
- [ ] Rename, batch rename, new folder/file
- [ ] Properties dialog

## M4 Thumbnails and previews (P0 images, P1 rest)

- [ ] Thumbnail scheduler with visible-first priority and cancel
- [ ] Freedesktop cache read/write
- [ ] Windows shell thumbnails on STA thread
- [ ] Image, SVG decoders with size/time limits
- [ ] Video, PDF, audio cover, text, Office embedded thumbnails
- [ ] Details and preview panes
- [ ] Remaining view modes and zoom

## M5 Tabs polish and search

- [ ] Tear-off tabs and move between windows
- [ ] Reopen closed tab, duplicate tab
- [ ] Drop files onto a tab (hover to switch)
- [ ] Session restore
- [ ] Single-instance IPC (D-Bus on Linux, named pipe on Windows)
- [ ] Search in folder (recursive optional)

## M6 Default file manager (P0)

- [ ] S2 Windows registry keys spike
- [ ] S3 Win+E hook spike
- [ ] S5 Linux Super+E binding spike (GNOME first, matches dev machine; then KDE, XFCE)
- [ ] S6 `FileManager1` ownership spike
- [ ] Linux: `.desktop` file, `mimeapps.list` default, D-Bus service
- [ ] Linux: Super+E (and Explorer media key) binding with undo
- [ ] Windows: launcher stub with Explorer fallback
- [ ] Windows: registry override with backup and restore
- [ ] Windows: Win+E helper (separate switch)
- [ ] `findex --repair`
- [ ] Settings page showing current handler status
- [ ] Installer and uninstaller revert-first order

## M7 Cleanup and deep clean (P1)

- [ ] Category scanner framework with allow-list
- [ ] Windows categories
- [ ] Linux categories (polkit for system ones)
- [ ] Dry run, profiles, CLI
- [ ] Disk analyzer with tree and treemap
- [ ] Large-file, duplicate, empty-folder, broken-symlink tools
- [ ] Elevated helper process
- [ ] Low-space banner

## M8 Hardening and release

- [ ] Accessibility audit on both platforms
- [ ] DPI and scale tests (100% to 300%)
- [ ] Decoder fuzzing and worker-process isolation
- [ ] Crash reporting to local log
- [ ] Packaging: deb, rpm, AppImage, Flatpak, MSI/MSIX
- [ ] Code signing
- [ ] Release checklist in `docs/08-roadmap.md`

## P2 backlog

- [ ] Dual pane / split view
- [ ] Pinned tabs
- [ ] Index search, NTFS MFT / USN fast path
- [ ] Archive browsing and creation
- [ ] Portal file chooser backend (Linux)
- [ ] Gallery view, Mica backdrop (Windows)
- [ ] Command palette, plugins
- [ ] Scheduled cleanup
