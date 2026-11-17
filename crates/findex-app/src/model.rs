//! The listing model behind every file view.
//!
//! Rows are formatted lazily in `row_data`, which Slint calls only for rows on screen, so a 100k-entry
//! folder costs nothing per hidden row. The same call queues thumbnails, which gives visible-first order.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use findex_core::{Entry, Kind, format};
use findex_ui::FileRow;
use slint::{Image, Model, ModelNotify, ModelTracker, SharedString};

#[derive(Default)]
pub struct Inner {
    pub folder: PathBuf,
    pub entries: Arc<Vec<Entry>>,
    /// Display order: indices into `entries` (hidden entries filtered out unless shown).
    pub order: Vec<u32>,
    pub selected: Vec<bool>,
    /// Fuzzy scores, search mode only.
    pub scores: Vec<u32>,
    pub thumbs: HashMap<u32, Image>,
    pub generation: u64,
    pub want_thumbs: bool,
    /// "Hide extensions for known file types".
    pub hide_extensions: bool,
    pub show_hidden: bool,
    /// Focused display row and selection anchor.
    pub focus: Option<usize>,
    pub anchor: Option<usize>,
    /// Selection at the start of a rubber-band drag, by entry index (empty without Ctrl/Shift).
    pub band_base: Vec<bool>,
}

pub struct ListingModel {
    pub inner: RefCell<Inner>,
    notify: ModelNotify,
    thumbs: findex_thumbs::Service,
}

impl ListingModel {
    pub fn new(thumbs: findex_thumbs::Service) -> ListingModel {
        ListingModel {
            inner: Default::default(),
            notify: Default::default(),
            thumbs,
        }
    }

    pub fn reset(&self, folder: PathBuf, generation: u64, show_hidden: bool) {
        {
            let mut guard = self.inner.borrow_mut();
            let i = &mut *guard;
            let (want_thumbs, hide_extensions) = (i.want_thumbs, i.hide_extensions);
            *i = Inner {
                folder,
                generation,
                show_hidden,
                want_thumbs,
                hide_extensions,
                ..Default::default()
            };
        }
        self.notify.reset();
    }

    fn visible(i: &Inner, idx: u32) -> bool {
        i.show_hidden || !i.entries[idx as usize].is_hidden()
    }

    /// Appends streamed entries at the end (unsorted until `set_order`).
    pub fn append(&self, chunk: Vec<Entry>) {
        let (start, added) = {
            let mut guard = self.inner.borrow_mut();
            let i = &mut *guard;
            let base = i.entries.len() as u32;
            let n = chunk.len();
            Arc::make_mut(&mut i.entries).extend(chunk);
            i.selected.resize(base as usize + n, false);
            let start = i.order.len();
            for idx in base..base + n as u32 {
                if Self::visible(i, idx) {
                    i.order.push(idx);
                }
            }
            (start, i.order.len() - start)
        };
        if added > 0 {
            self.notify.row_added(start, added);
        }
    }

    /// Replaces the display order (after sorting), keeping selection by entry.
    pub fn set_order(&self, order: Vec<u32>) {
        {
            let mut guard = self.inner.borrow_mut();
            let i = &mut *guard;
            let focus_entry = i.focus.and_then(|f| i.order.get(f).copied());
            let order: Vec<u32> = order.into_iter().filter(|&x| Self::visible(i, x)).collect();
            i.focus = focus_entry.and_then(|e| order.iter().position(|&x| x == e));
            i.anchor = i.focus;
            i.order = order;
        }
        self.notify.reset();
    }

    pub fn set_all(&self, entries: Vec<Entry>, order: Vec<u32>) {
        {
            let mut guard = self.inner.borrow_mut();
            let i = &mut *guard;
            i.selected = vec![false; entries.len()];
            i.entries = Arc::new(entries);
        }
        self.set_order(order);
    }

