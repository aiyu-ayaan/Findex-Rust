# 06 — Disk Cleanup, Deep Clean and Thumbnails

## 1. Disk cleanup

Modelled on Windows Disk Cleanup and Storage Sense: scan a drive, group files into categories, estimate recoverable space, let the user choose, then delete. **[V]**

### 1.1 Flow

1. User opens Cleanup from a drive's context menu or the command bar.
2. Scanner runs per category in parallel, read-only, cancellable. Each category reports size and file count as it goes.
3. UI lists categories with a check box, size, and a description of what deleting means. Risky categories are unchecked by default and show a warning.
4. "View files" opens the exact list for any category. Nothing is hidden.
5. Confirm dialog states total size and whether items go to trash or are deleted permanently.
6. Delete runs through the ops engine (progress in the transfer panel, undo where trash was used). Open or locked files are skipped and reported, not forced. **[V]** (Windows has the same limit)
7. Final report: space freed (measured from free-space difference, not just the estimate, since figures are approximate **[V]**), items skipped, errors.

### 1.2 Categories

Windows (drive C: only unless noted):

| Category | Location | Default | Warning |
|---|---|---|---|
| Temporary files | `%TEMP%`, `%SystemRoot%\Temp` files older than 24 h | on | |
| Recycle Bin | via shell API | off | Permanent. **[V]** |
| Thumbnail cache | `%LocalAppData%\Microsoft\Windows\Explorer\thumbcache_*.db` and Findex's own cache | on | Rebuilt on demand |
| Error reports / crash dumps | WER folders, `%LocalAppData%\CrashDumps` | on | |
| Browser caches | Chrome, Edge, Firefox cache dirs (not cookies, history or passwords) | off | Browser must be closed |
| Windows Update cleanup | needs elevation, uses the Windows servicing API, not file deletion | off | Cannot uninstall the current update afterwards. **[V]** |
| Delivery Optimization files | system, elevated | off | |
| Previous Windows installation | `C:\Windows.old` | off | Cannot roll back. **[V]** |
| Downloads folder (older than N days) | user-chosen | off | User data |

Linux:

| Category | Location | Default | Warning |
|---|---|---|---|
| Trash | `$XDG_DATA_HOME/Trash` and per-volume `.Trash-$uid` | off | Permanent |
| Thumbnail cache | `$XDG_CACHE_HOME/thumbnails` | on | Rebuilt on demand |
| User cache | `~/.cache` entries older than 30 days, per-app list, excluding Findex's own index | off | Some apps re-download |
| Package manager cache | `/var/cache/apt/archives`, `pacman -Sc` style, `dnf clean`; run the package manager's own command through polkit | off | Elevated |
| Journal logs | `journalctl --vacuum-size` through polkit | off | Elevated |
| Old logs | `/var/log/*.gz`, `*.[0-9]` | off | Elevated |
| Crash reports | `/var/crash`, `~/.local/share/*/crashes`, coredumps | on | |
| Flatpak unused runtimes | `flatpak uninstall --unused` | off | |
| Snap old revisions | `snap list --all` disabled revisions | off | Elevated |
| Docker / Podman | reported only, not deleted | n/a | Findex shows size and the command to run |
| Language caches | `~/.cargo/registry`, `~/.npm/_cacache`, `~/.cache/pip` | off | Re-download |

Rules:

- Only paths from this allow-list, or items the user picks by hand, are ever deleted. No wildcard from user input is expanded.
- System-wide categories need privilege. Findex uses polkit (`pkexec`) on Linux and a UAC-elevated helper on Windows for those rows only. The app itself never runs elevated.
- Package manager and journal cleanup call the official tools. Findex does not delete their files directly.
- A dry-run mode prints exactly what would be removed (`findex clean --dry-run`).

### 1.3 Profiles and automation

- Saved profiles, like `cleanmgr /sageset:N` and `/sagerun:N`. **[V]** `findex clean --profile work`.
- Scheduling: systemd user timer on Linux, Task Scheduler on Windows (P2). Scheduled runs only touch categories marked "safe to automate" and write a log.
- Low-space nudge: when a drive drops below 10% or 5 GB, a non-modal banner offers Cleanup.

## 2. Deep clean

Heavier, user-driven tools. Everything here shows results first and deletes only on explicit confirm.

| Tool | Method |
|---|---|
| Disk analyzer | Walk the drive once with a parallel walker, build a size tree, show a sortable tree plus a treemap. Re-scan only changed folders using the tree and watchers. NTFS MFT fast path on Windows (elevated helper, P2). **[V]** that WizTree is fast for this reason. |
| Large files | Top N by size across a scope, filter by age and type. |
| Duplicates | Group by size → hash first 64 KB → full hash (BLAKE3). Hardlinks and reflinks are detected and not counted as waste. Keep-one selection helpers (oldest, newest, shortest path). Delete goes to trash. |
| Old / unused files | Filter by last access or modification date. `noatime` mounts make access time unreliable; the UI says so. |
| Empty folders and broken symlinks | Walk, list, delete on confirm. |
| App leftovers | P2. Orphan config and cache folders for uninstalled apps, matched against installed package lists, shown as suggestions only. |

