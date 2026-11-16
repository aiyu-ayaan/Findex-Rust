# 03 — Features and Scope

Priority: **P0** = v1.0 blocker, **P1** = v1.0 target, **P2** = after v1.0. "Win11" means parity with Windows 11 File Explorer.

## 1. Window shell (P0)

Layout, top to bottom: tab strip, command bar, address/search row, then three columns (navigation pane, content, details pane), status bar. Exact metrics are in `07-ui-design-language.md`.

- Back, forward, up, refresh. Alt+Left, Alt+Right, Alt+Up, F5.
- Address bar with breadcrumbs. Click empty space or Ctrl+L / Alt+D / F4 to edit the path as text. Each `>` separator opens a dropdown of sibling folders. Path autocomplete.
- Search box (Ctrl+E / Ctrl+F). Searches the current folder first, recursion optional. Index-backed search is P2.
- Navigation pane: Home, Gallery (P2), pinned folders, This PC with all drives, Network, Trash. Expand/collapse tree, lazy-loaded, drag-and-drop onto nodes.
- Details pane (Alt+Shift+P) and Preview pane (Alt+P). Preview supports image, text, PDF first page, audio/video poster.
- Status bar: item count, selection count and total size, view toggle buttons.
- Light and dark theme following the OS, with a manual override. Accent colour from the OS where available.

## 2. Tabs (P0) — added to scope on request

Tabs behave like Windows 11 and like a browser.

| Feature | Detail | Priority |
|---|---|---|
| Open, close | `+` button, Ctrl+T, Ctrl+W, middle-click on a tab closes it. Closing the last tab closes the window. | P0 |
| Switch | Ctrl+Tab / Ctrl+Shift+Tab, Ctrl+1..8 jump to tab N, Ctrl+9 jumps to the last tab. | P0 |
| Open folder in new tab | Middle-click or Ctrl+click on a folder, context menu "Open in new tab", Ctrl+Enter. | P0 |
| Per-tab state | Own path, back/forward history, view mode, sort, scroll offset, selection, search text. | P0 |
| Reorder | Drag to reorder inside the strip. | P0 |
| Tear off | Drag a tab out of the strip to make a new window. Drag a tab into another Findex window to move it. | P1 |
| Reopen closed | Ctrl+Shift+T. Keeps the last 20 closed tabs with history. | P1 |
| Duplicate | Context menu "Duplicate tab". | P1 |
| Drop files onto a tab | Hovering 600 ms switches to the tab. Dropping copies or moves into that tab's folder. Modifier keys as in Windows (Ctrl copy, Shift move). | P1 |
| Pin tab | Pinned tabs sit left, show icon only, survive restart. | P2 |
| Overflow | When the strip is full, tabs shrink to a minimum width of 96 px, then the strip scrolls with arrow buttons. | P0 |
| Session restore | Reopen last session's tabs on start (setting, default on). | P1 |
| Titles | Folder name plus icon. Drive roots show the drive label. Tooltip shows the full path. | P0 |
| Single instance | Opening a folder from another program (default file manager) adds a tab to the existing window. Setting: "new tab" or "new window". Default: new window for Win+E / Super+E, new tab for external "open folder" requests. | P1 |
| Background cost | Inactive tabs stop file watching for their folder after 30 s, drop thumbnails, and keep only the entry list. Watching resumes and a refresh runs when the tab is activated. | P0 |
| Split view (dual pane) | Two folders side by side inside one tab. | P2 |

## 3. View modes (P0)

All modes share one virtualized item model, so switching is instant.

