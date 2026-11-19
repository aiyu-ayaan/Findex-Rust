# 02 — Architecture

## 1. Principles

1. The UI thread draws and handles input. It never touches the filesystem, the shell, the network or a decoder.
2. All I/O is cancellable. Navigating away cancels the previous listing, thumbnail queue and watcher.
3. Data flows one way: provider → store → view. Views read immutable snapshots.
4. Platform code sits behind traits in one crate each. The rest of the code has no `cfg(target_os)`.
5. A stuck mount, a slow share or a bad file affects only its own tab or job.

## 2. ADR-001 — GUI toolkit (provisional)

Decision: **Slint**, with the Skia renderer on Windows and Linux and the software renderer as a fallback.
Why: `ListView` instantiates only visible rows **[V]**, ships a Fluent widget style **[K]**, declarative layout suits the tab strip, command bar and panes, and it avoids a web view.
Costs and risks:

- Licence. Slint is GPLv3, royalty-free (with conditions) or commercial. Findex must be GPLv3, or accept the royalty-free terms. Decide before M1. **[?]**
- Native drag-and-drop of files into and out of the window may need windowing-layer work beyond Slint's widgets. **[?]**
- Multi-window tear-off tabs need several top-level windows sharing one app state. **[?]**
- Accessibility coverage must be checked against screen readers on both platforms. **[?]**

Gate: spike S1 builds a 1,000,000-row Details list, a 5,000-item icon grid, a tab strip with drag-reorder, and an external file drop. If any item fails, switch to GPUI with the same spike. Record the outcome as ADR-001-final.

## 3. Workspace layout

```
findex-rust/
  Cargo.toml                # workspace
  crates/
    findex-core/            # no UI, no OS calls: models, sorting, filters, selection, history
    findex-vfs/             # Provider trait + local, trash, archive, network providers
    findex-platform/        # trait definitions: Drives, Shell, Clipboard, Trash, Thumbnails, DefaultHandler
    findex-platform-linux/  # getdents64, udisks2, gio, D-Bus, xdg, gsettings
    findex-platform-win/    # FindFirstFileExW, shell, registry, WNet, hooks
    findex-ops/             # copy/move/delete engine, job queue, conflicts, undo log
    findex-thumbs/          # decode, resize, cache, scheduler
    findex-scan/            # recursive walker, disk analyzer, duplicate finder, cleanup rules
    findex-link/            # Findex Link with the Android app: identity, pairing, TLS server/client, mDNS (see 10-ecosystem.md)
    findex-index/           # search index (P2)
    findex-ui/              # Slint UI, view-models, theme, shortcuts
    findex-app/             # main binary, single instance, session restore, CLI
    findex-launcher/        # Windows stub that falls back to explorer.exe
    findex-helper/          # Windows Win+E hook helper (tiny)
  development/docs/
```

`findex-core` and `findex-ops` have no dependency on the UI crate, so they can be tested and benchmarked headless.

## 4. Core model

```
App
 └─ Window[*]
     └─ Tab[*]
         ├─ Location (URI: file:///, trash:///, smb://..., archive:///...)
         ├─ History (back/forward stacks, bounded)
         ├─ ViewState (mode, sort, group, columns, scroll, zoom)
         ├─ Listing (Arc<Snapshot>)           # immutable entries + order index
         ├─ Selection (bitset over entry ids)
         └─ Watcher handle (dropped when tab is inactive 30 s)
TransferQueue (app-wide, outlives windows)
ThumbService  (app-wide)
DriveService  (app-wide, hotplug)
```

An `Entry` is a compact struct: interned name id, kind, size, mtime, flags, icon key. Strings live in an arena per listing. Target: under 64 bytes per entry plus the name. A 1M-entry folder then uses about 100 MB; larger folders use windowed loading (section 6).

## 5. Provider trait

```rust
trait Provider: Send + Sync {
    fn list(&self, loc: &Location, sink: ChunkSink, cancel: CancelToken) -> Result<()>;
    fn stat(&self, loc: &Location) -> Result<Meta>;
    fn watch(&self, loc: &Location, tx: EventSender) -> Result<WatchHandle>;
    fn open_read(&self, loc: &Location) -> Result<Box<dyn Read + Send>>;
    // create, rename, remove, copy_hint(), same_volume(), capabilities()
}
```