    /// Search hits arrive in batches; keep them ordered by score (best first), then name.
    pub fn add_hits(&self, hits: Vec<(Entry, u32)>) {
        {
            let mut guard = self.inner.borrow_mut();
            let i = &mut *guard;
            let base = i.entries.len();
            for (e, s) in hits {
                Arc::make_mut(&mut i.entries).push(e);
                i.scores.push(s);
            }
            i.selected.resize(i.entries.len(), false);
            let new: Vec<u32> = (base as u32..i.entries.len() as u32)
                .filter(|&x| Self::visible(i, x))
                .collect();
            i.order.extend(new);
            let (entries, scores) = (i.entries.clone(), std::mem::take(&mut i.scores));
            i.order.sort_by(|&a, &b| {
                scores[b as usize].cmp(&scores[a as usize]).then_with(|| {
                    findex_core::natural_cmp(&entries[a as usize].name, &entries[b as usize].name)
                })
            });
            i.scores = scores;
        }
        self.notify.reset();
    }

    pub fn len(&self) -> usize {
        self.inner.borrow().order.len()
    }

    pub fn entries(&self) -> Arc<Vec<Entry>> {
        self.inner.borrow().entries.clone()
    }

    pub fn set_thumb(&self, generation: u64, idx: u32, img: Image) {
        let row = {
            let mut guard = self.inner.borrow_mut();
            let i = &mut *guard;
            if i.generation != generation {
                return;
            }
            i.thumbs.insert(idx, img);
            i.order.iter().position(|&x| x == idx)
        };
        if let Some(r) = row {
            self.notify.row_changed(r);
        }
    }

    pub fn set_display(&self, want_thumbs: bool, hide_extensions: bool) {
        {
            let mut i = self.inner.borrow_mut();
            i.want_thumbs = want_thumbs;
            i.hide_extensions = hide_extensions;
        }
        self.notify.reset();
    }

    pub fn set_show_hidden(&self, show: bool) {
        let order = {
            let mut guard = self.inner.borrow_mut();
            let i = &mut *guard;
            i.show_hidden = show;
            // Rebuild from all entries in current relative order plus newly visible ones.
            let mut o: Vec<u32> = (0..i.entries.len() as u32).collect();
            let pos: HashMap<u32, usize> =
                i.order.iter().enumerate().map(|(p, &x)| (x, p)).collect();
            o.sort_by_key(|x| pos.get(x).copied().unwrap_or(usize::MAX));
            o
        };
        self.set_order(order);
    }

    pub fn sort_key(&self) -> (Arc<Vec<Entry>>, u64) {
        let i = self.inner.borrow();
        (i.entries.clone(), i.generation)
    }

    // ---- selection ----

    fn changed(&self, rows: Vec<usize>) {
        if rows.len() > 64 {
            self.notify.reset();
        } else {
            for r in rows {
                self.notify.row_changed(r);
            }
        }
    }

    /// Mouse press on a row: plain selects one, Ctrl toggles, Shift extends from the anchor.
    pub fn click(&self, row: usize, ctrl: bool, shift: bool) {
        let changed = {
            let mut guard = self.inner.borrow_mut();
            let i = &mut *guard;
            if row >= i.order.len() {
                return;
            }
            let mut changed = Vec::new();
            if shift {
                let a = i.anchor.unwrap_or(row);
                let (lo, hi) = (a.min(row), a.max(row));
                for r in 0..i.order.len() {
                    let want = (lo..=hi).contains(&r) || (ctrl && i.selected[i.order[r] as usize]);
                    let idx = i.order[r] as usize;
                    if i.selected[idx] != want {
                        i.selected[idx] = want;
                        changed.push(r);
                    }
                }
            } else if ctrl {
                let idx = i.order[row] as usize;
                i.selected[idx] = !i.selected[idx];
                changed.push(row);
                i.anchor = Some(row);
            } else {
                let already_only = i.selected[i.order[row] as usize];
                // Right-click or press on an already selected item keeps a multi-selection.
                if !already_only {
                    for r in 0..i.order.len() {
                        let idx = i.order[r] as usize;
                        if i.selected[idx] {
                            i.selected[idx] = false;
                            changed.push(r);
                        }
                    }
                    i.selected[i.order[row] as usize] = true;
                    changed.push(row);
                }
                i.anchor = Some(row);
            }
            i.focus = Some(row);
            changed
        };
        self.changed(changed);
    }

