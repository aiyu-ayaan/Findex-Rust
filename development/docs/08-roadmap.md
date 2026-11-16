# 08 — Roadmap, Spikes and Risks

No development has started. This file orders the work for when it does.

## 1. Spikes (prove before building)

| ID | Question | Pass condition | Blocks |
|---|---|---|---|
| S1 | Toolkit: can Slint hold a 1M-row Details list at 60 fps, a 5k-item icon grid, a drag-reorder tab strip, multi-window tear-off and an external file drop? | All five work. Frame p99 < 20 ms. Otherwise repeat with GPUI. | M1 |
| S2 | Windows: which registry keys make folder opening go to a third-party app on current Windows 11, and do updates reset them? | Documented key set, verified on 23H2 and 24H2+, plus a restore script. | M6 |
| S3 | Windows: does a `WH_KEYBOARD_LL` hook catch Win+E reliably without opening the Start menu? | 100 trials pass on a clean VM. | M6 |
| S4 | Copy: measure `copy_file_range`, `FICLONE`, `CopyFile2`, buffer sizes versus `cp` and `robocopy`. | Pick defaults with numbers. | M3 |
| S5 | Linux: set `<Super>e` from a user process on GNOME (X11 and Wayland), KDE, XFCE. | Works and can be undone on each. | M6 |
| S6 | Linux: `org.freedesktop.FileManager1` activation order versus Nautilus' service file. | Findex wins after opt-in, restore works. | M6 |
| S7 | Licence: Slint GPLv3 versus royalty-free versus the project's intended licence. | Decision recorded. | M1 |

## 2. Milestones

| M | Name | Contents | Exit criteria |
|---|---|---|---|
| M0 | Foundations | Workspace, CI (Linux + Windows), benchmark harness and fixtures, theme tokens, spikes S1, S4, S7 | Benchmarks run in CI. ADR-001-final written. |
| M1 | Browse | Window shell, tab strip, command bar, address bar, nav pane, Details/List/Icons views, streaming listing, sorting, keyboard nav, status bar, light/dark | Opens 100k-entry folder within budget. Tabs work (open, close, reorder, switch, per-tab history). |
| M2 | Drives and places | Linux drive discovery (mountinfo, udisks2, GVfs), Windows drives, This PC page, hotplug, network entries, trash view | All mounted, unmounted and network drives listed on Linux. No UI freeze with a dead NFS mount. |
| M3 | Operations | Clipboard interop, drag and drop, transfer panel, copy/move/delete engine, conflict dialog, undo, rename, new folder, properties | Interop with Nautilus/Dolphin (Linux) and Explorer (Windows). Copy within 3% of `cp`/`robocopy`. |
| M4 | Thumbnails and previews | Thumbnail service, freedesktop cache, Windows shell thumbs, preview and details panes, remaining view modes | 50 visible images within 300 ms. Cache shared with other managers. |
| M5 | Search and tabs polish | Search in folder, tear-off tabs, reopen closed tab, session restore, drop on tab, single instance | Tab feature table in `03-features.md` complete for P0 and P1. |
| M6 | Default file manager | Linux desktop entry, MIME, FileManager1, Super+E; Windows registry override, launcher stub, helper for Win+E, repair command, installer | Spikes S2, S3, S5, S6 resolved. Revert tested. Uninstall never breaks folder opening. |
| M7 | Cleanup and deep clean | Cleanup categories, profiles, disk analyzer, large and duplicate finders, elevated helper | Dry-run output matches deletions. Measured space freed. |
| M8 | Hardening and release | Accessibility audit, DPI/scale tests, crash handling, packaging (deb, rpm, AppImage, Flatpak, MSI/MSIX), docs, signed binaries | Release checklist (section 5) passes. |
| M9+ | P2 | Dual pane, index search, NTFS MFT fast path, archive browsing, portal file chooser, plugins, Mica | Per-feature. |

Tabs are in M1 (core) and M5 (advanced), so they are never deferred past the first usable build.

## 3. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Toolkit lacks native drag-and-drop or multi-window needs | High | S1 decides early. Fallback GPUI, or a thin windowing layer on `winit`. |
| Win+E cannot be intercepted reliably | Medium | S3. Mechanism A still works. Document the limit, never promise it without the helper. |
| Registry override breaks folder opening | High | Launcher stub falls back to Explorer. Backup values. `--repair`. Uninstaller reverts first. **[V]** that the failure mode is real. |
| Windows resets default handlers after updates | Medium | Detect on startup and offer to reapply. |
| Shell extension crashes inside our process | Medium | Load shell menus on the STA thread, guard with a watchdog, option to disable third-party extensions. |
| Hung network mounts freeze the app | High | Every probe on a timeout thread. Never `stat` mount roots on the UI path. |
| Cleanup deletes something important | High | Allow-list only, dry run, trash first, exclusions, confirmation with a path preview. |
| Decoder bug or hostile image crashes the app | Medium | Limits, then a separate worker process (P1). |
| Licence conflicts (Slint, Pdfium, FFmpeg, thumbnails crate AGPL) | Medium | S7. Keep Pdfium and FFmpeg as optional dynamic dependencies. Avoid AGPL crates. |
| Windows 11 look drifts between builds | Low | Tokens in one file. Re-measure at each Windows feature update. |
| Scope growth | High | P0/P1/P2 gates in `03-features.md`. Nothing in P2 starts before M8. |

## 4. Assumptions and open questions

1. "Copy and paste same panel" is read as one unified transfer panel for all jobs. Confirm.
2. Project licence is not yet chosen. It affects the toolkit (S7).
3. Windows minimum version: Windows 10 22H2 and later, with Windows 11 as the design target. Confirm.
4. Linux minimum: x86_64 and aarch64, glibc 2.31+, X11 and Wayland. Confirm.
5. Installer scope on Windows: per-user (no admin) by default, with an optional machine-wide install.
6. Telemetry: none. No network calls except user-initiated (network locations, update check if added).

## 5. Release checklist

- All P0 features done and tested on Windows 11 and one Linux GNOME and one KDE system.
- Performance budgets in `05-performance.md` measured and met or the gap documented.
- Accessibility pass with a screen reader on both platforms, keyboard-only run, reduced motion, high contrast.
- Restore-to-Explorer tested from the GUI, from `--repair`, and from the uninstaller.
- Dry run versus real clean compared on fixtures.
- Fuzz run on thumbnail decoders and the trashinfo/desktop-entry parsers.
- Licences audited (`cargo deny`).
- Crash log contains no file names unless the user opts in.

## 6. Commit conventions

Conventional Commits (`feat:`, `fix:`, `perf:`, `docs:`, `chore:`, `test:`, `refactor:`). One logical stage per commit. No co-author trailers. Performance commits include before and after numbers.
