# Findex TODO

Legend: `[ ]` open, `[x]` done. P0 = v1.0 blocker, P1 = v1.0 target, P2 = later. Design sources are in `docs/`. Implementation is under way: see the checked items below. Scripted UI suites live in `tests/ui`.

## Analysis (done)

- [x] Research Windows 11 Explorer feature set (`docs/01-research.md`)
- [x] Research Rust GUI toolkits, directory walking, mounts, thumbnails, trash, cleanup
- [x] Write architecture, features, platform integration, performance, cleanup/thumbnail and UI docs
- [x] Add tabs and Win+E / Super+E to scope
- [x] Write roadmap, spikes and risks

## Decisions needed from the owner

- [ ] Confirm "copy and paste same panel" means one unified transfer panel
- [x] Choose project licence: Apache-2.0 (`LICENSE`). Still check Slint terms, see S7
- [ ] Confirm minimum Windows and Linux versions
- [ ] Confirm per-user installer as default on Windows

## M0 Foundations

- [ ] S1 toolkit spike (1M-row list, icon grid, tab strip, tear-off, file drop)
- [ ] S4 copy engine measurements
- [ ] S7 Slint licence check against Apache-2.0 (Slint is GPLv3 / royalty-free / commercial), write ADR-001-final
- [x] Cargo workspace and crate skeletons
- [x] `run.sh` to build and run the app (`./run.sh [--release] [path]`)
- [x] CI for Linux and Windows (fmt, clippy, test; `cargo deny` not yet)
- [ ] Benchmark harness and fixture generator (fixture generator done: `development/scripts/make_fixture.py --big N`)
- [ ] `theme` tokens file (light/dark) and contrast check script (tokens done: `crates/findex-ui/ui/theme.slint`; script open)
- [ ] Verify the `~` design numbers against a real Windows 11 machine

## M1 Browse (P0)

- [x] Window shell with custom title bar
- [ ] Tab strip: open, close, switch, reorder, overflow, per-tab state (open/close/switch/per-tab state done; drag reorder open)
- [ ] Command bar with overflow (labels collapse; item overflow open)
- [x] Address bar with breadcrumbs and editable path
- [ ] Navigation pane tree (lazy) (flat places + drives done; expandable tree open)
- [x] Linux directory reader (`getdents64`) and Windows reader (`FindFirstFileExW`)
- [ ] Streaming listing, cancel on navigate, stale-while-revalidate (streaming + cancel done; SWR open)
- [x] Virtualized Details, List, Icon, Tiles views (all 8 modes)
- [ ] Natural sort, group by, column picker (natural sort done)
- [x] Keyboard navigation and type-ahead
- [x] Status bar
- [x] Light/dark theme following OS

## M2 Drives and places (P0)

- [x] Linux: mountinfo parser and change watcher (4 s poll; inotify on mountinfo later)
- [ ] Linux: udisks2 via D-Bus (mount, unmount, eject, hotplug) (mount/eject via `udisksctl` done; D-Bus open)
- [x] Linux: GVfs locations (smb, sftp, mtp) as optional (listed from the gvfs FUSE dir)
- [ ] Linux: filter pseudo filesystems, "show all" toggle (filter done)
- [x] Drives: readable names (label, else "Local Disk (size)"), rename (persisted), hide and restore from Options > View, separate Network group in the nav pane
- [x] Timeout-guarded free-space probes
- [ ] Windows: logical drives, network drives, known folders, Recycle Bin (drives + known folders done, untested on hardware)
- [x] This PC page with usage bars and groups
- [ ] Trash provider and Trash view with restore

## M3 Operations (P0)

- [ ] Clipboard interop (Windows `CF_HDROP`, Linux uri-list plus gnome/kde cut formats)
- [ ] Drag and drop inside app and with other apps
- [x] Mouse side buttons (back / forward) and Esc closes the Options window
- [x] Unified transfer panel with graph, pause, resume, cancel (separate window)
- [ ] Copy, move, delete engine with per-device queue (engine done; one global queue for now)
- [ ] Reflink / `copy_file_range` / `CopyFile2` paths (Linux done; Windows buffered)
- [ ] Conflict dialog with compare and apply-to-all (keep-both default only)
- [ ] Undo and redo journal (single-step undo for rename/copy/move/trash done)
- [ ] Rename, batch rename, new folder/file (rename + new done)
- [ ] Properties dialog

## M4 Thumbnails and previews (P0 images, P1 rest)

- [x] Thumbnail scheduler with visible-first priority and cancel
- [x] Freedesktop cache read/write
- [ ] Windows shell thumbnails on STA thread
- [ ] Image, SVG decoders with size/time limits (raster images done; SVG open)
- [ ] Video, PDF, audio cover, text, Office embedded thumbnails
- [ ] Details and preview panes (details pane done)
- [ ] Remaining view modes and zoom (all modes done; zoom open)

## M5 Tabs polish and search

- [x] Folder Options window (General / View / Search / Findex)

- [ ] Tear-off tabs and move between windows
- [ ] Reopen closed tab, duplicate tab
- [ ] Drop files onto a tab (hover to switch)
- [x] Session restore (Folder Options > restore tabs)
- [ ] Single-instance IPC (D-Bus on Linux, named pipe on Windows)
- [x] Search in folder (recursive optional) — parallel fuzzy search

## M6 Default file manager (P0)

- [ ] S2 Windows registry keys spike
- [ ] S3 Win+E hook spike
- [ ] S5 Linux Super+E binding spike (GNOME first, matches dev machine; then KDE, XFCE)
- [ ] S6 `FileManager1` ownership spike
- [ ] Linux: `.desktop` file, `mimeapps.list` default, D-Bus service (desktop + default done; FileManager1 D-Bus open)
- [ ] Linux: Super+E (and Explorer media key) binding with undo
- [ ] Windows: launcher stub with Explorer fallback
- [ ] Windows: registry override with backup and restore
- [ ] Windows: Win+E helper (separate switch)
- [x] `findex --repair`
- [x] Settings page showing current handler status (Folder Options > Findex)
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
- [ ] Packaging: deb, rpm, AppImage, Flatpak, MSI/MSIX (AppImage + deb + install script + release workflow done)
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