- Extra large icons (256), Large icons (96), Medium icons (48), Small icons (16), List, Details, Tiles, Content. Gallery (P2).
- Details columns: Name, Date modified, Type, Size, Date created, Date accessed, Attributes, Owner, Permissions (Linux), Dimensions, Length, Tags. Column picker, resize, reorder, auto-fit on double-click divider.
- Group by (name, type, size bucket, date bucket) and sort by any column, ascending or descending. Folders first is a setting.
- Natural sort order (`file2` before `file10`) like Windows. Case-insensitive default.
- Per-folder view memory (like Explorer's folder templates), stored in a small local database, bounded in size.
- Compact mode (reduced row height), as in Win11 View > Compact view.
- Show hidden items, show file extensions, item check boxes.
- Zoom with Ctrl+mouse wheel switches size continuously, snapping to the modes above.

## 4. Selection, clipboard and the unified transfer panel (P0)

Request: "Copy and paste same panel". Interpretation used here: copy, cut, paste, move and delete from every tab and window all report into **one** transfer panel, as in Windows, and not into separate dialogs. If this reading is wrong, change this section only.

- Standard selection: click, Ctrl+click, Shift+click, rubber-band, Ctrl+A, invert selection, type-ahead find.
- Clipboard interop so copy in Findex pastes in other file managers and the reverse:
  - Windows: `CF_HDROP` plus the `Preferred DropEffect` format (copy vs cut). **[K]**
  - Linux: `text/uri-list` plus `x-special/gnome-copied-files` (GNOME/Nautilus convention) and `application/x-kde-cutselection` (KDE). **[K]**
- Drag and drop between tabs, windows and other apps. Default action rules follow Windows: same volume moves, different volume copies, modifiers override, drop target shows a hint ("Move to Documents").
- **Transfer panel** (single window or docked pane, one per application):
  - One row per job with source, destination, progress bar, speed, time remaining, items done.
  - Expand a row for a live throughput graph, as in Windows "More details".
  - Pause, resume, cancel per job. Pause turns the graph yellow, error turns it red.
  - Queue with a concurrency limit per physical device (default 1 write job per device, so HDDs do not thrash). Jobs on different devices run in parallel.
  - Conflict handling: Replace / Skip / Keep both, with a side-by-side compare (name, size, modified date, thumbnail), and "Apply to all conflicts". Extra options Windows lacks: keep newer, keep larger, compare by hash.
  - Error handling per item: Retry / Skip / Skip all / Cancel, with the exact OS error text.
  - Undo and redo of the last operation (Ctrl+Z / Ctrl+Y) for rename, move, copy, new folder, delete-to-trash.
  - Verification option: compare hash after copy (off by default).
  - Survives tab and window close: jobs belong to the application, not to a tab.
- Delete: to trash by default, Shift+Delete permanently with a confirmation. Trash view with restore and empty.
- Rename: F2, batch rename for multiple selection (Win11 renames sequentially with "name (n)"), optional pattern rename (P2).
- New: folder, file from templates, shortcut/symlink.
- Properties dialog: general, security/permissions, details, with checksum tab (P1).
- Open with: default app, "Open with..." list from the OS database. Linux uses `.desktop` entries and `mimeapps.list`. Windows uses the shell.

## 5. Drives, locations and network (P0 Linux, P0 Windows)

Details in `04-platform-integration.md` section 4.

- **Linux "This PC":** every mounted filesystem, every unmounted block device, every network share, GVfs locations (smb, sftp, mtp), removable media, loop devices and bind mounts behind a "show all" toggle. Free/total space bar per drive, filesystem type, mount point, label, mount/unmount/eject buttons. Hotplug updates without refresh.
- **Windows "This PC":** logical drives, mapped network drives, UNC paths, known folders, removable media with eject.
- Network: add network location by URL (`smb://`, `sftp://`, `ftp://`, `nfs://`, UNC on Windows). Browse neighbours through mDNS/Avahi on Linux and the network neighbourhood on Windows (P1).
- Stale or hung mounts must never freeze the UI: every drive probe runs with a timeout on a worker thread.

## 6. Thumbnails and previews (P0 images, P1 rest)

Details in `06-cleanup-and-thumbnails.md`. Images, SVG, video poster frames, PDF, text, office documents (via embedded thumbnails), audio cover art, folder thumbnails showing contents (P2).

## 7. Disk cleanup and deep clean (P1)

Details in `06-cleanup-and-thumbnails.md`.

- **Cleanup:** per-drive scan of safe categories, size estimate, select, preview list, confirm, delete (trash or permanent per category).
- **Deep clean:** disk analyzer (treemap plus sorted tree), large-file finder, duplicate finder (size, then partial hash, then full hash), old-file finder, empty-folder finder, broken-symlink finder.
- Saved cleanup profiles, as in `cleanmgr /sageset`, usable from the command line (`findex clean --profile N`).

## 8. Integration (P0)

- Set as default file manager on Linux and Windows. Details, including **Win+E** on Windows and **Super+E** on Linux, in `04-platform-integration.md`.
- Command line: `findex [path...]`, `findex --select file`, `findex --new-window`, `findex --new-tab`.
- Context menu: native shell extensions on Windows (loaded through the shell, so "Open with", "Send to", 7-Zip and others show up); `.desktop` actions and Nautilus-style scripts on Linux (P1).
- Open terminal here, copy path (Ctrl+Shift+C), copy as path, open in new window.
- Archives: browse and extract zip, tar, 7z as virtual folders (P1). Create archive (P2).

## 9. Beyond Windows (P2 unless noted)

- Dual-pane mode, bookmarks bar, tags, quick filter-as-you-type (P1).
- Instant search index: NTFS MFT / USN on Windows, background indexer with `inotify`/`fanotify` on Linux.
- Command palette (Ctrl+Shift+P).
- Plugins through WASM.
- Portal file chooser backend on Linux (`org.freedesktop.impl.portal.FileChooser`) so GTK/Qt apps use Findex for Open/Save. Windows has no equivalent, so this is Linux only.

## 10. Explicit non-goals for v1.0

- Replacing Explorer's Open/Save dialogs on Windows (not possible through supported means).
- Replacing the Windows desktop, taskbar or Start menu.
- macOS support.
- Cloud sync clients.

## 11. Keyboard shortcuts (P0, same as Windows unless noted)

| Action | Keys |
|---|---|
| New window / tab / close tab | Ctrl+N / Ctrl+T / Ctrl+W |
| Next / previous tab | Ctrl+Tab / Ctrl+Shift+Tab |
| Jump to tab | Ctrl+1..9 |
| Reopen closed tab | Ctrl+Shift+T |
| Address bar | Ctrl+L, Alt+D, F4 |
| Search | Ctrl+E, Ctrl+F, F3 |
| Back / forward / up | Alt+Left / Alt+Right / Alt+Up |
| Refresh | F5, Ctrl+R |
| Rename | F2 |
| Delete / permanent delete | Del / Shift+Del |
| Copy / cut / paste | Ctrl+C / Ctrl+X / Ctrl+V |
| New folder | Ctrl+Shift+N |
| Properties | Alt+Enter |
| Preview / details pane | Alt+P / Alt+Shift+P |
| View modes | Ctrl+Shift+1..8 |
| Select all / invert selection | Ctrl+A / Ctrl+Shift+I |
| Undo / redo | Ctrl+Z / Ctrl+Y |
| New window at this folder | Ctrl+N |
| Open the shell hotkey | Win+E (Windows), Super+E (Linux), see `04-platform-integration.md` |

All shortcuts are remappable. Every action has a menu or button path, so nothing is keyboard-only (accessibility).
