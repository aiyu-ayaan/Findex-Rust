# 01 — Research Findings

Status: analysis only. No application code exists yet. Every claim below is tagged:

- **[V]** verified in a source linked at the bottom of this file
- **[K]** known from platform documentation or prior experience, not re-checked in this pass
- **[?]** unverified, needs a spike (see `08-roadmap.md`, spikes S1–S6)

The REA reverse-engineering toolchain was not used. It targets shipped binaries, and Windows Explorer cannot be run or inspected on this Linux host. Windows 11 behaviour was therefore taken from public documentation and third-party write-ups.

## 1. What Windows 11 File Explorer actually offers (parity target)

| Area | Behaviour | Tag |
|---|---|---|
| Tabs | Multiple folders per window. Tabs can be dragged out into a new window and moved between windows. | [V] |
| Command bar | Replaced the Windows 10 ribbon. New, Cut, Copy, Paste, Rename, Share, Delete, Sort, View, then a "..." overflow menu. | [V] |
| Navigation pane | Left side. Pinned folders (Quick access), cloud, This PC, drives, network. Toggle under View > Show. | [V] |
| Details pane | Right side. Shows properties and actions for the selected item. | [V] |
| View modes | Extra large / large / medium / small icons, List, Details, Tiles, Content. Gallery view arrived in 23H2. | [V] (Gallery), [K] (rest) |
| Details view columns | Name, Date modified, Type, Size, plus any property column. | [V] |
| Copy dialog | One transfer window per job group. Collapsed view has a progress bar. "More details" shows a throughput graph, speed, time remaining. Pause, resume and stop per job. | [V] |
| Conflict dialog | Replace the file / Skip this file / Let me decide for each file. Compare info for both versions. | [V] (replace/skip/keep), [K] (compare pane) |
| Address bar | Breadcrumbs that turn into an editable path on click. Each separator opens a sibling-folder dropdown. | [K] |
| Search | Box top-right, searches the current folder, indexed. | [K] |
| Status bar | Item count, selection count and size, view toggle buttons at the right. | [V] |

## 2. Rust GUI toolkit landscape

Outside web views, the realistic native options are egui, iced and Slint. GPUI (Zed) and Xilem are newer and less documented. **[V]**

| Toolkit | Model | Fit for a Fluent-style file manager | Risk |
|---|---|---|---|
| Slint | Declarative, retained. `ListView` only instantiates visible elements, so very large models stay cheap. **[V]** Ships a Fluent std-widgets style **[K]**. | Best fit on paper: virtualized list, Fluent look, software and GPU renderers. | Licence (GPLv3, royalty-free with conditions, or commercial). Native file drag-and-drop from/to other apps is **[?]**. |
| egui | Immediate mode. Aimed at tools and debug UIs. **[V]** | Fast to prototype. Redraws everything per frame, weak native look, tricky text rendering and accessibility. | Fluent fidelity costs a lot of custom painting. |
| iced | Retained, Elm-style. **[V]** | Good architecture, fewer ready widgets. Virtualized large lists need custom work **[?]**. | Maturity of advanced widgets. |
| GPUI | GPU-first, from Zed. Documentation is thin; examples live in Zed's source. **[V]** | Very fast list rendering. | Docs, API churn, Windows/Linux polish. |
| Xilem | Early. **[V]** | Not ready to bet on. | Maturity. |

Conclusion: provisional choice is Slint, locked only after spike S1 (1M-row list, drag-and-drop, tab strip). Fallback: GPUI. See ADR-001 in `02-architecture.md`.

## 3. Fast directory enumeration

- `jwalk` walks in parallel on rayon and streams results in order. Its own docs say `walkdir` is simpler if raw speed does not matter. **[V]**
- Raw `getdents64` walkers exist in Rust (`minifind`, `fdf`) and report gains from avoiding per-entry overhead. These figures are self-reported. **[V]**
- `io_uring` does not support the `getdents` syscall, so it does not help directory reads. One ring per thread beat a shared ring for stat-heavy work. Source is from 2023; re-check on current kernels. **[V]**, **[?]** for 2026 kernels.
- A file manager lists one folder at a time. The hot path is "open one directory with up to 100k entries", not a recursive walk. Recursive walks matter for search, folder size and the disk analyzer.