Safety:

- Exclusion list for system paths (`/`, `/usr`, `/etc`, `/boot`, `C:\Windows`, `C:\Program Files`) so duplicate and large-file tools never offer them for deletion.
- Never follow symlinks. Track `(dev, inode)` to avoid double counting hardlinks.
- Never cross filesystem boundaries unless the user ticks the option (`-xdev` behaviour by default).
- Delete uses trash by default. Permanent delete asks again and lists the first 20 paths.

### 2.1 Treemap and tree UI

Squarified treemap drawn on the GPU, colour by file type group, tooltip with path and size, click to drill, breadcrumb to go up, double-click opens the folder in a tab. Keyboard navigable, with a table alternative (accessibility). Details in `07-ui-design-language.md`.

## 3. Thumbnail pipeline

### 3.1 Shape

```
visible range changes
  └─ request(path, mtime, size, wanted_px) ── priority queue (visible first, nearest to viewport centre)
        ├─ L1: in-memory LRU (decoded, GPU-ready)
        ├─ L2: on-disk cache
        │     Linux: freedesktop thumbnail store (shared with other file managers)
        │     Windows: shell thumbnail cache through IShellItemImageFactory
        ├─ miss → decode on compute pool → resize → store L2 → publish L1
        └─ failure → record in fail dir, show type icon, do not retry until file changes
cancel when row scrolls out of range plus margin
```

### 3.2 Linux cache (interoperable)

- Location `$XDG_CACHE_HOME/thumbnails` or `~/.cache/thumbnails`. Buckets `normal` (128), `large` (256), `x-large` (512), `xx-large` (1024) and `fail`. **[V]** (buckets and location), pixel sizes from secondary sources.
- PNG files named by the MD5 of the file URI, with `Thumb::URI`, `Thumb::MTime` and `Thumb::Size` text chunks. A thumbnail is stale when mtime differs. **[K]**
- Reading existing thumbnails from GNOME and KDE gives instant results for files they already processed. We write the same format so they benefit from ours.
- Write atomically (temp file, then rename). Permission `0600`.
- Findex also honours `.thumbnailer` entries from `/usr/share/thumbnailers` so custom formats work, run with a timeout and in a restricted process. **[K]**

### 3.3 Windows

- Visible rows use `IShellItemImageFactory::GetImage` with `SIIGBF_THUMBNAILONLY` first, then icon fallback. Run on the STA worker, never the UI thread. Use `SIIGBF_INCACHEONLY` for a quick first pass. **[V]**
- Convert the `HBITMAP` to RGBA on the worker, free it with `DeleteObject`. **[V]**
- This reuses all installed shell thumbnail handlers (RAW, PSD, Office, HEIC with the right codec).
- Findex's own decoders are used for formats where they are faster (JPEG, PNG, WebP).

### 3.4 Decoders (Rust)

| Format | Crate | Notes |
|---|---|---|
| JPEG | `zune-jpeg`, or `turbojpeg` binding with DCT scaling | Decode at 1/2, 1/4, 1/8 size when the target is small. |
| PNG, GIF, WebP, BMP, TIFF, AVIF | `image` | AVIF and HEIC decoding is slow; budget and fall back to the shell/thumbnailer. |
| SVG | `resvg` | Rasterize at target size. **[V]** |
| PDF | `pdfium-render` | Needs the Pdfium library at runtime. Optional download, not bundled. **[V]** |
| Video | `ffmpeg` bindings, or call `ffmpegthumbnailer` / the platform shell | Seek to 10% of duration, decode one frame, skip black frames. Optional dependency. **[V]** |
| Audio cover art | `lofty` (ID3, FLAC, MP4 tags) | |
| Text / code | internal renderer | First lines rendered to a bitmap. |
| Office | embedded thumbnail inside the zip (`docProps/thumbnail.jpeg`) | |
| RAW, HEIC | shell (Windows), `.thumbnailer` (Linux) | Not decoded in-process in v1.0. |

- Resize with `fast_image_resize` (SSE4.1, AVX2, NEON). **[V]**
- Do not use the `thumbnails` crate: AGPL-3.0 licence. **[V]**
- All decoders run with size and time limits (max pixels, max file size, 5 s timeout) against decompression bombs and hostile files. Decoding of untrusted files runs in a separate worker process in P1 so a decoder crash cannot take down the app.

### 3.5 Size classes and view modes

| View | Thumbnail px | Cache bucket |
|---|---|---|
| Small / List / Details | icon only (16) | none |
| Medium icons | 48 | normal |
| Large icons | 96 | normal / large |
| Extra large icons, Gallery, Preview pane | 256 | large / x-large |

### 3.6 Settings

On/off per file class, "do not generate for files larger than N MB", "do not generate on network locations" (default on for network, since thumbnailing over SMB is slow), cache size cap with LRU purge, clear cache button (also available in Cleanup).
