# 05 — Performance Plan

Goal: feel faster than Windows 11 File Explorer on the same hardware. Targets below are design budgets, not measurements. Spike S1 and the benchmark harness replace them with real numbers.

## 1. Budgets

| Metric | Target | Measured on |
|---|---|---|
| Cold start to first window drawn | < 150 ms | NVMe, warm binary |
| Warm start (second instance, single-instance handoff) | < 50 ms | |
| Open folder with 1,000 entries: first rows visible | < 30 ms | warm cache |
| Open folder with 100,000 entries: first rows visible | < 80 ms, complete list < 600 ms | warm cache |
| Open folder with 1,000,000 entries | first rows < 150 ms, UI stays at 60 fps while loading | |
| Scroll Details or icon view | 60 fps, p99 frame < 20 ms | 100k entries |
| Sort 100k entries | < 60 ms (off UI thread) | |
| Switch tab | < 16 ms when snapshot cached | |
| Memory, idle, one window | < 60 MB RSS | |
| Memory, 1M-entry folder | < 250 MB RSS | |
| Thumbnail first batch for visible 50 images | < 300 ms | 12 MP JPEG, SSD |
| Copy throughput | within 3% of `cp` / `robocopy` on same device | large files |
| Many small files copy | at least as fast as Explorer | 100k files × 4 KB |
| Input latency (click to highlight) | < 50 ms | |

## 2. Listing speed

Linux:

- Read directories with `getdents64` through `rustix` using a 64 KB to 256 KB buffer. Entry type comes from `d_type`, so no `stat` per entry is needed to separate files and folders. **[K]**
- Metadata for visible rows through `statx` with only the needed mask bits (`STATX_SIZE | STATX_MTIME | STATX_TYPE`). Fetch lazily. **[K]**
- No `io_uring` for listing, because `getdents` is not supported by it. **[V]**
- Use `openat` + `fstatat` relative to a directory fd, which avoids repeated path resolution. **[K]**

Windows:

- `FindFirstFileExW` with `FindExInfoBasic` (skips the 8.3 short name) and `FIND_FIRST_EX_LARGE_FETCH`. The directory read already returns size and timestamps, so no extra call is needed. **[K]**
- Use `\\?\` prefixed paths to allow paths longer than 260 characters. **[K]**
- Shell icons and overlays are fetched only for visible rows, on the STA thread. Prefer extension-based generic icons for speed, shell icons for `.exe`, `.lnk`, `.ico` and folders with custom icons.

Both:

- Names are copied once into a per-listing arena. No `String` per entry.
- Chunked results, one UI notification per frame at most.
- Stale-while-revalidate: show the cached snapshot while a fresh read runs, then diff.

## 3. Rendering

- Virtualized rows: draw only visible rows plus a margin of two screens.
- Text layout cache keyed by (string id, font, size). Names are shaped once per row recycle, not per frame.
- Icons come from a texture atlas. Thumbnails upload to the GPU on a budget of N bytes per frame.
- Damage-based redraw: no continuous redraw while idle. Animations request frames only while running.
- Avoid per-row allocation during scroll. Row view-models are pooled.
- Scrolling with 1M rows uses an index-to-position function, not a layout pass over all rows.

## 4. Sorting, filtering, grouping

- Natural sort key computed once per entry (pre-split digit runs) and stored in the arena.
- Sort an index array (`Vec<u32>`), not the entries.
- Parallel sort with rayon above 50k entries.
- Incremental insert while streaming for small and medium folders, one final sort for large ones.
- Filters and group-by produce new index arrays, never copy entries.

## 5. Copy engine

Linux:

1. Same filesystem move: `rename`. Cross-device move: copy then delete after verification of size and mtime.
2. Try `FICLONE` (reflink) on btrfs and XFS. If supported it is instant and uses no extra space. **[K]**
3. Otherwise `copy_file_range`, then fall back to `read`/`write` with 1 to 4 MiB buffers. **[K]**
4. `fallocate` the destination size up front to avoid fragmentation and to fail early on a full disk.
5. `posix_fadvise(SEQUENTIAL)` on the source and `DONTNEED` after large copies to avoid evicting the page cache.
6. Preserve mode, timestamps, xattrs, and ownership when permitted. Sparse file detection via `SEEK_HOLE`/`SEEK_DATA`.

Windows:

1. Same volume move: `MoveFileExW`. Cross-volume: copy then delete.
2. `CopyFile2` with a progress callback, `COPY_FILE_NO_BUFFERING` for very large files, ReFS block clone when available. **[K]**
3. Parallel small-file copy with a bounded worker count (default 4), single stream for large files.

Common:

- Small-file batches use a worker pool so latency is hidden. Large files use a single stream, because parallel streams on an HDD slow it down.
- Per-device write queue: one write-heavy job per physical device by default.
- Progress is sent to the UI at 10 Hz, never per block.
- Speed graph uses a 1 s sliding window sampled at 4 Hz.
- Resume after pause is exact, by byte offset.

## 6. Thumbnails and icons

See `06-cleanup-and-thumbnails.md` section 3. Key budgets: decode only what is visible, cancel when scrolled past, use JPEG DCT scaling to decode at 1/2, 1/4 or 1/8 size when the target is small.

## 7. Memory

- Inactive tabs drop thumbnails and, after 5 minutes, the entry list (keeping scroll, selection and sort).
- Thumbnail RAM cache capped (default 128 MB), LRU by bytes.
- Icon cache capped, shared across tabs.
- Listing arena freed on navigation. Back navigation re-lists with the stale-while-revalidate path.

## 8. Benchmark harness (build before features)

A `bench/` workspace member that creates and reuses fixtures: 1k, 100k, 1M files in one folder; a deep tree of 500k files; 10k images; a folder of 50 × 2 GB files; 200k × 4 KB files.

Reported per run: time to first row, time to complete, peak RSS, frame-time histogram while scrolling, CPU time. Compared against `ls -f`, `fd`, `find`, and, on Windows, `dir /b` and Explorer (manual stopwatch plus ETW trace).

Regression gate: CI runs the headless subset and fails on a >15% slowdown against the stored baseline.

## 9. Profiling tools

`cargo flamegraph`, `perf`, `tracy` (frame spans in the UI crate), `strace -c` for syscall counts, ETW/WPA on Windows. Every optimization needs a before and after number in the commit message.

## 10. NTFS fast path (P2)

Reading the MFT directly is what makes WizTree fast. **[V]** For the disk analyzer and an instant search index on Windows, read the MFT or the USN journal through the volume handle. It needs administrator rights, so it runs in the elevated helper only. Non-NTFS and network drives use the normal walker. Linux has no equivalent; it uses the parallel walker plus an optional `fanotify`-fed index.