Decision: single-directory listing uses direct `getdents64` (Linux, via `rustix`) and `FindFirstFileExW` with `FIND_FIRST_EX_LARGE_FETCH` and `FindExInfoBasic` (Windows). Recursive jobs use a work-stealing walker. No `io_uring` for listing.

## 4. Linux drives, mounts and network

- `xdg-mime query default inode/directory` reads and `xdg-mime default <app>.desktop inode/directory` writes the default handler into `mimeapps.list`. **[V]**
- Some apps (Firefox, `KIO::highlightInFileManager`) ignore that setting and call the D-Bus interface `org.freedesktop.FileManager1` (`ShowItems`, `ShowFolders`, `ShowItemProperties`). Whoever owns that bus name wins. The `.service` file's `Exec=` must point at the same app. **[V]**
- Parsing `/proc/self/mountinfo` sees CIFS and NFS mounts. The `sysinfo` crate excludes network devices by default and can hang on stale hard-mounted shares. **[V]**
- Crates: `mountinfo` (unstable, 2022), `drives` (reads `/sys`, `/proc`, `/dev`). Neither covers unmounted devices, udisks2 or GVfs. **[V]**
- For unmounted block devices, mount/unmount/eject, and hotplug, udisks2 over D-Bus is the standard route. GVfs covers `smb://`, `sftp://`, `mtp://`. **[K]**, **[?]** for crate choice (`zbus` for udisks2, `gio` for GVfs).

## 5. Thumbnails

- Freedesktop Thumbnail Managing Standard: store under `$XDG_CACHE_HOME/thumbnails` (fallback `~/.cache/thumbnails`). Size buckets `normal`, `large`, `x-large`, `xx-large`, and a `fail` directory. `x-large` and `xx-large` came in spec 0.9.0 (December 2020). **[V]**
- Pixel limits per bucket are 128, 256, 512 and 1024. These come from secondary sources, not the spec text I could read. **[V]** (secondary)
- Rust building blocks: `fast_image_resize` (SIMD: SSE4.1, AVX2, NEON), `zune-jpeg`, `resvg`, `pdfium-render` (needs the Pdfium library at runtime), `ffmpeg` bindings for video frames. **[V]**
- Existing crates `thumbnails` (AGPL-3.0) and `media-decode` bundle these. The AGPL licence rules out `thumbnails` for us. **[V]**
- Windows: `IShellItemImageFactory::GetImage` returns an `HBITMAP` thumbnail or falls back to the icon. Flags include `SIIGBF_THUMBNAILONLY`, `SIIGBF_INCACHEONLY`, `SIIGBF_BIGGERSIZEOK`. Microsoft advises against calling it on a UI thread. Shell calls are not reliably thread-safe; one crate serializes them. **[V]**

## 6. Trash and copy engine

- Freedesktop Trash spec 1.0: home trash at `$XDG_DATA_HOME/Trash` with `info/<name>.trashinfo` and `files/<name>`. Other volumes use `$topdir/.Trash` (and a per-user fallback). `.trashinfo` holds `Path=` and `DeletionDate=` (`YYYYMMDDThh:mm:ss`). **[V]**
- `trash-rs` follows the spec and refuses to trash across volumes when no top-dir trash exists. **[V]**
- `copy_file_range`, `FICLONE` reflink, Windows `CopyFile2`, ReFS block clone. **[K]**, not searched. Validated in spike S4.

## 7. Windows: replacing Explorer as default

- Making a third-party app the folder handler is a **registry** change under the `Directory` and `Drive` shell keys, not an `IExplorerCommand` job. `IExplorerCommand` only adds context-menu entries. **[V]**
- Reverting means deleting the `open` key under `HKEY_CLASSES_ROOT\Drives\shell` and `\Directory\shell`. **If the replacement is uninstalled first, Windows cannot open any folder.** **[V]** Hard requirement for our installer (see `04-platform-integration.md`).
- Open/Save file dialogs and some programs still use Explorer. The Files app documents the same limit. **[V]**
- The Files app ships an "official-looking" toggle that intercepts system calls and reroutes to Files, enabled in Settings > Advanced. Exact registry keys not found. **[V]** (existence), **[?]** (keys)
- **Win+E:** Directory Opus reports that newer Windows handles Win+E at a very early stage, before normal applications see it. Explorer can re-claim the hotkey after an app registers it. Directory Opus and OneCommander ship a dedicated "Win+E hotkey" setting. AutoHotkey scripts (`#e::Run ...`) are the common workaround and must keep running. **[V]**
- Whether `RegisterHotKey` can ever claim Win+E: reported as unreliable. A `WH_KEYBOARD_LL` hook is the route AutoHotkey-style tools use. **[K]**, **[?]**