Providers: `LocalFs`, `Trash`, `Archive`, `Gvfs` / `Smb`, `Sftp` (P1). The URI scheme picks the provider. Capabilities (`can_rename`, `can_reflink`, `can_trash`, `atomic_move_with`) drive menu enablement.

## 6. Listing pipeline

```
navigate(loc)
  ├─ cancel previous listing for this tab
  ├─ show cached snapshot immediately if fresh (< 2 s) or stale-while-revalidate
  ├─ worker: provider.list() emits chunks of ~2,000 entries
  │     each chunk: names + minimal metadata from the directory read itself
  ├─ store merges chunks, keeps sort order with an incremental insert (or sorts once at end for > 200k)
  ├─ UI receives "snapshot v" notifications, coalesced to at most 1 per frame (16 ms)
  └─ lazy metadata: size/mtime/owner/type for visible rows only, fetched in batches
```

- First rows appear after the first chunk, not after the whole directory.
- Sorting runs off the UI thread. For >200k entries the UI shows the unsorted stream with a "sorting..." indicator and swaps once ready.
- Visible-range requests (`first..last` row) drive metadata, icon and thumbnail work, with a small prefetch margin.

## 7. Threading

| Pool | Threads | Work |
|---|---|---|
| UI | 1 | Slint event loop, input, painting. |
| IO | `min(cores, 8)` | Directory reads, stat, watchers. |
| Compute | `cores - 1` (rayon) | Sorting, thumbnail decode and resize, hashing. |
| Ops | 1 per physical device, max 4 | Copy, move, delete. |
| Blocking-risk | dedicated, timeout-guarded | `statvfs`, `GetDiskFreeSpaceEx`, network probes, shell COM calls. |

On Windows, shell and COM objects live on a dedicated STA thread, because shell extensions are not reliably thread-safe. **[V]** (one crate serializes shell icon calls)

## 8. Change events

- Linux: `inotify` via the `notify` crate for the visible folders only. On `IN_Q_OVERFLOW`, trigger a full re-list. Recursive watches are never created for the whole disk.
- Windows: `ReadDirectoryChangesW` per watched folder with overlapped I/O.
- Events are debounced for 100 ms and applied to the snapshot as insert, remove or update operations, preserving selection and scroll position.

## 9. Persistence

- Settings: TOML in the platform config dir (`$XDG_CONFIG_HOME/findex`, `%APPDATA%\Findex`).
- State (sessions, per-folder view memory, recents): SQLite (`rusqlite`, bundled), size-capped with LRU trimming.
- Thumbnail cache: see `06-cleanup-and-thumbnails.md`.
- Undo log for operations: in memory, plus a small journal file so a crash mid-move can be recovered.

## 10. Errors and logging

`thiserror` in library crates, `anyhow` only in the app crate. `tracing` with a rolling file in the state dir. Every user-facing error shows the OS message text and the path. No panics on I/O errors; a panic hook writes a crash report to the log dir (no network upload).

## 11. Security and safety

- Never follow symlinks during recursive delete, size or cleanup scans. Detect loops with `(dev, inode)`.
- Cleanup and deep-clean deletes only paths from an allow-list of known locations, plus user-selected items. See `06-cleanup-and-thumbnails.md`.
- Privileged actions (MFT read, system cleanup categories) run through a separate elevated helper, started on demand, never the whole app elevated.
- Registry and `gsettings` changes are logged, backed up and reversible (`04-platform-integration.md`).
- The Windows Win+E helper observes one key chord only and stores nothing.

## 12. Testing strategy

- Unit tests for sorting (natural order), selection, history, conflict resolution, path handling, trash info parsing.
- Property tests (`proptest`) for sort stability and incremental merge equal to full sort.
- Filesystem fixtures in tempdirs, including huge directories, deep trees, symlink loops, invalid UTF-8 names (Linux), long paths and reserved names (Windows).
- Integration tests for the ops engine: kill mid-copy, disk full, permission denied, cross-device move.
- Platform tests run in CI on `ubuntu-latest` and `windows-latest`; registry and hook behaviour on a dedicated Windows VM (manual checklist in the roadmap).
- Benchmarks with `criterion` (microbenchmarks) and a scripted UI benchmark that reports time-to-first-row and frame times.
