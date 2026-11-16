# 07 — UI Design Language (Windows 11 File Explorer)

Source of guidance: the `ui-ux-pro-max` skill (design-system query for "desktop file manager productivity dense fluent", density 8, motion 3; product match "File Manager & Transfer"; UX and chart rules). Where the skill suggests web defaults, this document overrides them in favour of the Windows 11 / Fluent look the user asked for. Overrides are listed in section 12.

Numbers marked **~** are approximations from memory of Windows 11 / WinUI 3. They must be checked against a real Windows 11 machine with a pixel ruler during M1 and updated here. Everything is defined as a token, so correcting a number is a one-line change.

## 1. Principles

1. Flat and quiet. Content (files) is the visual hero; chrome recedes. Skill match: Flat Design + Minimalism, no heavy shadows.
2. Dense but legible. 32 px default rows, compact mode 24 px.
3. One accent colour. Selection, focus ring, progress and links use it.
4. Motion explains cause and effect, stays short, and respects reduced motion.
5. Everything is reachable by keyboard and has an accessible name.

## 2. Layout

```
┌──────────────────────────────────────────────────────────────┐
│ Tab strip                                          [–][□][×] │  40 px ~
├──────────────────────────────────────────────────────────────┤
│ Command bar: New ▾  Cut Copy Paste Rename Share Delete  Sort ▾  View ▾  …  │  48 px ~
├──────────────────────────────────────────────────────────────┤
│ Back Fwd Up Refresh  [ > This PC > Documents > Projects ]   [ Search ]    │  40 px ~
├────────────┬──────────────────────────────────┬──────────────┤
│ Nav pane   │ Content                           │ Details pane │
│ ~240 px    │ list / details / icons            │ ~320 px      │
│ resizable  │                                   │ optional     │
├────────────┴──────────────────────────────────┴──────────────┤
│ 24 items   1 item selected 4.2 MB                  ▤  ▦       │  28 px ~
└──────────────────────────────────────────────────────────────┘
```