    /// Plain click on a selected item collapses the selection to it (on release in Explorer; on press here).
    pub fn select_only(&self, row: usize) {
        self.clear_selection();
        self.click(row, false, false);
    }

    pub fn clear_selection(&self) {
        let changed = {
            let mut guard = self.inner.borrow_mut();
            let i = &mut *guard;
            let mut changed = Vec::new();
            for r in 0..i.order.len() {
                let idx = i.order[r] as usize;
                if i.selected[idx] {
                    i.selected[idx] = false;
                    changed.push(r);
                }
            }
            changed
        };
        self.changed(changed);
    }

    /// Rubber-band drag start: remembers what stays selected (`additive`: Ctrl/Shift) and drops the rest.
    pub fn band_begin(&self, additive: bool) {
        {
            let mut i = self.inner.borrow_mut();
            i.band_base = if additive {
                i.selected.clone()
            } else {
                Vec::new()
            };
        }
        if !additive {
            self.clear_selection();
        }
    }

    /// Selects the cells in `rows` x `cols` (grid of `ncols` columns; 1 for the details list) plus the
    /// band base. Out-of-range or inverted ranges select nothing.
    pub fn band_select(&self, rows: (i32, i32), cols: (i32, i32), ncols: i32) {
        let changed = {
            let mut guard = self.inner.borrow_mut();
            let i = &mut *guard;
            let ncols = ncols.max(1) as usize;
            let mut changed = Vec::new();
            for r in 0..i.order.len() {
                let (line, col) = ((r / ncols) as i32, (r % ncols) as i32);
                let hit = (rows.0..=rows.1).contains(&line) && (cols.0..=cols.1).contains(&col);
                let idx = i.order[r] as usize;
                let want = hit || i.band_base.get(idx).copied().unwrap_or(false);
                if i.selected[idx] != want {
                    i.selected[idx] = want;
                    changed.push(r);
                }
            }
            changed
        };
        self.changed(changed);
    }

    pub fn select_all(&self) {
        {
            let mut guard = self.inner.borrow_mut();
            let i = &mut *guard;
            for r in 0..i.order.len() {
                let idx = i.order[r] as usize;
                i.selected[idx] = true;
            }
        }
        self.notify.reset();
    }

    /// Keyboard move. `shift` extends from the anchor, `ctrl` moves focus without selecting.
    pub fn move_focus(&self, to: isize, shift: bool, ctrl: bool) {
        let n = self.len();
        if n == 0 {
            return;
        }
        let row = to.clamp(0, n as isize - 1) as usize;
        if ctrl {
            let old = self.inner.borrow_mut().focus.replace(row);
            self.changed(old.into_iter().chain(Some(row)).collect());
        } else if shift {
            let anchor = self.inner.borrow().anchor;
            self.inner.borrow_mut().anchor = anchor.or(Some(row));
            self.click(row, false, true);
        } else {
            self.select_only(row);
        }
    }

    pub fn focus(&self) -> Option<usize> {
        self.inner.borrow().focus
    }

    /// Selected entry indices, in display order.
    pub fn selected_entries(&self) -> Vec<u32> {
        let i = self.inner.borrow();
        i.order
            .iter()
            .copied()
            .filter(|&x| i.selected[x as usize])
            .collect()
    }

    pub fn path_of(&self, idx: u32) -> PathBuf {
        let i = self.inner.borrow();
        i.entries[idx as usize].path_in(&i.folder)
    }

    pub fn selected_paths(&self) -> Vec<PathBuf> {
        self.selected_entries()
            .into_iter()
            .map(|x| self.path_of(x))
            .collect()
    }

    pub fn entry_at(&self, row: usize) -> Option<(u32, Entry)> {
        let i = self.inner.borrow();
        let idx = *i.order.get(row)?;
        Some((idx, i.entries[idx as usize].clone()))
    }

    /// Selects entries by name after a refresh (new folder, paste).
    pub fn select_names(&self, names: &[std::ffi::OsString]) -> Option<usize> {
        let first = {
            let mut guard = self.inner.borrow_mut();
            let i = &mut *guard;
            let mut first = None;
            for r in 0..i.order.len() {
                let idx = i.order[r] as usize;
                let hit = names
                    .iter()
                    .any(|n| i.entries[idx].os_name() == n.as_os_str());
                i.selected[idx] = hit;
                if hit && first.is_none() {
                    first = Some(r);
                }
            }
            i.focus = first.or(i.focus);
            i.anchor = i.focus;
            first
        };
        self.notify.reset();
        first
    }

