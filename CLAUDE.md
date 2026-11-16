# Findex (findex-rust)

A fast, cross-platform file explorer in Rust. The UI matches Windows 11 File Explorer. It runs on Linux and Windows and can be set as the default file manager on both.

**Status: early alpha.** M0–M1 are largely implemented and tested on Linux, with parts of M2–M6 and M8 done. `development/TODO.md` tracks what is done, partial and open. The Windows build compiles in CI but is untested on hardware.

Full detail lives in `development/docs/`. This file is the compiled summary. When this file and a doc disagree, the doc wins; fix this file.

## Doc map

| File | Read when |
|---|---|
| `development/docs/01-research.md` | You need evidence, sources, or the verified/unverified tags |
| `development/docs/02-architecture.md` | Touching crate layout, threading, providers, listing pipeline |
| `development/docs/03-features.md` | Deciding scope, tabs, views, shortcuts, transfer panel |
| `development/docs/04-platform-integration.md` | Default file manager, Win+E / Super+E, drives, network |
| `development/docs/05-performance.md` | Budgets, copy engine, rendering, benchmarks |
| `development/docs/06-cleanup-and-thumbnails.md` | Disk cleanup, deep clean, analyzer, thumbnail pipeline |
| `development/docs/07-ui-design-language.md` | Any UI work: tokens, metrics, components, motion, accessibility |
| `development/docs/08-roadmap.md` | Spikes, milestones, risks, open questions |
| `development/docs/09-release.md` | Release markers, workflow, installer |
| `development/TODO.md` | What to do next |

## Product requirements (from the owner)

1. Windows 11 File Explorer look and feel, on Linux and Windows.
2. Copy, paste, move and delete report into one unified transfer panel (interpretation to be confirmed).
3. Linux: list all drives (mounted, unmounted, network). All Windows view types (extra large to small icons, list, details, tiles, content).
4. Disk cleanup and deep disk clean.
5. Thumbnail generation.
6. Set as default file manager on Linux, and on Windows where it is possible. Win+E (Windows) and Super+E (Linux) must open Findex once default is set.
7. Tabs.
8. Performance is the top engineering goal.

## Key decisions

- **Toolkit:** Slint (provisional, ADR-001). Gate: spike S1. Fallback: GPUI. Licence decision S7 pending.
- **Layering:** UI thread never does I/O. All I/O is cancellable. Platform code only in `findex-platform-*` crates. No `cfg(target_os)` elsewhere.
- **Listing:** `getdents64` via `rustix` on Linux, `FindFirstFileExW` with `FindExInfoBasic` and `LARGE_FETCH` on Windows. No `io_uring` for listing. Chunked streaming, virtualized rows, lazy metadata, arena-allocated names.
- **Copy engine:** `rename` on same volume, `FICLONE` then `copy_file_range` then buffered copy on Linux, `CopyFile2` on Windows. One write-heavy job per physical device. Pause and resume exact by offset.
- **Thumbnails:** freedesktop cache on Linux (shared with other managers), `IShellItemImageFactory` on Windows (STA thread). `fast_image_resize` for scaling. Do not use the AGPL `thumbnails` crate.
- **Drives (Linux):** `/proc/self/mountinfo` plus udisks2 (D-Bus) plus optional GVfs. All probes timeout-guarded. Never use `sysinfo` disk list for network mounts.
- **Tabs:** per-tab history, view state, selection. Inactive tabs release watchers (30 s) and thumbnails. Tear-off and cross-window move are P1. Win+E opens a new window; external "open folder" requests open a new tab (setting).
- **Default file manager, Linux:** `.desktop` + `mimeapps.list` + `org.freedesktop.FileManager1` D-Bus service + desktop-environment keybinding for Super+E (GNOME first).
- **Default file manager, Windows:** registry shell-verb override (Mechanism A) plus a tiny `WH_KEYBOARD_LL` helper for Win+E (Mechanism B). Separate switches. A launcher stub falls back to `explorer.exe`. The uninstaller reverts before deleting. `findex --repair` restores. Open/Save dialogs and "Show in folder" from other apps cannot be replaced; this is a documented limit.
- **Cleanup:** allow-list of known locations only, dry run, trash first, privileged steps in a separate elevated helper, never the whole app.

## Evidence tags used in docs

- **[V]** verified in a linked source
- **[K]** known from platform docs or experience, not re-checked
- **[?]** unverified; needs a spike (S1 to S7 in `08-roadmap.md`)

Windows registry keys, the Win+E hook behaviour, GNOME/KDE/XFCE keybinding keys, and all `~` design numbers are **[?]**. Prove them before relying on them.

## Design language summary

Flat, quiet, dense. Rows 32 px (compact 24). Tab strip ~40, command bar ~48, address row ~40, nav pane ~240, details pane ~320, status bar ~28. 4 px grid. Radius 4 (controls) and 8 (overlays). Two elevation levels. Accent `#0067C0` light, `#60CDFF` dark, or the OS accent. Segoe UI Variable on Windows, bundled Inter on Linux. Fluent System Icons. Motion tokens 100 / 167 / 250 ms, transform and opacity only, zero with reduced motion. Contrast at least 4.5:1, focus ring always visible, colour never the only signal, every drag action has a keyboard or menu path. No emoji as icons. Tokens only, no raw hex in components.

## Performance budgets (design targets)

Cold start < 150 ms. 100k-entry folder first rows < 80 ms, complete < 600 ms. 60 fps scroll. Idle RSS < 60 MB. 50 visible thumbnails < 300 ms. Copy within 3% of `cp` / `robocopy`. Every perf change needs before and after numbers.

## Working rules for this repo

- Commits: Conventional Commits, one logical stage per commit, **no co-author trailers**. Check `git log` before committing to follow the existing style.
- Do not commit `.codegraph/` or `.claude/settings.local.json` (already ignored).
- Docs are English. Keep them factual; tag unverified claims.
- Safety rules are not optional: revert-first uninstall, Explorer fallback stub, allow-list cleanup, no symlink following in recursive deletes, timeouts on every mount probe.
- Prefer CodeGraph (`codegraph_explore`) over grep for code questions.
- Tests: `cargo test --workspace`, then `tests/ui/run.sh` (scripted end-to-end UI suites with snapshots; see `crates/findex-app/src/ui_test.rs`). Run both before committing UI or behaviour changes.
- Releases: a `!alpha` / `!fix` / ... marker at the start of a commit subject on master opens a release PR (see `development/docs/09-release.md`). Never add a marker unless the owner asks for a release.
- UI work must follow `development/docs/07-ui-design-language.md`; use the `ui-ux-pro-max` skill for design queries (`python3 .claude/skills/ui-ux-pro-max/scripts/search.py ...`, run without `-I`).

## Open questions

1. Does "copy and paste same panel" mean one unified transfer panel?
2. Project licence (drives the Slint decision). No licence is declared yet; the Android Findex app is Apache-2.0.
3. Minimum Windows (10 22H2?) and Linux (glibc 2.31?) versions.
4. Per-user installer default on Windows.