(The sketch uses text labels. The real UI uses vector icons with labels, never emoji, per the skill's `no-emoji-icons` rule.)

- Minimum window size 640 × 400. Below 900 px width the details pane auto-hides; below 700 px the navigation pane collapses to an overlay.
- Panes resize with a 4 px drag handle, double-click resets. Pane widths are remembered.
- Tab strip merges with the title bar (custom title bar, caption buttons drawn by us on Windows; on Linux use client-side decorations that match the system's button layout).

## 3. Design tokens

All values live in one `theme.slint` file, exposed as semantic tokens. No raw hex inside components (skill: `color-semantic`).

### 3.1 Colour (light / dark) ~

| Token | Light | Dark | Use |
|---|---|---|---|
| `bg-base` (Mica stand-in) | `#F3F3F3` | `#202020` | Window background |
| `bg-layer` | `#FFFFFF` (80% opacity feel) | `#2B2B2B` | Content area |
| `bg-layer-alt` | `#F9F9F9` | `#323232` | Command bar, panes |
| `bg-subtle-hover` | `#0000000A` | `#FFFFFF0F` | Hover fill |
| `bg-subtle-pressed` | `#00000006` | `#FFFFFF0A` | Pressed fill |
| `stroke-default` | `#0000000F` | `#FFFFFF12` | Dividers, control borders |
| `text-primary` | `#1A1A1A` | `#FFFFFF` | Names |
| `text-secondary` | `#5E5E5E` | `#C5C5C5` | Metadata |
| `text-disabled` | `#00000072` | `#FFFFFF5D` | Disabled |
| `accent` | `#0067C0` | `#60CDFF` | Selection bar, focus, progress |
| `accent-fill-selected` | accent at 15% | accent at 20% | Row selection |
| `danger` | `#C42B1C` | `#FF99A4` | Errors, destructive |
| `warning` | `#9D5D00` | `#FCE100` | Pause, low space |
| `success` | `#0F7B0F` | `#6CCB5F` | Done |

Rules:

- Text contrast at least 4.5:1 in both themes, checked with a script in CI over the token table (skill: `color-accessible-pairs`). The `text-secondary` and `accent` pairs must be validated; the table above is the starting point.
- Dark mode uses its own tonal values, not inverted colours (skill: `color-dark-mode`).
- Mica/acrylic translucency is optional on Windows 11 (`DwmSetWindowAttribute` backdrop type). Default to solid `bg-base` everywhere, so Linux and old Windows look identical. Translucency is a P2 Windows-only nicety.
- Accent follows the OS accent colour when available, otherwise the default above.
- File-type colours for icons and treemap (colour is never the only cue; a type glyph is always present): folder amber `#F4B400`-ish, image purple, video red, audio teal, document blue, PDF orange, archive brown, code green, other grey. (Skill product match: "PDF orange, doc blue, image purple".)

### 3.2 Typography

| Platform | Font | Fallback |
|---|---|---|
| Windows 11 | Segoe UI Variable (system) | Segoe UI |
| Linux | Inter, bundled (skill typography match for system UI, cross-platform) | system sans (Cantarell, Noto Sans) |

Scale (px): caption 12, body 14, body-strong 14/600, subtitle 20, title 28. Row text 14 (Details view may use 13 in compact mode, never below 12). Line height 1.4 for body. Numbers in size and date columns use tabular figures so columns do not jitter (skill: `number-tabular`).

### 3.3 Spacing, shape, elevation

- 4 px base grid: 4, 8, 12, 16, 24 (skill: `spacing-scale`). Dense preset (skill density 8) uses 4/8/12 mostly.
- Corner radius: controls 4, overlays and menus 8, window 8 (Windows 11 rounds the window; compositor does it).
- Elevation: two levels only. Flyouts: 1 px stroke plus soft shadow; dialogs: stroke plus larger shadow. No shadows on in-page elements (skill: Flat Design, avoid complex shadows).
- Z-order scale: content 0, panes 10, sticky bars 20, flyouts 40, dialogs 100, toasts 1000.

### 3.4 Motion

Skill dials: motion 3 (subtle). Tokens:

| Token | Duration | Easing | Use |
|---|---|---|---|
| `motion-fast` | 100 ms | decelerate | Hover, press fills |
| `motion-base` | 167 ms | decelerate | Selection, flyout open, tab switch |
| `motion-slow` | 250 ms | standard | Pane show/hide, dialog |
| exit | 70% of enter | accelerate | Closing flyouts |

- Animate opacity and transform only. Never animate width/height of lists.
- Tab open/close: width animates over 167 ms; reorder uses a 167 ms slide.
- Progress bars animate value changes smoothly but never loop indefinitely when paused.
- With "reduce motion" set (OS setting is read), every animation duration becomes 0 and only state changes remain.
- No animation may block input and every animation can be interrupted.

## 4. Components

### 4.1 Tab strip

Tab height 32 inside a 40 px strip ~. Selected tab shares the colour of the command bar (merged look), unselected tabs are transparent with hover fill. Icon 16, title 12/14, close button 16 appears on hover or when selected. `+` button at the end. Drag ghost follows the pointer. Minimum width 96, maximum 240. Tooltip shows the full path after 500 ms. Focus ring 2 px accent, visible with keyboard navigation. Accessible role "tab", with arrow-key movement and Delete to close.

### 4.2 Command bar

Icon + label buttons, 36 px high ~. Collapses labels, then items, into the "..." overflow as width shrinks (skill: `overflow-menu`). Context-sensitive: Cut/Copy/Rename/Share/Delete are enabled only with a selection and show disabled state at 0.38 opacity with the cursor unchanged (skill: `disabled-states`). Destructive Delete is visually separated and uses danger colour only on its confirmation.

### 4.3 Address bar

Breadcrumb segments each with a chevron dropdown. Click empty area switches to a text field with autocomplete. Escape returns. The text field shows the full path with a visible label for assistive tech.

### 4.4 Navigation pane

Tree rows 32 px. Expand chevron rotates over 100 ms. Current location has a 3 px accent bar on the left plus a selected fill (skill: `nav-state-active`, not colour-only because of the bar). Drag-over highlights with an outline and a "Copy to X / Move to X" hint.

### 4.5 File list

- Details row 32 px (compact 24). Hover fill, selected fill with accent bar on the left edge of the row, focused row has a 2 px inner ring.
- Header cells are sortable buttons with an arrow and `aria-sort` equivalent; column resize handles are 8 px wide hit areas (skill: sortable-table).
- Icon grid cell sizes follow view modes. Thumbnail has a 4 px radius, never stretched. Names wrap to two lines, then ellipsize with full name in tooltip (skill: `truncation-strategy`).
- Check boxes (optional setting) 16 px with a 24 px hit area.
- Empty folder: icon, "This folder is empty", hint line. Loading: a 2 px indeterminate bar under the address row for slow loads; rows appear as chunks arrive (no spinner covering content). Error: inline message with cause, path and Retry (skill: `error-recovery`).
- Rubber-band selection with a translucent accent rectangle. A 4 px drag threshold prevents accidental drags (skill: `drag-threshold`).

### 4.6 Transfer panel

Flyout anchored to the status area or a separate small window. Per-job row: icons, "Copying 128 items from A to B", progress bar 4 px high ~, text "34% complete", speed, time left; chevron expands to the throughput graph (area chart, accent colour, 1 s resolution, paused = warning colour, error = danger). Pause/Resume and Cancel icon buttons with tooltips and names. Conflict dialog: two cards side by side (source / destination) with thumbnail, name, size, modified date, and radio choices Replace / Skip / Keep both plus "Apply to all". Default focus is the safest option (Skip or Keep both); Replace is never the default.

### 4.7 Dialogs, flyouts, toasts

Dialog max width 480, padding 24, buttons right-aligned, primary button filled with accent, only one primary action per dialog (skill: `primary-action`). Escape closes, Enter triggers the primary action unless it is destructive. Toasts auto-dismiss in 4 s, do not steal focus, have a polite live region and an Undo action where possible (skill: `toast-dismiss`, `toast-accessibility`, `undo-support`).

### 4.8 Context menus

Windows 11 style: compact top row of icon actions (Cut, Copy, Rename, Share, Delete), then a list. Items 32 px, 8 px radius, shortcut hints right-aligned. Submenus open after 200 ms hover or on Right arrow. On Windows, native shell items come from the shell menu and appear under "Show more options"-style grouping.

### 4.9 Cleanup and disk analyzer screens

Category list with check boxes, size bars (the bar is a redundant cue next to the number), and warning icons plus text for risky categories. Treemap: type colours from section 3.1, a 1 px gap between tiles, labels only when a tile is big enough, legend always visible, table alternative one click away, keyboard navigable. Charts follow the skill's chart rules: legend, tooltip, pattern or label in addition to colour, subtle gridlines, large datasets aggregated.

## 5. Icons

- Fluent System Icons (MIT licence), regular weight for UI chrome, filled for selected states. One family, one stroke weight (skill: `icon-style-consistent`, `no-emoji-icons`).
- Sizes: 16 (list, tree, tabs), 20 (command bar), 24 (dialogs). File-type icons in 16, 32, 48, 96, 256, drawn as SVG and rasterized at build time.
- Decorative icons next to a label are hidden from assistive tech; icon-only buttons have an accessible name.
- Windows: shell icons for files with custom icons (exe, lnk, drive icons) so branding of installed apps shows.

## 6. States

Every interactive element defines: default, hover, pressed, focused (keyboard), selected, disabled. Hover and pressed never change layout size. Focus ring is 2 px accent with 2 px offset, never removed (skill: `focus-states`). Cursor is the arrow for rows (Windows convention) and hand only on links and breadcrumbs.

## 7. Dark mode, high contrast, scaling

- Light and dark designed together. Both checked for contrast.
- High-contrast mode on Windows: use system colours and 2 px borders. Linux follows `GTK` high-contrast if present.
- DPI scale 100% to 300%, fractional scaling supported. Icons use SVG so they stay sharp. Text scale follows the OS setting.

## 8. Accessibility

Keyboard-complete (see shortcut table in `03-features.md`), screen reader roles and names (via AccessKit if the toolkit supports it; verified in S1), focus order matches visual order, focus is never hidden by sticky bars (skill: `focus-not-obscured`), drag-and-drop always has a menu or keyboard alternative (skill: `dragging-alternative`), target size at least 24 × 24 px for pointer targets (skill: `web-target-size` adapted to desktop), reduced motion honoured, status changes announced politely, colour never the only signal.

## 9. Responsive behaviour

Breakpoints (window width): ≥ 1100 three columns; 900–1099 details pane hidden; 700–899 nav pane narrow icon rail; < 700 nav pane as overlay. Command bar overflows as width shrinks. Verified at 640, 900, 1280, 1920 and ultra-wide 3440.

## 10. Linux look

Same visuals as Windows by default (the user asked for a Windows 11 look on both). Window controls follow the desktop's layout setting (left or right), `prefers-color-scheme` via the settings portal, fractional scaling on Wayland.

## 11. Delivery checklist (per screen)

Contrast checked in light and dark; focus order tested; keyboard-only run; reduced motion tested; 100%/150%/200% scaling; no layout shift on load (reserve space for thumbnails and async content); icons from one family; tokens only, no raw colours; empty, loading and error states present.

## 12. Deviations from the skill's raw output

| Skill suggestion | Decision | Reason |
|---|---|---|
| Plus Jakarta Sans, blue `#2563EB`, amber CTA | Segoe UI Variable / Inter, Windows accent `#0067C0` | Match Windows 11 |
| Pattern "Product Demo + Features" (landing page) | Ignored | Marketing-page pattern, not an app shell |
| Phosphor / Lucide icons | Fluent System Icons | Windows 11 icon language |
| Mobile rules (44 pt targets, safe areas, bottom nav) | Not applied | Desktop app. 24 px minimum pointer target instead |
| Web fonts via Google Fonts | Bundled font files | Offline desktop app |
| GSAP snippets | Not applicable | Native toolkit animations with the motion tokens above |