## 8. Disk cleanup and disk analysis

- Windows Disk Cleanup scans a drive, groups eligible files into categories, estimates recoverable space, user picks categories. Categories include temporary files, Recycle Bin, thumbnails, error reports, Windows Update cleanup, device driver packages, Defender files, Delivery Optimization files. **[V]**
- Windows Update Cleanup makes the current update impossible to uninstall. Recycle Bin deletion is permanent. Both need a warning. **[V]**
- Saved profiles: `cleanmgr /sageset:N` then `/sagerun:N`. Space figures are approximate. Open files cannot be deleted. **[V]**
- Microsoft treats Storage Sense as a superset and the classic tool as legacy. **[V]** (secondary mirror)
- WizTree is fast because it reads the NTFS Master File Table directly instead of walking the tree. Non-NTFS and network drives fall back to normal scanning. **[V]** The Rust tools `dua` and `dust` are parallel walkers. Their internals were not covered. **[?]**

## 9. Sources

- Rust GUI landscape: <https://users.rust-lang.org/t/why-i-am-making-a-declarative-ui-in-pure-rust/141777>, <https://areweguiyet.com/>, <https://blog.logrocket.com/state-of-rust-gui-libraries>
- Slint ListView: <https://docs.slint.dev/latest/docs/slint/reference/std-widgets/views/listview/>
- Windows 11 Explorer: <https://www.thurrott.com/books/windows-11-field-guide/archived-22h2/312540/file-explorer-24h2>, <https://www.windowscentral.com/software-apps/windows-11/whats-new-with-file-explorer-on-windows-11-2023-update>, <https://pureinfotech.com/?p=155391>
- Copy dialog: <https://learn.microsoft.com/fr-fr/archive/blogs/b8/improving-our-file-management-basics-copy-move-rename-and-delete>, <https://www.neowin.net/news/microsoft-is-fixing-windows-11-copypaste-dialogs/>
- Default file manager (Linux): <https://wiki.archlinux.org/title/FAM>, <https://bugs.kde.org/show_bug.cgi?id=397953>, <https://bugzilla.gnome.org/show_bug.cgi?id=636269>
- Default file manager (Windows): <https://files.community/docs/configuring/replace-file-explorer>, <https://www.neowin.net/guides/how-to-permanently-restore-old-file-explorer-in-windows-11/>, <https://learn.microsoft.com/en-us/answers/a/1162613>
- Win+E: <https://resource.dopus.com/t/windows-10-win-e-hotkey-issue/27083>, <https://gpsoft.com.au/help/opus12/Documents/Prefs/launchingopusfromthewinehotkey.htm>, <https://www.winhelponline.com/blog/winkey-e-shortcut-target-change/>
- Directory walking: <https://rust-digger.code-maven.com/src/jwalk-0.8.1/README.md>, <https://tavianator.com/2023/bfs_3.0.html>, <https://docs.rs/crate/minifind/0.8.0>
- Mounts: <https://docs.rs/crate/drives/0.0.1>, <https://lib.rs/crates/mountinfo>, <https://codebrowser.dev/rust/crates/sysinfo/src/common/disk.rs.html>
- Thumbnails: <https://specifications.freedesktop.org/thumbnail-spec/latest/x127.html>, <https://github.com/cykooz/fast_image_resize>, <https://lib.rs/crates/thumbnails>, <https://docs.rs/crate/media-decode/0.3.0>
- Windows thumbnails: <https://learn.microsoft.com/windows/win32/api/shobjidl_core/nf-shobjidl_core-ishellitemimagefactory-getimage>, <https://docs.rs/crate/wintheon/0.1.0/source/src/win/files/icon.rs>
- Trash: <https://specifications.freedesktop.org/trash/1.0>
- Disk cleanup: <https://learn.microsoft.com/windows-server/administration/windows-commands/cleanmgr>, <https://www.techrepublic.com/article/how-to-clean-up-system-files-with-the-windows-10-disk-cleanup-tool/>
- Disk analysis: <https://diskanalyzer.com/about>
