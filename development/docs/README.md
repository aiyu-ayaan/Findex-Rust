# Findex — Design Docs

Findex is a fast, cross-platform (Linux + Windows) file explorer written in Rust, with a UI that matches Windows 11 File Explorer.

Status: **analysis and design only. Development has not started.**

| Doc | Content |
|---|---|
| [01-research.md](01-research.md) | Research findings, verified vs unverified, sources |
| [02-architecture.md](02-architecture.md) | Toolkit decision, crate layout, threading, data flow |
| [03-features.md](03-features.md) | Feature matrix, tabs, views, transfer panel, extras beyond Windows |
| [04-platform-integration.md](04-platform-integration.md) | Default file manager on Linux and Windows, Win+E / Super+E, drives and network |
| [05-performance.md](05-performance.md) | Budgets, listing pipeline, caches, benchmark plan |
| [06-cleanup-and-thumbnails.md](06-cleanup-and-thumbnails.md) | Disk cleanup, deep clean, disk analyzer, thumbnail pipeline |
| [07-ui-design-language.md](07-ui-design-language.md) | Windows 11 look: tokens, layout metrics, components, motion, accessibility |
| [08-roadmap.md](08-roadmap.md) | Spikes, milestones, risks, open questions |
| [../TODO.md](../TODO.md) | Working task list, grouped by milestone |
| [../../CLAUDE.md](../../CLAUDE.md) | Compiled summary for agents |

## Reading order

1. `01-research.md` for evidence.
2. `03-features.md` for scope.
3. `02-architecture.md` and `05-performance.md` for how.
4. `04-platform-integration.md` for the OS-specific hard parts.