    /// Type-ahead: next row after the focus whose name starts with `prefix`.
    pub fn find_prefix(&self, prefix: &str) -> Option<usize> {
        let i = self.inner.borrow();
        let n = i.order.len();
        let start = i
            .focus
            .map_or(0, |f| f + usize::from(prefix.chars().count() == 1));
        let p = prefix.to_lowercase();
        (0..n).map(|k| (start + k) % n).find(|&r| {
            i.entries[i.order[r] as usize]
                .name
                .to_lowercase()
                .starts_with(&p)
        })
    }
}

/// Name as shown. Known types (those with a kind) lose their extension when the option is on;
/// unknown types and dotfiles keep it, as in Explorer.
fn display_name(e: &Entry, hide_extensions: bool) -> &str {
    if hide_extensions
        && !e.is_dir()
        && e.kind != Kind::File
        && let Some((stem, _)) = e.name.rsplit_once('.').filter(|(stem, _)| !stem.is_empty())
    {
        return stem;
    }
    &e.name
}

impl Model for ListingModel {
    type Data = FileRow;

    fn row_count(&self) -> usize {
        self.inner.borrow().order.len()
    }

    fn row_data(&self, row: usize) -> Option<FileRow> {
        let i = self.inner.borrow();
        let idx = *i.order.get(row)?;
        let e = &i.entries[idx as usize];
        let thumb = i.thumbs.get(&idx).cloned();
        if thumb.is_none() && i.want_thumbs && findex_thumbs::supported(&e.name) {
            self.thumbs
                .request(i.generation, idx as u64, e.path_in(&i.folder), e.mtime);
        }
        Some(FileRow {
            name: display_name(e, i.hide_extensions).into(),
            kind: e.kind as i32,
            type_name: e.kind.label().into(),
            size: if e.is_dir() {
                SharedString::new()
            } else {
                format::size(e.size).into()
            },
            date: format::date(e.mtime).into(),
            location: e
                .parent
                .as_deref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default()
                .into(),
            selected: i.selected[idx as usize],
            hidden: e.is_hidden(),
            has_thumb: thumb.is_some(),
            thumb: thumb.unwrap_or_default(),
        })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hides_only_known_extensions() {
        let e = |n: &str| Entry::new(n.into(), false, 0, 0, 0);
        assert_eq!(display_name(&e("photo.png"), true), "photo");
        assert_eq!(display_name(&e("photo.png"), false), "photo.png");
        assert_eq!(display_name(&e("data.xyz"), true), "data.xyz");
        assert_eq!(display_name(&e(".bashrc"), true), ".bashrc");
    }

    fn model(n: usize) -> ListingModel {
        let m = ListingModel::new(findex_thumbs::Service::new(
            1,
            std::env::temp_dir(),
            |_, _, _| {},
        ));
        let entries = (0..n)
            .map(|k| Entry::new(format!("f{k}").into(), false, 0, 0, 0))
            .collect();
        m.set_all(entries, (0..n as u32).collect());
        m
    }

    #[test]
    fn band_selects_grid_cells_and_keeps_base_when_additive() {
        let m = model(10); // 4 columns: rows of 0-3, 4-7, 8-9
        m.click(0, false, false);
        m.band_begin(true);
        m.band_select((1, 1), (1, 2), 4);
        assert_eq!(m.selected_entries(), vec![0, 5, 6]);
        m.band_select((1, 2), (3, 3), 4); // shrink and move: 7 and 11 (missing)
        assert_eq!(m.selected_entries(), vec![0, 7]);
        m.band_begin(false);
        m.band_select((0, 0), (0, 0), 4);
        assert_eq!(m.selected_entries(), vec![0]);
        m.band_select((1, 0), (0, 0), 4); // inverted: nothing
        assert!(m.selected_entries().is_empty());
    }
}
